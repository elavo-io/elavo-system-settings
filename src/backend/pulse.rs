//! Sound over the PulseAudio protocol, which PipeWire serves through
//! `pipewire-pulse`. This is the same state the control center's slider
//! changes, so a mute in either place shows in both.
//!
//! libpulse's threaded mainloop runs the protocol on its own thread. Its
//! callbacks only write into a shared snapshot and wake the GTK thread;
//! requests from the GTK thread take the mainloop lock first. The mainloop
//! and context live for the whole process.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use gtk::glib;
use libpulse_binding as pa;
use pa::callbacks::ListResult;
use pa::context::subscribe::InterestMaskSet;
use pa::context::{Context, FlagSet, State};
use pa::mainloop::threaded::Mainloop;
use pa::proplist::{properties, Proplist};
use pa::stream::{PeekResult, Stream};
use pa::volume::{ChannelVolumes, Volume};

/// The loudest a slider goes, as a fraction of 100%.
pub const MAX_VOLUME: f64 = 1.0;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    pub name: String,
    pub description: String,
    pub volume: f64,
    pub muted: bool,
    pub channels: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct App {
    pub index: u32,
    pub name: String,
    pub icon: String,
    pub volume: f64,
    pub muted: bool,
    pub channels: u8,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub ready: bool,
    pub failed: bool,
    pub sinks: Vec<Device>,
    pub sources: Vec<Device>,
    pub apps: Vec<App>,
    pub default_sink: String,
    pub default_source: String,
}

type Listener = Box<dyn Fn(&Snapshot) -> bool>;

pub struct Pulse {
    ml: *mut Mainloop,
    ctx: *mut Context,
    shared: Arc<Mutex<Snapshot>>,
    listeners: RefCell<Vec<Listener>>,
    peak: Arc<AtomicU32>,
    peak_stream: RefCell<Option<*mut Stream>>,
}

thread_local! {
    static PULSE: RefCell<Option<Rc<Pulse>>> = const { RefCell::new(None) };
}

fn fraction(v: Volume) -> f64 {
    f64::from(v.0) / f64::from(Volume::NORMAL.0)
}

fn volumes(channels: u8, frac: f64) -> ChannelVolumes {
    let mut cv = ChannelVolumes::default();
    let raw = (frac.clamp(0.0, 1.5) * f64::from(Volume::NORMAL.0)).round() as u32;
    cv.set(channels.max(1), Volume(raw));
    cv
}

impl Pulse {
    /// The process-wide connection, started on first use.
    pub fn get() -> Option<Rc<Pulse>> {
        PULSE.with(|cell| {
            if cell.borrow().is_none() {
                *cell.borrow_mut() = Self::start().map(Rc::new);
            }
            cell.borrow().clone()
        })
    }

    fn start() -> Option<Pulse> {
        let ml: &'static mut Mainloop = Box::leak(Box::new(Mainloop::new()?));
        let mut props = Proplist::new()?;
        let _ = props.set_str(properties::APPLICATION_NAME, "System Settings");
        let _ = props.set_str(properties::APPLICATION_ID, crate::APP_ID);
        let _ = props.set_str(properties::APPLICATION_ICON_NAME, crate::APP_ID);
        let ctx: &'static mut Context = Box::leak(Box::new(Context::new_with_proplist(
            ml,
            "System Settings",
            &props,
        )?));
        let ml_ptr: *mut Mainloop = ml;
        let ctx_ptr: *mut Context = ctx;

        let shared = Arc::new(Mutex::new(Snapshot::default()));
        let (tx, rx) = async_channel::unbounded::<()>();

        {
            let shared = shared.clone();
            let tx = tx.clone();
            ctx.set_state_callback(Some(Box::new(move || {
                // SAFETY: runs on the mainloop thread with the lock held;
                // the context is leaked, so the pointer stays valid.
                let ctx = unsafe { &mut *ctx_ptr };
                match ctx.get_state() {
                    State::Ready => {
                        if let Ok(mut s) = shared.lock() {
                            s.ready = true;
                        }
                        let shared_sub = shared.clone();
                        let tx_sub = tx.clone();
                        ctx.set_subscribe_callback(Some(Box::new(move |_, _, _| {
                            refresh(ctx_ptr, &shared_sub, &tx_sub);
                        })));
                        ctx.subscribe(
                            InterestMaskSet::SINK
                                | InterestMaskSet::SOURCE
                                | InterestMaskSet::SINK_INPUT
                                | InterestMaskSet::SERVER,
                            |_| {},
                        );
                        refresh(ctx_ptr, &shared, &tx);
                    }
                    State::Failed | State::Terminated => {
                        if let Ok(mut s) = shared.lock() {
                            s.failed = true;
                            s.ready = false;
                        }
                        let _ = tx.send_blocking(());
                    }
                    _ => {}
                }
            })));
        }

        ml.lock();
        let connected = ctx.connect(None, FlagSet::NOFAIL, None).is_ok();
        ml.unlock();
        if !connected || ml.start().is_err() {
            return None;
        }

        let pulse = Pulse {
            ml: ml_ptr,
            ctx: ctx_ptr,
            shared,
            listeners: RefCell::default(),
            peak: Arc::new(AtomicU32::new(0)),
            peak_stream: RefCell::default(),
        };
        glib::spawn_future_local(async move {
            while rx.recv().await.is_ok() {
                while rx.try_recv().is_ok() {}
                if let Some(p) = PULSE.with(|c| c.borrow().clone()) {
                    p.notify();
                }
            }
        });
        Some(pulse)
    }

    fn notify(&self) {
        let snapshot = self.snapshot();
        self.listeners.borrow_mut().retain(|f| f(&snapshot));
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Call `f` now and after every change, until it returns false.
    pub fn listen(&self, f: impl Fn(&Snapshot) -> bool + 'static) {
        if f(&self.snapshot()) {
            self.listeners.borrow_mut().push(Box::new(f));
        }
    }

    fn with_lock(&self, f: impl FnOnce(&mut Context)) {
        // SAFETY: both are leaked; the lock serializes with the mainloop
        // thread, which is the only other user.
        unsafe {
            (*self.ml).lock();
            f(&mut *self.ctx);
            (*self.ml).unlock();
        }
    }

    pub fn set_sink_volume(&self, dev: &Device, frac: f64) {
        let cv = volumes(dev.channels, frac);
        self.with_lock(|ctx| {
            ctx.introspect()
                .set_sink_volume_by_name(&dev.name, &cv, None);
        });
    }

    pub fn set_sink_mute(&self, name: &str, mute: bool) {
        self.with_lock(|ctx| {
            ctx.introspect().set_sink_mute_by_name(name, mute, None);
        });
    }

    pub fn set_source_volume(&self, dev: &Device, frac: f64) {
        let cv = volumes(dev.channels, frac);
        self.with_lock(|ctx| {
            ctx.introspect()
                .set_source_volume_by_name(&dev.name, &cv, None);
        });
    }

    pub fn set_source_mute(&self, name: &str, mute: bool) {
        self.with_lock(|ctx| {
            ctx.introspect().set_source_mute_by_name(name, mute, None);
        });
    }

    pub fn set_app_volume(&self, app: &App, frac: f64) {
        let cv = volumes(app.channels, frac);
        self.with_lock(|ctx| {
            ctx.introspect().set_sink_input_volume(app.index, &cv, None);
        });
    }

    pub fn set_app_mute(&self, index: u32, mute: bool) {
        self.with_lock(|ctx| {
            ctx.introspect().set_sink_input_mute(index, mute, None);
        });
    }

    pub fn set_default_sink(&self, name: &str) {
        self.with_lock(|ctx| {
            ctx.set_default_sink(name, |_| {});
        });
    }

    pub fn set_default_source(&self, name: &str) {
        self.with_lock(|ctx| {
            ctx.set_default_source(name, |_| {});
        });
    }

    /// The input level, 0 to 1, of the stream `start_peak` opened.
    pub fn peak(&self) -> f32 {
        f32::from_bits(self.peak.load(Ordering::Relaxed))
    }

    /// Record a 25 Hz peak stream from `source`, replacing any earlier one.
    pub fn start_peak(&self, source: &str) {
        self.stop_peak();
        let spec = pa::sample::Spec {
            format: pa::sample::Format::FLOAT32NE,
            channels: 1,
            rate: 25,
        };
        let attr = pa::def::BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: 4,
        };
        let peak = self.peak.clone();
        let mut created: Option<*mut Stream> = None;
        self.with_lock(|ctx| {
            let Some(stream) = Stream::new(ctx, "Input level", &spec, None) else {
                return;
            };
            let stream: *mut Stream = Box::leak(Box::new(stream));
            // SAFETY: the stream is leaked until `stop_peak`, which clears
            // this callback under the lock before freeing it.
            let s = unsafe { &mut *stream };
            s.set_read_callback(Some(Box::new(move |_| {
                let s = unsafe { &mut *stream };
                loop {
                    match s.peek() {
                        Ok(PeekResult::Data(data)) => {
                            if let Some(chunk) = data.rchunks_exact(4).next() {
                                let v =
                                    f32::from_ne_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                                peak.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
                            }
                            let _ = s.discard();
                        }
                        Ok(PeekResult::Hole(_)) => {
                            let _ = s.discard();
                        }
                        _ => break,
                    }
                }
            })));
            let flags = pa::stream::FlagSet::PEAK_DETECT
                | pa::stream::FlagSet::ADJUST_LATENCY
                | pa::stream::FlagSet::DONT_MOVE;
            if s.connect_record(Some(source), Some(&attr), flags).is_ok() {
                created = Some(stream);
            } else {
                s.set_read_callback(None);
                drop(unsafe { Box::from_raw(stream) });
            }
        });
        self.peak_stream.replace(created);
    }

    pub fn stop_peak(&self) {
        let Some(stream) = self.peak_stream.take() else {
            return;
        };
        self.with_lock(|_| {
            // SAFETY: created by `start_peak`; freed exactly once here.
            let mut s = unsafe { Box::from_raw(stream) };
            s.set_read_callback(None);
            let _ = s.disconnect();
        });
        self.peak.store(0, Ordering::Relaxed);
    }
}

fn refresh(ctx_ptr: *mut Context, shared: &Arc<Mutex<Snapshot>>, tx: &async_channel::Sender<()>) {
    // SAFETY: called on the mainloop thread inside a callback.
    let ctx = unsafe { &mut *ctx_ptr };
    let introspect = ctx.introspect();

    let mut sinks = Vec::new();
    let (s, t) = (shared.clone(), tx.clone());
    introspect.get_sink_info_list(move |item| match item {
        ListResult::Item(info) => sinks.push(Device {
            name: info.name.as_deref().unwrap_or("").to_owned(),
            description: info.description.as_deref().unwrap_or("").to_owned(),
            volume: fraction(info.volume.avg()),
            muted: info.mute,
            channels: info.volume.len(),
        }),
        ListResult::End | ListResult::Error => {
            if let Ok(mut snap) = s.lock() {
                snap.sinks = std::mem::take(&mut sinks);
            }
            let _ = t.send_blocking(());
        }
    });

    let mut sources = Vec::new();
    let (s, t) = (shared.clone(), tx.clone());
    introspect.get_source_info_list(move |item| match item {
        ListResult::Item(info) => {
            if info.monitor_of_sink.is_none() {
                sources.push(Device {
                    name: info.name.as_deref().unwrap_or("").to_owned(),
                    description: info.description.as_deref().unwrap_or("").to_owned(),
                    volume: fraction(info.volume.avg()),
                    muted: info.mute,
                    channels: info.volume.len(),
                });
            }
        }
        ListResult::End | ListResult::Error => {
            if let Ok(mut snap) = s.lock() {
                snap.sources = std::mem::take(&mut sources);
            }
            let _ = t.send_blocking(());
        }
    });

    let mut apps = Vec::new();
    let (s, t) = (shared.clone(), tx.clone());
    introspect.get_sink_input_info_list(move |item| match item {
        ListResult::Item(info) => {
            if !info.has_volume {
                return;
            }
            let name = info
                .proplist
                .get_str(properties::APPLICATION_NAME)
                .or_else(|| info.name.as_deref().map(str::to_owned))
                .unwrap_or_default();
            let icon = info
                .proplist
                .get_str(properties::APPLICATION_ICON_NAME)
                .unwrap_or_default();
            apps.push(App {
                index: info.index,
                name,
                icon,
                volume: fraction(info.volume.avg()),
                muted: info.mute,
                channels: info.volume.len(),
            });
        }
        ListResult::End | ListResult::Error => {
            if let Ok(mut snap) = s.lock() {
                snap.apps = std::mem::take(&mut apps);
            }
            let _ = t.send_blocking(());
        }
    });

    let (s, t) = (shared.clone(), tx.clone());
    introspect.get_server_info(move |info| {
        if let Ok(mut snap) = s.lock() {
            snap.default_sink = info.default_sink_name.as_deref().unwrap_or("").to_owned();
            snap.default_source = info.default_source_name.as_deref().unwrap_or("").to_owned();
        }
        let _ = t.send_blocking(());
    });
}
