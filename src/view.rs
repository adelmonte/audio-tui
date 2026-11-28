//! Rendering for audio-tui.

use ratatui::{
    layout::{Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::action::Action;
use crate::app::{App, CardRow, MixSel, Popup, PrivRow, SetRow, Tab, Target, VirtRow};
use crate::backend::model::*;
use crate::backend::state::EntryKind;
use crate::config::{IconSet, PrivacyMode, VirtualKind};
use crate::core::keymap::ActionSet;
use crate::ui::theme::Theme;
use crate::ui::widgets::{self, centered, checkbox, empty_state, fit_line, panel, popup, spinner, toggle, truncate, HelpRow};

pub fn render(app: &mut App, f: &mut Frame) {
    app.mouse.clear();
    app.visible.clear();
    let area = f.area();
    let [header, body, footer] = Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(area);
    render_header(app, f, header);
    if !app.loaded {
        empty_state(f, body, &app.theme, &format!("{} Connecting to PipeWire…", spinner(app.tick)), &[]);
    } else if let (Some(m), true) = (app.disconnected.clone(), app.tab.kind().is_some() || app.tab == Tab::Cards) {
        empty_state(f, body, &app.theme, "PipeWire is not reachable", &[&m, "", "The Settings and Privacy tabs still work."]);
    } else {
        match app.tab {
            Tab::Playback | Tab::Recording | Tab::Outputs | Tab::Inputs => render_mixer(app, f, body),
            Tab::Cards => render_cards(app, f, body),
            Tab::Virtual => render_virtual(app, f, body),
            Tab::Settings => render_settings(app, f, body),
            Tab::Privacy => render_privacy(app, f, body),
        }
    }
    render_footer(app, f, footer);
    app.toasts.render(f, body, &app.theme);
    render_popup(app, f, area);
}

fn render_header(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme.clone();
    let mut right: Vec<Span> = Vec::new();
    if !app.pending.is_empty() {
        let k = app.keymap.hint(Action::Apply).unwrap_or_default();
        right.push(Span::styled(format!(" ● {} unapplied · {k} ", app.pending.len()), t.warn().add_modifier(Modifier::REVERSED)));
        right.push(Span::raw(" "));
    }
    if app.demo {
        right.push(Span::styled(" DEMO ", Style::default().add_modifier(Modifier::REVERSED)));
        right.push(Span::raw(" "));
    }
    let tabs_full: u16 = Tab::ALL.iter().map(|t| t.title().width() as u16 + 3).sum::<u16>() + 1;
    let room_for_clock = area.width.saturating_sub(tabs_full) > right.iter().map(|s| s.content.width() as u16).sum::<u16>() + 20;
    if app.disconnected.is_some() {
        right.push(Span::styled("○ disconnected ", t.error()));
    } else if let (Some(r), true) = (app.graph.clock_rate(), room_for_clock) {
        let q: u32 = app.graph.settings.get("clock.quantum").and_then(|v| v.parse().ok()).unwrap_or(0);
        right.push(Span::styled(format!("{:.1} kHz", r as f32 / 1000.0), t.dim()));
        if q > 0 {
            right.push(Span::styled(format!(" · {:.1} ms ", q as f32 / r as f32 * 1000.0), t.dim()));
        } else {
            right.push(Span::raw(" "));
        }
    }
    let rw: u16 = right.iter().map(|s| s.content.width() as u16).sum();
    let titles: Vec<&str> = Tab::ALL.iter().map(|t| t.title()).collect();
    let sel = Tab::ALL.iter().position(|x| *x == app.tab).unwrap_or(0);
    let tabs_area = Rect { width: area.width.saturating_sub(rw), ..area };
    widgets::tabs(f, tabs_area, &titles, sel, &t, &mut app.mouse, Target::Tab);
    if rw <= area.width {
        let r = Rect { x: area.right() - rw, width: rw, ..area };
        f.render_widget(Line::from(right), r);
        if !app.pending.is_empty() {
            app.mouse.add(Rect { width: 22.min(rw), ..r }, Target::PendingBanner);
        }
    }
}

fn render_footer(app: &mut App, f: &mut Frame, area: Rect) {
    let items: Vec<(String, String, Target)> = Action::footer(app.ctx())
        .iter()
        .filter_map(|&a| app.keymap.hint(a).map(|k| (k, app.short_label(a), Target::Run(a))))
        .collect();
    let area = Rect { x: area.x + 1, width: area.width.saturating_sub(2), ..area };
    widgets::hints(f, area, &items, &app.theme, &mut app.mouse);
}

fn split(app: &App, area: Rect, w: u16) -> (Rect, Option<Rect>) {
    if app.show_details && area.width >= 64 + w {
        let [l, d] = Layout::horizontal([Constraint::Min(50), Constraint::Length(w)]).areas(area);
        (l, Some(d))
    } else {
        (area, None)
    }
}

// ---------------------------------------------------------------- icons

fn node_icon(app: &App, n: &Node) -> &'static str {
    let nerd = app.cfg.general.icons == IconSet::Nerd;
    let ascii = app.cfg.general.icons == IconSet::Ascii;
    if ascii {
        return match n.kind() {
            NodeKind::Sink => ">",
            NodeKind::Source => "<",
            _ => "*",
        };
    }
    let app_name = format!("{} {}", n.app_name, n.app_binary).to_lowercase();
    match n.kind() {
        NodeKind::Playback | NodeKind::Recording if nerd => {
            if app_name.contains("firefox") {
                "󰈹"
            } else if app_name.contains("chrom") {
                "󰊯"
            } else if app_name.contains("spotify") {
                "󰓇"
            } else if app_name.contains("discord") {
                "󰙯"
            } else if app_name.contains("obs") {
                "󰑋"
            } else if app_name.contains("mpv") || app_name.contains("vlc") {
                "󰕼"
            } else if n.media_role == "Notification" || n.props.get("media.role").is_some_and(|r| r == "Notification") {
                "󰂚"
            } else if n.kind() == NodeKind::Recording {
                "󰍬"
            } else {
                "󰐊"
            }
        }
        NodeKind::Playback => "▶",
        NodeKind::Recording => "●",
        NodeKind::Sink | NodeKind::Source if nerd => {
            let name = n.name.to_lowercase();
            if n.is_virtual() {
                "󰡀"
            } else if n.is_bluetooth() {
                "󰋋"
            } else if name.contains("hdmi") {
                "󰡁"
            } else if n.kind() == NodeKind::Source {
                if name.contains("usb") { "󰍬" } else { "󰍮" }
            } else if name.contains("usb") {
                "󰓃"
            } else {
                "󰕾"
            }
        }
        NodeKind::Sink => "♪",
        NodeKind::Source => "◉",
        NodeKind::Other => "·",
    }
}

fn mute_icon(app: &App, muted: bool) -> &'static str {
    match (app.cfg.general.icons, muted) {
        (IconSet::Nerd, false) => "󰕾",
        (IconSet::Nerd, true) => "󰖁",
        (_, false) => "♪",
        (_, true) => "M",
    }
}

// ---------------------------------------------------------------- bars & meters

fn level_color(t: &Theme, frac: f32) -> Color {
    if t.mono {
        Color::Reset
    } else if frac < 0.75 {
        t.ok
    } else if frac < 0.92 {
        t.warn
    } else {
        t.error
    }
}

/// Volume slider: `━━━━━━━●──────┆──`. Returns nothing; caller registers the rect.
fn volume_bar(f: &mut Frame, r: Rect, t: &Theme, pct: f32, max: f32, muted: bool, focused: bool) {
    let w = r.width as usize;
    if w == 0 {
        return;
    }
    let pos = ((pct / max) * w as f32).round().clamp(0.0, w as f32) as usize;
    let mark100 = if max > 100.0 { Some(((100.0 / max) * w as f32).round() as usize) } else { None };
    let fill_style = if muted {
        t.dim().add_modifier(Modifier::CROSSED_OUT)
    } else if pct > 100.0 {
        t.warn()
    } else {
        t.accent()
    };
    let mut spans = Vec::with_capacity(w);
    for i in 0..w {
        let (ch, st) = if i + 1 == pos && pos > 0 {
            (if focused { "●" } else { "━" }, fill_style.add_modifier(Modifier::BOLD))
        } else if i < pos {
            ("━", fill_style)
        } else if Some(i) == mark100 {
            ("┆", t.dim())
        } else {
            ("─", t.border(false))
        };
        spans.push(Span::styled(ch, st));
    }
    f.render_widget(Line::from(spans), r);
}

/// Two-channel meter in one row using half blocks: top = left, bottom = right.
fn meter(f: &mut Frame, r: Rect, t: &Theme, levels: &[f32], holds: &[(f32, u64)]) {
    let w = r.width as usize;
    if w == 0 {
        return;
    }
    let l = levels.first().copied().unwrap_or(0.0);
    let rr = levels.get(1).copied().unwrap_or(l);
    let hl = holds.first().map(|h| h.0).unwrap_or(0.0);
    let hr = holds.get(1).map(|h| h.0).unwrap_or(hl);
    let track = if t.mono { Color::Reset } else { Color::Rgb(0x31, 0x32, 0x44) };
    let mut spans = Vec::with_capacity(w);
    for i in 0..w {
        let frac = (i as f32 + 0.5) / w as f32;
        let hold_cell = |h: f32| h > 0.02 && ((h * w as f32) as usize).min(w - 1) == i;
        let on_l = l >= frac || hold_cell(hl);
        let on_r = rr >= frac || hold_cell(hr);
        let c = level_color(t, frac);
        if t.mono {
            let ch = match (on_l, on_r) {
                (true, true) => "█",
                (true, false) => "▀",
                (false, true) => "▄",
                _ => " ",
            };
            spans.push(Span::raw(ch));
        } else {
            let fg = if on_l { c } else { track };
            let bg = if on_r { c } else { track };
            spans.push(Span::styled("▀", Style::default().fg(fg).bg(bg)));
        }
    }
    f.render_widget(Line::from(spans), r);
}

// ---------------------------------------------------------------- mixer

enum MixLine {
    Title(u32),
    Volume(MixSel),
    Meter(u32),
    Channel(MixSel),
    Gap,
}

fn render_mixer(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme.clone();
    let (list_area, details) = split(app, area, 46);
    let title = format!("{} · {}", app.tab.title(), app.items(app.tab).len());
    let block = panel(&t, &title, !app.filter_focus);
    let mut inner = block.inner(list_area);
    f.render_widget(block, list_area);
    if let Some(fi) = &app.filter {
        let fb = Rect { y: inner.bottom().saturating_sub(1), height: 1, ..inner };
        inner.height = inner.height.saturating_sub(1);
        f.render_widget(Span::styled(" / ", t.key()), Rect { width: 3, ..fb });
        fi.render(f, Rect { x: fb.x + 3, width: fb.width.saturating_sub(3), ..fb }, &t, app.filter_focus);
    }
    let items = app.items(app.tab);
    if items.is_empty() {
        let msg = match app.tab {
            Tab::Playback => "Nothing is playing. Apps appear here while they make sound.",
            Tab::Recording => "Nothing is recording. Apps appear here while they use a microphone.",
            Tab::Outputs => "No outputs.",
            _ => "No inputs.",
        };
        f.render_widget(Paragraph::new(msg).style(t.dim()).wrap(Wrap { trim: true }), inner.inner(Margin::new(2, 1)));
        if let Some(d) = details {
            let block = panel(&t, "Details", false);
            f.render_widget(block, d);
        }
        return;
    }

    let meters = app.cfg.general.meters;
    let mut lines: Vec<MixLine> = Vec::new();
    for id in &items {
        lines.push(MixLine::Title(*id));
        lines.push(MixLine::Volume(MixSel { node: *id, channel: None }));
        if app.expanded.contains(id) {
            let n = app.graph.nodes.get(id).cloned().unwrap_or_default();
            let (vols, _) = app.graph.volume_of(&n);
            for c in 0..vols.len() {
                lines.push(MixLine::Channel(MixSel { node: *id, channel: Some(c) }));
            }
        }
        if meters {
            lines.push(MixLine::Meter(*id));
        }
        lines.push(MixLine::Gap);
    }
    let sel = app.current_mix();
    let sel_line = sel.and_then(|s| {
        lines.iter().position(|l| match (l, s.channel) {
            (MixLine::Volume(m), None) => m.node == s.node,
            (MixLine::Channel(m), Some(_)) => *m == s,
            _ => false,
        })
    });
    let height = inner.height as usize;
    let off = app.list_offset.entry(app.tab).or_insert(0);
    if let Some(i) = sel_line {
        let top = i.saturating_sub(1); // keep the title row visible
        if top < *off {
            *off = top;
        } else if i + 2 >= *off + height {
            *off = (i + 3).saturating_sub(height);
        }
    }
    *off = (*off).min(lines.len().saturating_sub(height));
    let off = *off;
    app.list_height = height / 4;
    let max = app.cfg.general.max_volume.clamp(100, 200) as f32;

    for (row, line) in lines.iter().enumerate().skip(off).take(height) {
        let r = Rect { y: inner.y + (row - off) as u16, height: 1, ..inner };
        match line {
            MixLine::Gap => {}
            MixLine::Title(id) => {
                let Some(n) = app.graph.nodes.get(id).cloned() else { continue };
                let selected = sel.is_some_and(|s| s.node == *id);
                let marker = if selected { Span::styled("▌", t.accent()) } else { Span::raw(" ") };
                let mut left = vec![marker, Span::styled(format!("{} ", node_icon(app, &n)), if selected { t.accent() } else { t.dim() }), Span::styled(app.title(&n), t.bold())];
                let sub = n.subtitle().to_string();
                // Right side: where a stream goes / default star for devices.
                let (right, target): (Vec<Span>, Option<Target>) = if n.kind().is_stream() {
                    let devs = app.graph.connected_devices(&n);
                    let name = devs.first().map(|d| app.title(d)).unwrap_or_else(|| "not connected".into());
                    let pinned = !app.graph.follows_default(&n);
                    let arrow = if n.kind() == NodeKind::Playback { "→" } else { "←" };
                    (vec![Span::styled(format!("{arrow} "), t.dim()), Span::styled(truncate(&name, 28), t.text()), Span::styled(if pinned { if app.cfg.general.icons == IconSet::Nerd { " 󰐃▾ " } else { " *▾ " } } else { " ▾ " }, t.dim())], Some(Target::MoveBtn(*id)))
                } else if app.graph.is_default(&n) {
                    (vec![Span::styled("★ default ", t.warn())], None)
                } else {
                    (vec![Span::styled("☆ ", t.dim())], Some(Target::DefaultBtn(*id)))
                };
                let rw: u16 = right.iter().map(|s| s.content.width() as u16).sum();
                let lw: usize = left.iter().map(|s| s.content.width()).sum();
                let room = (r.width as usize).saturating_sub(lw + rw as usize + 2);
                if !sub.is_empty() && room > 6 {
                    left.push(Span::styled(format!("  {}", truncate(&sub, room - 2)), t.dim()));
                }
                f.render_widget(fit_line(Line::from(left), (r.width as usize).saturating_sub(rw as usize + 1)), r);
                let rr = Rect { x: r.right().saturating_sub(rw), width: rw, ..r };
                f.render_widget(Line::from(right), rr);
                app.mouse.add(Rect { width: r.width.saturating_sub(rw), ..r }, Target::Mix(MixSel { node: *id, channel: None }));
                if let Some(tg) = target {
                    app.mouse.add(rr, tg);
                }
            }
            MixLine::Volume(s) => {
                let Some(n) = app.graph.nodes.get(&s.node).cloned() else { continue };
                let (vols, muted) = app.graph.volume_of(&n);
                let pct = avg_percent(&vols);
                let focused = sel.is_some_and(|x| x == *s);
                let mb = Rect { x: r.x + 3, width: 2, ..r };
                f.render_widget(Span::styled(mute_icon(app, muted), if muted { t.error() } else { t.dim() }), mb);
                app.mouse.add(mb, Target::MuteBtn(s.node));
                let label = if muted { "muted".to_string() } else { format!("{pct:>3.0}%") };
                let lw = 7u16;
                let bar = Rect { x: r.x + 6, width: r.width.saturating_sub(6 + lw + 1), ..r };
                volume_bar(f, bar, &t, pct, max, muted, focused);
                app.mouse.add(bar, Target::VolBar(*s));
                f.render_widget(Span::styled(format!(" {label:>6}"), if focused { t.bold() } else { t.text() }), Rect { x: bar.right(), width: lw, ..r });
            }
            MixLine::Channel(s) => {
                let Some(n) = app.graph.nodes.get(&s.node).cloned() else { continue };
                let (vols, muted) = app.graph.volume_of(&n);
                let c = s.channel.unwrap_or(0);
                let pct = vols.get(c).map(|v| linear_to_percent(*v)).unwrap_or(0.0);
                let focused = sel.is_some_and(|x| x == *s);
                let name = n.channels.get(c).cloned().unwrap_or_else(|| format!("{}", c + 1));
                f.render_widget(Span::styled(format!("{:>6} ", truncate(&name, 5)), if focused { t.accent() } else { t.dim() }), Rect { x: r.x, width: 7, ..r });
                let bar = Rect { x: r.x + 8, width: r.width.saturating_sub(8 + 8), ..r };
                volume_bar(f, bar, &t, pct, max, muted, focused);
                app.mouse.add(bar, Target::VolBar(*s));
                app.mouse.add(Rect { width: 7, ..r }, Target::Mix(*s));
                f.render_widget(Span::styled(format!(" {pct:>5.0}%"), if focused { t.bold() } else { t.dim() }), Rect { x: bar.right(), width: 8, ..r });
            }
            MixLine::Meter(id) => {
                app.visible.insert(*id);
                let levels = app.levels.get(id).cloned().unwrap_or_default();
                let holds = app.holds.get(id).cloned().unwrap_or_default();
                let mr = Rect { x: r.x + 6, width: r.width.saturating_sub(6 + 8), ..r };
                meter(f, mr, &t, &levels, &holds);
                let db = levels.iter().copied().fold(0.0f32, f32::max);
                let label = if db <= 0.001 { "   —".to_string() } else { format!("{:>4.0}", db * 60.0 - 60.0) };
                f.render_widget(Span::styled(format!(" {label} dB"), t.dim()), Rect { x: mr.right(), width: 8, ..r });
            }
        }
    }
    if lines.len() > height && height > 0 {
        let pos = (off * (height - 1)) / lines.len().saturating_sub(height).max(1);
        f.render_widget(Span::styled("┃", t.accent()), Rect { x: list_area.right() - 1, y: inner.y + pos as u16, width: 1, height: 1 });
    }
    if let Some(d) = details {
        mixer_details(app, f, d);
    }
}

enum KvLine<'a> {
    Line(Line<'a>, Option<Target>),
    Text(String, Style),
}

struct Kv<'a> {
    lines: Vec<KvLine<'a>>,
    t: Theme,
}

impl<'a> Kv<'a> {
    fn new(t: &Theme) -> Self {
        Self { lines: Vec::new(), t: t.clone() }
    }
    fn title(&mut self, s: &str) {
        self.lines.push(KvLine::Line(Line::from(Span::styled(s.to_string(), self.t.accent().add_modifier(Modifier::BOLD))), None));
    }
    fn kv(&mut self, k: &str, v: impl Into<String>) {
        let v = v.into();
        if !v.is_empty() {
            self.lines.push(KvLine::Line(Line::from(vec![Span::styled(format!("{k:<12}"), self.t.dim()), Span::styled(v, self.t.text())]), None));
        }
    }
    fn kvs(&mut self, k: &str, v: Vec<Span<'a>>, tg: Option<Target>) {
        let mut s = vec![Span::styled(format!("{k:<12}"), self.t.dim())];
        s.extend(v);
        self.lines.push(KvLine::Line(Line::from(s), tg));
    }
    fn gap(&mut self) {
        self.lines.push(KvLine::Line(Line::raw(""), None));
    }
    fn section(&mut self, s: &str) {
        self.gap();
        self.lines.push(KvLine::Line(Line::from(Span::styled(s.to_uppercase(), self.t.dim().add_modifier(Modifier::BOLD))), None));
    }
    /// Free text; wrapped to the panel width when rendered.
    fn note(&mut self, s: impl Into<String>, st: Style) {
        self.lines.push(KvLine::Text(s.into(), st));
    }
    fn render(self, f: &mut Frame, area: Rect, app: &mut App, title: &str) {
        let block = panel(&app.theme, title, false);
        let inner = block.inner(area).inner(Margin::new(1, 0));
        f.render_widget(block, area);
        let w = inner.width as usize;
        let mut rows: Vec<(Line, Option<Target>)> = Vec::new();
        for l in self.lines {
            match l {
                KvLine::Line(l, tg) => rows.push((fit_line(l, w), tg)),
                KvLine::Text(s, st) => rows.extend(wrap_text(&s, w).into_iter().map(|l| (Line::from(Span::styled(l, st)), None))),
            }
        }
        for (i, (l, tg)) in rows.into_iter().enumerate() {
            if i as u16 >= inner.height {
                break;
            }
            let r = Rect { y: inner.y + i as u16, height: 1, ..inner };
            f.render_widget(l, r);
            if let Some(tg) = tg {
                app.mouse.add(r, tg);
            }
        }
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// First visible row of a list of `len` rows `height` tall so `sel` stays in view.
fn scroll(app: &mut App, sel: usize, len: usize, height: usize) -> usize {
    let off = app.list_offset.entry(app.tab).or_insert(0);
    if sel < *off {
        *off = sel;
    } else if height > 0 && sel >= *off + height {
        *off = sel + 1 - height;
    }
    *off = (*off).min(len.saturating_sub(height));
    *off
}

/// Word-wraps `s` to `w` cells, hard-breaking words longer than a line.
fn wrap_text(s: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let lead: String = s.chars().take_while(|c| *c == ' ').collect();
    let mut out = Vec::new();
    let mut line = lead.clone();
    for word in s.split_whitespace() {
        if line.trim().is_empty() {
            line = lead.clone();
        } else if line.width() + 1 + word.width() <= w {
            line.push(' ');
        } else {
            out.push(std::mem::take(&mut line));
        }
        for c in word.chars() {
            if !line.trim().is_empty() && line.width() + c.to_string().width() > w {
                out.push(std::mem::take(&mut line));
            }
            line.push(c);
        }
    }
    if !line.trim().is_empty() {
        out.push(line);
    }
    out
}

/// What WirePlumber uses to remember this stream (mirrors state-stream.lua formKey).
fn remember_key(n: &Node) -> (String, bool) {
    if n.media_role == "Notification" || n.props.get("media.role").is_some_and(|r| r == "Notification") {
        return ("media.role = Notification".into(), false);
    }
    for k in ["application.id", "application.name", "media.name", "node.name"] {
        if let Some(v) = n.props.get(k).filter(|v| !v.is_empty()) {
            return (format!("{k} = {v}"), k == "media.name");
        }
    }
    ("(not remembered)".into(), false)
}

fn mixer_details(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme.clone();
    let width = area.width.saturating_sub(4) as usize;
    let mut kv = Kv::new(&t);
    let Some(n) = app.selected_node().cloned() else {
        return kv.render(f, area, app, "Details");
    };
    let (vols, muted) = app.graph.volume_of(&n);
    kv.title(&app.title(&n));
    kv.gap();
    if n.kind().is_stream() {
        kv.kv("App", if n.app_binary.is_empty() { n.app_name.clone() } else { format!("{} ({})", n.app_name, n.app_binary) });
        if !n.media_name.is_empty() {
            kv.kv("Playing", truncate(&n.media_name, width.saturating_sub(11)));
        }
        let api = match n.props.get("client.api").map(String::as_str) {
            Some("pipewire-pulse") => "PulseAudio API",
            Some("alsa") => "ALSA (pipewire-alsa)",
            Some("jack") => "JACK API",
            _ => "native PipeWire",
        };
        kv.kv("Talks via", api);
        let devs: Vec<String> = app.graph.connected_devices(&n).iter().map(|d| app.title(d)).collect();
        kv.kv(if n.kind() == NodeKind::Playback { "Plays to" } else { "Records" }, if devs.is_empty() { "nothing".into() } else { devs.join(", ") });
        kv.kvs(
            "Routing",
            vec![Span::styled(if app.graph.follows_default(&n) { "follows default".to_string() } else { "pinned to a device".to_string() }, t.text())],
            Some(Target::Run(Action::MoveTo)),
        );
        if let Some(r) = app.cfg.routing.iter().find(|r| r.app == n.app_name || r.app == n.app_binary) {
            kv.kv("Rule", format!("always → {}", r.target));
        }
    } else {
        kv.kv("Name", truncate(&n.name, width.saturating_sub(11)));
        if let Some(dev) = n.device_id.and_then(|d| app.graph.devices.get(&d)) {
            kv.kv("Card", dev.title().to_string());
            if let Some(p) = dev.active_profile() {
                kv.kvs("Profile", vec![Span::styled(truncate(&p.description, width.saturating_sub(11)), t.text())], Some(Target::Run(Action::Profile)));
            }
        } else {
            kv.kv("Type", "virtual (no hardware)");
        }
        if let Some((_, r)) = app.graph.route_for(&n) {
            kv.kvs("Port", vec![Span::styled(r.description.clone(), t.text()), Span::styled(" ▾", t.dim())], Some(Target::Run(Action::Port)));
            if r.latency_offset_ns != 0 {
                kv.kv("Latency", format!("{:+.1} ms offset", r.latency_offset_ns as f64 / 1e6));
            }
        }
        if let Some(c) = n.props.get("api.bluez5.codec") {
            kv.kv("Codec", format!("{} — {}", c.to_uppercase(), crate::app::codec_note(c)));
        }
        if let Some(r) = n.props.get("audio.rate") {
            kv.kv("Rate", format!("{r} Hz"));
        }
        let def = app.graph.is_default(&n);
        kv.kvs("Default", vec![checkbox(def, &t)], if def { None } else { Some(Target::Run(Action::SetDefault)) });
        let streams: Vec<String> = app.graph.connected_streams(&n).iter().map(|s| app.title(s)).collect();
        if !streams.is_empty() {
            kv.kv("In use by", truncate(&streams.join(", "), width.saturating_sub(11)));
        }
    }
    kv.section("Volume");
    for (i, v) in vols.iter().enumerate() {
        let ch = n.channels.get(i).map(|c| channel_label(c)).unwrap_or("Channel");
        let db = linear_to_db(*v);
        let dbs = if db.is_finite() { format!("{db:+.1} dB") } else { "-∞ dB".into() };
        kv.kv(ch, format!("{:>3.0}%  {dbs}", linear_to_percent(*v)));
    }
    if muted {
        kv.note("muted", t.error());
    }
    if n.kind().is_stream() {
        kv.section("Privacy");
        let (key, leaks) = remember_key(&n);
        let remembered = app.cfg.privacy.remember_volumes && app.cfg.privacy.mode != PrivacyMode::None;
        if !remembered {
            kv.note("WirePlumber won't remember this (your settings)", t.ok());
        } else {
            kv.note(format!("WirePlumber remembers it as: {key}"), t.dim());
            if leaks {
                kv.note("⚠ keyed by a title — see Privacy tab", t.warn());
            }
        }
    }
    let title = if n.kind().is_stream() { "Stream" } else { "Device" };
    kv.render(f, area, app, title);
}

// ---------------------------------------------------------------- cards

fn bus_icon(app: &App, d: &Device) -> &'static str {
    if app.cfg.general.icons != IconSet::Nerd {
        return match d.bus.as_str() {
            "bluetooth" => "B",
            "usb" => "U",
            _ => "■",
        };
    }
    match d.bus.as_str() {
        "bluetooth" => "󰂯",
        "usb" => "󰕓",
        _ => "󰘚",
    }
}

fn render_cards(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme.clone();
    let (list_area, details) = split(app, area, 56);
    let block = panel(&t, "Cards", true);
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);
    let rows = app.card_rows();
    let sel = app.row_index();
    let off = scroll(app, sel, rows.len(), inner.height as usize);
    for (i, row) in rows.iter().enumerate().skip(off).take(inner.height as usize) {
        let r = Rect { y: inner.y + (i - off) as u16, height: 1, ..inner };
        let selected = i == sel;
        let base = if selected { t.selected(true) } else { Style::default() };
        f.render_widget(Span::styled(" ".repeat(r.width as usize), base), r);
        let line = match row {
            CardRow::Device(id) => {
                let Some(d) = app.graph.devices.get(id) else { continue };
                let prof = d.active_profile().map(|p| p.description.clone()).unwrap_or_default();
                let off = d.active_profile().is_some_and(|p| p.name == "off");
                Line::from(vec![
                    if selected { Span::styled("▌", t.accent()) } else { Span::raw(" ") },
                    Span::styled(format!("{} ", bus_icon(app, d)), t.dim()),
                    Span::styled(format!("{:<28}", truncate(d.title(), 28)), t.bold()),
                    Span::styled(truncate(&prof, (r.width as usize).saturating_sub(33)), if off { t.dim() } else { t.text() }),
                ])
            }
            CardRow::Disabled(name) => Line::from(vec![
                if selected { Span::styled("▌", t.accent()) } else { Span::raw(" ") },
                Span::styled("⊘ ", t.error()),
                Span::styled(truncate(name, 40), t.dim()),
                Span::styled("  disabled by your config · Enter to re-enable", t.dim()),
            ]),
        };
        f.render_widget(fit_line(line, r.width as usize).style(base), r);
        app.mouse.add(r, Target::Row(i));
    }
    let Some(d_area) = details else { return };
    let mut kv = Kv::new(&t);
    let Some(CardRow::Device(id)) = rows.get(sel).cloned() else {
        kv.note("Disabled devices are hidden by a WirePlumber rule that audio-tui wrote.", t.dim());
        return kv.render(f, d_area, app, "Card");
    };
    let Some(d) = app.graph.devices.get(&id).cloned() else { return };
    kv.title(d.title());
    kv.kv("Name", d.name.clone());
    kv.kv("Connection", match d.bus.as_str() {
        "bluetooth" => "Bluetooth",
        "usb" => "USB",
        "pci" => "built-in (PCI)",
        other => other,
    }.to_string());
    kv.section("Profile (click to switch)");
    let mut profs = d.profiles.clone();
    profs.sort_by_key(|p| (p.name == "off", -p.priority));
    for p in &profs {
        let active = Some(p.index) == d.active_profile;
        let mut spans = vec![Span::styled(if active { "(•) " } else { "( ) " }, if active { t.accent() } else { t.dim() })];
        let st = if p.available == Avail::No { t.dim() } else if active { t.bold() } else { t.text() };
        spans.push(Span::styled(truncate(&p.description, (d_area.width as usize).saturating_sub(8)), st));
        kv.lines.push(KvLine::Line(Line::from(spans), Some(Target::ProfilePick { device: id, index: p.index })));
        if let (Some(c), true) = (p.codec(), active) {
            kv.note(format!("     {}", crate::app::codec_note(c)), t.dim());
        }
    }
    let active_routes: Vec<i32> = d.routes.iter().map(|r| r.index).collect();
    let ports: Vec<Route> = d.enum_routes.iter().filter(|r| d.active_profile.is_none_or(|p| r.profiles.is_empty() || r.profiles.contains(&p))).cloned().collect();
    if !ports.is_empty() {
        kv.section("Ports");
        for r in ports {
            let on = active_routes.contains(&r.index);
            let dir = if r.direction == Dir::Output { "out" } else { "in " };
            let avail = match r.available {
                Avail::No => "  unplugged",
                Avail::Yes => "  plugged in",
                Avail::Unknown => "",
            };
            let dev = r.devices.first().copied().unwrap_or(0);
            kv.lines.push(KvLine::Line(
                Line::from(vec![
                    Span::styled(if on { "● " } else { "○ " }, if on { t.ok() } else { t.dim() }),
                    Span::styled(format!("{dir} "), t.dim()),
                    Span::styled(r.description.clone(), if r.available == Avail::No { t.dim() } else { t.text() }),
                    Span::styled(avail, t.dim()),
                ]),
                Some(Target::PortPick { device: id, index: r.index, dev }),
            ));
        }
    }
    if let Some(rule) = app.cfg.devices.get(&d.name) {
        kv.section("Your config");
        if let Some(desc) = &rule.description {
            kv.kv("Renamed", desc.clone());
        }
    }
    kv.render(f, d_area, app, "Card");
}

// ---------------------------------------------------------------- virtual

fn render_virtual(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme.clone();
    let (list_area, details) = split(app, area, 50);
    let block = panel(&t, "Virtual devices", true);
    let inner = block.inner(list_area);
    f.render_widget(block, list_area);
    let rows = app.virt_rows();
    let sel = app.row_index();
    if rows.is_empty() {
        let k = app.keymap.hint(Action::NewVirtual).unwrap_or_default();
        let text = vec![
            Line::from(Span::styled("No virtual devices yet.", t.bold())),
            Line::raw(""),
            Line::from(Span::styled(format!("Press {k} to add one:"), t.dim())),
            Line::raw(""),
            Line::from(Span::styled("  • Virtual output — play into it, record its monitor (streaming, recording one app)", t.dim())),
            Line::from(Span::styled("  • Combined output — speakers and headphones at the same time", t.dim())),
            Line::from(Span::styled("  • Loopback — hear a microphone or line-in through your speakers", t.dim())),
            Line::from(Span::styled("  • Echo-cancelled / noise-suppressed microphone — cleaner calls", t.dim())),
            Line::from(Span::styled("  • Equalizer — shape the sound of an output", t.dim())),
            Line::raw(""),
            Line::from(Span::styled("They're saved in config.toml and created by PipeWire on every start.", t.dim())),
        ];
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), inner.inner(Margin::new(2, 1)));
        return;
    }
    let off = scroll(app, sel, rows.len(), inner.height as usize);
    for (i, row) in rows.iter().enumerate().skip(off).take(inner.height as usize) {
        let r = Rect { y: inner.y + (i - off) as u16, height: 1, ..inner };
        let selected = i == sel;
        let base = if selected { t.selected(true) } else { Style::default() };
        f.render_widget(Span::styled(" ".repeat(r.width as usize), base), r);
        let line = match row {
            VirtRow::Config(ci) => {
                let v = &app.cfg.virtuals[*ci];
                let live = app.graph.find_by_name(&v.name).is_some();
                let status = if live { Span::styled("● active", t.ok()) } else { Span::styled("○ not created yet (Ctrl-S)", t.warn()) };
                Line::from(vec![
                    if selected { Span::styled("▌", t.accent()) } else { Span::raw(" ") },
                    Span::styled(format!("{:<24}", truncate(if v.description.is_empty() { &v.name } else { &v.description }, 23)), t.bold()),
                    Span::styled(format!("{:<20}", short_kind(v.kind)), t.dim()),
                    status,
                ])
            }
            VirtRow::Live(id) => {
                let Some(n) = app.graph.nodes.get(id) else { continue };
                Line::from(vec![
                    if selected { Span::styled("▌", t.accent()) } else { Span::raw(" ") },
                    Span::styled(format!("{:<24}", truncate(&app.title(n), 23)), t.bold()),
                    Span::styled(format!("{:<20}", if n.kind() == NodeKind::Sink { "virtual output" } else { "virtual input" }), t.dim()),
                    Span::styled("temporary / made elsewhere", t.dim()),
                ])
            }
        };
        f.render_widget(fit_line(line, r.width as usize).style(base), r);
        app.mouse.add(r, Target::Row(i));
    }
    let Some(d_area) = details else { return };
    let mut kv = Kv::new(&t);
    match rows.get(sel) {
        Some(VirtRow::Config(ci)) => {
            let v = app.cfg.virtuals[*ci].clone();
            kv.title(if v.description.is_empty() { &v.name } else { &v.description });
            kv.kv("Kind", v.kind.label().to_string());
            kv.kv("Name", v.name.clone());
            let name_of = |n: &str| app.graph.find_by_name(n).map(|x| app.title(x)).unwrap_or_else(|| n.to_string());
            if !v.source.is_empty() {
                kv.kv("Listens to", name_of(&v.source));
            }
            if !v.target.is_empty() {
                kv.kv(if matches!(v.kind, VirtualKind::EchoCancel | VirtualKind::Noise) { "Microphone" } else { "Plays to" }, name_of(&v.target));
            }
            for tg in &v.targets {
                kv.kv("Plays to", name_of(tg));
            }
            if !v.bands.is_empty() {
                kv.section("Equalizer bands");
                for b in &v.bands {
                    kv.note(format!("{:>6} Hz  {:+5.1} dB  Q {}", b.freq, b.gain, b.q), t.text());
                }
            }
            kv.section("Stored in");
            kv.note("config.toml  →  60-audio-tui-virtual.conf", t.dim());
            if v.kind == VirtualKind::Noise {
                kv.gap();
                kv.note("Needs the RNNoise LADSPA plugin: pacman -S noise-suppression-for-voice", t.info());
            }
        }
        Some(VirtRow::Live(id)) => {
            if let Some(n) = app.graph.nodes.get(id).cloned() {
                kv.title(&app.title(&n));
                kv.kv("Name", n.name.clone());
                kv.kv("Factory", n.props.get("factory.name").cloned().unwrap_or_default());
                kv.gap();
                kv.note("Not managed by your config: it was created temporarily or by another tool. It disappears when PipeWire restarts unless that tool recreates it.", t.dim());
            }
        }
        None => {}
    }
    kv.render(f, d_area, app, "Details");
}

fn short_kind(k: VirtualKind) -> &'static str {
    match k {
        VirtualKind::NullSink => "virtual output",
        VirtualKind::Combine => "combined output",
        VirtualKind::Loopback => "loopback",
        VirtualKind::EchoCancel => "echo-cancel mic",
        VirtualKind::Noise => "noise-suppressed mic",
        VirtualKind::Equalizer => "equalizer",
    }
}

// ---------------------------------------------------------------- settings

fn render_settings(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme.clone();
    let block = panel(&t, "Settings", true);
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    let rows = app.set_rows();
    let sel = app.row_index();
    let g = &app.graph.settings;
    let rate: u32 = g.get("clock.rate").and_then(|v| v.parse().ok()).unwrap_or(48000);
    let force_rate = g.get("clock.force-rate").filter(|v| *v != "0").cloned();
    let quantum: u32 = g.get("clock.quantum").and_then(|v| v.parse().ok()).unwrap_or(0);
    let force_q = g.get("clock.force-quantum").filter(|v| *v != "0").cloned();
    let mut out: Vec<(Line, Option<Target>, Option<Style>)> = Vec::new();
    let mut sel_line = 0;
    out.push((Line::from(Span::styled("AUDIO ENGINE (PipeWire)", t.dim().add_modifier(Modifier::BOLD))), None, None));
    let mut sm_header = false;
    for (i, row) in rows.iter().enumerate() {
        if matches!(row, SetRow::Sm(_)) && !sm_header {
            sm_header = true;
            out.push((Line::raw(""), None, None));
            out.push((Line::from(Span::styled("WIREPLUMBER (session manager) — changes are saved like `wpctl settings --save`", t.dim().add_modifier(Modifier::BOLD))), None, None));
        }
        if row == &SetRow::MaxVolume {
            out.push((Line::raw(""), None, None));
            out.push((Line::from(Span::styled("AUDIO-TUI", t.dim().add_modifier(Modifier::BOLD))), None, None));
        }
        let selected = i == sel;
        let base = if selected { Some(t.selected(true)) } else { None };
        let (label, value, note): (String, Span, String) = match row {
            SetRow::Rate => (
                "Sample rate".into(),
                Span::styled(format!("{:.1} kHz", rate as f32 / 1000.0), t.bold()),
                match &force_rate {
                    Some(r) => format!("forced to {r} Hz now · x to reset"),
                    None => "PipeWire picks · Enter to force a rate now".into(),
                },
            ),
            SetRow::Quantum => (
                "Buffer size".into(),
                Span::styled(if quantum > 0 { format!("{quantum} ({:.1} ms)", quantum as f32 / rate as f32 * 1000.0) } else { "auto".into() }, t.bold()),
                match &force_q {
                    Some(q) => format!("forced to {q} now · x to reset"),
                    None => "smaller = lower latency, more CPU · Enter to force".into(),
                },
            ),
            SetRow::AllowedRates => (
                "Allowed rates".into(),
                Span::styled(if app.cfg.clock.allowed_rates.is_empty() { "default only".into() } else { app.cfg.clock.allowed_rates.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", ") }, t.bold()),
                "switch rate to match the music (bit-perfect-ish) · saved in config".into(),
            ),
            SetRow::SaveClock => (
                "Keep these".into(),
                Span::styled("[ Save as default ]", t.accent()),
                "keep the current rate/buffer after restarts ([clock]; Ctrl-S applies)".into(),
            ),
            SetRow::MaxVolume => ("Max volume".into(), Span::styled(format!("{}%", app.cfg.general.max_volume), t.bold()), "above 100% boosts digitally and can distort".into()),
            SetRow::VolumeStep => ("Volume step".into(), Span::styled(format!("{}%", app.cfg.general.volume_step), t.bold()), "per keypress (Shift+←/→ = 1%)".into()),
            SetRow::Meters => ("Level meters".into(), toggle(app.cfg.general.meters, &t), "live levels under each slider".into()),
            SetRow::Sm(k) => {
                let v = app.graph.sm_settings.get(k).cloned().unwrap_or_default();
                let val = match v.as_str() {
                    "true" => toggle(true, &t),
                    "false" => toggle(false, &t),
                    other => Span::styled(other.to_string(), t.bold()),
                };
                (k.clone(), val, sm_note(k).into())
            }
        };
        let pad = 18usize.saturating_sub(value.content.width());
        let line = Line::from(vec![
            if selected { Span::styled("▌", t.accent()) } else { Span::raw(" ") },
            Span::styled(format!("{:<38}", truncate(&label, 37)), t.text()),
            value,
            Span::raw(" ".repeat(pad + 1)),
            Span::styled(note, t.dim()),
        ]);
        if selected {
            sel_line = out.len();
        }
        out.push((line, Some(Target::Row(i)), base));
    }
    let height = inner.height as usize;
    let off = app.list_offset.entry(Tab::Settings).or_insert(0);
    // Keep a section header above the first row visible when scrolled to the top.
    let top = if sel_line <= 2 { 0 } else { sel_line - 1 };
    if top < *off {
        *off = top;
    } else if sel_line >= *off + height {
        *off = sel_line + 1 - height;
    }
    *off = (*off).min(out.len().saturating_sub(height));
    let off = *off;
    let w = inner.width as usize;
    for (row, (line, target, st)) in out.into_iter().enumerate().skip(off).take(height) {
        let r = Rect { y: inner.y + (row - off) as u16, height: 1, ..inner };
        if let Some(st) = st {
            f.render_widget(Span::styled(" ".repeat(r.width as usize), st), r);
        }
        f.render_widget(fit_line(line, w), r);
        if let Some(tg) = target {
            app.mouse.add(r, tg);
        }
    }
}

fn sm_note(k: &str) -> &'static str {
    match k {
        "bluetooth.autoswitch-restore-timeout" => "ms to wait before leaving mic mode when recording stops",
        "bluetooth.autoswitch-switch-timeout" => "ms to wait before entering mic mode when recording starts",
        "bluetooth.autoswitch-to-headset-profile" => "switch headset to mic mode when an app records",
        "bluetooth.profile-preference" => "\"quality\" or \"latency\" when picking a Bluetooth profile",
        "bluetooth.use-persistent-storage" => "remember each headset's mode (music / mic)",
        "device.restore-profile" => "remember card profiles",
        "device.restore-routes" => "remember ports & device volumes",
        "device.routes.default-sink-volume" => "starting volume for new outputs (linear: 0.064 ≈ 40%)",
        "device.routes.default-source-volume" => "starting volume for new inputs (linear)",
        "device.routes.mute-on-alsa-playback-removed" => "mute everything when wired headphones are unplugged",
        "device.routes.mute-on-bluetooth-playback-removed" => "mute everything when Bluetooth headphones disconnect",
        "linking.allow-moving-streams" => "let apps be moved to another device while playing",
        "linking.follow-default-target" => "move apps when the default changes",
        "linking.pause-playback" => "pause players when their output disappears",
        "linking.role-based.duck-level" => "volume other audio drops to during calls/alerts",
        "monitor.alsa.autodetect-hdmi-channels" => "detect HDMI channel layout (experimental)",
        "monitor.camera-discovery-timeout" => "ms to wait for cameras at startup",
        "node.features.audio.control-port" => "add control ports to audio nodes (advanced)",
        "node.features.audio.monitor-ports" => "add monitor ports to outputs (to record what plays)",
        "node.features.audio.mono" => "mix every output down to mono",
        "node.features.audio.no-dsp" => "skip converting audio to float (advanced)",
        "node.restore-default-targets" => "remember your default output & input",
        "node.stream.default-capture-volume" => "volume for new recorders",
        "node.stream.default-media-role" => "role given to apps that don't set one (null = none)",
        "node.stream.default-playback-volume" => "volume for new apps",
        "node.stream.restore-props" => "remember app volumes (see Privacy)",
        "node.stream.restore-target" => "remember app devices (see Privacy)",
        _ => "",
    }
}

// ---------------------------------------------------------------- privacy

fn render_privacy(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme.clone();
    let block = panel(&t, "Privacy — what WirePlumber remembers", true);
    let inner = block.inner(area).inner(Margin::new(1, 0));
    f.render_widget(block, area);
    let intro = "WirePlumber saves each app's volume and chosen device so they come back next time. It keys them by the app's name, or — for apps that don't give one — by what's playing (a song, video or page title). Entries marked ⚠ are those.";
    let intro_h = wrap_text(intro, inner.width as usize).len() as u16;
    f.render_widget(Paragraph::new(intro).wrap(Wrap { trim: true }).style(t.dim()), Rect { height: intro_h, ..inner });
    let list = Rect { y: inner.y + intro_h + 1, height: inner.height.saturating_sub(intro_h + 1), ..inner };
    let rows = app.priv_rows();
    let sel = app.row_index();
    let height = list.height as usize;
    let off_ref = app.list_offset.entry(Tab::Privacy).or_insert(0);
    if sel < *off_ref {
        *off_ref = sel.saturating_sub(1);
    } else if sel >= *off_ref + height {
        *off_ref = sel + 1 - height;
    }
    let off = *off_ref;
    let leaks: usize = app.state_files.iter().map(|f| f.entries.iter().filter(|e| e.leaks_title()).count()).sum();
    let p = app.cfg.privacy.clone();
    for (i, row) in rows.iter().enumerate().skip(off).take(height) {
        let r = Rect { y: list.y + (i - off) as u16, height: 1, ..list };
        let selected = i == sel;
        let base = if selected { t.selected(true) } else { Style::default() };
        f.render_widget(Span::styled(" ".repeat(r.width as usize), base), r);
        let mark = if selected { Span::styled("▌", t.accent()) } else { Span::raw(" ") };
        let setting = |label: &str, v: Span<'static>, note: &str| {
            let pad = 16usize.saturating_sub(v.content.width()).max(1);
            Line::from(vec![mark.clone(), Span::styled(format!("{label:<28}"), t.text()), v, Span::raw(" ".repeat(pad)), Span::styled(note.to_string(), t.dim())])
        };
        let line = match row {
            PrivRow::RememberVolumes => setting("Remember app volumes", toggle(p.remember_volumes, &t), "off = every app starts at the default volume"),
            PrivRow::RememberDevices => setting("Remember app devices", toggle(p.remember_devices, &t), "off = apps always follow the default output"),
            PrivRow::Mode => {
                let (v, note) = match p.mode {
                    PrivacyMode::All => ("all apps", "WirePlumber's default"),
                    PrivacyMode::Allowlist => ("only listed apps", "everything else starts fresh"),
                    PrivacyMode::None => ("nothing", "no per-app memory at all"),
                };
                setting("Remember", Span::styled(v, t.bold()), note)
            }
            PrivRow::Allowlist => setting("  Allowlist", Span::styled(if p.allowlist.is_empty() { "(empty)".to_string() } else { p.allowlist.join(", ") }, t.bold()), "Enter to edit"),
            PrivRow::Prune => setting("Clean titles at start", toggle(p.prune_titles_on_start, &t), "delete ⚠ entries each time audio-tui opens"),
            PrivRow::ForgetAll => {
                let n: usize = app.state_files.iter().map(|f| f.entries.len()).sum();
                setting("Forget everything", Span::styled(plural(n, "entry", "entries"), t.warn()), "Enter: delete all of WirePlumber's memory")
            }
            PrivRow::File(fi) => {
                let sf = &app.state_files[*fi];
                let open = app.priv_open.contains(&sf.name);
                Line::from(vec![
                    mark,
                    Span::styled(if open { "▾ " } else { "▸ " }, t.dim()),
                    Span::styled(format!("{} ", sf.name), t.accent().add_modifier(Modifier::BOLD)),
                    Span::styled(format!("· {} · {}", plural(sf.entries.len(), "entry", "entries"), sf.description()), t.dim()),
                ])
            }
            PrivRow::Entry(fi, ei) => {
                let e = &app.state_files[*fi].entries[*ei];
                let (warn, who, by) = match &e.kind {
                    EntryKind::Stream { dir, by, who } => (e.leaks_title(), format!("{} {who}", if dir == "Output" { "▶" } else { "●" }), by.replace("application.", "app.")),
                    _ => (false, e.key.clone(), String::new()),
                };
                let sum = e.summary();
                let right_w = sum.width().min(30) as u16;
                let name_w = (r.width as usize).saturating_sub(right_w as usize + 22);
                Line::from(vec![
                    mark,
                    Span::styled(if warn { " ⚠ " } else { "   " }, t.warn()),
                    Span::styled(format!("{:<w$}", truncate(&who, name_w), w = name_w), if warn { t.warn() } else { t.text() }),
                    Span::styled(format!(" {:<14}", truncate(&by, 14)), t.dim()),
                    Span::styled(truncate(&sum, 30), t.dim()),
                ])
            }
        };
        f.render_widget(fit_line(line, r.width as usize).style(base), r);
        app.mouse.add(r, Target::Row(i));
    }
    if app.state_files.is_empty() {
        let y = list.y + 6;
        if y < list.bottom() {
            f.render_widget(Span::styled(format!(" No state files in {} — nothing is remembered.", app.state_dir().display()), t.ok()), Rect { y, height: 1, ..list });
        }
    } else if leaks > 0 {
        let mut msg = format!(" {leaks} entr{} keyed by a media title ", if leaks == 1 { "y is" } else { "ies are" });
        let title_w = "Privacy — what WirePlumber remembers".width() as u16 + 5;
        if area.width < title_w + msg.width() as u16 + 2 {
            msg = format!(" ⚠ {leaks} ");
        }
        let w = msg.width() as u16;
        f.render_widget(Span::styled(msg, t.warn().add_modifier(Modifier::REVERSED)), Rect::new(area.right().saturating_sub(w + 2), area.y, w, 1));
    }
}

// ---------------------------------------------------------------- popups

fn render_popup(app: &mut App, f: &mut Frame, area: Rect) {
    let t = app.theme.clone();
    let Some(mut p) = app.popup.take() else { return };
    match &mut p {
        Popup::Help { scroll, max } => {
            app.mouse.modal_barrier();
            let rows = help_rows(app);
            *max = widgets::help(f, area, &t, "audio-tui · help", &rows, *scroll);
        }
        Popup::Palette(pal) => pal.render(f, area, &t, &mut app.mouse, Target::PaletteRow),
        Popup::Menu(m) => {
            app.mouse.modal_barrier();
            let w = (m.items.iter().map(|(l, h, _, _)| l.width() + h.width() + 7).max().unwrap_or(10).max(m.title.width() + 6) as u16).min(area.width.saturating_sub(2));
            let h = (m.items.len() as u16 + 2).min(area.height.saturating_sub(2));
            let (mut x, mut y) = m.at;
            if (x, y) == (0, 0) {
                let c = centered(area, w, h);
                (x, y) = (c.x, c.y);
            }
            let r = Rect::new(x.min(area.right().saturating_sub(w)), y.min(area.bottom().saturating_sub(h)), w, h);
            let inner = popup(f, r, &t, &m.title);
            let visible = inner.height as usize;
            let start = m.sel.saturating_sub(visible.saturating_sub(1));
            for (i, (label, hint, _, current)) in m.items.iter().enumerate().skip(start).take(visible) {
                let rr = Rect { y: inner.y + (i - start) as u16, height: 1, ..inner };
                let st = if i == m.sel { t.selected(true) } else { t.text() };
                let mark = if *current { "● " } else { "  " };
                let pad = (inner.width as usize).saturating_sub(label.width() + hint.width() + 3);
                f.render_widget(
                    Line::from(vec![
                        Span::styled(mark, st.patch(t.ok())),
                        Span::styled(label.clone(), st),
                        Span::styled(" ".repeat(pad), st),
                        Span::styled(format!("{hint} "), st.patch(t.dim())),
                    ]),
                    rr,
                );
                app.mouse.add(rr, Target::MenuRow(i));
            }
        }
        Popup::Confirm { title, msg, yes, yes_sel, .. } => widgets::confirm(f, area, &t, title, msg, yes, *yes_sel, &mut app.mouse, Target::Confirm),
        Popup::Dialog(d) => d.render(f, area, &t, &mut app.mouse, Target::DialogField, Target::DialogButton),
    }
    app.popup = Some(p);
}

fn help_rows(app: &App) -> Vec<HelpRow> {
    let mut rows = Vec::new();
    for section in ["General", "Mixer tabs (1–4)", "Playback & Recording", "Outputs, Inputs & Cards", "Virtual devices", "Settings & Privacy"] {
        rows.push(HelpRow::Section(section.into()));
        for &a in Action::all().iter().filter(|a| a.section() == section) {
            rows.push(HelpRow::Key(app.keymap.keys_label(a), a.description().to_string(), a.name().to_string()));
        }
    }
    rows.push(HelpRow::Section("Mouse".into()));
    rows.push(HelpRow::Text("click or drag a slider · wheel over a slider = volume, elsewhere = scroll".into()));
    rows.push(HelpRow::Text("click the speaker icon to mute, “→ device” to move an app, ☆ to make default".into()));
    rows.push(HelpRow::Text("double-click an app to move it, a device to make it default · right-click for actions".into()));
    rows.push(HelpRow::Section("System changes".into()));
    rows.push(HelpRow::Text("Renames, disabling, privacy, routing rules, virtual devices and clock settings are saved".into()));
    rows.push(HelpRow::Text("in config.toml first; Ctrl-S writes 60-audio-tui-*.conf drop-ins and restarts what's needed.".into()));
    rows.push(HelpRow::Section("Files".into()));
    rows.push(HelpRow::Text(format!("config  {}", app.cfg_path.display())));
    rows.push(HelpRow::Text(format!("log     {}", crate::core::logging::log_dir(crate::config::APP).join("audio-tui.log").display())));
    rows
}

#[cfg(test)]
mod snapshots {
    //! `cargo test snap -- --nocapture` prints each screen as text.
    use super::*;
    use crate::backend::{demo, Ev};
    use crate::config::Config;
    use ratatui::{backend::TestBackend, Terminal};
    use std::sync::Arc;

    fn app() -> App {
        let tx: crate::backend::CmdTx = Arc::new(|_| {});
        let (atx, _arx) = tokio::sync::mpsc::unbounded_channel();
        let peaks = crate::backend::Peaks::default();
        let root = std::env::temp_dir().join(format!("audio-tui-snap-{}", std::process::id()));
        let dirs = crate::backend::conf::Dirs { pipewire: root.join("pw"), pulse: root.join("pulse"), client: root.join("client"), wireplumber: root.join("wp") };
        let mut a = App::new(Config::default(), root.join("config.toml"), tx, atx, peaks, true, vec![], dirs);
        a.toasts.dismiss_all();
        a.on_backend(Ev::Graph(Box::new(demo::sample_graph())));
        // Pretend meters have been running.
        for (id, l) in [(60u32, [0.8f32, 0.7]), (61, [0.55, 0.6]), (52, [0.85, 0.8]), (50, [0.0, 0.0])] {
            a.levels.insert(id, l.to_vec());
        }
        a
    }

    fn dump(app: &mut App, w: u16, h: u16) -> String {
        let mut t = Terminal::new(TestBackend::new(w, h)).unwrap();
        t.draw(|f| render(app, f)).unwrap();
        let buf = t.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..h {
            let mut skip = 0;
            for x in 0..w {
                if skip > 0 {
                    skip -= 1;
                    continue;
                }
                let s = buf[(x, y)].symbol();
                skip = s.width().saturating_sub(1);
                out.push_str(s);
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn snap_playback_outputs() {
        let mut a = app();
        a.tab = Tab::Playback;
        println!("{}", dump(&mut a, 130, 26));
        a.tab = Tab::Outputs;
        a.expanded.insert(52);
        println!("{}", dump(&mut a, 130, 30));
    }

    #[test]
    fn snap_cards_virtual_settings() {
        let mut a = app();
        a.tab = Tab::Cards;
        a.select_row(3);
        println!("{}", dump(&mut a, 130, 28));
        a.tab = Tab::Settings;
        println!("{}", dump(&mut a, 130, 26));
        a.tab = Tab::Virtual;
        println!("{}", dump(&mut a, 110, 18));
    }

    #[test]
    fn snap_privacy() {
        let mut a = app();
        let dir = a.state_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("stream-properties"), "[stream-properties]\nOutput/Audio:application.name:Firefox={\"volume\":0.42, \"mute\":false}\nOutput/Audio:media.name:Never\\sGonna\\sGive\\sYou\\sUp={\"volume\":1.0}\nOutput/Audio:media.role:Notification={\"volume\":0.3, \"mute\":true}\n").unwrap();
        std::fs::write(dir.join("default-nodes"), "[default-nodes]\ndefault.configured.audio.sink={\"name\":\"alsa_output.usb-FiiO_K5-00.analog-stereo\"}\n").unwrap();
        a.tab = Tab::Privacy;
        a.reload_state();
        println!("{}", dump(&mut a, 120, 24));
    }

    #[test]
    fn snap_sweep() {
        let mut a = app();
        a.show_details = true;
        for tab in Tab::ALL {
            a.tab = tab;
            for (w, h) in [(70, 20), (100, 24), (140, 30)] {
                println!("=== {tab:?} {w}x{h}\n{}", dump(&mut a, w, h));
            }
        }
    }
}
