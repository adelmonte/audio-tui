//! Backend interface. The PipeWire backend (`pw.rs`) runs its own thread
//! with a PipeWire main loop; the demo (`demo.rs`) fakes a whole graph.
//! Both publish a full `Graph` snapshot whenever something changes and
//! write live levels into the shared `Peaks` map.

pub mod conf;
pub mod demo;
pub mod model;
pub mod pw;
pub mod state;
pub mod sys;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedReceiver;

use model::Graph;

#[derive(Clone, Debug)]
pub enum Cmd {
    /// Per-channel linear volumes. Goes to the card Route for hardware
    /// outputs/inputs, to node Props for streams and virtual devices.
    SetVolume { node: u32, volumes: Vec<f32> },
    SetMute { node: u32, mute: bool },
    SetDefault { node: u32 },
    /// Send a stream to a device; None = follow the default again.
    MoveStream { stream: u32, target: Option<u32> },
    SetProfile { device: u32, index: i32 },
    SetPort { device: u32, route_index: i32, route_device: i32 },
    SetLatencyOffset { node: u32, ns: i64 },
    /// PipeWire "settings" metadata (e.g. clock.force-rate). None = unset.
    SetSetting { key: String, value: Option<String> },
    /// WirePlumber "sm-settings" metadata; `persist` also saves it.
    SetSmSetting { key: String, value: String, persist: bool },
    /// Temporary virtual output that lasts until PipeWire restarts.
    CreateNullSink { name: String, description: String },
    DestroyNode { id: u32 },
    /// Which nodes to show live levels for (only visible ones, to save CPU).
    Meter { nodes: Vec<u32> },
}

#[derive(Clone, Debug)]
pub enum Ev {
    Graph(Box<Graph>),
    Disconnected(String),
    Ok(String),
    Error(String),
}

/// node id -> peak level per channel (linear 0..1+), maxed since last read.
pub type Peaks = Arc<Mutex<HashMap<u32, Vec<f32>>>>;

pub type CmdTx = Arc<dyn Fn(Cmd) + Send + Sync>;

pub struct Handle {
    pub tx: CmdTx,
    pub rx: UnboundedReceiver<Ev>,
}

/// Name given to our own meter streams so we can hide them (and keep
/// WirePlumber from remembering them).
pub const METER_NODE_NAME: &str = "audio-tui-meter";
