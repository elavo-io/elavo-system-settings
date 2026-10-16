//! BlueZ over D-Bus: the adapter, its devices, and a pairing agent so a
//! keyboard or phone can show a code to confirm.

use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::dbus::{self, prop, prop_str, Props};

pub const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const AGENT_MANAGER: &str = "org.bluez.AgentManager1";
const AGENT_PATH: &str = "/io/elavo/SystemSettings/BluetoothAgent";

#[derive(Clone, Debug, Default)]
pub struct Adapter {
    pub path: String,
    pub powered: bool,
    pub discovering: bool,
    pub name: String,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub path: String,
    pub name: String,
    pub address: String,
    pub icon: String,
    pub paired: bool,
    pub connected: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Overview {
    pub adapter: Option<Adapter>,
    pub devices: Vec<Device>,
}

pub fn bus() -> Option<gio::DBusConnection> {
    dbus::system()
}

pub async fn overview(conn: &gio::DBusConnection) -> Overview {
    let Ok(reply) = dbus::call(
        conn,
        BLUEZ,
        "/",
        "org.freedesktop.DBus.ObjectManager",
        "GetManagedObjects",
        None,
    )
    .await
    else {
        return Overview::default();
    };
    let mut overview = Overview::default();
    for entry in reply.child_value(0).iter() {
        let path = entry.child_value(0).str().unwrap_or("").to_owned();
        let Some(ifaces) = entry.child_value(1).get::<HashMap<String, Props>>() else {
            continue;
        };
        if let Some(a) = ifaces.get(ADAPTER) {
            if overview.adapter.is_none() {
                overview.adapter = Some(Adapter {
                    path: path.clone(),
                    powered: prop::<bool>(a, "Powered").unwrap_or(false),
                    discovering: prop::<bool>(a, "Discovering").unwrap_or(false),
                    name: prop_str(a, "Alias"),
                });
            }
        }
        if let Some(d) = ifaces.get(DEVICE) {
            let address = prop_str(d, "Address");
            let mut name = prop_str(d, "Alias");
            if name.is_empty() {
                name = address.clone();
            }
            overview.devices.push(Device {
                path,
                name,
                address,
                icon: prop_str(d, "Icon"),
                paired: prop::<bool>(d, "Paired").unwrap_or(false),
                connected: prop::<bool>(d, "Connected").unwrap_or(false),
            });
        }
    }
    // Unnamed devices nearby are noise; keep them only once paired.
    overview
        .devices
        .retain(|d| d.paired || d.name.replace([':', '-'], "") != d.address.replace(':', ""));
    overview.devices.sort_by(|a, b| {
        b.connected
            .cmp(&a.connected)
            .then(b.paired.cmp(&a.paired))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    overview
}

/// Clear a software block on every Bluetooth radio. BlueZ leaves the
/// adapter in `off-blocked` while rfkill is set, and powering it on then
/// fails with no useful message. The session can write `/dev/rfkill`.
fn unblock_radio() -> std::io::Result<()> {
    use std::io::Write;
    // `struct rfkill_event`, version 1: idx, type, op, soft, hard.
    // Type 2 is Bluetooth. Op 3 applies to every device of that type.
    let mut event = [0u8; 8];
    event[4] = 2;
    event[5] = 3;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open("/dev/rfkill")?;
    file.write_all(&event)
}

pub async fn set_powered(
    conn: &gio::DBusConnection,
    adapter: &str,
    on: bool,
) -> Result<(), glib::Error> {
    if on {
        let _ = unblock_radio();
    }
    dbus::set(conn, BLUEZ, adapter, ADAPTER, "Powered", on.to_variant()).await
}

pub async fn discover(
    conn: &gio::DBusConnection,
    adapter: &str,
    on: bool,
) -> Result<(), glib::Error> {
    let method = if on {
        "StartDiscovery"
    } else {
        "StopDiscovery"
    };
    dbus::call(conn, BLUEZ, adapter, ADAPTER, method, None)
        .await
        .map(|_| ())
}

pub async fn pair(conn: &gio::DBusConnection, device: &str) -> Result<(), glib::Error> {
    dbus::call(conn, BLUEZ, device, DEVICE, "Pair", None).await?;
    let _ = dbus::set(conn, BLUEZ, device, DEVICE, "Trusted", true.to_variant()).await;
    dbus::call(conn, BLUEZ, device, DEVICE, "Connect", None)
        .await
        .map(|_| ())
}

pub async fn connect(conn: &gio::DBusConnection, device: &str) -> Result<(), glib::Error> {
    dbus::call(conn, BLUEZ, device, DEVICE, "Connect", None)
        .await
        .map(|_| ())
}

pub async fn disconnect(conn: &gio::DBusConnection, device: &str) -> Result<(), glib::Error> {
    dbus::call(conn, BLUEZ, device, DEVICE, "Disconnect", None)
        .await
        .map(|_| ())
}

pub async fn remove(
    conn: &gio::DBusConnection,
    adapter: &str,
    device: &str,
) -> Result<(), glib::Error> {
    let path = glib::variant::ObjectPath::try_from(device.to_owned())
        .map_err(|_| glib::Error::new(gio::IOErrorEnum::InvalidArgument, "bad device path"))?;
    dbus::call(
        conn,
        BLUEZ,
        adapter,
        ADAPTER,
        "RemoveDevice",
        Some((path,).to_variant()),
    )
    .await
    .map(|_| ())
}

pub fn watch(conn: &gio::DBusConnection, f: impl Fn() + 'static) -> Vec<gio::SignalSubscription> {
    let f = Rc::new(f);
    let g = f.clone();
    vec![
        dbus::on_properties_changed(conn, BLUEZ, None, move |_, _, _| f()),
        dbus::on_signal(
            conn,
            BLUEZ,
            Some("org.freedesktop.DBus.ObjectManager"),
            None,
            move |_, _, _| g(),
        ),
    ]
}

/// The device's alias, or its address when it has none.
pub async fn device_name(conn: &gio::DBusConnection, path: &str) -> String {
    dbus::get(conn, BLUEZ, path, DEVICE, "Alias")
        .await
        .and_then(|v| v.str().map(str::to_owned))
        .unwrap_or_default()
}

/// What the agent needs from the person.
pub enum Request {
    /// Confirm that both sides show this six-digit code.
    Confirm { device: String, passkey: u32 },
    /// Type the PIN shown on, or printed on, the device.
    Pin { device: String },
    /// Type a numeric passkey.
    Passkey { device: String },
    /// Type this code on the device's keyboard.
    Display { device: String, code: String },
}

pub enum Answer {
    Accept,
    Reject,
    Text(String),
}

const AGENT_XML: &str = r#"<node>
  <interface name="org.bluez.Agent1">
    <method name="Release"/>
    <method name="RequestPinCode"><arg type="o" direction="in"/><arg type="s" direction="out"/></method>
    <method name="DisplayPinCode"><arg type="o" direction="in"/><arg type="s" direction="in"/></method>
    <method name="RequestPasskey"><arg type="o" direction="in"/><arg type="u" direction="out"/></method>
    <method name="DisplayPasskey"><arg type="o" direction="in"/><arg type="u" direction="in"/><arg type="q" direction="in"/></method>
    <method name="RequestConfirmation"><arg type="o" direction="in"/><arg type="u" direction="in"/></method>
    <method name="RequestAuthorization"><arg type="o" direction="in"/></method>
    <method name="AuthorizeService"><arg type="o" direction="in"/><arg type="s" direction="in"/></method>
    <method name="Cancel"/>
  </interface>
</node>"#;

pub struct Agent {
    conn: gio::DBusConnection,
    id: Option<gio::RegistrationId>,
}

impl Drop for Agent {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let _ = self.conn.unregister_object(id);
        }
    }
}

/// Export the agent and make it the default. `ask` is called on the GTK
/// thread for each request and answers asynchronously.
pub fn register_agent(
    conn: &gio::DBusConnection,
    ask: impl Fn(Request) -> std::pin::Pin<Box<dyn std::future::Future<Output = Answer>>> + 'static,
) -> Option<Agent> {
    let info = gio::DBusNodeInfo::for_xml(AGENT_XML).ok()?;
    let iface = info.lookup_interface("org.bluez.Agent1")?;
    let ask = Rc::new(ask);
    let id =
        conn.register_object(AGENT_PATH, &iface)
            .method_call(move |_, _, _, _, method, params, invocation| {
                let device = params
                    .try_child_value(0)
                    .and_then(|v| v.str().map(str::to_owned))
                    .unwrap_or_default();
                let request = match method {
                    "RequestConfirmation" => Some(Request::Confirm {
                        device,
                        passkey: params.child_value(1).get::<u32>().unwrap_or(0),
                    }),
                    "RequestPinCode" => Some(Request::Pin { device }),
                    "RequestPasskey" => Some(Request::Passkey { device }),
                    "DisplayPinCode" => Some(Request::Display {
                        device,
                        code: params.child_value(1).str().unwrap_or("").to_owned(),
                    }),
                    "DisplayPasskey" => Some(Request::Display {
                        device,
                        code: format!("{:06}", params.child_value(1).get::<u32>().unwrap_or(0)),
                    }),
                    _ => None,
                };
                let Some(request) = request else {
                    // Release, Cancel, and authorizing a paired device's services.
                    invocation.return_value(None);
                    return;
                };
                let returns_value = matches!(method, "RequestPinCode" | "RequestPasskey");
                let numeric = method == "RequestPasskey";
                let future = ask(request);
                glib::spawn_future_local(async move {
                    match future.await {
                        Answer::Accept if !returns_value => invocation.return_value(None),
                        Answer::Text(text) if numeric => match text.trim().parse::<u32>() {
                            Ok(n) => invocation.return_value(Some(&(n,).to_variant())),
                            Err(_) => invocation
                                .return_dbus_error("org.bluez.Error.Rejected", "not a number"),
                        },
                        Answer::Text(text) if returns_value => {
                            invocation.return_value(Some(&(text,).to_variant()))
                        }
                        _ => invocation.return_dbus_error("org.bluez.Error.Rejected", "rejected"),
                    }
                });
            })
            .build()
            .ok()?;
    let agent = Agent {
        conn: conn.clone(),
        id: Some(id),
    };
    let conn = conn.clone();
    glib::spawn_future_local(async move {
        let path = glib::variant::ObjectPath::try_from(AGENT_PATH.to_owned()).ok();
        if let Some(path) = path {
            let _ = dbus::call(
                &conn,
                BLUEZ,
                "/org/bluez",
                AGENT_MANAGER,
                "RegisterAgent",
                Some((path.clone(), "KeyboardDisplay").to_variant()),
            )
            .await;
            let _ = dbus::call(
                &conn,
                BLUEZ,
                "/org/bluez",
                AGENT_MANAGER,
                "RequestDefaultAgent",
                Some((path,).to_variant()),
            )
            .await;
        }
    });
    Some(agent)
}
