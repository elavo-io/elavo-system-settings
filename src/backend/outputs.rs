//! Displays over `zwlr_output_manager_v1`, on its own Wayland connection and
//! thread like Folio's window list. The thread sends a full snapshot after
//! every `done`; configurations are sent from the GTK thread, and their
//! results come back on the same channel.

use std::cell::Cell;
use std::collections::HashMap;

use wayland_client::backend::ObjectId;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_output::Transform, wl_registry};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub proxy: ZwlrOutputModeV1,
    pub width: i32,
    pub height: i32,
    /// Millihertz.
    pub refresh: i32,
    pub preferred: bool,
}

#[derive(Clone, Debug)]
pub struct Output {
    pub key: ObjectId,
    pub head: ZwlrOutputHeadV1,
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub serial: String,
    pub enabled: bool,
    pub modes: Vec<Mode>,
    pub current_mode: Option<usize>,
    pub x: i32,
    pub y: i32,
    pub transform: Transform,
    pub scale: f64,
}

impl Output {
    pub fn label(&self) -> String {
        let model = [self.make.as_str(), self.model.as_str()]
            .iter()
            .filter(|s| !s.is_empty() && **s != "Unknown")
            .copied()
            .collect::<Vec<_>>()
            .join(" ");
        if model.is_empty() {
            self.name.clone()
        } else {
            format!("{model} ({})", self.name)
        }
    }

    /// The size in layout coordinates: mode size, rotated, divided by scale.
    pub fn logical_size(
        &self,
        mode: Option<usize>,
        transform: Transform,
        scale: f64,
    ) -> (i32, i32) {
        let (w, h) = mode
            .and_then(|i| self.modes.get(i))
            .map(|m| (m.width, m.height))
            .unwrap_or((1920, 1080));
        let (w, h) = if rotated(transform) { (h, w) } else { (w, h) };
        let scale = if scale > 0.0 { scale } else { 1.0 };
        (
            (f64::from(w) / scale).round() as i32,
            (f64::from(h) / scale).round() as i32,
        )
    }
}

pub fn rotated(t: Transform) -> bool {
    matches!(
        t,
        Transform::_90 | Transform::_270 | Transform::Flipped90 | Transform::Flipped270
    )
}

/// What to ask for one head.
#[derive(Clone, Debug)]
pub struct HeadConfig {
    pub head: ZwlrOutputHeadV1,
    pub enabled: bool,
    pub mode: Option<ZwlrOutputModeV1>,
    pub x: i32,
    pub y: i32,
    pub transform: Transform,
    pub scale: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    Failed,
    /// The outputs changed while the request was in flight.
    Cancelled,
}

pub enum Event {
    Snapshot { outputs: Vec<Output>, serial: u32 },
    Result { id: u32, outcome: Outcome },
}

pub struct Controller {
    conn: Connection,
    manager: ZwlrOutputManagerV1,
    qh: QueueHandle<State>,
    next_id: Cell<u32>,
}

impl Controller {
    /// Send a configuration. With `test_only` the compositor checks it but
    /// changes nothing. The result arrives as `Event::Result` with this id.
    pub fn send(&self, serial: u32, heads: &[HeadConfig], test_only: bool) -> u32 {
        let id = self.next_id.get().wrapping_add(1);
        self.next_id.set(id);
        let config = self.manager.create_configuration(serial, &self.qh, id);
        for h in heads {
            if !h.enabled {
                config.disable_head(&h.head);
                continue;
            }
            let ch = config.enable_head(&h.head, &self.qh, ());
            if let Some(mode) = &h.mode {
                ch.set_mode(mode);
            }
            ch.set_position(h.x, h.y);
            ch.set_transform(h.transform);
            ch.set_scale(h.scale);
        }
        if test_only {
            config.test();
        } else {
            config.apply();
        }
        let _ = self.conn.flush();
        id
    }
}

#[derive(Default)]
struct HeadData {
    head: Option<ZwlrOutputHeadV1>,
    name: String,
    description: String,
    make: String,
    model: String,
    serial: String,
    enabled: bool,
    modes: Vec<ObjectId>,
    current_mode: Option<ObjectId>,
    x: i32,
    y: i32,
    transform: Option<Transform>,
    scale: f64,
}

struct ModeData {
    proxy: ZwlrOutputModeV1,
    width: i32,
    height: i32,
    refresh: i32,
    preferred: bool,
}

pub struct State {
    tx: async_channel::Sender<Event>,
    heads: Vec<(ObjectId, HeadData)>,
    modes: HashMap<ObjectId, ModeData>,
}

impl State {
    fn head(&mut self, id: &ObjectId) -> Option<&mut HeadData> {
        self.heads.iter_mut().find(|(k, _)| k == id).map(|(_, h)| h)
    }

    fn snapshot(&self) -> Vec<Output> {
        self.heads
            .iter()
            .filter_map(|(key, h)| {
                let head = h.head.clone()?;
                let modes: Vec<Mode> = h
                    .modes
                    .iter()
                    .filter_map(|id| self.modes.get(id))
                    .map(|m| Mode {
                        proxy: m.proxy.clone(),
                        width: m.width,
                        height: m.height,
                        refresh: m.refresh,
                        preferred: m.preferred,
                    })
                    .collect();
                let current_mode = h.current_mode.as_ref().and_then(|id| {
                    h.modes
                        .iter()
                        .filter(|m| self.modes.contains_key(m))
                        .position(|m| m == id)
                });
                Some(Output {
                    key: key.clone(),
                    head,
                    name: h.name.clone(),
                    description: h.description.clone(),
                    make: h.make.clone(),
                    model: h.model.clone(),
                    serial: h.serial.clone(),
                    enabled: h.enabled,
                    modes,
                    current_mode,
                    x: h.x,
                    y: h.y,
                    transform: h.transform.unwrap_or(Transform::Normal),
                    scale: if h.scale > 0.0 { h.scale } else { 1.0 },
                })
            })
            .collect()
    }
}

pub fn start() -> Result<(Controller, async_channel::Receiver<Event>), String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ZwlrOutputManagerV1 = globals
        .bind(&qh, 1..=4, ())
        .map_err(|e| format!("zwlr_output_manager_v1: {e}"))?;
    let (tx, rx) = async_channel::unbounded();
    let mut state = State {
        tx,
        heads: Vec::new(),
        modes: HashMap::new(),
    };
    std::thread::Builder::new()
        .name("output-management".into())
        .spawn(move || {
            while queue.blocking_dispatch(&mut state).is_ok() {
                if state.tx.is_closed() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((
        Controller {
            conn,
            manager,
            qh,
            next_id: Cell::new(0),
        },
        rx,
    ))
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_manager_v1::Event as E;
        match event {
            E::Head { head } => {
                state.heads.push((
                    head.id(),
                    HeadData {
                        head: Some(head),
                        scale: 1.0,
                        ..HeadData::default()
                    },
                ));
            }
            E::Done { serial } => {
                let outputs = state.snapshot();
                let _ = state.tx.send_blocking(Event::Snapshot { outputs, serial });
            }
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for State {
    fn event(
        state: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_head_v1::Event as E;
        let id = head.id();
        if let E::Finished = event {
            state.heads.retain(|(k, _)| *k != id);
            if head.version() >= 3 {
                head.release();
            }
            return;
        }
        if let E::Mode { mode } = &event {
            state.modes.insert(
                mode.id(),
                ModeData {
                    proxy: mode.clone(),
                    width: 0,
                    height: 0,
                    refresh: 0,
                    preferred: false,
                },
            );
        }
        let Some(h) = state.head(&id) else {
            return;
        };
        match event {
            E::Name { name } => h.name = name,
            E::Description { description } => h.description = description,
            E::Mode { mode } => h.modes.push(mode.id()),
            E::Enabled { enabled } => h.enabled = enabled != 0,
            E::CurrentMode { mode } => h.current_mode = Some(mode.id()),
            E::Position { x, y } => {
                h.x = x;
                h.y = y;
            }
            E::Transform {
                transform: WEnum::Value(t),
            } => h.transform = Some(t),
            E::Scale { scale } => h.scale = scale,
            E::Make { make } => h.make = make,
            E::Model { model } => h.model = model,
            E::SerialNumber { serial_number } => h.serial = serial_number,
            _ => {}
        }
    }

    event_created_child!(State, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for State {
    fn event(
        state: &mut Self,
        mode: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_mode_v1::Event as E;
        let id = mode.id();
        if let E::Finished = event {
            state.modes.remove(&id);
            for (_, h) in state.heads.iter_mut() {
                h.modes.retain(|m| *m != id);
            }
            if mode.version() >= 3 {
                mode.release();
            }
            return;
        }
        let Some(m) = state.modes.get_mut(&id) else {
            return;
        };
        match event {
            E::Size { width, height } => {
                m.width = width;
                m.height = height;
            }
            E::Refresh { refresh } => m.refresh = refresh,
            E::Preferred => m.preferred = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, u32> for State {
    fn event(
        state: &mut Self,
        config: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwlr_output_configuration_v1::Event as E;
        let outcome = match event {
            E::Succeeded => Outcome::Succeeded,
            E::Failed => Outcome::Failed,
            E::Cancelled => Outcome::Cancelled,
            _ => return,
        };
        config.destroy();
        let _ = state.tx.send_blocking(Event::Result { id: *id, outcome });
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputConfigurationHeadV1,
        _: <ZwlrOutputConfigurationHeadV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// `wl_output.transform` names as kanshi writes them.
pub fn transform_name(t: Transform) -> &'static str {
    match t {
        Transform::_90 => "90",
        Transform::_180 => "180",
        Transform::_270 => "270",
        Transform::Flipped => "flipped",
        Transform::Flipped90 => "flipped-90",
        Transform::Flipped180 => "flipped-180",
        Transform::Flipped270 => "flipped-270",
        _ => "normal",
    }
}
