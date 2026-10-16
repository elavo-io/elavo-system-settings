//! Thin async wrappers over `gio::DBusConnection`, so the services run on
//! the GTK main loop and no second runtime is needed.

use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gio, glib};

pub type Props = HashMap<String, glib::Variant>;

const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
/// Calls that may show a polkit prompt wait for the person to answer.
const TIMEOUT_MS: i32 = 5 * 60 * 1000;

pub fn system() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE).ok()
}

pub fn session() -> Option<gio::DBusConnection> {
    gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).ok()
}

pub async fn call(
    conn: &gio::DBusConnection,
    dest: &str,
    path: &str,
    iface: &str,
    method: &str,
    args: Option<glib::Variant>,
) -> Result<glib::Variant, glib::Error> {
    conn.call_future(
        Some(dest),
        path,
        iface,
        method,
        args.as_ref(),
        None,
        gio::DBusCallFlags::ALLOW_INTERACTIVE_AUTHORIZATION,
        TIMEOUT_MS,
    )
    .await
}

pub async fn get_all(conn: &gio::DBusConnection, dest: &str, path: &str, iface: &str) -> Props {
    match call(
        conn,
        dest,
        path,
        PROPERTIES,
        "GetAll",
        Some((iface,).to_variant()),
    )
    .await
    {
        Ok(reply) => reply.child_value(0).get::<Props>().unwrap_or_default(),
        Err(_) => Props::new(),
    }
}

pub async fn get(
    conn: &gio::DBusConnection,
    dest: &str,
    path: &str,
    iface: &str,
    name: &str,
) -> Option<glib::Variant> {
    let reply = call(
        conn,
        dest,
        path,
        PROPERTIES,
        "Get",
        Some((iface, name).to_variant()),
    )
    .await
    .ok()?;
    reply.child_value(0).as_variant()
}

pub async fn set(
    conn: &gio::DBusConnection,
    dest: &str,
    path: &str,
    iface: &str,
    name: &str,
    value: glib::Variant,
) -> Result<(), glib::Error> {
    call(
        conn,
        dest,
        path,
        PROPERTIES,
        "Set",
        Some((iface, name, value).to_variant()),
    )
    .await
    .map(|_| ())
}

pub fn prop<T: glib::variant::FromVariant>(props: &Props, key: &str) -> Option<T> {
    props.get(key)?.get::<T>()
}

pub fn prop_str(props: &Props, key: &str) -> String {
    props
        .get(key)
        .and_then(|v| v.str().map(str::to_owned))
        .unwrap_or_default()
}

/// The strings of an `as` or `ao` value.
pub fn strings(value: &glib::Variant) -> Vec<String> {
    value
        .iter()
        .filter_map(|v| v.str().map(str::to_owned))
        .collect()
}

/// Every PropertiesChanged signal from `dest`, optionally for one path.
pub fn on_properties_changed(
    conn: &gio::DBusConnection,
    dest: &str,
    path: Option<&str>,
    f: impl Fn(&str, &str, Props) + 'static,
) -> gio::SignalSubscription {
    conn.subscribe_to_signal(
        Some(dest),
        Some(PROPERTIES),
        Some("PropertiesChanged"),
        path,
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| {
            let params = signal.parameters;
            let iface = params.child_value(0);
            let changed = params.child_value(1).get::<Props>().unwrap_or_default();
            f(signal.object_path, iface.str().unwrap_or(""), changed);
        },
    )
}

/// Any signal from `dest` on `iface`.
pub fn on_signal(
    conn: &gio::DBusConnection,
    dest: &str,
    iface: Option<&str>,
    member: Option<&str>,
    f: impl Fn(&str, &str, &glib::Variant) + 'static,
) -> gio::SignalSubscription {
    conn.subscribe_to_signal(
        Some(dest),
        iface,
        member,
        None,
        None,
        gio::DBusSignalFlags::NONE,
        move |signal| f(signal.object_path, signal.signal_name, signal.parameters),
    )
}

/// Ask the user's systemd to `StartUnit`, `TryRestartUnit`, and so on, in
/// the background. Failures are ignored: the unit may not exist outside
/// the Elavo image.
pub fn user_unit(method: &'static str, unit: &'static str) {
    glib::spawn_future_local(async move {
        if let Some(conn) = session() {
            let _ = call(
                &conn,
                "org.freedesktop.systemd1",
                "/org/freedesktop/systemd1",
                "org.freedesktop.systemd1.Manager",
                method,
                Some((unit, "replace").to_variant()),
            )
            .await;
        }
    });
}

/// Coalesces bursts of change signals into one refresh on the main loop.
pub struct Debounce {
    pending: std::cell::Cell<bool>,
    delay_ms: u64,
}

impl Debounce {
    pub fn new(delay_ms: u64) -> std::rc::Rc<Self> {
        std::rc::Rc::new(Self {
            pending: std::cell::Cell::new(false),
            delay_ms,
        })
    }

    pub fn run(self: &std::rc::Rc<Self>, f: impl FnOnce() + 'static) {
        if self.pending.replace(true) {
            return;
        }
        let this = self.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(self.delay_ms), move || {
            this.pending.set(false);
            f();
        });
    }
}
