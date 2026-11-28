//! audio-tui configuration (`~/.config/audio-tui/config.toml`).
//!
//! This file is the single source of truth for everything audio-tui changes
//! on your system. The PipeWire/WirePlumber drop-ins it writes
//! (`60-audio-tui-*.conf`) are generated from it and can be regenerated or
//! removed at any time.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::ui::theme::ThemeConfig;

pub const APP: &str = "audio-tui";

pub const DEFAULT_TOML: &str = r##"# audio-tui configuration
# Every option is listed with its default. Delete a line to restore it.
# Sections marked (system) are turned into PipeWire/WirePlumber drop-in files
# named 60-audio-tui-*.conf when you apply them from the app.

[general]
# Icon set: "nerd" (kitty ships Nerd Font symbols), "unicode", or "ascii"
icons = "nerd"
# Mouse: click/drag sliders, wheel over a slider changes volume, right-click for menus
mouse = true
# Show the details panel when the terminal is wide enough
details = true
# Live level meters (each visible device/app gets a tiny monitor stream)
meters = true
# Volume keys change by this many percent (Shift = 1%)
volume_step = 5
# Highest volume allowed, in percent. Above 100 boosts digitally (may distort).
max_volume = 100
# Show audio-tui's own meter streams and other monitor/peak streams
show_internal_streams = false
# Ask before restarting PipeWire/WirePlumber to apply system changes
confirm_restart = true

# Display-only names (instant, nothing else changes). Key = node.name.
# Use "Rename" in the app for a real rename that other apps see too.
[aliases]
# "alsa_output.pci-0000_00_1f.3.analog-stereo" = "Desk speakers"

# (system) Real device settings, applied through WirePlumber rules.
# Key = node.name (an output/input) or device.name (a whole card).
# description = "Speakers"   rename everywhere (pavucontrol, browsers, games...)
# disabled = true            hide it completely
# priority = 2000            higher = preferred as automatic default
[devices]
# [devices."alsa_output.pci-0000_00_1f.3.hdmi-stereo"]
# disabled = true

# (system) What WirePlumber is allowed to remember about the apps you use.
[privacy]
# Remember each app's volume/mute between runs
remember_volumes = true
# Remember which output/input each app was sent to
remember_devices = true
# "all": remember every app (WirePlumber default)
# "none": remember nothing about apps (except the allowlist below)
mode = "all"
# Apps always remembered (volume and device), even when the switches above
# are off. Names as shown in the Playback tab, e.g. ["Firefox", "Spotify"].
# Toggle the selected app with "s" in Playback/Recording.
allowlist = []
# On start, delete remembered entries whose key is a media/page title
prune_titles_on_start = false

# (system) Sample rate and buffer size. 0 = leave PipeWire's default.
[clock]
rate = 0
# e.g. [44100, 48000, 96000] lets PipeWire switch rate to match the music
allowed_rates = []
quantum = 0
min_quantum = 0
max_quantum = 0

# (system) Always send an app to a device. app = name in the Playback tab,
# target = node.name of the output. Example:
# [[routing]]
# app = "Spotify"
# target = "alsa_output.usb-FiiO_K5-00.analog-stereo"

# (system) Virtual devices. kinds:
#   "null-sink"   an empty output you can record from (e.g. for streaming)
#   "combine"     play to several outputs at once (targets = [...])
#   "loopback"    hear an input through an output (source -> target)
#   "echo-cancel" a cleaned-up microphone that removes speaker echo
#   "noise"       a noise-suppressed microphone (needs noise-suppression-for-voice)
#   "equalizer"   an output with an equalizer in front of `target`
# [[virtual]]
# kind = "null-sink"
# name = "stream_mix"
# description = "Stream mix"
#
# [[virtual]]
# kind = "equalizer"
# name = "eq_speakers"
# description = "Speakers (EQ)"
# target = "alsa_output.pci-0000_00_1f.3.analog-stereo"
# bands = [ { freq = 60, gain = 3.0, q = 0.7 }, { freq = 1000, gain = 0.0, q = 1.0 }, { freq = 8000, gain = -2.0, q = 0.7 } ]

[theme]
# preset = "dark"        # "dark" or "mono". The NO_COLOR env var also forces mono.
# accent = "#cba6f7"

[keys]
# Rebind any action: action_name = ["key", ...]. [] unbinds. Press ? for names.
# volume_up = ["l", "right", "+"]
"##;

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct Config {
    pub general: General,
    pub aliases: BTreeMap<String, String>,
    pub devices: BTreeMap<String, DeviceRule>,
    pub privacy: Privacy,
    pub clock: Clock,
    pub routing: Vec<RouteRule>,
    #[serde(rename = "virtual")]
    pub virtuals: Vec<Virtual>,
    pub theme: ThemeConfig,
    pub keys: std::collections::HashMap<String, Vec<String>>,
}


#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum IconSet {
    Nerd,
    Unicode,
    Ascii,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct General {
    #[serde(default = "d_icons")]
    pub icons: IconSet,
    #[serde(default = "d_true")]
    pub mouse: bool,
    #[serde(default = "d_true")]
    pub details: bool,
    #[serde(default = "d_true")]
    pub meters: bool,
    #[serde(default = "d_step")]
    pub volume_step: u8,
    #[serde(default = "d_max")]
    pub max_volume: u16,
    #[serde(default)]
    pub show_internal_streams: bool,
    #[serde(default = "d_true")]
    pub confirm_restart: bool,
}

impl Default for General {
    fn default() -> Self {
        toml::from_str("").expect("defaults")
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DeviceRule {
    pub description: Option<String>,
    pub disabled: Option<bool>,
    pub priority: Option<i64>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PrivacyMode {
    All,
    Allowlist,
    None,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Privacy {
    #[serde(default = "d_true")]
    pub remember_volumes: bool,
    #[serde(default = "d_true")]
    pub remember_devices: bool,
    #[serde(default = "d_mode")]
    pub mode: PrivacyMode,
    #[serde(default)]
    pub allowlist: Vec<String>,
    #[serde(default)]
    pub prune_titles_on_start: bool,
}

impl Privacy {
    /// Whether apps not on the allowlist get their (volume, device) remembered.
    pub fn remembers(&self) -> (bool, bool) {
        let on = self.mode == PrivacyMode::All;
        (self.remember_volumes && on, self.remember_devices && on)
    }

    pub fn is_listed(&self, names: &[&str]) -> bool {
        self.allowlist.iter().any(|a| names.iter().any(|n| !n.is_empty() && a.trim() == *n))
    }
}

impl Default for Privacy {
    fn default() -> Self {
        toml::from_str("").expect("defaults")
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Clock {
    pub rate: u32,
    pub allowed_rates: Vec<u32>,
    pub quantum: u32,
    pub min_quantum: u32,
    pub max_quantum: u32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RouteRule {
    pub app: String,
    pub target: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum VirtualKind {
    NullSink,
    Combine,
    Loopback,
    EchoCancel,
    Noise,
    Equalizer,
}

impl VirtualKind {
    pub const ALL: [VirtualKind; 6] = [Self::NullSink, Self::Combine, Self::Loopback, Self::EchoCancel, Self::Noise, Self::Equalizer];
    pub fn key(self) -> &'static str {
        match self {
            Self::NullSink => "null-sink",
            Self::Combine => "combine",
            Self::Loopback => "loopback",
            Self::EchoCancel => "echo-cancel",
            Self::Noise => "noise",
            Self::Equalizer => "equalizer",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::NullSink => "Virtual output (null sink)",
            Self::Combine => "Combined output (several devices)",
            Self::Loopback => "Loopback (hear an input)",
            Self::EchoCancel => "Echo-cancelled microphone",
            Self::Noise => "Noise-suppressed microphone",
            Self::Equalizer => "Equalizer",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct Band {
    pub freq: f32,
    pub gain: f32,
    #[serde(default = "d_q")]
    pub q: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Virtual {
    pub kind: VirtualKind,
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// Output to play into (loopback, equalizer) / source to capture (noise, echo-cancel).
    #[serde(default)]
    pub target: String,
    /// Input to listen to (loopback).
    #[serde(default)]
    pub source: String,
    /// Outputs for "combine".
    #[serde(default)]
    pub targets: Vec<String>,
    #[serde(default)]
    pub bands: Vec<Band>,
    #[serde(default = "d_channels")]
    pub channels: u32,
}

fn d_true() -> bool {
    true
}
fn d_icons() -> IconSet {
    IconSet::Nerd
}
fn d_step() -> u8 {
    5
}
fn d_max() -> u16 {
    100
}
fn d_mode() -> PrivacyMode {
    PrivacyMode::All
}
fn d_q() -> f32 {
    1.0
}
fn d_channels() -> u32 {
    2
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_matches_struct_defaults() {
        let c: Config = toml::from_str(DEFAULT_TOML).unwrap();
        let d = Config::default();
        assert_eq!(c.general.volume_step, d.general.volume_step);
        assert_eq!(c.general.max_volume, d.general.max_volume);
        assert_eq!(c.privacy, d.privacy);
        assert_eq!(c.clock, d.clock);
        assert!(c.virtuals.is_empty() && c.routing.is_empty() && c.devices.is_empty());
    }

    #[test]
    fn full_example_parses() {
        let t = r#"
[devices."alsa_output.hdmi"]
disabled = true
[devices."alsa_output.usb"]
description = "Desk DAC"
priority = 2000
[privacy]
mode = "allowlist"
allowlist = ["Firefox"]
[[routing]]
app = "Spotify"
target = "alsa_output.usb"
[[virtual]]
kind = "equalizer"
name = "eq"
target = "alsa_output.usb"
bands = [ { freq = 60, gain = 3.0 } ]
"#;
        let c: Config = toml::from_str(t).unwrap();
        assert_eq!(c.devices["alsa_output.hdmi"].disabled, Some(true));
        assert_eq!(c.privacy.mode, PrivacyMode::Allowlist);
        assert_eq!(c.virtuals[0].kind, VirtualKind::Equalizer);
        assert_eq!(c.virtuals[0].bands[0].q, 1.0);
    }
}
