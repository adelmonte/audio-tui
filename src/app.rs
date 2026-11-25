//! audio-tui state and input handling. Rendering is in `view.rs`.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Rect;
use ratatui::style::Color;
use tokio::sync::mpsc::UnboundedSender;
use toml_edit::value;

use crate::action::{Action, Ctx};
use crate::backend::conf::{self, Dirs, Service};
use crate::backend::model::*;
use crate::backend::state::{self, StateFile};
use crate::backend::{Cmd, CmdTx, Ev, Peaks, METER_NODE_NAME};
use crate::cfgedit::{self, Edit};
use crate::config::{Config, PrivacyMode, RouteRule, Virtual, VirtualKind};
use crate::core::keymap::{ActionSet, Keymap};
use crate::core::keys::KeyCombo;
use crate::ui::dialog::{Dialog, DialogKey, Field};
use crate::ui::input::{InputOutcome, TextInput};
use crate::ui::mouse::MouseMap;
use crate::ui::palette::{Entry, Palette, PaletteOutcome};
use crate::ui::theme::Theme;
use crate::ui::widgets::{ConfirmHit, Level, Toasts};

pub const ACCENT: Color = Color::Rgb(0xcb, 0xa6, 0xf7);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tab {
    Playback,
    Recording,
    Outputs,
    Inputs,
    Cards,
    Virtual,
    Settings,
    Privacy,
}

impl Tab {
    pub const ALL: [Tab; 8] = [Tab::Playback, Tab::Recording, Tab::Outputs, Tab::Inputs, Tab::Cards, Tab::Virtual, Tab::Settings, Tab::Privacy];
    pub fn title(self) -> &'static str {
        match self {
            Tab::Playback => "Playback",
            Tab::Recording => "Recording",
            Tab::Outputs => "Outputs",
            Tab::Inputs => "Inputs",
            Tab::Cards => "Cards",
            Tab::Virtual => "Virtual",
            Tab::Settings => "Settings",
            Tab::Privacy => "Privacy",
        }
    }
    pub fn ctx(self) -> Ctx {
        match self {
            Tab::Playback | Tab::Recording => Ctx::Streams,
            Tab::Outputs | Tab::Inputs => Ctx::Devices,
            Tab::Cards => Ctx::Cards,
            Tab::Virtual => Ctx::Virtual,
            Tab::Settings => Ctx::Settings,
            Tab::Privacy => Ctx::Privacy,
        }
    }
    pub fn kind(self) -> Option<NodeKind> {
        Some(match self {
            Tab::Playback => NodeKind::Playback,
            Tab::Recording => NodeKind::Recording,
            Tab::Outputs => NodeKind::Sink,
            Tab::Inputs => NodeKind::Source,
            _ => return None,
        })
    }
}

/// One selectable line in a mixer tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MixSel {
    pub node: u32,
    /// Some(i) = per-channel row i of an expanded item.
    pub channel: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SetRow {
    Rate,
    Quantum,
    AllowedRates,
    SaveClock,
    MaxVolume,
    VolumeStep,
    Meters,
    Sm(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum PrivRow {
    RememberVolumes,
    RememberDevices,
    Mode,
    Allowlist,
    Prune,
    ForgetAll,
    File(usize),
    Entry(usize, usize),
}

#[derive(Clone, Debug, PartialEq)]
pub enum CardRow {
    Device(u32),
    /// Disabled through audio-tui's config (so it's not in the graph).
    Disabled(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum VirtRow {
    Config(usize),
    Live(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Tab(usize),
    Row(usize),
    Mix(MixSel),
    VolBar(MixSel),
    MuteBtn(u32),
    MoveBtn(u32),
    DefaultBtn(u32),
    Run(Action),
    Confirm(ConfirmHit),
    PaletteRow(usize),
    MenuRow(usize),
    DialogField(usize),
    DialogButton(bool),
    PendingBanner,
    ProfilePick { device: u32, index: i32 },
    PortPick { device: u32, index: i32, dev: i32 },
}

#[derive(Clone, Debug)]
pub enum MenuItem {
    Run(Action),
    Move { stream: u32, target: Option<u32> },
    Profile { device: u32, index: i32 },
    Port { device: u32, index: i32, dev: i32 },
    NewVirtual(VirtualKind),
    Setting { key: &'static str, value: Option<String> },
}

pub struct Menu {
    pub title: String,
    /// (label, hint, item, is_current)
    pub items: Vec<(String, String, MenuItem, bool)>,
    pub sel: usize,
    pub at: (u16, u16),
}

#[derive(Clone, Debug)]
pub enum Purpose {
    Rename { key: String },
    Alias { key: String },
    Volume { node: u32 },
    Latency { node: u32 },
    NewVirtual(VirtualKind),
    TempSink,
    SmValue { key: String },
    AllowedRates,
    Allowlist,
}

#[derive(Clone, Debug)]
pub enum OnYes {
    Restart(Vec<Service>),
    ForgetEntries { path: PathBuf, keys: Vec<String> },
    ForgetFiles(Vec<PathBuf>),
    Disable(String),
    RemoveVirtual(String),
}

pub enum Popup {
    Help { scroll: u16, max: u16 },
    Palette(Palette<Action>),
    Menu(Menu),
    Confirm { title: String, msg: String, yes: String, yes_sel: bool, on_yes: OnYes },
    Dialog(Box<Dialog<Purpose>>),
}

/// Results of background work (restarts, state edits).
pub enum AppEv {
    Toast(Level, String),
    ReloadState,
}

pub struct App {
    pub cfg: Config,
    pub cfg_path: PathBuf,
    pub theme: Theme,
    pub keymap: Keymap<Action>,
    pub tx: CmdTx,
    pub app_tx: UnboundedSender<AppEv>,
    pub demo: bool,
    pub dirs: Dirs,

    pub graph: Graph,
    pub loaded: bool,
    pub disconnected: Option<String>,
    pub tab: Tab,
    pub mix_sel: HashMap<Tab, MixSel>,
    pub row_sel: HashMap<Tab, usize>,
    pub expanded: HashSet<u32>,
    /// Privacy tab: state files whose entries are shown.
    pub priv_open: HashSet<String>,
    /// Volume/mute we set but PipeWire hasn't echoed back yet. Kept on top of
    /// incoming snapshots so the slider doesn't jump back to stale values.
    pending_vol: HashMap<u32, (Vec<f32>, Option<bool>, Instant)>,
    pub list_offset: HashMap<Tab, usize>,
    pub list_height: usize,
    pub filter: Option<TextInput>,
    pub filter_focus: bool,
    pub popup: Option<Popup>,
    pub toasts: Toasts,
    pub tick: u64,
    pub show_details: bool,
    pub mouse: MouseMap<Target>,
    last_click: Option<(Instant, Target)>,
    last_toggle: Option<(Action, Instant)>,
    pub drag: Option<(MixSel, Rect)>,
    pub quit: bool,

    // meters
    pub peaks: Peaks,
    pub levels: HashMap<u32, Vec<f32>>,
    pub holds: HashMap<u32, Vec<(f32, u64)>>,
    pub visible: BTreeSet<u32>,
    metered: BTreeSet<u32>,

    // system config
    pub pending: Vec<(PathBuf, &'static str)>,
    pub config_errors: Vec<String>,
    pub state_files: Vec<StateFile>,
}

impl App {
    #[allow(clippy::too_many_arguments)]
    pub fn new(cfg: Config, cfg_path: PathBuf, tx: CmdTx, app_tx: UnboundedSender<AppEv>, peaks: Peaks, demo: bool, mut warnings: Vec<String>, dirs: Dirs) -> Self {
        let (keymap, kw) = Keymap::build(&cfg.keys);
        warnings.extend(kw);
        let theme = Theme::from_config(&cfg.theme, ACCENT, &mut warnings);
        let mut toasts = Toasts::default();
        for w in warnings {
            toasts.push(Level::Warn, w);
        }
        if demo {
            toasts.push(Level::Info, "Demo mode — fake devices; system files go to a temp folder");
        }
        let show_details = cfg.general.details;
        let mut app = Self {
            cfg,
            cfg_path,
            theme,
            keymap,
            tx,
            app_tx,
            demo,
            dirs,
            graph: Graph::default(),
            loaded: false,
            disconnected: None,
            tab: Tab::Playback,
            mix_sel: HashMap::new(),
            row_sel: HashMap::new(),
            expanded: HashSet::new(),
            priv_open: HashSet::new(),
            pending_vol: HashMap::new(),
            list_offset: HashMap::new(),
            list_height: 10,
            filter: None,
            filter_focus: false,
            popup: None,
            toasts,
            tick: 0,
            show_details,
            mouse: MouseMap::default(),
            last_click: None,
            last_toggle: None,
            drag: None,
            quit: false,
            peaks,
            levels: HashMap::new(),
            holds: HashMap::new(),
            visible: BTreeSet::new(),
            metered: BTreeSet::new(),
            pending: Vec::new(),
            config_errors: Vec::new(),
            state_files: Vec::new(),
        };
        app.refresh_pending();
        app.reload_state();
        app
    }

    pub fn send(&self, c: Cmd) {
        (self.tx)(c);
    }

    pub fn ctx(&self) -> Ctx {
        self.tab.ctx()
    }

    pub fn state_dir(&self) -> PathBuf {
        if self.demo {
            std::env::temp_dir().join("audio-tui-demo/state/wireplumber")
        } else {
            state::state_dir()
        }
    }

    pub fn reload_state(&mut self) {
        self.state_files = state::load_all(&self.state_dir());
    }

    pub fn refresh_pending(&mut self) {
        let (files, errors) = conf::generate(&self.cfg, &self.dirs);
        self.pending = conf::pending(&files).into_iter().map(|f| (f.path.clone(), f.topic)).collect();
        self.config_errors = errors;
    }

    // ------------------------------------------------------------ names

    /// Display name, honouring `[aliases]` (by node.name or app name).
    pub fn title(&self, n: &Node) -> String {
        if let Some(a) = self.cfg.aliases.get(&n.name) {
            return a.clone();
        }
        if n.kind().is_stream()
            && let Some(a) = self.cfg.aliases.get(&n.app_name) {
                return a.clone();
            }
        n.title()
    }

    pub fn is_internal(&self, n: &Node) -> bool {
        n.name == METER_NODE_NAME
            || n.props.get("stream.monitor").is_some_and(|v| v == "true")
            || n.media_name == "Peak detect"
            || n.app_name == "audio-tui"
    }

    // ------------------------------------------------------------ mixer model

    pub fn items(&self, tab: Tab) -> Vec<u32> {
        let Some(kind) = tab.kind() else { return Vec::new() };
        let q = self.filter.as_ref().map(|f| f.value().to_lowercase()).unwrap_or_default();
        let mut v: Vec<&Node> = self
            .graph
            .nodes
            .values()
            .filter(|n| n.kind() == kind)
            .filter(|n| self.cfg.general.show_internal_streams || !self.is_internal(n))
            .filter(|n| q.is_empty() || self.title(n).to_lowercase().contains(&q) || n.media_name.to_lowercase().contains(&q) || n.name.to_lowercase().contains(&q))
            .collect();
        v.sort_by_key(|n| (self.title(n).to_lowercase(), n.id));
        v.into_iter().map(|n| n.id).collect()
    }

    /// Every selectable line in a mixer tab, in order.
    pub fn mix_rows(&self, tab: Tab) -> Vec<MixSel> {
        let mut out = Vec::new();
        for id in self.items(tab) {
            out.push(MixSel { node: id, channel: None });
            if self.expanded.contains(&id)
                && let Some(n) = self.graph.nodes.get(&id) {
                    let (vols, _) = self.graph.volume_of(n);
                    for c in 0..vols.len() {
                        out.push(MixSel { node: id, channel: Some(c) });
                    }
                }
        }
        out
    }

    pub fn current_mix(&self) -> Option<MixSel> {
        let rows = self.mix_rows(self.tab);
        match self.mix_sel.get(&self.tab) {
            Some(cur) => rows.contains(cur).then_some(*cur).or_else(|| rows.iter().find(|r| r.node == cur.node).copied()).or_else(|| rows.first().copied()),
            None => rows.first().copied(),
        }
    }

    pub fn selected_node(&self) -> Option<&Node> {
        self.current_mix().and_then(|s| self.graph.nodes.get(&s.node))
    }

    pub fn card_rows(&self) -> Vec<CardRow> {
        let mut devs: Vec<&Device> = self.graph.devices.values().collect();
        devs.sort_by_key(|d| d.title().to_lowercase());
        let mut v: Vec<CardRow> = devs.into_iter().map(|d| CardRow::Device(d.id)).collect();
        for (name, r) in &self.cfg.devices {
            if r.disabled == Some(true) && !self.graph.devices.values().any(|d| &d.name == name) && !self.graph.nodes.values().any(|n| &n.name == name) {
                v.push(CardRow::Disabled(name.clone()));
            }
        }
        v
    }

    pub fn virt_rows(&self) -> Vec<VirtRow> {
        let mut v: Vec<VirtRow> = (0..self.cfg.virtuals.len()).map(VirtRow::Config).collect();
        for n in self.graph.nodes.values() {
            if n.is_virtual() && !self.cfg.virtuals.iter().any(|c| n.name == c.name || n.name.starts_with(&format!("{}.", c.name))) {
                v.push(VirtRow::Live(n.id));
            }
        }
        v
    }

    pub fn set_rows(&self) -> Vec<SetRow> {
        let mut v = vec![SetRow::Rate, SetRow::Quantum, SetRow::AllowedRates, SetRow::SaveClock, SetRow::MaxVolume, SetRow::VolumeStep, SetRow::Meters];
        v.extend(self.graph.sm_settings.keys().map(|k| SetRow::Sm(k.clone())));
        v
    }

    pub fn priv_rows(&self) -> Vec<PrivRow> {
        let mut v = vec![PrivRow::RememberVolumes, PrivRow::RememberDevices, PrivRow::Mode];
        if self.cfg.privacy.mode == PrivacyMode::Allowlist {
            v.push(PrivRow::Allowlist);
        }
        v.push(PrivRow::Prune);
        if !self.state_files.is_empty() {
            v.push(PrivRow::ForgetAll);
        }
        for (fi, f) in self.state_files.iter().enumerate() {
            v.push(PrivRow::File(fi));
            if self.priv_open.contains(&f.name) {
                for ei in 0..f.entries.len() {
                    v.push(PrivRow::Entry(fi, ei));
                }
            }
        }
        v
    }

    pub fn row_count(&self) -> usize {
        match self.tab {
            Tab::Cards => self.card_rows().len(),
            Tab::Virtual => self.virt_rows().len(),
            Tab::Settings => self.set_rows().len(),
            Tab::Privacy => self.priv_rows().len(),
            _ => self.mix_rows(self.tab).len(),
        }
    }

    pub fn row_index(&self) -> usize {
        match self.tab.kind() {
            Some(_) => {
                let rows = self.mix_rows(self.tab);
                self.current_mix().and_then(|c| rows.iter().position(|r| *r == c)).unwrap_or(0)
            }
            None => self.row_sel.get(&self.tab).copied().unwrap_or(0).min(self.row_count().saturating_sub(1)),
        }
    }

    pub fn select_row(&mut self, i: usize) {
        match self.tab.kind() {
            Some(_) => {
                if let Some(r) = self.mix_rows(self.tab).get(i) {
                    self.mix_sel.insert(self.tab, *r);
                }
            }
            None => {
                self.row_sel.insert(self.tab, i.min(self.row_count().saturating_sub(1)));
            }
        }
    }

    fn move_sel(&mut self, d: isize) {
        let n = self.row_count();
        if n == 0 {
            return;
        }
        let i = (self.row_index() as isize + d).clamp(0, n as isize - 1) as usize;
        self.select_row(i);
    }

    // ------------------------------------------------------------ backend events

    pub fn on_backend(&mut self, ev: Ev) {
        match ev {
            Ev::Graph(g) => {
                let first = !self.loaded;
                if self.disconnected.take().is_some() {
                    self.toasts.push(Level::Ok, "Connected to PipeWire");
                    self.metered.clear(); // meters must be re-created on the new connection
                }
                self.graph = *g;
                self.reapply_pending_vol();
                self.loaded = true;
                if first {
                    // Start on the tab that has something going on.
                    if self.items(Tab::Playback).is_empty() {
                        self.tab = Tab::Outputs;
                    }
                }
            }
            Ev::Disconnected(m) => {
                self.disconnected = Some(m);
                self.loaded = true;
                self.metered.clear();
            }
            Ev::Ok(m) => self.toasts.push(Level::Ok, m),
            Ev::Error(m) => self.toasts.push(Level::Error, m),
        }
    }

    pub fn on_app_event(&mut self, ev: AppEv) {
        match ev {
            AppEv::Toast(l, m) => self.toasts.push(l, m),
            AppEv::ReloadState => self.reload_state(),
        }
    }

    pub fn on_tick(&mut self) {
        self.tick = self.tick.wrapping_add(1);
        self.toasts.prune();
        // Meter ballistics: instant attack, ~20 dB/s release, 1.5 s peak hold.
        let fresh: HashMap<u32, Vec<f32>> = std::mem::take(&mut *self.peaks.lock().expect("peaks"));
        let ids: BTreeSet<u32> = self.levels.keys().chain(fresh.keys()).copied().collect();
        for id in ids {
            let new = fresh.get(&id).cloned().unwrap_or_default();
            let cur = self.levels.entry(id).or_default();
            if cur.len() < new.len() {
                cur.resize(new.len(), 0.0);
            }
            for (i, c) in cur.iter_mut().enumerate() {
                let n = new.get(i).copied().map(meter_scale).unwrap_or(0.0);
                *c = if n > *c { n } else { (*c - 0.011).max(n) };
            }
            let tick = self.tick;
            let hold = self.holds.entry(id).or_default();
            hold.resize(cur.len(), (0.0, 0));
            for (h, c) in hold.iter_mut().zip(cur.iter()) {
                if *c >= h.0 || tick.saturating_sub(h.1) > 45 {
                    *h = (*c, tick);
                }
            }
        }
    }

    /// Ask the backend to meter exactly the visible nodes.
    pub fn sync_meters(&mut self) {
        let want: BTreeSet<u32> = if self.cfg.general.meters && self.tab.kind().is_some() && self.disconnected.is_none() { self.visible.clone() } else { BTreeSet::new() };
        if want != self.metered {
            self.metered = want.clone();
            self.send(Cmd::Meter { nodes: want.into_iter().collect() });
        }
    }

    // ------------------------------------------------------------ input

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Mouse(m) if self.cfg.general.mouse => self.on_mouse(m),
            Event::Paste(s) => {
                let line = s.lines().next().unwrap_or("").to_string();
                match &mut self.popup {
                    Some(Popup::Dialog(d)) => d.paste(&line),
                    Some(Popup::Palette(p)) => p.input.insert_str(&line),
                    None if self.filter_focus => {
                        if let Some(f) = &mut self.filter {
                            f.insert_str(&line);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn on_key(&mut self, k: KeyEvent) {
        if self.popup.is_some() {
            return self.popup_key(k);
        }
        if self.filter_focus {
            if let Some(f) = &mut self.filter {
                match f.handle_key(&k) {
                    InputOutcome::Submit => self.filter_focus = false,
                    InputOutcome::Cancel => {
                        self.filter = None;
                        self.filter_focus = false;
                    }
                    InputOutcome::Ignored => {
                        let combo = KeyCombo::from_event(&k);
                        if let Some(&a) = self.keymap.actions_for(&combo).iter().find(|a| matches!(a, Action::Up | Action::Down)) {
                            self.run(a);
                        }
                    }
                    _ => {}
                }
            }
            return;
        }
        if k.code == KeyCode::Esc {
            if self.filter.is_some() {
                self.filter = None;
            } else {
                self.toasts.dismiss_all();
            }
            return;
        }
        let combo = KeyCombo::from_event(&k);
        let ctx = self.ctx();
        if let Some(&a) = self.keymap.actions_for(&combo).iter().find(|a| a.available(ctx)) {
            if a.is_toggle() {
                if let Some((last, at)) = self.last_toggle
                    && last == a && at.elapsed() < Duration::from_millis(250) {
                        return;
                    }
                self.last_toggle = Some((a, Instant::now()));
            }
            self.run(a);
        }
    }

    fn popup_key(&mut self, k: KeyEvent) {
        use KeyCode::*;
        let Some(mut p) = self.popup.take() else { return };
        let mut close = false;
        match &mut p {
            Popup::Help { scroll, max } => match k.code {
                Esc | Char('q') | Char('?') | Enter => close = true,
                Down | Char('j') => *scroll = (*scroll + 1).min(*max),
                Up | Char('k') => *scroll = scroll.saturating_sub(1),
                PageDown | Char(' ') => *scroll = (*scroll + 10).min(*max),
                PageUp => *scroll = scroll.saturating_sub(10),
                Home | Char('g') => *scroll = 0,
                End | Char('G') => *scroll = *max,
                _ => {}
            },
            Popup::Palette(pal) => match pal.handle_key(&k) {
                PaletteOutcome::Run(a) => {
                    self.run(a);
                    return;
                }
                PaletteOutcome::Close => close = true,
                PaletteOutcome::None => {}
            },
            Popup::Menu(m) => {
                let n = m.items.len().max(1);
                match k.code {
                    Esc | Char('q') => close = true,
                    Down | Char('j') | Tab => m.sel = (m.sel + 1) % n,
                    Up | Char('k') | BackTab => m.sel = (m.sel + n - 1) % n,
                    Enter | Char(' ') => {
                        if let Some(item) = m.items.get(m.sel).map(|x| x.2.clone()) {
                            self.menu_pick(item);
                        }
                        return;
                    }
                    _ => {
                        let combo = KeyCombo::from_event(&k);
                        let hit = m.items.iter().find_map(|x| match x.2 {
                            MenuItem::Run(a) if self.keymap.keys_for(a).contains(&combo) => Some(x.2.clone()),
                            _ => None,
                        });
                        if let Some(item) = hit {
                            self.menu_pick(item);
                            return;
                        }
                    }
                }
            }
            Popup::Confirm { yes_sel, on_yes, .. } => match k.code {
                Esc | Char('n') | Char('q') => close = true,
                Left | Right | Tab | BackTab | Char('h') | Char('l') => *yes_sel = !*yes_sel,
                Char('y') => {
                    let a = on_yes.clone();
                    self.confirmed(a);
                    close = true;
                }
                Enter => {
                    if *yes_sel {
                        let a = on_yes.clone();
                        self.confirmed(a);
                    }
                    close = true;
                }
                _ => {}
            },
            Popup::Dialog(d) => match d.handle_key(&k) {
                DialogKey::Submit => close = self.submit(d),
                DialogKey::Cancel => close = true,
                DialogKey::None => {}
            },
        }
        if !close && self.popup.is_none() {
            self.popup = Some(p);
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let hit = self.mouse.hit(m.column, m.row);
        match m.kind {
            MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                let d: isize = if m.kind == MouseEventKind::ScrollDown { 1 } else { -1 };
                match &mut self.popup {
                    Some(Popup::Help { scroll, max }) => *scroll = (*scroll as isize + d * 3).clamp(0, *max as isize) as u16,
                    Some(Popup::Menu(menu)) => menu.sel = (menu.sel as isize + d).clamp(0, menu.items.len() as isize - 1) as usize,
                    Some(_) => {}
                    None => match hit {
                        // Wheel over a volume bar changes the volume.
                        Some((_, Target::VolBar(s))) => {
                            let step = self.cfg.general.volume_step as f32;
                            self.change_volume(s, if d < 0 { step } else { -step });
                        }
                        _ => self.move_sel(d),
                    },
                }
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some((rect, t)) = hit else {
                    if matches!(self.popup, Some(Popup::Help { .. } | Popup::Menu(_) | Popup::Palette(_))) {
                        self.popup = None;
                    }
                    return;
                };
                let double = self.last_click.as_ref().is_some_and(|(at, prev)| *prev == t && at.elapsed() < Duration::from_millis(400));
                self.last_click = Some((Instant::now(), t.clone()));
                if let Target::VolBar(s) = t {
                    self.mix_sel.insert(self.tab, s);
                    self.drag = Some((s, rect));
                    self.volume_at(s, rect, m.column);
                    return;
                }
                self.click(t, double, m.column.saturating_sub(rect.x), (m.column, m.row));
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                if let Some((s, rect)) = self.drag {
                    self.volume_at(s, rect, m.column);
                }
            }
            MouseEventKind::Up(MouseButton::Left) => self.drag = None,
            MouseEventKind::Down(MouseButton::Right) => {
                if self.popup.is_some() {
                    return;
                }
                match hit {
                    Some((_, Target::Mix(s) | Target::VolBar(s))) => {
                        self.mix_sel.insert(self.tab, s);
                        self.open_menu(Some((m.column, m.row)));
                    }
                    Some((_, Target::Row(i))) => {
                        self.select_row(i);
                        self.open_menu(Some((m.column, m.row)));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn click(&mut self, t: Target, double: bool, rel_x: u16, at: (u16, u16)) {
        match t {
            Target::Tab(i) => self.tab = Tab::ALL[i.min(Tab::ALL.len() - 1)],
            Target::Mix(s) => {
                self.filter_focus = false;
                self.mix_sel.insert(self.tab, s);
                if double && s.channel.is_none() {
                    self.last_click = None;
                    let a = if self.ctx() == Ctx::Streams { Action::MoveTo } else { Action::SetDefault };
                    self.run(a);
                }
            }
            Target::MuteBtn(id) => {
                self.mix_sel.insert(self.tab, MixSel { node: id, channel: None });
                self.run(Action::Mute);
            }
            Target::DefaultBtn(id) => {
                self.mix_sel.insert(self.tab, MixSel { node: id, channel: None });
                self.run(Action::SetDefault);
            }
            Target::MoveBtn(id) => {
                self.mix_sel.insert(self.tab, MixSel { node: id, channel: None });
                self.open_move_menu(Some(at));
            }
            Target::Row(i) => {
                let again = self.row_index() == i;
                self.select_row(i);
                if double || (again && matches!(self.tab, Tab::Settings | Tab::Privacy)) {
                    self.last_click = None;
                    let a = match self.tab {
                        Tab::Cards => Action::CardProfile,
                        Tab::Settings | Tab::Privacy => Action::Change,
                        _ => return,
                    };
                    self.run(a);
                }
            }
            Target::Run(a) => {
                if matches!(self.popup, Some(Popup::Menu(_))) {
                    self.popup = None;
                } else if self.popup.is_some() {
                    return;
                }
                self.run(a);
            }
            Target::Confirm(h) => {
                if let Some(Popup::Confirm { on_yes, .. }) = self.popup.take()
                    && h == ConfirmHit::Yes {
                        self.confirmed(on_yes);
                    }
            }
            Target::PaletteRow(row) => {
                if let Some(Popup::Palette(p)) = &self.popup
                    && let Some(a) = p.value_at(row) {
                        self.popup = None;
                        self.run(a);
                    }
            }
            Target::MenuRow(row) => {
                if let Some(Popup::Menu(m)) = &self.popup
                    && let Some(item) = m.items.get(row).map(|x| x.2.clone()) {
                        self.menu_pick(item);
                    }
            }
            Target::DialogField(i) => {
                if let Some(Popup::Dialog(d)) = &mut self.popup {
                    d.click_field(i, rel_x);
                }
            }
            Target::DialogButton(ok) => {
                if let Some(Popup::Dialog(mut d)) = self.popup.take()
                    && ok && !self.submit(&mut d) {
                        self.popup = Some(Popup::Dialog(d));
                    }
            }
            Target::PendingBanner => self.run(Action::Apply),
            Target::ProfilePick { device, index } => self.send(Cmd::SetProfile { device, index }),
            Target::PortPick { device, index, dev } => self.send(Cmd::SetPort { device, route_index: index, route_device: dev }),
            Target::VolBar(_) => {}
        }
    }

    // ------------------------------------------------------------ volume

    fn max_pct(&self) -> f32 {
        self.cfg.general.max_volume.clamp(100, 200) as f32
    }

    /// Optimistically update our copy so held keys accumulate before the
    /// backend's snapshot arrives.
    fn local_set(&mut self, id: u32, vols: Vec<f32>, mute: Option<bool>) {
        let mute = mute.or_else(|| self.pending_vol.get(&id).and_then(|p| p.1));
        self.pending_vol.insert(id, (vols.clone(), mute, Instant::now()));
        self.overlay_vol(id, vols, mute);
    }

    fn reapply_pending_vol(&mut self) {
        let pending = std::mem::take(&mut self.pending_vol);
        for (id, (vols, mute, at)) in pending {
            let Some(n) = self.graph.nodes.get(&id) else { continue };
            let (cur, cur_mute) = self.graph.volume_of(n);
            let echoed = cur.len() == vols.len()
                && cur.iter().zip(&vols).all(|(a, b)| (linear_to_percent(*a) - linear_to_percent(*b)).abs() < 0.5)
                && mute.is_none_or(|m| m == cur_mute);
            if echoed || at.elapsed() > Duration::from_millis(600) {
                continue;
            }
            self.overlay_vol(id, vols.clone(), mute);
            self.pending_vol.insert(id, (vols, mute, at));
        }
    }

    fn overlay_vol(&mut self, id: u32, vols: Vec<f32>, mute: Option<bool>) {
        let Some(n) = self.graph.nodes.get(&id).cloned() else { return };
        let route = n.device_id.zip(n.profile_device);
        if let Some((d, pd)) = route {
            let dir = if n.kind() == NodeKind::Source { Dir::Input } else { Dir::Output };
            if let Some(r) = self.graph.devices.get_mut(&d).and_then(|dev| dev.routes.iter_mut().find(|r| r.device == pd && r.direction == dir))
                && !r.volumes.is_empty() {
                    r.volumes = vols;
                    if let Some(m) = mute {
                        r.mute = m;
                    }
                    return;
                }
        }
        if let Some(n) = self.graph.nodes.get_mut(&id) {
            n.volumes = vols;
            if let Some(m) = mute {
                n.mute = m;
            }
        }
    }

    pub fn change_volume(&mut self, s: MixSel, delta: f32) {
        let Some(n) = self.graph.nodes.get(&s.node) else { return };
        let (vols, _) = self.graph.volume_of(n);
        let max = self.max_pct();
        let new = match s.channel {
            Some(c) if c < vols.len() => {
                let mut v = vols.clone();
                v[c] = percent_to_linear((linear_to_percent(v[c]) + delta).clamp(0.0, max));
                v
            }
            _ => step_all(&vols, delta, max),
        };
        self.local_set(s.node, new.clone(), None);
        self.send(Cmd::SetVolume { node: s.node, volumes: new });
    }

    fn set_percent(&mut self, s: MixSel, pct: f32) {
        let Some(n) = self.graph.nodes.get(&s.node) else { return };
        let (vols, _) = self.graph.volume_of(n);
        let max = self.max_pct();
        let new = match s.channel {
            Some(c) if c < vols.len() => {
                let mut v = vols.clone();
                v[c] = percent_to_linear(pct.clamp(0.0, max));
                v
            }
            _ => set_all(vols.len(), pct, max),
        };
        self.local_set(s.node, new.clone(), None);
        self.send(Cmd::SetVolume { node: s.node, volumes: new });
    }

    fn volume_at(&mut self, s: MixSel, bar: Rect, col: u16) {
        let x = col.saturating_sub(bar.x) as f32 + 0.5;
        let pct = (x / bar.width.max(1) as f32 * self.max_pct()).clamp(0.0, self.max_pct());
        // Snap near 100% so it's easy to hit exactly.
        let pct = if (pct - 100.0).abs() < 1.5 { 100.0 } else { pct.round() };
        self.set_percent(s, pct);
    }

    // ------------------------------------------------------------ menus

    fn open_move_menu(&mut self, at: Option<(u16, u16)>) {
        let Some(n) = self.selected_node().cloned() else { return };
        if !n.kind().is_stream() {
            return;
        }
        let dev_kind = if n.kind() == NodeKind::Playback { NodeKind::Sink } else { NodeKind::Source };
        let current: Vec<u32> = self.graph.connected_devices(&n).iter().map(|d| d.id).collect();
        let follows = self.graph.follows_default(&n);
        let default_name = if dev_kind == NodeKind::Sink { &self.graph.default_sink } else { &self.graph.default_source };
        let default_title = default_name.as_ref().and_then(|d| self.graph.find_by_name(d)).map(|d| self.title(d)).unwrap_or_else(|| "none".into());
        let mut items = vec![(format!("Follow default ({default_title})"), "f".into(), MenuItem::Move { stream: n.id, target: None }, follows)];
        for d in self.graph.nodes_of(dev_kind) {
            items.push((self.title(d), String::new(), MenuItem::Move { stream: n.id, target: Some(d.id) }, !follows && current.contains(&d.id)));
        }
        let sel = items.iter().position(|x| x.3).unwrap_or(0);
        let title = if n.kind() == NodeKind::Playback { "Play through" } else { "Record from" };
        self.popup = Some(Popup::Menu(crate::app::Menu { title: title.into(), items, sel, at: at.unwrap_or((0, 0)) }));
    }

    fn open_port_menu(&mut self) {
        let Some(n) = self.selected_node().cloned() else { return };
        let Some((dev, cur)) = self.graph.route_for(&n).map(|(d, r)| (d.clone(), r.index)) else {
            return self.toasts.push(Level::Info, "This device has no selectable ports");
        };
        let pd = n.profile_device.unwrap_or(0);
        let items: Vec<(String, String, MenuItem, bool)> = dev
            .ports_for(pd)
            .into_iter()
            .map(|r| {
                let hint = match r.available {
                    Avail::No => "unplugged",
                    Avail::Yes => "plugged in",
                    Avail::Unknown => "",
                };
                (r.description.clone(), hint.into(), MenuItem::Port { device: dev.id, index: r.index, dev: pd }, r.index == cur)
            })
            .collect();
        if items.is_empty() {
            return self.toasts.push(Level::Info, "No other ports for this profile");
        }
        let sel = items.iter().position(|x| x.3).unwrap_or(0);
        self.popup = Some(Popup::Menu(crate::app::Menu { title: format!("{} — port", dev.title()), items, sel, at: (0, 0) }));
    }

    fn open_profile_menu(&mut self, device: u32) {
        let Some(dev) = self.graph.devices.get(&device).cloned() else { return };
        let mut profs = dev.profiles.clone();
        profs.sort_by_key(|p| (p.index == 0 && p.name == "off", -p.priority));
        let items: Vec<(String, String, MenuItem, bool)> = profs
            .iter()
            .map(|p| {
                let mut hint = String::new();
                if let Some(c) = p.codec() {
                    hint = codec_note(c).to_string();
                }
                if p.available == Avail::No {
                    hint = "unavailable".into();
                }
                (p.description.clone(), hint, MenuItem::Profile { device, index: p.index }, Some(p.index) == dev.active_profile)
            })
            .collect();
        let sel = items.iter().position(|x| x.3).unwrap_or(0);
        self.popup = Some(Popup::Menu(crate::app::Menu { title: format!("{} — profile", dev.title()), items, sel, at: (0, 0) }));
    }

    fn open_menu(&mut self, at: Option<(u16, u16)>) {
        use Action::*;
        let acts: Vec<Action> = match self.ctx() {
            Ctx::Streams => vec![MoveTo, FollowDefault, AlwaysHere, Mute, Channels, SetVolume, Alias],
            Ctx::Devices => vec![SetDefault, Mute, Port, Profile, Channels, SetVolume, Latency, Rename, Alias, Disable],
            Ctx::Cards => vec![CardProfile, Rename, Disable],
            Ctx::Virtual => vec![NewVirtual, DeleteVirtual, TempSink, Apply],
            Ctx::Settings => vec![Change, Reset, Apply],
            Ctx::Privacy => vec![Change, Forget, ForgetFile, Reload, Apply],
        };
        let items = acts.into_iter().map(|a| (self.action_label(a), self.keymap.hint(a).unwrap_or_default(), MenuItem::Run(a), false)).collect();
        self.popup = Some(Popup::Menu(crate::app::Menu { title: "Actions".into(), items, sel: 0, at: at.unwrap_or((0, 0)) }));
    }

    fn menu_pick(&mut self, item: MenuItem) {
        self.popup = None;
        match item {
            MenuItem::Run(a) => self.run(a),
            MenuItem::Move { stream, target } => {
                self.send(Cmd::MoveStream { stream, target });
            }
            MenuItem::Profile { device, index } => self.send(Cmd::SetProfile { device, index }),
            MenuItem::Port { device, index, dev } => self.send(Cmd::SetPort { device, route_index: index, route_device: dev }),
            MenuItem::NewVirtual(kind) => self.new_virtual_dialog(kind),
            MenuItem::Setting { key, value } => {
                self.send(Cmd::SetSetting { key: key.to_string(), value: value.clone() });
                let shown = value.filter(|v| v != "0").unwrap_or_else(|| "default".into());
                self.toasts.push(Level::Ok, format!("{key} → {shown} (until PipeWire restarts; Save to keep)"));
            }
        }
    }

    // ------------------------------------------------------------ dialogs

    fn dialog(&mut self, d: Dialog<Purpose>) {
        self.popup = Some(Popup::Dialog(Box::new(d)));
    }

    fn device_choices(&self, kind: NodeKind) -> Vec<(String, String)> {
        self.graph.nodes_of(kind).into_iter().filter(|n| !n.is_virtual()).map(|n| (n.name.clone(), self.title(n))).collect()
    }

    fn new_virtual_dialog(&mut self, kind: VirtualKind) {
        let sinks = self.device_choices(NodeKind::Sink);
        let sources = self.device_choices(NodeKind::Source);
        let default_sink = self.graph.default_sink.clone().unwrap_or_default();
        let default_source = self.graph.default_source.clone().unwrap_or_default();
        let base = |slug: &str, desc: &str| vec![Field::text("Name", slug), Field::text("Description", desc)];
        let (intro, fields): (&str, Vec<Field>) = match kind {
            VirtualKind::NullSink => ("An output that plays nowhere. Apps can play into it and OBS/Discord can record its monitor.", base("virtual_out", "Virtual output")),
            VirtualKind::Combine => {
                let mut f = base("combined", "All outputs");
                f.push(Field::text("Outputs", &sinks.iter().map(|s| s.0.clone()).collect::<Vec<_>>().join(", ")).placeholder("node names, comma separated"));
                ("Plays to several outputs at once (e.g. speakers + headphones).", f)
            }
            VirtualKind::Loopback => {
                let mut f = base("loopback", "Hear input");
                f.push(Field::choice("Listen to", sources.clone(), &default_source));
                f.push(Field::choice("Play through", sinks.clone(), &default_sink));
                ("Routes an input to an output so you can hear it (mic monitoring, line-in).", f)
            }
            VirtualKind::EchoCancel => {
                let mut f = base("echo_cancel", "Microphone (echo cancelled)");
                f.push(Field::choice("Microphone", sources.clone(), &default_source));
                ("A cleaned-up microphone that removes what your speakers play (WebRTC AEC).", f)
            }
            VirtualKind::Noise => {
                let mut f = base("noise_cancel", "Microphone (noise suppressed)");
                f.push(Field::choice("Microphone", sources.clone(), &default_source));
                ("A microphone with background noise removed (RNNoise; Arch package noise-suppression-for-voice).", f)
            }
            VirtualKind::Equalizer => {
                let mut f = base("eq", "Equalizer");
                f.push(Field::choice("Output", sinks.clone(), &default_sink));
                f.push(Field::text("Bands", "60:+3:0.7, 250:0, 1000:0, 4000:0, 8000:+1:0.7").placeholder("freq:gain[:q], …"));
                ("An output with an equalizer in front of a real one. Bands: freq Hz : gain dB : Q.", f)
            }
        };
        let d = Dialog::new(&format!("New {}", kind.label().to_lowercase()), intro, fields, "Add", Purpose::NewVirtual(kind));
        self.dialog(d);
    }

    /// Validate and act on a dialog. Returns true to close it.
    fn submit(&mut self, d: &mut Dialog<Purpose>) -> bool {
        match d.purpose.clone() {
            Purpose::Rename { key } => {
                let v = d.value(0).trim().to_string();
                let r = self.edit_config(|e| e.device(&key, "description", if v.is_empty() { None } else { Some(value(v.as_str())) }));
                if r {
                    self.toasts.push(Level::Info, "Renamed in config — press Ctrl-S to apply (restarts WirePlumber)");
                }
            }
            Purpose::Alias { key } => {
                let v = d.value(0);
                self.edit_config(|e| e.alias(&key, Some(&v)));
            }
            Purpose::Volume { node } => {
                let t = d.value(0).trim().trim_end_matches('%').to_string();
                let Ok(p) = t.parse::<f32>() else {
                    d.fail(0, "enter a number, e.g. 65");
                    return false;
                };
                if !(0.0..=self.max_pct()).contains(&p) {
                    d.fail(0, format!("0–{} (raise max_volume in config for more)", self.max_pct()));
                    return false;
                }
                let ch = self.current_mix().filter(|s| s.node == node).and_then(|s| s.channel);
                self.set_percent(MixSel { node, channel: ch }, p);
            }
            Purpose::Latency { node } => {
                let t = d.value(0).trim().trim_end_matches("ms").trim().to_string();
                let Ok(ms) = t.parse::<f64>() else {
                    d.fail(0, "milliseconds, e.g. 40 or -20");
                    return false;
                };
                self.send(Cmd::SetLatencyOffset { node, ns: (ms * 1_000_000.0) as i64 });
                self.toasts.push(Level::Ok, format!("Latency offset {ms} ms"));
            }
            Purpose::TempSink => {
                let name = d.value(0).trim().to_string();
                if !valid_name(&name) {
                    d.fail(0, "letters, digits, '_', '-', '.'");
                    return false;
                }
                let desc = d.value(1);
                self.send(Cmd::CreateNullSink { name: name.clone(), description: if desc.is_empty() { name } else { desc } });
            }
            Purpose::NewVirtual(kind) => {
                let name = d.value(0).trim().to_string();
                if !valid_name(&name) {
                    d.fail(0, "letters, digits, '_', '-', '.'");
                    return false;
                }
                if self.cfg.virtuals.iter().any(|v| v.name == name) || self.graph.find_by_name(&name).is_some() {
                    d.fail(0, "that name is already used");
                    return false;
                }
                let mut v = Virtual { kind, name, description: d.value(1), target: String::new(), source: String::new(), targets: vec![], bands: vec![], channels: 2 };
                match kind {
                    VirtualKind::NullSink => {}
                    VirtualKind::Combine => {
                        v.targets = d.value(2).split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                        if v.targets.len() < 2 {
                            d.fail(2, "list at least two outputs");
                            return false;
                        }
                    }
                    VirtualKind::Loopback => {
                        v.source = d.value(2);
                        v.target = d.value(3);
                    }
                    VirtualKind::EchoCancel | VirtualKind::Noise => {
                        v.target = d.value(2);
                        if kind == VirtualKind::Noise {
                            v.channels = 1;
                        }
                    }
                    VirtualKind::Equalizer => {
                        v.target = d.value(2);
                        match cfgedit::parse_bands(&d.value(3)) {
                            Ok(b) => v.bands = b,
                            Err(e) => {
                                d.fail(3, e);
                                return false;
                            }
                        }
                    }
                }
                if self.edit_config(|e| e.add_virtual(&v)) {
                    self.toasts.push(Level::Info, "Added — press Ctrl-S to create it (restarts PipeWire)");
                }
            }
            Purpose::SmValue { key } => {
                let v = d.value(0).trim().to_string();
                self.send(Cmd::SetSmSetting { key, value: v, persist: true });
            }
            Purpose::AllowedRates => {
                let mut rates = Vec::new();
                for p in d.value(0).split([',', ' ']).filter(|p| !p.is_empty()) {
                    match p.parse::<u32>() {
                        Ok(r) if (8000..=768_000).contains(&r) => rates.push(r),
                        _ => {
                            d.fail(0, format!("“{p}” isn't a sample rate"));
                            return false;
                        }
                    }
                }
                let mut a = toml_edit::Array::new();
                for r in &rates {
                    a.push(*r as i64);
                }
                self.edit_config(|e| e.set("clock", "allowed_rates", value(a)));
            }
            Purpose::Allowlist => {
                let list: Vec<String> = d.value(0).split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                self.edit_config(|e| e.set("privacy", "allowlist", Edit::string_list(&list)));
            }
        }
        true
    }

    /// Edit config.toml, reload, and recompute pending system changes.
    pub fn edit_config(&mut self, f: impl FnOnce(&mut Edit)) -> bool {
        let r = Edit::open(&self.cfg_path).and_then(|mut e| {
            f(&mut e);
            e.save(&self.cfg_path)
        });
        match r {
            Ok(cfg) => {
                let keys = self.cfg.keys.clone();
                self.cfg = cfg;
                self.cfg.keys = keys;
                self.refresh_pending();
                for e in self.config_errors.clone() {
                    self.toasts.push(Level::Warn, e);
                }
                true
            }
            Err(e) => {
                self.toasts.push(Level::Error, format!("Couldn't save config: {e}"));
                false
            }
        }
    }

    fn confirmed(&mut self, a: OnYes) {
        self.popup = None;
        match a {
            OnYes::Restart(services) => self.restart(services),
            OnYes::ForgetEntries { path, keys } => {
                let tx = self.app_tx.clone();
                let demo = self.demo;
                tokio::spawn(async move {
                    let edit = move || state::remove_entries(&path, &keys);
                    let r = if demo { edit().map_err(|e| e.to_string()) } else { crate::backend::sys::with_wireplumber_stopped(edit).await };
                    let _ = match r {
                        Ok(n) => tx.send(AppEv::Toast(Level::Ok, format!("Forgot {n} entr{}", if n == 1 { "y" } else { "ies" }))),
                        Err(e) => tx.send(AppEv::Toast(Level::Error, format!("Couldn't edit state: {e}"))),
                    };
                    let _ = tx.send(AppEv::ReloadState);
                });
            }
            OnYes::ForgetFiles(paths) => {
                let tx = self.app_tx.clone();
                let demo = self.demo;
                tokio::spawn(async move {
                    let edit = move || {
                        for p in &paths {
                            if p.exists() {
                                std::fs::remove_file(p)?;
                            }
                        }
                        Ok(paths.len())
                    };
                    let r = if demo { edit().map_err(|e: std::io::Error| e.to_string()) } else { crate::backend::sys::with_wireplumber_stopped(edit).await };
                    let _ = match r {
                        Ok(_) => tx.send(AppEv::Toast(Level::Ok, "Deleted — WirePlumber starts fresh".into())),
                        Err(e) => tx.send(AppEv::Toast(Level::Error, format!("Couldn't delete: {e}"))),
                    };
                    let _ = tx.send(AppEv::ReloadState);
                });
            }
            OnYes::Disable(name) => {
                if self.edit_config(|e| e.device(&name, "disabled", Some(value(true)))) {
                    self.toasts.push(Level::Info, "Disabled in config — press Ctrl-S to apply (restarts WirePlumber)");
                }
            }
            OnYes::RemoveVirtual(name) => {
                if self.edit_config(|e| e.remove_virtual(&name)) {
                    self.toasts.push(Level::Info, "Removed from config — press Ctrl-S to apply (restarts PipeWire)");
                }
            }
        }
    }

    fn restart(&mut self, services: Vec<Service>) {
        let tx = self.app_tx.clone();
        if self.demo {
            self.toasts.push(Level::Info, format!("Demo: would restart {:?}", services));
            return;
        }
        tokio::spawn(async move {
            let _ = match crate::backend::sys::restart(&services).await {
                Ok(m) => tx.send(AppEv::Toast(Level::Ok, m)),
                Err(e) => tx.send(AppEv::Toast(Level::Error, format!("Restart failed: {e}"))),
            };
        });
    }

    fn apply_system(&mut self) {
        let (files, errors) = conf::generate(&self.cfg, &self.dirs);
        for e in &errors {
            self.toasts.push(Level::Warn, e.clone());
        }
        match conf::apply(&files) {
            Ok(changes) if changes.is_empty() => self.toasts.push(Level::Info, "Everything is already applied"),
            Ok(changes) => {
                let mut services: Vec<Service> = changes.iter().map(|c| c.2).collect();
                services.sort();
                services.dedup();
                let files: Vec<String> = changes.iter().map(|c| c.0.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()).collect();
                self.toasts.push(Level::Ok, format!("Wrote {}", files.join(", ")));
                self.refresh_pending();
                let units: Vec<&str> = services.iter().filter_map(|s| s.unit()).collect();
                if units.is_empty() {
                    self.toasts.push(Level::Info, "Apps pick up the new rules next time they start");
                } else if self.cfg.general.confirm_restart {
                    self.popup = Some(Popup::Confirm {
                        title: "Restart audio services".into(),
                        msg: format!("Restart {} now so the changes take effect? Audio stops for about a second.", units.join(", ")),
                        yes: "Restart".into(),
                        yes_sel: true,
                        on_yes: OnYes::Restart(services),
                    });
                } else {
                    self.restart(services);
                }
            }
            Err(e) => self.toasts.push(Level::Error, format!("Couldn't write drop-ins: {e}")),
        }
    }

    // ------------------------------------------------------------ labels

    pub fn action_label(&self, a: Action) -> String {
        let n = self.selected_node();
        match a {
            Action::Mute => if n.map(|n| self.graph.volume_of(n).1).unwrap_or(false) { "Unmute" } else { "Mute" }.into(),
            Action::Channels => if n.is_some_and(|n| self.expanded.contains(&n.id)) { "Hide channels" } else { "Per-channel volume" }.into(),
            Action::MoveTo => if self.tab == Tab::Recording { "Record from…" } else { "Play through…" }.into(),
            Action::Rename => "Rename (system)".into(),
            Action::Disable => "Disable (system)".into(),
            Action::Alias => "Display name".into(),
            other => other.description().into(),
        }
    }

    pub fn short_label(&self, a: Action) -> String {
        use Action::*;
        match a {
            VolDown => "−".into(),
            VolUp => "+".into(),
            Mute => self.action_label(a).to_lowercase(),
            MoveTo => "send to".into(),
            SetDefault => "default".into(),
            Port => "port".into(),
            Profile | CardProfile => "profile".into(),
            Rename => "rename".into(),
            Disable => "disable".into(),
            Channels => "channels".into(),
            Menu => "menu".into(),
            Help => "help".into(),
            NewVirtual => "new".into(),
            DeleteVirtual => "remove".into(),
            TempSink => "temporary".into(),
            Apply => "apply".into(),
            Change => "change".into(),
            Reset => "reset".into(),
            Forget => "forget".into(),
            ForgetFile => "forget file".into(),
            Down => "scroll".into(),
            Up => "up".into(),
            other => other.name().replace('_', " "),
        }
    }

    // ------------------------------------------------------------ run

    pub fn run(&mut self, a: Action) {
        use Action::*;
        if matches!(self.popup, Some(Popup::Menu(_)) | Some(Popup::Palette(_))) {
            self.popup = None;
        }
        let step = self.cfg.general.volume_step as f32;
        match a {
            Quit => self.quit = true,
            Help => self.popup = Some(Popup::Help { scroll: 0, max: 0 }),
            Palette => {
                let ctx = self.ctx();
                let entries = Action::all()
                    .iter()
                    .copied()
                    .filter(|a| a.available(ctx) && !matches!(a, Palette | Up | Down | Top | Bottom | PageUp | PageDown))
                    .map(|a| Entry { label: self.action_label(a), keys: self.keymap.hint(a).unwrap_or_default(), value: a })
                    .collect();
                self.popup = Some(Popup::Palette(crate::ui::palette::Palette::new(entries)));
            }
            NextTab | PrevTab => {
                let i = Tab::ALL.iter().position(|t| *t == self.tab).unwrap_or(0);
                let n = Tab::ALL.len();
                self.tab = Tab::ALL[if a == NextTab { (i + 1) % n } else { (i + n - 1) % n }];
                self.on_tab_change();
            }
            Tab1 | Tab2 | Tab3 | Tab4 | Tab5 | Tab6 | Tab7 | Tab8 => {
                let i = [Tab1, Tab2, Tab3, Tab4, Tab5, Tab6, Tab7, Tab8].iter().position(|x| *x == a).unwrap_or(0);
                self.tab = Tab::ALL[i];
                self.on_tab_change();
            }
            Up => self.move_sel(-1),
            Down => self.move_sel(1),
            Top => self.move_sel(-100_000),
            Bottom => self.move_sel(100_000),
            PageUp => self.move_sel(-(self.list_height.max(2) as isize - 1)),
            PageDown => self.move_sel(self.list_height.max(2) as isize - 1),
            Filter => {
                if self.filter.is_none() {
                    self.filter = Some(TextInput::default().with_placeholder("app, song or device"));
                }
                self.filter_focus = true;
            }
            Menu => self.open_menu(None),
            Details => self.show_details = !self.show_details,
            Apply => self.apply_system(),

            VolUp | VolDown | VolUpFine | VolDownFine => {
                let Some(s) = self.current_mix() else { return };
                let d = match a {
                    VolUp => step,
                    VolDown => -step,
                    VolUpFine => 1.0,
                    _ => -1.0,
                };
                self.change_volume(s, d);
            }
            SetVolume => {
                let Some(n) = self.selected_node().cloned() else { return };
                let (v, _) = self.graph.volume_of(&n);
                let cur = format!("{:.0}", avg_percent(&v));
                let d = Dialog::new("Set volume", &format!("{} — percent (0–{})", self.title(&n), self.max_pct()), vec![Field::text("Volume %", &cur)], "Set", Purpose::Volume { node: n.id });
                self.dialog(d);
            }
            Mute => {
                let Some(n) = self.selected_node().cloned() else { return };
                let (v, m) = self.graph.volume_of(&n);
                self.local_set(n.id, v, Some(!m));
                self.send(Cmd::SetMute { node: n.id, mute: !m });
            }
            Channels => {
                let Some(n) = self.selected_node().map(|n| n.id) else { return };
                if !self.expanded.remove(&n) {
                    self.expanded.insert(n);
                }
                self.mix_sel.insert(self.tab, MixSel { node: n, channel: None });
            }
            Alias => {
                let Some(n) = self.selected_node().cloned() else { return };
                let key = if n.kind().is_stream() && !n.app_name.is_empty() { n.app_name.clone() } else { n.name.clone() };
                let cur = self.cfg.aliases.get(&key).cloned().unwrap_or_default();
                let d = Dialog::new("Display name", &format!("Only changes how audio-tui shows “{}”. Empty = original.", n.title()), vec![Field::text("Name", &cur)], "Save", Purpose::Alias { key });
                self.dialog(d);
            }
            MoveTo => self.open_move_menu(None),
            FollowDefault => {
                let Some(n) = self.selected_node().cloned() else { return };
                self.send(Cmd::MoveStream { stream: n.id, target: None });
                self.toasts.push(Level::Ok, format!("{} follows the default device", self.title(&n)));
            }
            AlwaysHere => {
                let Some(n) = self.selected_node().cloned() else { return };
                let Some(dev) = self.graph.connected_devices(&n).first().map(|d| (d.name.clone(), self.title(d))) else {
                    return self.toasts.push(Level::Info, "Not connected to a device right now");
                };
                let app = if n.app_name.is_empty() { n.app_binary.clone() } else { n.app_name.clone() };
                if app.is_empty() {
                    return self.toasts.push(Level::Warn, "This stream doesn't say which app it is");
                }
                if self.edit_config(|e| e.add_route(&RouteRule { app: app.clone(), target: dev.0.clone() })) {
                    self.toasts.push(Level::Info, format!("{app} → {} saved — Ctrl-S to apply", dev.1));
                }
            }
            SetDefault => {
                let Some(n) = self.selected_node().cloned() else { return };
                self.send(Cmd::SetDefault { node: n.id });
            }
            Port => self.open_port_menu(),
            Profile => {
                let Some(d) = self.selected_node().and_then(|n| n.device_id) else {
                    return self.toasts.push(Level::Info, "Virtual devices have no profiles");
                };
                self.open_profile_menu(d);
            }
            CardProfile => match self.card_rows().get(self.row_index()) {
                Some(CardRow::Device(id)) => self.open_profile_menu(*id),
                Some(CardRow::Disabled(name)) => {
                    let name = name.clone();
                    if self.edit_config(|e| e.device(&name, "disabled", None)) {
                        self.toasts.push(Level::Info, "Re-enabled in config — Ctrl-S to apply");
                    }
                }
                None => {}
            },
            Rename => {
                let (key, cur, what) = match self.ctx() {
                    Ctx::Cards => match self.card_rows().get(self.row_index()) {
                        Some(CardRow::Device(id)) => match self.graph.devices.get(id) {
                            Some(d) => (d.name.clone(), d.description.clone(), "card"),
                            None => return,
                        },
                        _ => return,
                    },
                    _ => match self.selected_node() {
                        Some(n) => (n.name.clone(), n.description.clone(), "device"),
                        None => return,
                    },
                };
                let d = Dialog::new(
                    &format!("Rename {what}"),
                    "Renames it everywhere (pavucontrol, browsers, games…) through a WirePlumber rule. Empty = original name.",
                    vec![Field::text("New name", &cur)],
                    "Rename",
                    Purpose::Rename { key },
                );
                self.dialog(d);
            }
            Disable => {
                let (name, title) = match self.ctx() {
                    Ctx::Cards => match self.card_rows().get(self.row_index()) {
                        Some(CardRow::Device(id)) => match self.graph.devices.get(id) {
                            Some(d) => (d.name.clone(), d.title().to_string()),
                            None => return,
                        },
                        _ => return,
                    },
                    _ => match self.selected_node() {
                        Some(n) if n.device_id.is_some() => (n.name.clone(), self.title(n)),
                        Some(_) => return self.toasts.push(Level::Info, "Remove virtual devices in the Virtual tab"),
                        None => return,
                    },
                };
                self.popup = Some(Popup::Confirm {
                    title: "Disable device".into(),
                    msg: format!("Hide “{title}” from the whole system? Re-enable it later from the Cards tab."),
                    yes: "Disable".into(),
                    yes_sel: false,
                    on_yes: OnYes::Disable(name),
                });
            }
            Latency => {
                let Some(n) = self.selected_node().cloned() else { return };
                let cur = self.graph.route_for(&n).map(|(_, r)| r.latency_offset_ns).unwrap_or(0) as f64 / 1e6;
                let d = Dialog::new(
                    "Latency offset",
                    "Delay compensation in milliseconds, e.g. to sync Bluetooth audio with video. Positive = audio is late.",
                    vec![Field::text("Offset (ms)", &format!("{cur}"))],
                    "Set",
                    Purpose::Latency { node: n.id },
                );
                self.dialog(d);
            }

            NewVirtual => {
                let items = VirtualKind::ALL.iter().map(|k| (k.label().to_string(), String::new(), MenuItem::NewVirtual(*k), false)).collect();
                self.popup = Some(Popup::Menu(crate::app::Menu { title: "New virtual device".into(), items, sel: 0, at: (0, 0) }));
            }
            DeleteVirtual => match self.virt_rows().get(self.row_index()) {
                Some(VirtRow::Config(i)) => {
                    let name = self.cfg.virtuals[*i].name.clone();
                    self.popup = Some(Popup::Confirm {
                        title: "Remove virtual device".into(),
                        msg: format!("Remove “{name}” from your config?"),
                        yes: "Remove".into(),
                        yes_sel: false,
                        on_yes: OnYes::RemoveVirtual(name),
                    });
                }
                Some(VirtRow::Live(id)) => self.send(Cmd::DestroyNode { id: *id }),
                None => {}
            },
            TempSink => {
                let d = Dialog::new(
                    "Temporary virtual output",
                    "Created right now, gone when PipeWire restarts. Use “n” for a permanent one.",
                    vec![Field::text("Name", "temp_out"), Field::text("Description", "Temporary output")],
                    "Create",
                    Purpose::TempSink,
                );
                self.dialog(d);
            }

            Change => match self.tab {
                Tab::Settings => self.change_setting(),
                Tab::Privacy => self.change_privacy(),
                _ => {}
            },
            Reset => {
                if let Some(row) = self.set_rows().get(self.row_index()).cloned() {
                    match row {
                        SetRow::Rate => self.menu_pick(MenuItem::Setting { key: "clock.force-rate", value: Some("0".into()) }),
                        SetRow::Quantum => self.menu_pick(MenuItem::Setting { key: "clock.force-quantum", value: Some("0".into()) }),
                        SetRow::AllowedRates => {
                            self.edit_config(|e| e.set("clock", "allowed_rates", value(toml_edit::Array::new())));
                        }
                        _ => {}
                    }
                }
            }
            Forget => {
                let rows = self.priv_rows();
                if let Some(PrivRow::Entry(fi, ei)) = rows.get(self.row_index()) {
                    let f = &self.state_files[*fi];
                    let e = &f.entries[*ei];
                    self.popup = Some(Popup::Confirm {
                        title: "Forget entry".into(),
                        msg: format!("Forget “{}”? WirePlumber restarts briefly to apply this.", e.key),
                        yes: "Forget".into(),
                        yes_sel: true,
                        on_yes: OnYes::ForgetEntries { path: f.path.clone(), keys: vec![e.raw_key.clone()] },
                    });
                }
            }
            ForgetFile => {
                let rows = self.priv_rows();
                let fi = match rows.get(self.row_index()) {
                    Some(PrivRow::Entry(fi, _)) | Some(PrivRow::File(fi)) => *fi,
                    _ => return self.toasts.push(Level::Info, "Select a remembered entry first"),
                };
                let f = &self.state_files[fi];
                self.popup = Some(Popup::Confirm {
                    title: "Forget file".into(),
                    msg: format!("Delete {} ({} entries: {})? WirePlumber restarts briefly.", f.name, f.entries.len(), f.description()),
                    yes: "Delete".into(),
                    yes_sel: false,
                    on_yes: OnYes::ForgetFiles(vec![f.path.clone()]),
                });
            }
            Reload => {
                self.reload_state();
                self.toasts.push(Level::Info, "Re-read WirePlumber's state");
            }
        }
    }

    fn on_tab_change(&mut self) {
        if self.tab == Tab::Privacy {
            self.reload_state();
        }
        if self.tab.kind().is_none() && !self.row_sel.contains_key(&self.tab) {
            self.select_row(0);
            if self.tab == Tab::Privacy || self.tab == Tab::Settings {
                self.move_sel(0);
            }
        }
    }

    fn change_setting(&mut self) {
        let Some(row) = self.set_rows().get(self.row_index()).cloned() else { return };
        match row {
            SetRow::Rate => {
                let cur = self.graph.settings.get("clock.force-rate").cloned().unwrap_or_default();
                let mut items = vec![("PipeWire decides".to_string(), "default".to_string(), MenuItem::Setting { key: "clock.force-rate", value: Some("0".into()) }, cur == "0" || cur.is_empty())];
                for r in [44100, 48000, 88200, 96000, 176400, 192000] {
                    let label = format!("{:.1} kHz", r as f32 / 1000.0);
                    items.push((label, String::new(), MenuItem::Setting { key: "clock.force-rate", value: Some(r.to_string()) }, cur == r.to_string()));
                }
                let sel = items.iter().position(|x| x.3).unwrap_or(0);
                self.popup = Some(Popup::Menu(crate::app::Menu { title: "Sample rate (now)".into(), items, sel, at: (0, 0) }));
            }
            SetRow::Quantum => {
                let cur = self.graph.settings.get("clock.force-quantum").cloned().unwrap_or_default();
                let rate = self.graph.clock_rate().unwrap_or(48000) as f32;
                let mut items = vec![("PipeWire decides".to_string(), "default".to_string(), MenuItem::Setting { key: "clock.force-quantum", value: Some("0".into()) }, cur == "0" || cur.is_empty())];
                for q in [64u32, 128, 256, 512, 1024, 2048, 4096] {
                    let ms = q as f32 / rate * 1000.0;
                    let hint = match q {
                        64 | 128 => "very low latency, more CPU",
                        256 | 512 => "low latency",
                        1024 => "balanced",
                        _ => "saves power",
                    };
                    items.push((format!("{q} samples ({ms:.1} ms)"), hint.into(), MenuItem::Setting { key: "clock.force-quantum", value: Some(q.to_string()) }, cur == q.to_string()));
                }
                let sel = items.iter().position(|x| x.3).unwrap_or(0);
                self.popup = Some(Popup::Menu(crate::app::Menu { title: "Buffer size (now)".into(), items, sel, at: (0, 0) }));
            }
            SetRow::AllowedRates => {
                let cur = self.cfg.clock.allowed_rates.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ");
                let d = Dialog::new(
                    "Allowed sample rates",
                    "Let PipeWire switch rate to match what's playing (avoids resampling). Empty = always the default rate.",
                    vec![Field::text("Rates", &cur).placeholder("44100, 48000, 96000")],
                    "Save",
                    Purpose::AllowedRates,
                );
                self.dialog(d);
            }
            SetRow::SaveClock => {
                let rate: u32 = self.graph.settings.get("clock.force-rate").and_then(|v| v.parse().ok()).unwrap_or(0);
                let q: u32 = self.graph.settings.get("clock.force-quantum").and_then(|v| v.parse().ok()).unwrap_or(0);
                if self.edit_config(|e| {
                    e.set("clock", "rate", value(rate as i64));
                    e.set("clock", "quantum", value(q as i64));
                }) {
                    self.toasts.push(Level::Info, "Saved to [clock] — Ctrl-S writes the PipeWire drop-in");
                }
            }
            SetRow::MaxVolume => {
                let next = match self.cfg.general.max_volume {
                    0..=100 => 125,
                    101..=125 => 150,
                    _ => 100,
                };
                self.edit_config(|e| e.set("general", "max_volume", value(next as i64)));
            }
            SetRow::VolumeStep => {
                let next = match self.cfg.general.volume_step {
                    0..=1 => 2,
                    2..=4 => 5,
                    5..=9 => 10,
                    _ => 1,
                };
                self.edit_config(|e| e.set("general", "volume_step", value(next as i64)));
            }
            SetRow::Meters => {
                let on = !self.cfg.general.meters;
                self.edit_config(|e| e.set("general", "meters", value(on)));
            }
            SetRow::Sm(key) => {
                let cur = self.graph.sm_settings.get(&key).cloned().unwrap_or_default();
                match cur.as_str() {
                    "true" | "false" => {
                        let v = if cur == "true" { "false" } else { "true" };
                        self.send(Cmd::SetSmSetting { key, value: v.into(), persist: true });
                    }
                    _ => {
                        let d = Dialog::new("WirePlumber setting", &key, vec![Field::text("Value", &cur)], "Save", Purpose::SmValue { key: key.clone() });
                        self.dialog(d);
                    }
                }
            }
        }
    }

    fn change_privacy(&mut self) {
        let rows = self.priv_rows();
        let Some(row) = rows.get(self.row_index()).cloned() else { return };
        match row {
            PrivRow::RememberVolumes => {
                let on = !self.cfg.privacy.remember_volumes;
                if self.edit_config(|e| e.set("privacy", "remember_volumes", value(on))) {
                    self.apply_privacy_now();
                }
            }
            PrivRow::RememberDevices => {
                let on = !self.cfg.privacy.remember_devices;
                if self.edit_config(|e| e.set("privacy", "remember_devices", value(on))) {
                    self.apply_privacy_now();
                }
            }
            PrivRow::Mode => {
                let next = match self.cfg.privacy.mode {
                    PrivacyMode::All => "allowlist",
                    PrivacyMode::Allowlist => "none",
                    PrivacyMode::None => "all",
                };
                if self.edit_config(|e| e.set("privacy", "mode", value(next))) {
                    self.apply_privacy_now();
                    if next == "allowlist" && self.cfg.privacy.allowlist.is_empty() {
                        self.toasts.push(Level::Info, "Add the apps WirePlumber may remember on the Allowlist row (Ctrl-S applies per-app rules)");
                    }
                }
            }
            PrivRow::Allowlist => {
                let mut apps: Vec<String> = self.graph.nodes.values().filter(|n| n.kind().is_stream() && !n.app_name.is_empty() && !self.is_internal(n)).map(|n| n.app_name.clone()).collect();
                apps.sort();
                apps.dedup();
                let d = Dialog::new(
                    "Remember only these apps",
                    &format!("Comma-separated app names. Running now: {}", if apps.is_empty() { "none".into() } else { apps.join(", ") }),
                    vec![Field::text("Apps", &self.cfg.privacy.allowlist.join(", "))],
                    "Save",
                    Purpose::Allowlist,
                );
                self.dialog(d);
            }
            PrivRow::Prune => {
                let on = !self.cfg.privacy.prune_titles_on_start;
                self.edit_config(|e| e.set("privacy", "prune_titles_on_start", value(on)));
            }
            PrivRow::ForgetAll => {
                let paths: Vec<PathBuf> = self.state_files.iter().map(|f| f.path.clone()).collect();
                let n: usize = self.state_files.iter().map(|f| f.entries.len()).sum();
                let after = if self.remembers_apps() != (false, false) { "Apps will be remembered again from now on." } else { "App memory is off, so nothing per-app comes back." };
                self.popup = Some(Popup::Confirm {
                    title: "Forget everything".into(),
                    msg: format!(
                        "Delete all {} WirePlumber state files ({n} entries): app volumes and devices, default-device history, card ports and volumes. Your default output and card volumes reset until you pick them again. {after} WirePlumber restarts briefly.",
                        paths.len()
                    ),
                    yes: "Forget all".into(),
                    yes_sel: false,
                    on_yes: OnYes::ForgetFiles(paths),
                });
            }
            PrivRow::File(fi) => {
                let name = self.state_files[fi].name.clone();
                if !self.priv_open.remove(&name) {
                    self.priv_open.insert(name);
                }
            }
            PrivRow::Entry(..) => self.run(Action::Forget),
        }
    }

    pub fn remembers_apps(&self) -> (bool, bool) {
        let p = &self.cfg.privacy;
        let on = p.mode != PrivacyMode::None;
        (p.remember_volumes && on, p.remember_devices && on)
    }

    /// Privacy switches take effect at once: live via WirePlumber's settings
    /// metadata, and permanently via the privacy drop-in (no restart needed).
    fn apply_privacy_now(&mut self) {
        let (props, target) = self.remembers_apps();
        self.send(Cmd::SetSmSetting { key: "node.stream.restore-props".into(), value: props.to_string(), persist: false });
        self.send(Cmd::SetSmSetting { key: "node.stream.restore-target".into(), value: target.to_string(), persist: false });
        if self.demo {
            return;
        }
        let (files, _) = conf::generate(&self.cfg, &self.dirs);
        let privacy: Vec<_> = files.into_iter().filter(|f| f.topic == "privacy").collect();
        if let Err(e) = conf::apply(&privacy) {
            self.toasts.push(Level::Error, format!("Couldn't write privacy drop-in: {e}"));
        }
        self.refresh_pending();
    }

    /// Delete entries keyed by media titles (privacy.prune_titles_on_start).
    pub fn prune_titles(&mut self) {
        let mut jobs = Vec::new();
        for f in &self.state_files {
            let keys: Vec<String> = f.entries.iter().filter(|e| e.leaks_title()).map(|e| e.raw_key.clone()).collect();
            if !keys.is_empty() {
                jobs.push((f.path.clone(), keys));
            }
        }
        if jobs.is_empty() {
            return;
        }
        let tx = self.app_tx.clone();
        let demo = self.demo;
        tokio::spawn(async move {
            let edit = move || {
                let mut n = 0;
                for (p, k) in &jobs {
                    n += state::remove_entries(p, k)?;
                }
                Ok(n)
            };
            let r = if demo { edit().map_err(|e: std::io::Error| e.to_string()) } else { crate::backend::sys::with_wireplumber_stopped(edit).await };
            if let Ok(n) = r {
                let _ = tx.send(AppEv::Toast(Level::Ok, format!("Pruned {n} title-keyed entries from WirePlumber's memory")));
            }
            let _ = tx.send(AppEv::ReloadState);
        });
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Peak (linear) to meter position 0..1 on a -60..0 dB scale.
pub fn meter_scale(peak: f32) -> f32 {
    if peak <= 0.001 {
        return 0.0;
    }
    ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.05)
}

pub fn codec_note(c: &str) -> &'static str {
    match c {
        "sbc" => "basic quality, works everywhere",
        "sbc_xq" => "better SBC",
        "aac" => "good quality (Apple)",
        "aptx" => "good quality",
        "aptx_hd" => "high quality",
        "aptx_ll" => "low latency",
        "ldac" => "best quality, more battery",
        "lc3" => "LE Audio",
        "msbc" => "wideband call audio",
        "cvsd" => "narrowband call audio",
        "opus_05" | "opus_g" => "Opus",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_scale_is_db() {
        assert_eq!(meter_scale(0.0), 0.0);
        assert!((meter_scale(1.0) - 1.0).abs() < 1e-6);
        assert!((meter_scale(0.1) - (40.0 / 60.0)).abs() < 1e-4);
    }
}
