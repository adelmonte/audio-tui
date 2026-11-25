//! Real backend: a PipeWire client on its own thread.
//!
//! PipeWire proxies are `Rc` (not `Send`), so everything lives on one thread
//! running PipeWire's main loop. Commands arrive over a `pipewire::channel`;
//! graph snapshots go back to the UI over a tokio channel, coalesced by a
//! 50 ms timer. Level meters are small capture streams (stream.monitor)
//! that write peaks into a shared map.
//!
//! Approach and several details (Route vs Props volume, metadata targets,
//! monitor streams) follow wiremix by tsowell (MIT/Apache-2.0).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use libspa::param::audio::{AudioFormat, AudioInfoRaw};
use libspa::param::format::{MediaSubtype, MediaType};
use libspa::param::{format_utils, ParamType};
use libspa::pod::deserialize::PodDeserializer;
use libspa::pod::serialize::PodSerializer;
use libspa::pod::{Object, Pod, Property, PropertyFlags, Value, ValueArray};
use libspa::utils::dict::DictRef;
use pipewire as pw;
use pw::proxy::Listener;
use pw::types::ObjectType;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use super::model::*;
use super::{Cmd, CmdTx, Ev, Handle, Peaks, METER_NODE_NAME};

pub fn spawn(peaks: Peaks) -> Handle {
    let (ev_tx, rx) = unbounded_channel();
    let (pw_tx, pw_rx) = pw::channel::channel::<Cmd>();
    let ev2 = ev_tx.clone();
    std::thread::Builder::new()
        .name("pipewire".into())
        .spawn(move || {
            // Reconnect forever: PipeWire restarts (e.g. after applying
            // settings) should be invisible apart from a short notice.
            let mut rx = pw_rx;
            loop {
                let (back, res) = run(rx, ev_tx.clone(), peaks.clone());
                rx = back;
                if let Err(e) = res {
                    let _ = ev2.send(Ev::Disconnected(format!("Can't connect to PipeWire: {e}. Is it running? (systemctl --user status pipewire)")));
                }
                if ev2.is_closed() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        })
        .expect("spawn pipewire thread");
    let pw_tx = Mutex::new(pw_tx);
    let tx: CmdTx = Arc::new(move |c| {
        let _ = pw_tx.lock().expect("pw sender").send(c);
    });
    Handle { tx, rx }
}

// ---------------------------------------------------------------- pod helpers

fn parse_pod(param: Option<&Pod>) -> Option<Object> {
    let pod = param?;
    match PodDeserializer::deserialize_any_from(pod.as_bytes()).ok()?.1 {
        Value::Object(o) => Some(o),
        _ => None,
    }
}

fn serialize(obj: Object) -> Option<Vec<u8>> {
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(obj)).ok().map(|(c, _)| c.into_inner())
}

fn prop(key: u32, value: Value) -> Property {
    Property { key, flags: PropertyFlags::empty(), value }
}

fn avail(v: &Value) -> Option<Avail> {
    if let Value::Id(libspa::utils::Id(x)) = v {
        Some(match *x {
            libspa_sys::SPA_PARAM_AVAILABILITY_no => Avail::No,
            libspa_sys::SPA_PARAM_AVAILABILITY_yes => Avail::Yes,
            _ => Avail::Unknown,
        })
    } else {
        None
    }
}

fn int_array(v: &Value) -> Vec<i32> {
    match v {
        Value::ValueArray(ValueArray::Int(a)) => a.clone(),
        _ => Vec::new(),
    }
}

pub fn channel_name(id: u32) -> String {
    use libspa_sys::*;
    match id {
        SPA_AUDIO_CHANNEL_MONO => "MONO",
        SPA_AUDIO_CHANNEL_FL => "FL",
        SPA_AUDIO_CHANNEL_FR => "FR",
        SPA_AUDIO_CHANNEL_FC => "FC",
        SPA_AUDIO_CHANNEL_LFE => "LFE",
        SPA_AUDIO_CHANNEL_SL => "SL",
        SPA_AUDIO_CHANNEL_SR => "SR",
        SPA_AUDIO_CHANNEL_RL => "RL",
        SPA_AUDIO_CHANNEL_RR => "RR",
        SPA_AUDIO_CHANNEL_RC => "RC",
        _ => return format!("CH{id}"),
    }
    .to_string()
}

/// Volume/mute/channel map out of a Props object (node or route props).
fn read_props(o: &Object, volumes: &mut Vec<f32>, mute: &mut bool, channels: Option<&mut Vec<String>>, latency: Option<&mut i64>) {
    let mut ch_out = None;
    let mut lat_out = None;
    for p in &o.properties {
        match (p.key, &p.value) {
            (libspa_sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(v))) => *volumes = v.clone(),
            (libspa_sys::SPA_PROP_mute, Value::Bool(b)) => *mute = *b,
            (libspa_sys::SPA_PROP_channelMap, Value::ValueArray(ValueArray::Id(ids))) => {
                ch_out = Some(ids.iter().map(|i| channel_name(i.0)).collect::<Vec<_>>());
            }
            (libspa_sys::SPA_PROP_latencyOffsetNsec, Value::Long(n)) => lat_out = Some(*n),
            _ => {}
        }
    }
    if let (Some(c), Some(dst)) = (ch_out, channels) {
        *dst = c;
    }
    if let (Some(l), Some(dst)) = (lat_out, latency) {
        *dst = l;
    }
}

fn parse_route(o: Object) -> Option<Route> {
    let mut r = Route::default();
    let mut have_index = false;
    for p in o.properties {
        match p.key {
            libspa_sys::SPA_PARAM_ROUTE_index => {
                if let Value::Int(i) = p.value {
                    r.index = i;
                    have_index = true;
                }
            }
            libspa_sys::SPA_PARAM_ROUTE_device => {
                if let Value::Int(i) = p.value {
                    r.device = i;
                }
            }
            libspa_sys::SPA_PARAM_ROUTE_direction => {
                if let Value::Id(libspa::utils::Id(d)) = p.value {
                    r.direction = if d == libspa_sys::SPA_DIRECTION_INPUT { Dir::Input } else { Dir::Output };
                }
            }
            libspa_sys::SPA_PARAM_ROUTE_name => {
                if let Value::String(s) = p.value {
                    r.name = s;
                }
            }
            libspa_sys::SPA_PARAM_ROUTE_description => {
                if let Value::String(s) = p.value {
                    r.description = s;
                }
            }
            libspa_sys::SPA_PARAM_ROUTE_priority => {
                if let Value::Int(i) = p.value {
                    r.priority = i;
                }
            }
            libspa_sys::SPA_PARAM_ROUTE_available => r.available = avail(&p.value).unwrap_or_default(),
            libspa_sys::SPA_PARAM_ROUTE_profiles => r.profiles = int_array(&p.value),
            libspa_sys::SPA_PARAM_ROUTE_devices => r.devices = int_array(&p.value),
            libspa_sys::SPA_PARAM_ROUTE_props => {
                if let Value::Object(o) = p.value {
                    read_props(&o, &mut r.volumes, &mut r.mute, None, Some(&mut r.latency_offset_ns));
                }
            }
            _ => {}
        }
    }
    have_index.then_some(r)
}

fn parse_profile(o: Object) -> Option<Profile> {
    let mut pr = Profile::default();
    let mut have = false;
    for p in o.properties {
        match p.key {
            libspa_sys::SPA_PARAM_PROFILE_index => {
                if let Value::Int(i) = p.value {
                    pr.index = i;
                    have = true;
                }
            }
            libspa_sys::SPA_PARAM_PROFILE_name => {
                if let Value::String(s) = p.value {
                    pr.name = s;
                }
            }
            libspa_sys::SPA_PARAM_PROFILE_description => {
                if let Value::String(s) = p.value {
                    pr.description = s;
                }
            }
            libspa_sys::SPA_PARAM_PROFILE_priority => {
                if let Value::Int(i) = p.value {
                    pr.priority = i;
                }
            }
            libspa_sys::SPA_PARAM_PROFILE_available => pr.available = avail(&p.value).unwrap_or_default(),
            _ => {}
        }
    }
    have.then_some(pr)
}

fn dict(d: &DictRef) -> std::collections::BTreeMap<String, String> {
    d.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn fill_node(n: &mut Node, p: &std::collections::BTreeMap<String, String>) {
    let g = |k: &str| p.get(k).cloned().unwrap_or_default();
    n.name = g("node.name");
    n.description = g("node.description");
    n.nick = g("node.nick");
    n.media_class = g("media.class");
    n.kind = Some(NodeKind::from_media_class(&n.media_class));
    n.app_name = g("application.name");
    n.app_binary = g("application.process.binary");
    n.app_icon = g("application.icon-name");
    n.media_name = g("media.name");
    n.media_role = g("media.role");
    n.serial = p.get("object.serial").and_then(|s| s.parse().ok()).unwrap_or(n.serial);
    n.client_id = p.get("client.id").and_then(|s| s.parse().ok());
    n.device_id = p.get("device.id").and_then(|s| s.parse().ok());
    n.profile_device = p.get("card.profile.device").and_then(|s| s.parse().ok());
    if n.channels.is_empty() {
        if let Some(pos) = p.get("audio.position") {
            n.channels = pos.split([',', ' ']).filter(|x| !x.is_empty()).map(str::to_string).collect();
        }
    }
    n.props = p.clone();
}

// ---------------------------------------------------------------- state

#[derive(Default)]
struct Proxies {
    nodes: HashMap<u32, (pw::node::Node, Box<dyn Listener>)>,
    devices: HashMap<u32, (pw::device::Device, Box<dyn Listener>)>,
    metadata: HashMap<u32, (pw::metadata::Metadata, Box<dyn Listener>)>,
    /// metadata.name -> global id
    metadata_names: HashMap<String, u32>,
    meters: HashMap<u32, (pw::stream::StreamRc, pw::stream::StreamListener<MeterData>)>,
}

struct Shared {
    graph: Graph,
    dirty: bool,
    /// (device id, param id) -> seq of the batch being received.
    param_seq: HashMap<(u32, u32), i32>,
}

type St = Rc<RefCell<Shared>>;

fn json_name(v: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(v).ok()?.get("name")?.as_str().map(str::to_string)
}

type Setup = (pw::main_loop::MainLoopRc, pw::context::ContextRc, pw::core::CoreRc, pw::registry::RegistryRc);

fn connect() -> Result<Setup, pw::Error> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_rc(None)?;
    let registry = core.get_registry_rc()?;
    Ok((mainloop, context, core, registry))
}

/// One connection's lifetime. Returns the command receiver so the caller can
/// reconnect without losing the channel.
fn run(rx: pw::channel::Receiver<Cmd>, ev: UnboundedSender<Ev>, peaks: Peaks) -> (pw::channel::Receiver<Cmd>, Result<(), String>) {
    let (mainloop, _context, core, registry) = match connect() {
        Ok(s) => s,
        Err(e) => return (rx, Err(e.to_string())),
    };

    let st: St = Rc::new(RefCell::new(Shared { graph: Graph { connected: true, ..Default::default() }, dirty: true, param_seq: HashMap::new() }));
    let px: Rc<RefCell<Proxies>> = Rc::default();

    // Lost connection (e.g. pipewire restarted): tell the UI and stop.
    let _core_listener = core
        .add_listener_local()
        .info({
            let st = st.clone();
            move |info| {
                let mut s = st.borrow_mut();
                s.graph.server_version = info.version().to_string();
                s.dirty = true;
            }
        })
        .error({
            let ev = ev.clone();
            let ml = mainloop.downgrade();
            move |id, _seq, res, msg| {
                if id == 0 && res == -libc_epipe() {
                    if let Some(ml) = ml.upgrade() {
                        ml.quit();
                    }
                } else if id == 0 {
                    let _ = ev.send(Ev::Error(format!("PipeWire: {msg}")));
                }
            }
        })
        .register();

    let _reg_listener = registry
        .add_listener_local()
        .global({
            let st = st.clone();
            let px = px.clone();
            let reg = registry.downgrade();
            move |obj| {
                let Some(registry) = reg.upgrade() else { return };
                let props = obj.props.map(dict).unwrap_or_default();
                match obj.type_ {
                    ObjectType::Node => bind_node(&registry, obj, &props, &st, &px),
                    ObjectType::Device => bind_device(&registry, obj, &props, &st, &px),
                    ObjectType::Metadata => bind_metadata(&registry, obj, &props, &st, &px),
                    ObjectType::Link => {
                        let out = props.get("link.output.node").and_then(|v| v.parse().ok());
                        let inp = props.get("link.input.node").and_then(|v| v.parse().ok());
                        if let (Some(out_node), Some(in_node)) = (out, inp) {
                            let mut s = st.borrow_mut();
                            s.graph.links.push(Link { id: obj.id, out_node, in_node });
                            s.dirty = true;
                        }
                    }
                    ObjectType::Client => {
                        let c = Client {
                            id: obj.id,
                            app_name: props.get("application.name").cloned().unwrap_or_default(),
                            binary: props.get("application.process.binary").cloned().unwrap_or_default(),
                            api: props.get("client.api").cloned().unwrap_or_default(),
                            pid: props.get("application.process.id").and_then(|p| p.parse().ok()),
                        };
                        let mut s = st.borrow_mut();
                        s.graph.clients.insert(obj.id, c);
                        s.dirty = true;
                    }
                    _ => {}
                }
            }
        })
        .global_remove({
            let st = st.clone();
            let px = px.clone();
            move |id| {
                {
                    let mut p = px.borrow_mut();
                    p.nodes.remove(&id);
                    p.devices.remove(&id);
                    p.metadata.remove(&id);
                    p.metadata_names.retain(|_, v| *v != id);
                    p.meters.remove(&id);
                }
                let mut s = st.borrow_mut();
                let g = &mut s.graph;
                g.nodes.remove(&id);
                g.devices.remove(&id);
                g.clients.remove(&id);
                g.links.retain(|l| l.id != id);
                g.targets.remove(&id);
                s.dirty = true;
            }
        })
        .register();

    // Publish coalesced snapshots.
    let timer = mainloop.loop_().add_timer({
        let st = st.clone();
        let ev = ev.clone();
        move |_| {
            let mut s = st.borrow_mut();
            if s.dirty {
                s.dirty = false;
                let _ = ev.send(Ev::Graph(Box::new(s.graph.clone())));
            }
        }
    });
    let _ = timer.update_timer(Some(Duration::from_millis(50)), Some(Duration::from_millis(50)));

    let cmds = rx.attach(mainloop.loop_(), {
        let st = st.clone();
        let px = px.clone();
        let core = core.clone();
        let registry = registry.clone();
        let ev = ev.clone();
        move |cmd| execute(cmd, &core, &registry, &st, &px, &ev, &peaks)
    });

    mainloop.run();
    let rx = cmds.deattach();
    let _ = ev.send(Ev::Disconnected("PipeWire connection closed — reconnecting…".into()));
    (rx, Ok(()))
}

fn libc_epipe() -> i32 {
    32 // EPIPE: the connection to the daemon was closed
}

fn bind_node(registry: &pw::registry::RegistryRc, obj: &pw::registry::GlobalObject<&DictRef>, props: &std::collections::BTreeMap<String, String>, st: &St, px: &Rc<RefCell<Proxies>>) {
    let class = props.get("media.class").map(String::as_str).unwrap_or("");
    if !matches!(NodeKind::from_media_class(class), NodeKind::Sink | NodeKind::Source | NodeKind::Playback | NodeKind::Recording) {
        return;
    }
    let Ok(node) = registry.bind::<pw::node::Node, _>(obj) else { return };
    let id = obj.id;
    {
        let mut s = st.borrow_mut();
        let mut n = Node { id, ..Default::default() };
        fill_node(&mut n, props);
        s.graph.nodes.insert(id, n);
        s.dirty = true;
    }
    let listener = node
        .add_listener_local()
        .info({
            let st = st.clone();
            move |info| {
                if let Some(p) = info.props() {
                    let p = dict(p);
                    let mut s = st.borrow_mut();
                    if let Some(n) = s.graph.nodes.get_mut(&id) {
                        fill_node(n, &p);
                    }
                    s.dirty = true;
                }
            }
        })
        .param({
            let st = st.clone();
            move |_seq, pid, _index, _next, param| {
                if pid != ParamType::Props {
                    return;
                }
                let Some(o) = parse_pod(param) else { return };
                let mut s = st.borrow_mut();
                if let Some(n) = s.graph.nodes.get_mut(&id) {
                    let mut ch = n.channels.clone();
                    read_props(&o, &mut n.volumes, &mut n.mute, Some(&mut ch), None);
                    n.channels = ch;
                }
                s.dirty = true;
            }
        })
        .register();
    node.subscribe_params(&[ParamType::Props]);
    px.borrow_mut().nodes.insert(id, (node, Box::new(listener)));
}

fn bind_device(registry: &pw::registry::RegistryRc, obj: &pw::registry::GlobalObject<&DictRef>, props: &std::collections::BTreeMap<String, String>, st: &St, px: &Rc<RefCell<Proxies>>) {
    if props.get("media.class").map(String::as_str) != Some("Audio/Device") {
        return;
    }
    let Ok(device) = registry.bind::<pw::device::Device, _>(obj) else { return };
    let id = obj.id;
    {
        let g = |k: &str| props.get(k).cloned().unwrap_or_default();
        let mut s = st.borrow_mut();
        s.graph.devices.insert(
            id,
            Device {
                id,
                name: g("device.name"),
                description: g("device.description"),
                nick: g("device.nick"),
                api: g("device.api"),
                bus: g("device.bus"),
                form_factor: g("device.form-factor"),
                props: props.clone(),
                ..Default::default()
            },
        );
        s.dirty = true;
    }
    let params = [ParamType::EnumProfile, ParamType::Profile, ParamType::EnumRoute, ParamType::Route];
    let listener = device
        .add_listener_local()
        .info({
            let st = st.clone();
            move |info| {
                if let Some(p) = info.props() {
                    let p = dict(p);
                    let mut s = st.borrow_mut();
                    if let Some(d) = s.graph.devices.get_mut(&id) {
                        let g = |k: &str| p.get(k).cloned().unwrap_or_default();
                        d.description = g("device.description");
                        d.nick = g("device.nick");
                        d.props = p.clone();
                    }
                    s.dirty = true;
                }
            }
        })
        .param({
            let st = st.clone();
            move |seq, pid, _index, _next, param| {
                let Some(o) = parse_pod(param) else { return };
                let mut s = st.borrow_mut();
                // A new seq means a fresh enumeration: start the list over.
                let key = (id, pid.as_raw());
                let fresh = s.param_seq.get(&key) != Some(&seq);
                s.param_seq.insert(key, seq);
                let Some(d) = s.graph.devices.get_mut(&id) else { return };
                match pid {
                    ParamType::EnumProfile => {
                        if fresh {
                            d.profiles.clear();
                        }
                        if let Some(p) = parse_profile(o) {
                            d.profiles.retain(|x| x.index != p.index);
                            d.profiles.push(p);
                            d.profiles.sort_by_key(|p| p.index);
                        }
                    }
                    ParamType::Profile => {
                        if let Some(p) = parse_profile(o) {
                            d.active_profile = Some(p.index);
                        }
                    }
                    ParamType::EnumRoute => {
                        if fresh {
                            d.enum_routes.clear();
                        }
                        if let Some(r) = parse_route(o) {
                            d.enum_routes.retain(|x| x.index != r.index);
                            d.enum_routes.push(r);
                        }
                    }
                    ParamType::Route => {
                        if fresh {
                            d.routes.clear();
                        }
                        if let Some(r) = parse_route(o) {
                            d.routes.retain(|x| x.device != r.device);
                            d.routes.push(r);
                        }
                    }
                    _ => {}
                }
                s.dirty = true;
            }
        })
        .register();
    device.subscribe_params(&params);
    px.borrow_mut().devices.insert(id, (device, Box::new(listener)));
}

fn bind_metadata(registry: &pw::registry::RegistryRc, obj: &pw::registry::GlobalObject<&DictRef>, props: &std::collections::BTreeMap<String, String>, st: &St, px: &Rc<RefCell<Proxies>>) {
    let Some(name) = props.get("metadata.name").cloned() else { return };
    if !matches!(name.as_str(), "default" | "settings" | "sm-settings" | "persistent-sm-settings") {
        return;
    }
    let Ok(md) = registry.bind::<pw::metadata::Metadata, _>(obj) else { return };
    let listener = md
        .add_listener_local()
        .property({
            let st = st.clone();
            let name = name.clone();
            move |subject, key, _type, value| {
                let mut s = st.borrow_mut();
                let g = &mut s.graph;
                match name.as_str() {
                    "default" => match key {
                        Some("default.audio.sink") => g.default_sink = value.and_then(json_name),
                        Some("default.audio.source") => g.default_source = value.and_then(json_name),
                        Some("default.configured.audio.sink") => g.configured_sink = value.and_then(json_name),
                        Some("default.configured.audio.source") => g.configured_source = value.and_then(json_name),
                        Some("target.object") | Some("target.node") => match value {
                            Some(v) if v != "-1" => {
                                g.targets.insert(subject, v.to_string());
                            }
                            _ => {
                                g.targets.remove(&subject);
                            }
                        },
                        None => {
                            g.targets.remove(&subject);
                        }
                        _ => {}
                    },
                    "settings" => {
                        if let Some(k) = key {
                            match value {
                                Some(v) => g.settings.insert(k.to_string(), v.to_string()),
                                None => g.settings.remove(k),
                            };
                        }
                    }
                    "sm-settings" => {
                        if let Some(k) = key {
                            match value {
                                Some(v) => g.sm_settings.insert(k.to_string(), v.to_string()),
                                None => g.sm_settings.remove(k),
                            };
                        }
                    }
                    _ => {}
                }
                s.dirty = true;
                0
            }
        })
        .register();
    let mut p = px.borrow_mut();
    p.metadata_names.insert(name, obj.id);
    p.metadata.insert(obj.id, (md, Box::new(listener)));
}

// ---------------------------------------------------------------- commands

fn set_node_props(node: &pw::node::Node, properties: Vec<Property>) {
    if let Some(bytes) = serialize(Object { type_: libspa_sys::SPA_TYPE_OBJECT_Props, id: libspa_sys::SPA_PARAM_Props, properties }) {
        if let Some(pod) = Pod::from_bytes(&bytes) {
            node.set_param(ParamType::Props, 0, pod);
        }
    }
}

fn set_route(device: &pw::device::Device, index: i32, dev: i32, props: Vec<Property>) {
    let mut p = vec![prop(libspa_sys::SPA_PARAM_ROUTE_index, Value::Int(index)), prop(libspa_sys::SPA_PARAM_ROUTE_device, Value::Int(dev))];
    if !props.is_empty() {
        p.push(prop(
            libspa_sys::SPA_PARAM_ROUTE_props,
            Value::Object(Object { type_: libspa_sys::SPA_TYPE_OBJECT_Props, id: libspa_sys::SPA_PARAM_Route, properties: props }),
        ));
    }
    // save=true asks WirePlumber to remember it, like pavucontrol does.
    p.push(prop(libspa_sys::SPA_PARAM_ROUTE_save, Value::Bool(true)));
    if let Some(bytes) = serialize(Object { type_: libspa_sys::SPA_TYPE_OBJECT_ParamRoute, id: libspa_sys::SPA_PARAM_Route, properties: p }) {
        if let Some(pod) = Pod::from_bytes(&bytes) {
            device.set_param(ParamType::Route, 0, pod);
        }
    }
}

/// Apply props either to the card route that owns this node, or the node.
fn node_or_route(id: u32, props: Vec<Property>, st: &St, px: &Rc<RefCell<Proxies>>) {
    let s = st.borrow();
    let p = px.borrow();
    let Some(n) = s.graph.nodes.get(&id) else { return };
    if let Some((dev, r)) = s.graph.route_for(n) {
        if let Some((d, _)) = p.devices.get(&dev.id) {
            set_route(d, r.index, r.device, props);
            return;
        }
    }
    if let Some((node, _)) = p.nodes.get(&id) {
        set_node_props(node, props);
    }
}

fn metadata(px: &Rc<RefCell<Proxies>>, name: &str, f: impl FnOnce(&pw::metadata::Metadata)) -> bool {
    let p = px.borrow();
    match p.metadata_names.get(name).and_then(|id| p.metadata.get(id)) {
        Some((m, _)) => {
            f(m);
            true
        }
        None => false,
    }
}

fn execute(cmd: Cmd, core: &pw::core::CoreRc, registry: &pw::registry::RegistryRc, st: &St, px: &Rc<RefCell<Proxies>>, ev: &UnboundedSender<Ev>, peaks: &Peaks) {
    match cmd {
        Cmd::SetVolume { node, volumes } => {
            node_or_route(node, vec![prop(libspa_sys::SPA_PROP_channelVolumes, Value::ValueArray(ValueArray::Float(volumes)))], st, px)
        }
        Cmd::SetMute { node, mute } => node_or_route(node, vec![prop(libspa_sys::SPA_PROP_mute, Value::Bool(mute))], st, px),
        Cmd::SetLatencyOffset { node, ns } => node_or_route(node, vec![prop(libspa_sys::SPA_PROP_latencyOffsetNsec, Value::Long(ns))], st, px),
        Cmd::SetDefault { node } => {
            let (name, key) = {
                let s = st.borrow();
                let Some(n) = s.graph.nodes.get(&node) else { return };
                let key = if n.kind() == NodeKind::Source { "default.configured.audio.source" } else { "default.configured.audio.sink" };
                (n.name.clone(), key)
            };
            let json = serde_json::json!({ "name": name }).to_string();
            if !metadata(px, "default", |m| m.set_property(0, key, Some("Spa:String:JSON"), Some(&json))) {
                let _ = ev.send(Ev::Error("No “default” metadata — is WirePlumber running?".into()));
            }
        }
        Cmd::MoveStream { stream, target } => {
            // Same convention as wiremix/pavucontrol: -1 means "follow default".
            let ok = metadata(px, "default", |m| match target {
                Some(t) => {
                    m.set_property(stream, "target.object", None, None);
                    m.set_property(stream, "target.node", Some("Spa:Id"), Some(&t.to_string()));
                }
                None => {
                    m.set_property(stream, "target.object", Some("Spa:Id"), Some("-1"));
                    m.set_property(stream, "target.node", Some("Spa:Id"), Some("-1"));
                }
            });
            if !ok {
                let _ = ev.send(Ev::Error("No “default” metadata — is WirePlumber running?".into()));
            }
        }
        Cmd::SetProfile { device, index } => {
            let p = px.borrow();
            if let Some((d, _)) = p.devices.get(&device) {
                let props = vec![prop(libspa_sys::SPA_PARAM_PROFILE_index, Value::Int(index)), prop(libspa_sys::SPA_PARAM_PROFILE_save, Value::Bool(true))];
                if let Some(bytes) = serialize(Object { type_: libspa_sys::SPA_TYPE_OBJECT_ParamProfile, id: libspa_sys::SPA_PARAM_Profile, properties: props }) {
                    if let Some(pod) = Pod::from_bytes(&bytes) {
                        d.set_param(ParamType::Profile, 0, pod);
                    }
                }
            }
        }
        Cmd::SetPort { device, route_index, route_device } => {
            let p = px.borrow();
            if let Some((d, _)) = p.devices.get(&device) {
                set_route(d, route_index, route_device, Vec::new());
            }
        }
        Cmd::SetSetting { key, value } => {
            if !metadata(px, "settings", |m| m.set_property(0, &key, None, value.as_deref())) {
                let _ = ev.send(Ev::Error("PipeWire “settings” metadata not found".into()));
            }
        }
        Cmd::SetSmSetting { key, value, persist } => {
            let name = if persist { "persistent-sm-settings" } else { "sm-settings" };
            if !metadata(px, name, |m| m.set_property(0, &key, None, Some(&value))) {
                let _ = ev.send(Ev::Error(format!("WirePlumber “{name}” metadata not found (WirePlumber 0.5+ needed)")));
            }
        }
        Cmd::CreateNullSink { name, description } => {
            let props = pw::properties::properties! {
                "factory.name" => "support.null-audio-sink",
                "node.name" => name.as_str(),
                "node.description" => description.as_str(),
                "media.class" => "Audio/Sink",
                "audio.position" => "FL,FR",
                "monitor.channel-volumes" => "true",
                "object.linger" => "true",
            };
            match core.create_object::<pw::node::Node>("adapter", &props) {
                // object.linger: the node outlives this proxy (and this app).
                Ok(_) => {
                    let _ = ev.send(Ev::Ok(format!("Created “{description}” (until PipeWire restarts)")));
                }
                Err(e) => {
                    let _ = ev.send(Ev::Error(format!("Couldn't create virtual output: {e}")));
                }
            }
        }
        Cmd::DestroyNode { id } => {
            let _ = registry.destroy_global(id);
        }
        Cmd::Meter { nodes } => {
            let want: HashSet<u32> = nodes.into_iter().collect();
            let have: HashSet<u32> = px.borrow().meters.keys().copied().collect();
            for gone in have.difference(&want) {
                px.borrow_mut().meters.remove(gone);
                peaks.lock().expect("peaks").remove(gone);
            }
            for &new in want.difference(&have) {
                let info = {
                    let s = st.borrow();
                    s.graph.nodes.get(&new).map(|n| (n.serial, n.kind() == NodeKind::Sink))
                };
                if let Some((serial, is_sink)) = info {
                    if let Some(m) = start_meter(core, new, serial, is_sink, peaks.clone()) {
                        px.borrow_mut().meters.insert(new, m);
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- meters

#[derive(Default)]
struct MeterData {
    format: AudioInfoRaw,
    id: u32,
    buffers: u8,
    peaks: Option<Peaks>,
}

fn start_meter(core: &pw::core::CoreRc, id: u32, serial: u64, is_sink: bool, peaks: Peaks) -> Option<(pw::stream::StreamRc, pw::stream::StreamListener<MeterData>)> {
    let mut props = pw::properties::properties! {
        *pw::keys::TARGET_OBJECT => serial.to_string(),
        *pw::keys::STREAM_MONITOR => "true",
        *pw::keys::NODE_NAME => METER_NODE_NAME,
        *pw::keys::APP_NAME => "audio-tui",
        "node.passive" => "true",
        "node.dont-reconnect" => "true",
        // Don't let WirePlumber remember our meters (we're the privacy tool!).
        "state.restore-props" => "false",
        "state.restore-target" => "false",
    };
    if is_sink {
        props.insert(*pw::keys::STREAM_CAPTURE_SINK, "true");
    }
    let stream = pw::stream::StreamRc::new(core.clone(), METER_NODE_NAME, props).ok()?;
    let data = MeterData { id, peaks: Some(peaks), ..Default::default() };
    let listener = stream
        .add_local_listener_with_user_data(data)
        .param_changed(|_s, d, pid, param| {
            let Some(param) = param else { return };
            if pid != ParamType::Format.as_raw() {
                return;
            }
            let Ok((mt, ms)) = format_utils::parse_format(param) else { return };
            if mt != MediaType::Audio || ms != MediaSubtype::Raw {
                return;
            }
            let _ = d.format.parse(param);
        })
        .process(|s, d| {
            let Some(mut buf) = s.dequeue_buffer() else { return };
            d.buffers = d.buffers.saturating_add(1);
            if d.buffers == 1 {
                return; // the first buffer is always silence
            }
            let datas = buf.datas_mut();
            let n_ch = (d.format.channels() as usize).max(1);
            let mut levels = vec![0f32; n_ch];
            for (c, level) in levels.iter_mut().enumerate() {
                let Some(data) = datas.get_mut(c) else { continue };
                let size = data.chunk().size() as usize;
                let Some(bytes) = data.data() else { continue };
                let size = size.min(bytes.len()) / 4 * 4;
                let mut peak = 0f32;
                for chunk in bytes[..size].chunks_exact(4) {
                    let v = f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]).abs();
                    if v > peak {
                        peak = v;
                    }
                }
                *level = peak;
            }
            if let Some(p) = &d.peaks {
                let mut map = p.lock().expect("peaks");
                let e = map.entry(d.id).or_default();
                if e.len() != levels.len() {
                    *e = levels;
                } else {
                    for (a, b) in e.iter_mut().zip(levels) {
                        *a = a.max(b);
                    }
                }
            }
        })
        .register()
        .ok()?;

    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::F32P);
    let obj = Object { type_: libspa::utils::SpaTypes::ObjectParamFormat.as_raw(), id: ParamType::EnumFormat.as_raw(), properties: info.into() };
    let bytes = serialize(obj)?;
    let mut params = [Pod::from_bytes(&bytes)?];
    stream
        .connect(libspa::utils::Direction::Input, None, pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)
        .ok()?;
    Some((stream, listener))
}
