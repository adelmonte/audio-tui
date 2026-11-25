//! Edits to audio-tui's own config.toml that keep comments and layout
//! (toml_edit). After each edit the file is re-parsed so the in-memory
//! config always matches what's on disk.

use std::path::Path;

use toml_edit::{value, Array, ArrayOfTables, DocumentMut, InlineTable, Item, Table};

use crate::config::{Band, Config, RouteRule, Virtual};

pub struct Edit {
    doc: DocumentMut,
}

impl Edit {
    pub fn open(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let doc = text.parse::<DocumentMut>().map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { doc })
    }

    /// Write back and return the freshly parsed config.
    pub fn save(self, path: &Path) -> Result<Config, String> {
        let text = self.doc.to_string();
        let cfg: Config = toml::from_str(&text).map_err(|e| format!("edit produced an invalid config: {}", e.message()))?;
        crate::core::config::write_atomic(path, &text).map_err(|e| e.to_string())?;
        Ok(cfg)
    }

    fn table(&mut self, name: &str) -> &mut Table {
        if !self.doc.contains_key(name) || !self.doc[name].is_table() {
            self.doc[name] = Item::Table(Table::new());
        }
        self.doc[name].as_table_mut().expect("table")
    }

    pub fn set(&mut self, section: &str, key: &str, v: Item) {
        self.table(section)[key] = v;
    }

    pub fn alias(&mut self, node_name: &str, alias: Option<&str>) {
        let t = self.table("aliases");
        match alias.filter(|a| !a.trim().is_empty()) {
            Some(a) => t[node_name] = value(a.trim()),
            None => {
                t.remove(node_name);
            }
        }
    }

    /// Update one field of `[devices."name"]`; None removes it. Empty rules are removed.
    pub fn device(&mut self, name: &str, field: &str, v: Option<Item>) {
        let devices = self.table("devices");
        devices.set_implicit(true);
        if !devices.contains_key(name) {
            devices[name] = Item::Table(Table::new());
        }
        let t = devices[name].as_table_mut().expect("device table");
        match v {
            Some(v) => t[field] = v,
            None => {
                t.remove(field);
            }
        }
        if t.is_empty() {
            devices.remove(name);
        }
    }

    pub fn string_list(list: &[String]) -> Item {
        let mut a = Array::new();
        for s in list {
            a.push(s.as_str());
        }
        value(a)
    }

    fn aot(&mut self, name: &str) -> &mut ArrayOfTables {
        if !self.doc.contains_key(name) || !self.doc[name].is_array_of_tables() {
            self.doc[name] = Item::ArrayOfTables(ArrayOfTables::new());
        }
        self.doc[name].as_array_of_tables_mut().expect("aot")
    }

    pub fn add_route(&mut self, r: &RouteRule) {
        // Replace an existing rule for the same app.
        self.remove_route(&r.app);
        let mut t = Table::new();
        t["app"] = value(r.app.as_str());
        t["target"] = value(r.target.as_str());
        self.aot("routing").push(t);
    }

    pub fn remove_route(&mut self, app: &str) {
        if let Some(a) = self.doc.get_mut("routing").and_then(Item::as_array_of_tables_mut) {
            a.retain(|t| t.get("app").and_then(Item::as_str) != Some(app));
        }
    }

    pub fn add_virtual(&mut self, v: &Virtual) {
        let mut t = Table::new();
        t["kind"] = value(v.kind.key());
        t["name"] = value(v.name.as_str());
        if !v.description.is_empty() {
            t["description"] = value(v.description.as_str());
        }
        if !v.target.is_empty() {
            t["target"] = value(v.target.as_str());
        }
        if !v.source.is_empty() {
            t["source"] = value(v.source.as_str());
        }
        if !v.targets.is_empty() {
            t["targets"] = Self::string_list(&v.targets);
        }
        if !v.bands.is_empty() {
            let mut a = Array::new();
            for b in &v.bands {
                let mut it = InlineTable::new();
                it.insert("freq", (b.freq as f64).into());
                it.insert("gain", (b.gain as f64).into());
                it.insert("q", (b.q as f64).into());
                a.push(it);
            }
            t["bands"] = value(a);
        }
        if v.channels != 2 {
            t["channels"] = value(v.channels as i64);
        }
        self.aot("virtual").push(t);
    }

    pub fn remove_virtual(&mut self, name: &str) {
        if let Some(a) = self.doc.get_mut("virtual").and_then(Item::as_array_of_tables_mut) {
            a.retain(|t| t.get("name").and_then(Item::as_str) != Some(name));
        }
    }
}

/// "60:+3:0.7, 1000:0, 8000:-2:0.7" -> bands. Q defaults to 1.0.
pub fn parse_bands(s: &str) -> Result<Vec<Band>, String> {
    let mut out = Vec::new();
    for part in s.split([',', ';']).map(str::trim).filter(|p| !p.is_empty()) {
        let f: Vec<&str> = part.split(':').map(str::trim).collect();
        let num = |x: &str| x.trim_start_matches('+').trim_end_matches("dB").trim_end_matches("Hz").parse::<f32>().map_err(|_| format!("“{part}”: use freq:gain[:q], e.g. 60:+3:0.7"));
        let (freq, gain, q) = match f.as_slice() {
            [fr, g] => (num(fr)?, num(g)?, 1.0),
            [fr, g, q] => (num(fr)?, num(g)?, num(q)?),
            _ => return Err(format!("“{part}”: use freq:gain[:q], e.g. 60:+3:0.7")),
        };
        if !(10.0..=24_000.0).contains(&freq) || !(-24.0..=24.0).contains(&gain) || !(0.05..=20.0).contains(&q) {
            return Err(format!("“{part}”: freq 10–24000 Hz, gain ±24 dB, q 0.05–20"));
        }
        out.push(Band { freq, gain, q });
    }
    if out.is_empty() {
        return Err("add at least one band, e.g. 60:+3, 1000:0, 8000:-2".into());
    }
    Ok(out)
}

#[cfg_attr(not(test), allow(dead_code))]
pub fn bands_text(b: &[Band]) -> String {
    b.iter().map(|b| format!("{}:{:+}:{}", b.freq, b.gain, b.q)).collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{VirtualKind, DEFAULT_TOML};

    #[test]
    fn edits_keep_comments_and_parse() {
        let dir = std::env::temp_dir().join(format!("audio-tui-cfgedit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, DEFAULT_TOML).unwrap();
        let mut e = Edit::open(&p).unwrap();
        e.device("alsa_output.usb x", "description", Some(value("Desk DAC")));
        e.alias("alsa_output.hdmi", Some("TV"));
        e.set("privacy", "mode", value("allowlist"));
        e.set("privacy", "allowlist", Edit::string_list(&["Firefox".into()]));
        e.add_route(&RouteRule { app: "Spotify".into(), target: "alsa_output.usb".into() });
        e.add_route(&RouteRule { app: "Spotify".into(), target: "alsa_output.other".into() });
        e.add_virtual(&Virtual { kind: VirtualKind::Equalizer, name: "eq".into(), description: "EQ".into(), target: "x".into(), source: String::new(), targets: vec![], bands: parse_bands("60:+3:0.7, 8000:-2").unwrap(), channels: 2 });
        let cfg = e.save(&p).unwrap();
        assert_eq!(cfg.devices["alsa_output.usb x"].description.as_deref(), Some("Desk DAC"));
        assert_eq!(cfg.aliases["alsa_output.hdmi"], "TV");
        assert_eq!(cfg.routing.len(), 1);
        assert_eq!(cfg.routing[0].target, "alsa_output.other");
        assert_eq!(cfg.virtuals[0].bands.len(), 2);
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("# Remember each app's volume/mute between runs"), "comments kept");

        let mut e = Edit::open(&p).unwrap();
        e.device("alsa_output.usb x", "description", None);
        e.remove_virtual("eq");
        e.remove_route("Spotify");
        let cfg = e.save(&p).unwrap();
        assert!(cfg.devices.is_empty());
        assert!(cfg.virtuals.is_empty() && cfg.routing.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn band_parsing() {
        let b = parse_bands("60:+3:0.7, 1000:0, 8000Hz:-2.5dB").unwrap();
        assert_eq!(b.len(), 3);
        assert_eq!(b[1].q, 1.0);
        assert_eq!(b[2].gain, -2.5);
        assert!(parse_bands("5:+3").is_err());
        assert!(parse_bands("abc").is_err());
        assert_eq!(bands_text(&b[..1]), "60:+3:0.7");
    }
}
