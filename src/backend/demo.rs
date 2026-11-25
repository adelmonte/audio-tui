//! Demo backend: a realistic fake graph (laptop speakers + HDMI, a USB DAC,
//! Bluetooth headphones with codecs, a webcam mic, a few apps) with animated
//! level meters. Commands change the graph like the real thing would.

use std::collections::BTreeMap;
use std::time::Duration;

use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use super::model::*;
use super::{Cmd, CmdTx, Ev, Handle, Peaks};

fn props(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn stereo() -> Vec<String> {
    vec!["FL".into(), "FR".into()]
}

fn profile(index: i32, name: &str, description: &str, available: Avail, priority: i32) -> Profile {
    Profile { index, name: name.into(), description: description.into(), available, priority }
}

#[allow(clippy::too_many_arguments)]
fn route(index: i32, device: i32, name: &str, description: &str, dir: Dir, available: Avail, profiles: Vec<i32>, vol: f32) -> Route {
    Route {
        index,
        device,
        name: name.into(),
        description: description.into(),
        direction: dir,
        available,
        priority: 100,
        profiles,
        devices: vec![device],
        volumes: vec![vol, vol],
        mute: false,
        latency_offset_ns: 0,
    }
}

fn device_node(id: u32, name: &str, desc: &str, class: &str, dev: Option<u32>, pd: Option<i32>, vol: f32) -> Node {
    Node {
        id,
        serial: id as u64 + 1000,
        name: name.into(),
        description: desc.into(),
        media_class: class.into(),
        device_id: dev,
        profile_device: pd,
        volumes: vec![vol, vol],
        channels: stereo(),
        props: props(&[("node.name", name), ("media.class", class), ("audio.rate", "48000")]),
        ..Default::default()
    }
}

fn stream(id: u32, app: &str, binary: &str, media: &str, class: &str, vol: f32) -> Node {
    Node {
        id,
        serial: id as u64 + 1000,
        name: binary.to_string(),
        media_class: class.into(),
        app_name: app.into(),
        app_binary: binary.into(),
        media_name: media.into(),
        volumes: vec![vol, vol],
        channels: stereo(),
        props: props(&[("application.name", app), ("application.process.binary", binary), ("media.name", media), ("client.api", "pipewire-pulse")]),
        ..Default::default()
    }
}

pub fn sample_graph() -> Graph {
    let mut g = Graph { connected: true, server_version: "1.4.9".into(), ..Default::default() };
    // Built-in card: speakers/headphones + HDMI profile.
    g.devices.insert(
        40,
        Device {
            id: 40,
            name: "alsa_card.pci-0000_00_1f.3".into(),
            description: "Built-in Audio".into(),
            nick: "HDA Intel PCH".into(),
            api: "alsa".into(),
            bus: "pci".into(),
            form_factor: "internal".into(),
            profiles: vec![
                profile(0, "off", "Off", Avail::Yes, 0),
                profile(1, "output:analog-stereo+input:analog-stereo", "Analog Stereo Duplex", Avail::Yes, 6565),
                profile(2, "output:analog-stereo", "Analog Stereo Output", Avail::Yes, 6500),
                profile(3, "output:hdmi-stereo", "Digital Stereo (HDMI) Output", Avail::Yes, 5900),
                profile(4, "output:hdmi-surround", "Digital Surround 5.1 (HDMI) Output", Avail::No, 800),
                profile(5, "pro-audio", "Pro Audio", Avail::Yes, 1),
            ],
            active_profile: Some(1),
            routes: vec![
                route(2, 0, "analog-output-speaker", "Speakers", Dir::Output, Avail::Unknown, vec![1, 2], 0.41),
                route(0, 1, "analog-input-internal-mic", "Internal Microphone", Dir::Input, Avail::Unknown, vec![1], 0.60),
            ],
            enum_routes: vec![
                route(0, 1, "analog-input-internal-mic", "Internal Microphone", Dir::Input, Avail::Unknown, vec![1], 1.0),
                route(1, 1, "analog-input-mic", "Microphone (jack)", Dir::Input, Avail::No, vec![1], 1.0),
                route(2, 0, "analog-output-speaker", "Speakers", Dir::Output, Avail::Unknown, vec![1, 2], 1.0),
                route(3, 0, "analog-output-headphones", "Headphones", Dir::Output, Avail::No, vec![1, 2], 1.0),
                route(4, 2, "hdmi-output-0", "HDMI / DisplayPort", Dir::Output, Avail::Yes, vec![3, 4], 1.0),
            ],
            props: props(&[("device.api", "alsa"), ("device.bus", "pci"), ("api.alsa.card.name", "HDA Intel PCH")]),
        },
    );
    g.nodes.insert(50, device_node(50, "alsa_output.pci-0000_00_1f.3.analog-stereo", "Built-in Audio Analog Stereo", "Audio/Sink", Some(40), Some(0), 1.0));
    g.nodes.insert(51, device_node(51, "alsa_input.pci-0000_00_1f.3.analog-stereo", "Built-in Audio Analog Stereo", "Audio/Source", Some(40), Some(1), 1.0));

    // USB DAC.
    g.devices.insert(
        41,
        Device {
            id: 41,
            name: "alsa_card.usb-FiiO_K5-00".into(),
            description: "FiiO K5 Pro".into(),
            api: "alsa".into(),
            bus: "usb".into(),
            profiles: vec![profile(0, "off", "Off", Avail::Yes, 0), profile(1, "output:analog-stereo", "Analog Stereo Output", Avail::Yes, 6500)],
            active_profile: Some(1),
            routes: vec![route(0, 0, "analog-output", "Analog Output", Dir::Output, Avail::Unknown, vec![1], 0.22)],
            enum_routes: vec![route(0, 0, "analog-output", "Analog Output", Dir::Output, Avail::Unknown, vec![1], 1.0)],
            props: props(&[("device.api", "alsa"), ("device.bus", "usb"), ("device.vendor.name", "FiiO")]),
            ..Default::default()
        },
    );
    g.nodes.insert(52, device_node(52, "alsa_output.usb-FiiO_K5-00.analog-stereo", "FiiO K5 Pro Analog Stereo", "Audio/Sink", Some(41), Some(0), 1.0));

    // Bluetooth headphones with codec profiles.
    g.devices.insert(
        42,
        Device {
            id: 42,
            name: "bluez_card.AC_80_0A_3F_12_9E".into(),
            description: "WH-1000XM5".into(),
            api: "bluez5".into(),
            bus: "bluetooth".into(),
            form_factor: "headphone".into(),
            profiles: vec![
                profile(0, "off", "Off", Avail::Yes, 0),
                profile(1, "a2dp-sink-sbc", "High Fidelity Playback (A2DP Sink, codec SBC)", Avail::Yes, 18),
                profile(2, "a2dp-sink-aac", "High Fidelity Playback (A2DP Sink, codec AAC)", Avail::Yes, 19),
                profile(3, "a2dp-sink-ldac", "High Fidelity Playback (A2DP Sink, codec LDAC)", Avail::Yes, 20),
                profile(4, "headset-head-unit-msbc", "Headset Head Unit (HSP/HFP, codec mSBC)", Avail::Yes, 2),
            ],
            active_profile: Some(3),
            routes: vec![route(0, 0, "headset-output", "Headphones", Dir::Output, Avail::Yes, vec![1, 2, 3, 4], 0.34)],
            enum_routes: vec![
                route(0, 0, "headset-output", "Headphones", Dir::Output, Avail::Yes, vec![1, 2, 3, 4], 1.0),
                route(1, 1, "headset-input", "Headset microphone", Dir::Input, Avail::Yes, vec![4], 1.0),
            ],
            props: props(&[("device.api", "bluez5"), ("device.bus", "bluetooth"), ("api.bluez5.address", "AC:80:0A:3F:12:9E")]),
            ..Default::default()
        },
    );
    let mut bt = device_node(53, "bluez_output.AC_80_0A_3F_12_9E.1", "WH-1000XM5", "Audio/Sink", Some(42), Some(0), 1.0);
    bt.props.insert("device.api".into(), "bluez5".into());
    bt.props.insert("api.bluez5.codec".into(), "ldac".into());
    g.nodes.insert(53, bt);

    // Webcam mic.
    g.devices.insert(
        43,
        Device {
            id: 43,
            name: "alsa_card.usb-046d_C920".into(),
            description: "HD Pro Webcam C920".into(),
            api: "alsa".into(),
            bus: "usb".into(),
            profiles: vec![profile(0, "off", "Off", Avail::Yes, 0), profile(1, "input:analog-stereo", "Analog Stereo Input", Avail::Yes, 65)],
            active_profile: Some(1),
            routes: vec![route(0, 0, "analog-input-mic", "Microphone", Dir::Input, Avail::Unknown, vec![1], 0.85)],
            enum_routes: vec![route(0, 0, "analog-input-mic", "Microphone", Dir::Input, Avail::Unknown, vec![1], 1.0)],
            props: props(&[("device.api", "alsa"), ("device.bus", "usb")]),
            ..Default::default()
        },
    );
    g.nodes.insert(54, device_node(54, "alsa_input.usb-046d_C920.analog-stereo", "HD Pro Webcam C920", "Audio/Source", Some(43), Some(0), 1.0));

    // A virtual output, e.g. for OBS.
    let mut mix = device_node(55, "stream_mix", "Stream mix (virtual)", "Audio/Sink", None, None, 1.0);
    mix.props.insert("factory.name".into(), "support.null-audio-sink".into());
    g.nodes.insert(55, mix);

    // Apps.
    g.nodes.insert(60, stream(60, "Firefox", "firefox", "lofi hip hop radio 📚 beats to relax/study to - YouTube", "Stream/Output/Audio", 0.75));
    g.nodes.insert(61, stream(61, "Spotify", "spotify", "Daft Punk — Digital Love", "Stream/Output/Audio", 0.42));
    g.nodes.insert(62, stream(62, "Discord", "Discord", "playback", "Stream/Output/Audio", 1.0));
    g.nodes.insert(63, stream(63, "Discord", "Discord", "recStream", "Stream/Input/Audio", 1.0));
    g.nodes.insert(64, stream(64, "OBS Studio", "obs", "Desktop audio capture", "Stream/Input/Audio", 1.0));
    let mut notif = stream(65, "", "pw-play", "bell-ring.oga", "Stream/Output/Audio", 0.3);
    notif.props.insert("media.role".into(), "Notification".into());
    g.nodes.insert(65, notif);

    g.links = vec![
        Link { id: 100, out_node: 60, in_node: 52 },
        Link { id: 101, out_node: 61, in_node: 52 },
        Link { id: 102, out_node: 62, in_node: 53 },
        Link { id: 103, out_node: 54, in_node: 63 },
        Link { id: 104, out_node: 55, in_node: 64 },
        Link { id: 105, out_node: 65, in_node: 52 },
    ];
    g.targets.insert(62, "53".into());
    g.targets.insert(64, "55".into());
    g.default_sink = Some("alsa_output.usb-FiiO_K5-00.analog-stereo".into());
    g.configured_sink = g.default_sink.clone();
    g.default_source = Some("alsa_input.usb-046d_C920.analog-stereo".into());
    g.configured_source = g.default_source.clone();
    g.settings = BTreeMap::from([
        ("clock.rate".into(), "48000".into()),
        ("clock.allowed-rates".into(), "[ 48000 ]".into()),
        ("clock.quantum".into(), "1024".into()),
        ("clock.min-quantum".into(), "32".into()),
        ("clock.max-quantum".into(), "2048".into()),
        ("clock.force-rate".into(), "0".into()),
        ("clock.force-quantum".into(), "0".into()),
        ("log.level".into(), "2".into()),
    ]);
    g.sm_settings = BTreeMap::from([
        ("node.stream.restore-props".into(), "true".into()),
        ("node.stream.restore-target".into(), "true".into()),
        ("node.stream.default-playback-volume".into(), "1.000000".into()),
        ("device.restore-profile".into(), "true".into()),
        ("device.restore-routes".into(), "true".into()),
        ("bluetooth.autoswitch-to-headset-profile".into(), "true".into()),
        ("linking.follow-default-target".into(), "true".into()),
    ]);
    g
}

pub fn spawn(peaks: Peaks) -> Handle {
    let (tx, mut cmd_rx) = unbounded_channel::<Cmd>();
    let (ev_tx, rx) = unbounded_channel();
    tokio::spawn(async move {
        let mut g = sample_graph();
        tokio::time::sleep(Duration::from_millis(120)).await;
        let _ = ev_tx.send(Ev::Graph(Box::new(g.clone())));
        let mut metered: Vec<u32> = Vec::new();
        let mut t = tokio::time::interval(Duration::from_millis(33));
        let mut phase = 0f32;
        loop {
            tokio::select! {
                c = cmd_rx.recv() => {
                    let Some(c) = c else { break };
                    if let Cmd::Meter { nodes } = &c {
                        metered = nodes.clone();
                        continue;
                    }
                    apply(&mut g, c, &ev_tx);
                    let _ = ev_tx.send(Ev::Graph(Box::new(g.clone())));
                }
                _ = t.tick() => {
                    phase += 0.033;
                    fake_levels(&g, &metered, phase, &peaks);
                }
            }
        }
    });
    let tx: CmdTx = std::sync::Arc::new(move |c| {
        let _ = tx.send(c);
    });
    Handle { tx, rx }
}

/// Music-ish fake levels: each playing stream has its own rhythm; devices
/// get the (volume-scaled) sum of what's linked into them.
fn fake_levels(g: &Graph, metered: &[u32], t: f32, peaks: &Peaks) {
    let level = |id: u32| -> f32 {
        let Some(n) = g.nodes.get(&id) else { return 0.0 };
        let base = match n.app_name.as_str() {
            "Firefox" => 0.55 + 0.35 * ((t * 7.1).sin() * (t * 1.3).cos()).abs(),
            "Spotify" => 0.45 + 0.45 * ((t * 4.0).sin().abs() * (t * 9.0).sin().abs()),
            "Discord" if n.kind() == NodeKind::Playback => if (t * 0.7).sin() > 0.2 { 0.35 + 0.3 * (t * 13.0).sin().abs() } else { 0.0 },
            "Discord" | "OBS Studio" => 0.25 + 0.4 * (t * 11.0).sin().abs() * ((t * 0.9).sin() > -0.3) as i32 as f32,
            "" if (t % 6.0) < 0.4 => { 0.6 },
            _ => 0.0,
        };
        let (vols, mute) = g.volume_of(n);
        if mute { 0.0 } else { base * vols.first().copied().unwrap_or(1.0).cbrt() }
    };
    let mut map = peaks.lock().expect("peaks");
    for &id in metered {
        let Some(n) = g.nodes.get(&id) else { continue };
        let v = if n.kind().is_stream() {
            level(id)
        } else if n.kind() == NodeKind::Source && n.device_id.is_some() {
            // Microphones: speech-like bursts.
            let (vols, mute) = g.volume_of(n);
            if mute { 0.0 } else { (0.15 + 0.5 * (t * 5.0).sin().abs() * ((t * 0.5).sin() > 0.0) as i32 as f32) * vols[0].cbrt() }
        } else {
            let inputs: f32 = g.connected_streams(n).iter().map(|s| level(s.id)).fold(0.0, f32::max);
            let (vols, mute) = g.volume_of(n);
            if mute { 0.0 } else { inputs * vols.first().copied().unwrap_or(1.0).cbrt() }
        };
        let wobble = 1.0 + 0.06 * (t * 17.0 + id as f32).sin();
        let e = map.entry(id).or_default();
        let vals = vec![(v * wobble).min(1.2), (v / wobble).min(1.2)];
        if e.len() != 2 {
            *e = vals;
        } else {
            e[0] = e[0].max(vals[0]);
            e[1] = e[1].max(vals[1]);
        }
    }
}

fn apply(g: &mut Graph, c: Cmd, ev: &UnboundedSender<Ev>) {
    match c {
        Cmd::SetVolume { node, volumes } => {
            let Some(n) = g.nodes.get(&node).cloned() else { return };
            let route = n.device_id.zip(n.profile_device);
            if let Some((d, pd)) = route
                && let Some(r) = g.devices.get_mut(&d).and_then(|dev| dev.routes.iter_mut().find(|r| r.device == pd)) {
                    r.volumes = volumes;
                    return;
                }
            if let Some(n) = g.nodes.get_mut(&node) {
                n.volumes = volumes;
            }
        }
        Cmd::SetMute { node, mute } => {
            let Some(n) = g.nodes.get(&node).cloned() else { return };
            if let (Some(d), Some(pd)) = (n.device_id, n.profile_device)
                && let Some(r) = g.devices.get_mut(&d).and_then(|dev| dev.routes.iter_mut().find(|r| r.device == pd)) {
                    r.mute = mute;
                    return;
                }
            if let Some(n) = g.nodes.get_mut(&node) {
                n.mute = mute;
            }
        }
        Cmd::SetDefault { node } => {
            let Some(n) = g.nodes.get(&node).cloned() else { return };
            match n.kind() {
                NodeKind::Sink => {
                    g.default_sink = Some(n.name.clone());
                    g.configured_sink = Some(n.name.clone());
                }
                NodeKind::Source => {
                    g.default_source = Some(n.name.clone());
                    g.configured_source = Some(n.name.clone());
                }
                _ => return,
            }
            // Streams that follow the default move with it.
            let follow: Vec<u32> = g.nodes.values().filter(|s| s.kind().is_stream() && !g.targets.contains_key(&s.id)).map(|s| s.id).collect();
            for sid in follow {
                let is_play = g.nodes[&sid].kind() == NodeKind::Playback;
                if is_play == (n.kind() == NodeKind::Sink) {
                    relink(g, sid, node);
                }
            }
            let _ = ev.send(Ev::Ok(format!("Default {} → {}", if n.kind() == NodeKind::Sink { "output" } else { "input" }, n.title())));
        }
        Cmd::MoveStream { stream, target } => {
            let Some(s) = g.nodes.get(&stream).cloned() else { return };
            let want = if s.kind() == NodeKind::Playback { &g.default_sink } else { &g.default_source };
            let tgt = match target {
                Some(t) => {
                    g.targets.insert(stream, t.to_string());
                    t
                }
                None => {
                    g.targets.remove(&stream);
                    match want.as_ref().and_then(|w| g.find_by_name(w)) {
                        Some(n) => n.id,
                        None => return,
                    }
                }
            };
            relink(g, stream, tgt);
        }
        Cmd::SetProfile { device, index } => {
            if let Some(d) = g.devices.get_mut(&device) {
                d.active_profile = Some(index);
                let desc = d.active_profile().map(|p| p.description.clone()).unwrap_or_default();
                let _ = ev.send(Ev::Ok(format!("{}: {desc}", d.title())));
            }
        }
        Cmd::SetPort { device, route_index, route_device } => {
            if let Some(d) = g.devices.get_mut(&device)
                && let Some(er) = d.enum_routes.iter().find(|r| r.index == route_index).cloned() {
                    let vol = d.routes.iter().find(|r| r.device == route_device).map(|r| r.volumes.clone()).unwrap_or(vec![0.4, 0.4]);
                    d.routes.retain(|r| r.device != route_device);
                    d.routes.push(Route { device: route_device, volumes: vol, ..er });
                }
        }
        Cmd::SetLatencyOffset { node, ns } => {
            if let Some(n) = g.nodes.get(&node).cloned()
                && let (Some(d), Some(pd)) = (n.device_id, n.profile_device)
                    && let Some(r) = g.devices.get_mut(&d).and_then(|dev| dev.routes.iter_mut().find(|r| r.device == pd)) {
                        r.latency_offset_ns = ns;
                    }
        }
        Cmd::SetSetting { key, value } => {
            match value {
                Some(v) => g.settings.insert(key.clone(), v),
                None => g.settings.insert(key.clone(), "0".into()),
            };
            if key == "clock.force-rate" {
                let r = g.settings.get("clock.force-rate").cloned().unwrap_or_default();
                if r != "0" {
                    g.settings.insert("clock.rate".into(), r);
                } else {
                    g.settings.insert("clock.rate".into(), "48000".into());
                }
            }
            if key == "clock.force-quantum" {
                let q = g.settings.get("clock.force-quantum").cloned().unwrap_or_default();
                g.settings.insert("clock.quantum".into(), if q != "0" { q } else { "1024".into() });
            }
        }
        Cmd::SetSmSetting { key, value, persist } => {
            g.sm_settings.insert(key.clone(), value.clone());
            let _ = ev.send(Ev::Ok(format!("{key} = {value}{}", if persist { " (saved)" } else { "" })));
        }
        Cmd::CreateNullSink { name, description } => {
            let id = g.nodes.keys().max().copied().unwrap_or(100) + 1;
            let mut n = device_node(id, &name, &description, "Audio/Sink", None, None, 1.0);
            n.props.insert("factory.name".into(), "support.null-audio-sink".into());
            g.nodes.insert(id, n);
            let _ = ev.send(Ev::Ok(format!("Created “{description}” (until PipeWire restarts)")));
        }
        Cmd::DestroyNode { id } => {
            if let Some(n) = g.nodes.remove(&id) {
                g.links.retain(|l| l.in_node != id && l.out_node != id);
                let _ = ev.send(Ev::Ok(format!("Removed {}", n.title())));
            }
        }
        Cmd::Meter { .. } => {}
    }
}

fn relink(g: &mut Graph, stream: u32, target: u32) {
    let play = g.nodes.get(&stream).is_some_and(|n| n.kind() == NodeKind::Playback);
    g.links.retain(|l| if play { l.out_node != stream } else { l.in_node != stream });
    let id = g.links.iter().map(|l| l.id).max().unwrap_or(100) + 1;
    g.links.push(if play { Link { id, out_node: stream, in_node: target } } else { Link { id, out_node: target, in_node: stream } });
}
