//! Typed model of the PipeWire graph as audio-tui sees it. Pure data plus
//! helpers; the PipeWire thread (pw.rs) and the demo both produce `Graph`.

use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum NodeKind {
    /// Output device (speakers, headphones, HDMI, virtual sink).
    Sink,
    /// Input device (microphone, monitor, virtual source).
    Source,
    /// An app playing audio.
    Playback,
    /// An app recording audio.
    Recording,
    Other,
}

impl NodeKind {
    pub fn from_media_class(c: &str) -> Self {
        match c {
            "Audio/Sink" | "Audio/Duplex" => Self::Sink,
            "Audio/Source" | "Audio/Source/Virtual" => Self::Source,
            "Stream/Output/Audio" => Self::Playback,
            "Stream/Input/Audio" => Self::Recording,
            _ => Self::Other,
        }
    }
    pub fn is_device(self) -> bool {
        matches!(self, Self::Sink | Self::Source)
    }
    pub fn is_stream(self) -> bool {
        matches!(self, Self::Playback | Self::Recording)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Node {
    pub id: u32,
    pub serial: u64,
    pub name: String,
    pub description: String,
    pub nick: String,
    pub media_class: String,
    pub kind: Option<NodeKind>,
    pub app_name: String,
    pub app_binary: String,
    pub app_icon: String,
    pub media_name: String,
    pub media_role: String,
    pub client_id: Option<u32>,
    pub device_id: Option<u32>,
    /// `card.profile.device`: which route on the device this node belongs to.
    pub profile_device: Option<i32>,
    /// Linear amplitude per channel (PipeWire's native scale).
    pub volumes: Vec<f32>,
    pub mute: bool,
    /// Channel positions, e.g. ["FL", "FR"].
    pub channels: Vec<String>,
    pub props: BTreeMap<String, String>,
}

impl Node {
    pub fn kind(&self) -> NodeKind {
        self.kind.unwrap_or_else(|| NodeKind::from_media_class(&self.media_class))
    }

    /// Main label: app name for streams, friendly description for devices.
    pub fn title(&self) -> String {
        let pick = |xs: &[&String]| xs.iter().find(|s| !s.is_empty()).map(|s| s.to_string());
        if self.kind().is_stream() {
            pick(&[&self.app_name, &self.app_binary, &self.description, &self.name]).unwrap_or_else(|| format!("stream {}", self.id))
        } else {
            pick(&[&self.description, &self.nick, &self.name]).unwrap_or_else(|| format!("node {}", self.id))
        }
    }

    /// Secondary label: what's playing (streams).
    pub fn subtitle(&self) -> &str {
        if self.kind().is_stream() && self.media_name != self.app_name {
            &self.media_name
        } else {
            ""
        }
    }

    /// Virtual: a sink/source with no hardware device behind it.
    pub fn is_virtual(&self) -> bool {
        self.kind().is_device() && self.device_id.is_none()
    }

    pub fn is_bluetooth(&self) -> bool {
        self.name.starts_with("bluez_") || self.props.get("device.api").is_some_and(|a| a == "bluez5")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Avail {
    #[default]
    Unknown,
    No,
    Yes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dir {
    #[default]
    Output,
    Input,
}

/// A port on a card (speakers, headphones jack, HDMI 1, mic...).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Route {
    pub index: i32,
    /// Card-profile device this route applies to.
    pub device: i32,
    pub name: String,
    pub description: String,
    pub direction: Dir,
    pub available: Avail,
    pub priority: i32,
    pub profiles: Vec<i32>,
    pub devices: Vec<i32>,
    pub volumes: Vec<f32>,
    pub mute: bool,
    pub latency_offset_ns: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub index: i32,
    pub name: String,
    pub description: String,
    pub available: Avail,
    pub priority: i32,
}

impl Profile {
    /// Bluetooth profiles encode the codec in the name, e.g. "a2dp-sink-ldac".
    pub fn codec(&self) -> Option<&str> {
        let n = self.name.as_str();
        ["a2dp-sink-", "a2dp-source-", "headset-head-unit-", "bap-sink-", "bap-duplex-"]
            .iter()
            .find_map(|p| n.strip_prefix(p))
            .filter(|c| !c.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub nick: String,
    /// "alsa", "bluez5", ...
    pub api: String,
    /// "pci", "usb", "bluetooth"
    pub bus: String,
    pub form_factor: String,
    pub profiles: Vec<Profile>,
    pub active_profile: Option<i32>,
    /// Active routes (one per profile device).
    pub routes: Vec<Route>,
    /// All ports the card has.
    pub enum_routes: Vec<Route>,
    pub props: BTreeMap<String, String>,
}

impl Device {
    pub fn title(&self) -> &str {
        [&self.description, &self.nick, &self.name].into_iter().find(|s| !s.is_empty()).map(String::as_str).unwrap_or("device")
    }
    pub fn active_profile(&self) -> Option<&Profile> {
        let i = self.active_profile?;
        self.profiles.iter().find(|p| p.index == i)
    }
    /// Ports usable with the active profile for a given card-profile device.
    pub fn ports_for(&self, profile_device: i32) -> Vec<&Route> {
        let prof = self.active_profile;
        self.enum_routes
            .iter()
            .filter(|r| r.devices.contains(&profile_device) && prof.is_none_or(|p| r.profiles.is_empty() || r.profiles.contains(&p)))
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Link {
    pub id: u32,
    pub out_node: u32,
    pub in_node: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Client {
    pub id: u32,
    pub app_name: String,
    pub binary: String,
    pub api: String,
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Graph {
    pub nodes: BTreeMap<u32, Node>,
    pub devices: BTreeMap<u32, Device>,
    pub links: Vec<Link>,
    pub clients: BTreeMap<u32, Client>,
    /// node.name of the current default sink/source (what's actually in use).
    pub default_sink: Option<String>,
    pub default_source: Option<String>,
    /// What the user chose (may differ if that device is gone).
    pub configured_sink: Option<String>,
    pub configured_source: Option<String>,
    /// Stream node id -> target node (id or name) from the "default" metadata.
    pub targets: BTreeMap<u32, String>,
    /// "settings" metadata (clock.rate, clock.force-rate, ...).
    pub settings: BTreeMap<String, String>,
    /// WirePlumber "sm-settings" metadata.
    pub sm_settings: BTreeMap<String, String>,
    pub server_version: String,
    pub connected: bool,
}

impl Graph {
    pub fn nodes_of(&self, kind: NodeKind) -> Vec<&Node> {
        let mut v: Vec<&Node> = self.nodes.values().filter(|n| n.kind() == kind).collect();
        v.sort_by_key(|n| (n.title().to_lowercase(), n.id));
        v
    }

    /// The device route that carries this node's volume, if any. PipeWire
    /// device nodes are controlled through their card's Route param, the same
    /// way pavucontrol and wpctl do it.
    pub fn route_for(&self, node: &Node) -> Option<(&Device, &Route)> {
        let dev = self.devices.get(&node.device_id?)?;
        let pd = node.profile_device?;
        let dir = if node.kind() == NodeKind::Source { Dir::Input } else { Dir::Output };
        dev.routes.iter().find(|r| r.device == pd && r.direction == dir).map(|r| (dev, r))
    }

    /// (per-channel linear volumes, mute) as the user should see them.
    pub fn volume_of(&self, node: &Node) -> (Vec<f32>, bool) {
        match self.route_for(node) {
            Some((_, r)) if !r.volumes.is_empty() => (r.volumes.clone(), r.mute),
            _ => (node.volumes.clone(), node.mute),
        }
    }

    pub fn is_default(&self, node: &Node) -> bool {
        match node.kind() {
            NodeKind::Sink => self.default_sink.as_deref() == Some(node.name.as_str()),
            NodeKind::Source => self.default_source.as_deref() == Some(node.name.as_str()),
            _ => false,
        }
    }

    /// Device nodes a stream is currently linked to.
    pub fn connected_devices(&self, stream: &Node) -> Vec<&Node> {
        let mut ids: Vec<u32> = self
            .links
            .iter()
            .filter_map(|l| {
                if l.out_node == stream.id {
                    Some(l.in_node)
                } else if l.in_node == stream.id {
                    Some(l.out_node)
                } else {
                    None
                }
            })
            .collect();
        ids.sort();
        ids.dedup();
        ids.iter().filter_map(|i| self.nodes.get(i)).filter(|n| n.kind().is_device()).collect()
    }

    /// Streams currently linked to a device node.
    pub fn connected_streams(&self, dev: &Node) -> Vec<&Node> {
        let mut ids: Vec<u32> = self
            .links
            .iter()
            .filter_map(|l| if l.in_node == dev.id { Some(l.out_node) } else if l.out_node == dev.id { Some(l.in_node) } else { None })
            .collect();
        ids.sort();
        ids.dedup();
        ids.iter().filter_map(|i| self.nodes.get(i)).filter(|n| n.kind().is_stream()).collect()
    }

    /// Whether a stream follows the default device (no explicit target set).
    pub fn follows_default(&self, stream: &Node) -> bool {
        !self.targets.contains_key(&stream.id)
    }

    pub fn find_by_name(&self, name: &str) -> Option<&Node> {
        self.nodes.values().find(|n| n.name == name)
    }

    pub fn clock_rate(&self) -> Option<u32> {
        self.settings.get("clock.rate").and_then(|v| v.parse().ok())
    }
}

// ---------------------------------------------------------------- volume math

/// PipeWire volumes are linear amplitude. Like pavucontrol, wpctl and
/// wiremix we show them on a cubic scale, which matches loudness perception
/// much better: 50% on screen is about -18 dB.
pub fn linear_to_percent(v: f32) -> f32 {
    v.max(0.0).cbrt() * 100.0
}

pub fn percent_to_linear(p: f32) -> f32 {
    let x = (p / 100.0).max(0.0);
    x * x * x
}

pub fn linear_to_db(v: f32) -> f32 {
    if v <= 0.0 {
        f32::NEG_INFINITY
    } else {
        20.0 * v.log10()
    }
}

/// Average percent across channels (what a single slider shows).
pub fn avg_percent(vols: &[f32]) -> f32 {
    if vols.is_empty() {
        return 0.0;
    }
    vols.iter().map(|v| linear_to_percent(*v)).sum::<f32>() / vols.len() as f32
}

/// Change all channels by `delta` percent, keeping their balance.
pub fn step_all(vols: &[f32], delta: f32, max_percent: f32) -> Vec<f32> {
    let cur = avg_percent(vols);
    let target = (cur + delta).clamp(0.0, max_percent);
    if vols.is_empty() {
        return vec![percent_to_linear(target)];
    }
    if cur <= 0.01 {
        return vec![percent_to_linear(target); vols.len()];
    }
    let ratio = target / cur;
    vols.iter().map(|v| percent_to_linear((linear_to_percent(*v) * ratio).clamp(0.0, max_percent))).collect()
}

/// Set all channels to the same percent.
pub fn set_all(n: usize, percent: f32, max_percent: f32) -> Vec<f32> {
    vec![percent_to_linear(percent.clamp(0.0, max_percent)); n.max(1)]
}

pub fn channel_label(pos: &str) -> &'static str {
    match pos {
        "FL" => "Front left",
        "FR" => "Front right",
        "FC" => "Center",
        "LFE" => "Subwoofer",
        "RL" => "Rear left",
        "RR" => "Rear right",
        "SL" => "Side left",
        "SR" => "Side right",
        "MONO" => "Mono",
        _ => "Channel",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubic_scale() {
        assert!((linear_to_percent(1.0) - 100.0).abs() < 0.01);
        assert!((linear_to_percent(0.125) - 50.0).abs() < 0.01);
        assert!((percent_to_linear(50.0) - 0.125).abs() < 1e-6);
        assert!((linear_to_db(0.125) + 18.06).abs() < 0.05);
    }

    #[test]
    fn stepping_keeps_balance() {
        let v = vec![percent_to_linear(40.0), percent_to_linear(60.0)];
        let out = step_all(&v, 10.0, 100.0);
        let a = linear_to_percent(out[0]);
        let b = linear_to_percent(out[1]);
        assert!((a / b - 40.0 / 60.0).abs() < 0.01, "{a} {b}");
        assert!(((a + b) / 2.0 - 60.0).abs() < 0.1);
        let capped = step_all(&[1.0, 1.0], 20.0, 100.0);
        assert!((linear_to_percent(capped[0]) - 100.0).abs() < 0.01);
        let boosted = step_all(&[1.0, 1.0], 20.0, 150.0);
        assert!((linear_to_percent(boosted[0]) - 120.0).abs() < 0.1);
        let from_zero = step_all(&[0.0, 0.0], 5.0, 100.0);
        assert!((linear_to_percent(from_zero[1]) - 5.0).abs() < 0.01);
    }

    #[test]
    fn route_lookup_and_defaults() {
        let mut g = Graph::default();
        g.devices.insert(
            40,
            Device {
                id: 40,
                routes: vec![Route { index: 3, device: 1, direction: Dir::Output, volumes: vec![0.5, 0.5], mute: true, ..Default::default() }],
                ..Default::default()
            },
        );
        let n = Node { id: 50, name: "alsa_output.x".into(), media_class: "Audio/Sink".into(), device_id: Some(40), profile_device: Some(1), volumes: vec![1.0, 1.0], ..Default::default() };
        g.nodes.insert(50, n.clone());
        g.default_sink = Some("alsa_output.x".into());
        assert_eq!(g.volume_of(&n), (vec![0.5, 0.5], true));
        assert!(g.is_default(&n));
        let virt = Node { id: 60, media_class: "Audio/Sink".into(), volumes: vec![0.3], ..Default::default() };
        assert_eq!(g.volume_of(&virt), (vec![0.3], false));
        assert!(virt.is_virtual());
    }

    #[test]
    fn bluetooth_codec_from_profile() {
        let p = Profile { name: "a2dp-sink-ldac".into(), ..Default::default() };
        assert_eq!(p.codec(), Some("ldac"));
        assert_eq!(Profile { name: "output:analog-stereo".into(), ..Default::default() }.codec(), None);
    }
}
