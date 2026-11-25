//! WirePlumber's remembered state (`~/.local/state/wireplumber/`).
//!
//! WirePlumber writes GLib key files: one `[group]` per file and
//! `key=value` lines where the value is JSON. Keys escape a few characters
//! (see wireplumber lib/wp/state.c):  `\\`→`\`, `\s`→space, `\e`→`=`,
//! `\o`→`[`, `\c`→`]`.
//!
//! Stream keys look like `Output/Audio:application.name:Firefox` (0.5.x) and
//! may carry a port-group suffix on newer versions. The part after the
//! property name is whatever the app reported, so when an app has no name
//! WirePlumber falls back to `media.name` — which can be a song, video or
//! web page title. That's the privacy issue this module surfaces.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// Key exactly as stored (escaped).
    pub raw_key: String,
    /// Unescaped key.
    pub key: String,
    pub value: String,
    pub kind: EntryKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum EntryKind {
    /// A remembered app stream: direction ("Output"/"Input"), the property used
    /// to identify it, and that property's value.
    Stream { dir: String, by: String, who: String },
    Device,
    Other,
}

impl Entry {
    /// True when the key itself may reveal what you were playing/watching.
    pub fn leaks_title(&self) -> bool {
        matches!(&self.kind, EntryKind::Stream { by, .. } if by == "media.name")
    }

    /// Human summary of the stored value (volume, mute, target).
    pub fn summary(&self) -> String {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&self.value) else {
            return self.value.chars().take(60).collect();
        };
        let mut parts = Vec::new();
        if let Some(vol) = v.get("volume").and_then(|x| x.as_f64()) {
            parts.push(format!("volume {:.0}%", (vol as f32).cbrt() * 100.0));
        } else if let Some(cv) = v.get("channelVolumes").and_then(|x| x.as_array()) {
            let avg: f64 = cv.iter().filter_map(|x| x.as_f64()).sum::<f64>() / cv.len().max(1) as f64;
            parts.push(format!("volume {:.0}%", (avg as f32).cbrt() * 100.0));
        }
        if v.get("mute").and_then(|x| x.as_bool()) == Some(true) {
            parts.push("muted".into());
        }
        for k in ["target", "target.object", "target.node"] {
            if let Some(t) = v.get(k).and_then(|x| x.as_str()) {
                parts.push(format!("→ {t}"));
            }
        }
        if v.get("name").is_some() || parts.is_empty() {
            if let Some(n) = v.get("name").and_then(|x| x.as_str()) {
                parts.push(n.to_string());
            } else if v.is_string() {
                parts.push(v.as_str().unwrap_or_default().to_string());
            }
        }
        if parts.is_empty() {
            self.value.chars().take(60).collect()
        } else {
            parts.join(", ")
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StateFile {
    pub name: String,
    pub path: PathBuf,
    pub group: String,
    pub entries: Vec<Entry>,
    pub bytes: u64,
}

impl StateFile {
    pub fn description(&self) -> &'static str {
        match self.name.as_str() {
            "stream-properties" | "stream-props" | "restore-stream" => "volume & mute remembered per app",
            "stream-target" => "which device each app was sent to",
            "default-nodes" => "your chosen default output/input",
            "default-routes" => "port & volume per sound card profile",
            "default-profile" => "profile chosen per sound card",
            "sm-settings" => "settings saved with `wpctl settings --save`",
            "bluetooth-autoswitch" => "Bluetooth headset auto-switch memory",
            _ => "",
        }
    }
}

pub fn state_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"))
        .join("wireplumber")
}

pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => out.push('\\'),
            Some('s') => out.push(' '),
            Some('e') => out.push('='),
            Some('o') => out.push('['),
            Some('c') => out.push(']'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg_attr(not(test), allow(dead_code))] // counterpart of unescape; used by tests
pub fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ' ' => out.push_str("\\s"),
            '=' => out.push_str("\\e"),
            '[' => out.push_str("\\o"),
            ']' => out.push_str("\\c"),
            c => out.push(c),
        }
    }
    out
}

fn classify(key: &str) -> EntryKind {
    // "Output/Audio:application.name:Firefox[:stream:...]" or
    // "Output/Audio:media.role:Notification"
    let mut parts = key.splitn(3, ':');
    let (Some(head), Some(by), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return if key.contains("alsa_card") || key.contains("bluez_card") || key.contains(':') { EntryKind::Device } else { EntryKind::Other };
    };
    // state-stream keys are "<media.class minus Stream/>:<prop>:<value>", e.g.
    // "Output/Audio", "Input/Audio/Internal" or "Audio/Sink" for virtual sinks.
    let stream_prop = matches!(by, "application.id" | "application.name" | "application.process.binary" | "media.name" | "media.role" | "node.name");
    if stream_prop && !head.contains("_card") {
        let dir = if head.starts_with("Input") || head.starts_with("Audio/Source") { "Input" } else { "Output" };
        let who = rest.split(":stream:").next().unwrap_or(rest).to_string();
        return EntryKind::Stream { dir: dir.to_string(), by: by.to_string(), who };
    }
    EntryKind::Device
}

/// Parse one key file. Unknown lines are ignored.
pub fn parse(name: &str, path: &Path, text: &str) -> StateFile {
    let mut group = String::new();
    let mut entries = Vec::new();
    for line in text.lines() {
        let l = line.trim_end();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        if l.starts_with('[') && l.ends_with(']') {
            group = l[1..l.len() - 1].to_string();
            continue;
        }
        // Keys are escaped, so the first literal '=' separates key and value.
        if let Some((k, v)) = l.split_once('=') {
            let key = unescape(k);
            entries.push(Entry { raw_key: k.to_string(), kind: classify(&key), key, value: v.to_string() });
        }
    }
    StateFile { name: name.to_string(), path: path.to_path_buf(), group, entries, bytes: text.len() as u64 }
}

/// Serialize back (used after removing entries).
pub fn render(f: &StateFile) -> String {
    let mut out = format!("[{}]\n", f.group);
    for e in &f.entries {
        out.push_str(&e.raw_key);
        out.push('=');
        out.push_str(&e.value);
        out.push('\n');
    }
    out
}

/// Load every state file in `dir`, sorted with the privacy-relevant ones first.
pub fn load_all(dir: &Path) -> Vec<StateFile> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<StateFile> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') || name.ends_with('~') || name.ends_with(".tmp") || name.contains(".bak") {
                return None;
            }
            let text = std::fs::read_to_string(e.path()).ok()?;
            Some(parse(&name, &e.path(), &text))
        })
        .collect();
    let rank = |n: &str| match n {
        "stream-properties" | "stream-props" | "restore-stream" => 0,
        "stream-target" => 1,
        "default-nodes" => 2,
        _ => 3,
    };
    files.sort_by(|a, b| (rank(&a.name), &a.name).cmp(&(rank(&b.name), &b.name)));
    files
}

/// Remove entries (by raw key) from a state file on disk.
pub fn remove_entries(path: &Path, raw_keys: &[String]) -> std::io::Result<usize> {
    let text = std::fs::read_to_string(path)?;
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let mut f = parse(&name, path, &text);
    let before = f.entries.len();
    f.entries.retain(|e| !raw_keys.contains(&e.raw_key));
    let removed = before - f.entries.len();
    if removed > 0 {
        let tmp = path.with_extension("audio-tui.tmp");
        std::fs::write(&tmp, render(&f))?;
        std::fs::rename(&tmp, path)?;
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[stream-properties]\n\
Output/Audio:application.name:Firefox={\"volume\":0.421875, \"mute\":false, \"channelVolumes\":[0.42, 0.42], \"channelMap\":[\"FL\", \"FR\"]}\n\
Output/Audio:media.name:Never\\sGonna\\sGive\\sYou\\sUp\\s(Official\\sVideo)\\s\\o4K\\c={\"volume\":1.0, \"mute\":false}\n\
Output/Audio:media.role:Notification={\"volume\":0.3, \"mute\":true}\n\
Input/Audio:application.id:org.signal.Signal:stream:capture_1={\"volume\":1.0}\n";

    #[test]
    fn parses_and_flags_titles() {
        let f = parse("stream-properties", Path::new("/x"), SAMPLE);
        assert_eq!(f.group, "stream-properties");
        assert_eq!(f.entries.len(), 4);
        let title = &f.entries[1];
        assert_eq!(title.key, "Output/Audio:media.name:Never Gonna Give You Up (Official Video) [4K]");
        assert!(title.leaks_title());
        assert!(!f.entries[0].leaks_title());
        assert_eq!(f.entries[0].kind, EntryKind::Stream { dir: "Output".into(), by: "application.name".into(), who: "Firefox".into() });
        assert_eq!(f.entries[3].kind, EntryKind::Stream { dir: "Input".into(), by: "application.id".into(), who: "org.signal.Signal".into() });
        assert_eq!(f.entries[0].summary(), "volume 75%");
        assert_eq!(f.entries[2].summary(), "volume 67%, muted");
    }

    #[test]
    fn escape_round_trip() {
        let s = "a b=c[d]e\\f";
        assert_eq!(unescape(&escape(s)), s);
        assert_eq!(escape("x y"), "x\\sy");
    }

    #[test]
    fn render_round_trip_and_remove() {
        let dir = std::env::temp_dir().join(format!("audio-tui-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("stream-properties");
        std::fs::write(&p, SAMPLE).unwrap();
        let f = parse("stream-properties", &p, SAMPLE);
        assert_eq!(render(&f), SAMPLE);
        let key = f.entries[1].raw_key.clone();
        assert_eq!(remove_entries(&p, &[key]).unwrap(), 1);
        let after = load_all(&dir);
        assert_eq!(after[0].entries.len(), 3);
        assert!(after[0].entries.iter().all(|e| !e.leaks_title()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
