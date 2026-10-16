//! NetworkManager over D-Bus: devices, Wi-Fi networks, and saved
//! connections. The control center keeps the quick toggle; this is the full
//! list behind it.

use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gio, glib};

use super::dbus::{self, prop, prop_str, Props};

pub const NM: &str = "org.freedesktop.NetworkManager";
pub const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const IP4: &str = "org.freedesktop.NetworkManager.IP4Config";

const TYPE_ETHERNET: u32 = 1;
const TYPE_WIFI: u32 = 2;
const STATE_ACTIVATED: u32 = 100;
const STATE_UNAVAILABLE: u32 = 20;

// NM_802_11_AP_SEC_* flags.
const SEC_KEY_MGMT_PSK: u32 = 0x100;
const SEC_KEY_MGMT_802_1X: u32 = 0x200;
const SEC_KEY_MGMT_SAE: u32 = 0x400;
const AP_FLAGS_PRIVACY: u32 = 0x1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Security {
    Open,
    Psk,
    Sae,
    Enterprise,
}

#[derive(Clone, Debug)]
pub struct AccessPoint {
    pub path: String,
    pub ssid: String,
    pub strength: u8,
    pub security: Security,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub path: String,
    pub interface: String,
    pub kind: u32,
    pub state: u32,
    pub address: String,
    pub gateway: String,
    pub active_ap: String,
}

impl Device {
    pub fn is_wifi(&self) -> bool {
        self.kind == TYPE_WIFI
    }

    pub fn is_ethernet(&self) -> bool {
        self.kind == TYPE_ETHERNET
    }

    pub fn connected(&self) -> bool {
        self.state == STATE_ACTIVATED
    }

    pub fn available(&self) -> bool {
        self.state > STATE_UNAVAILABLE
    }
}

#[derive(Clone, Debug)]
pub struct Saved {
    pub path: String,
    pub id: String,
    pub ssid: Option<String>,
    pub autoconnect: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Overview {
    pub running: bool,
    pub wifi_enabled: bool,
    pub devices: Vec<Device>,
    pub networks: Vec<AccessPoint>,
    pub saved: Vec<Saved>,
}

pub fn bus() -> Option<gio::DBusConnection> {
    dbus::system()
}

fn ssid_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn security(flags: u32, wpa: u32, rsn: u32) -> Security {
    let all = wpa | rsn;
    if all & SEC_KEY_MGMT_802_1X != 0 {
        Security::Enterprise
    } else if all & SEC_KEY_MGMT_SAE != 0 && all & SEC_KEY_MGMT_PSK == 0 {
        Security::Sae
    } else if all & SEC_KEY_MGMT_PSK != 0 || flags & AP_FLAGS_PRIVACY != 0 {
        Security::Psk
    } else {
        Security::Open
    }
}

async fn device(conn: &gio::DBusConnection, path: &str) -> Device {
    let p = dbus::get_all(conn, NM, path, DEVICE).await;
    let mut address = String::new();
    let mut gateway = String::new();
    if let Some(ip4) = p.get("Ip4Config").and_then(|v| v.str().map(str::to_owned)) {
        if ip4 != "/" {
            let ip = dbus::get_all(conn, NM, &ip4, IP4).await;
            gateway = prop_str(&ip, "Gateway");
            if let Some(list) = ip.get("AddressData") {
                if let Some(first) = list.iter().next().and_then(|a| a.get::<Props>()) {
                    let addr = prop_str(&first, "address");
                    let prefix = prop::<u32>(&first, "prefix").unwrap_or(0);
                    if !addr.is_empty() {
                        address = format!("{addr}/{prefix}");
                    }
                }
            }
        }
    }
    let kind = prop::<u32>(&p, "DeviceType").unwrap_or(0);
    let active_ap = if kind == TYPE_WIFI {
        dbus::get(conn, NM, path, WIRELESS, "ActiveAccessPoint")
            .await
            .and_then(|v| v.str().map(str::to_owned))
            .unwrap_or_default()
    } else {
        String::new()
    };
    Device {
        path: path.to_owned(),
        interface: prop_str(&p, "Interface"),
        kind,
        state: prop::<u32>(&p, "State").unwrap_or(0),
        address,
        gateway,
        active_ap,
    }
}

async fn access_points(conn: &gio::DBusConnection, device: &str) -> Vec<AccessPoint> {
    let Ok(reply) = dbus::call(conn, NM, device, WIRELESS, "GetAllAccessPoints", None).await else {
        return Vec::new();
    };
    let mut best: HashMap<String, AccessPoint> = HashMap::new();
    for path in dbus::strings(&reply.child_value(0)) {
        let p = dbus::get_all(conn, NM, &path, AP).await;
        let ssid = ssid_of(&prop::<Vec<u8>>(&p, "Ssid").unwrap_or_default());
        if ssid.is_empty() {
            continue;
        }
        let ap = AccessPoint {
            path,
            strength: prop::<u8>(&p, "Strength").unwrap_or(0),
            security: security(
                prop::<u32>(&p, "Flags").unwrap_or(0),
                prop::<u32>(&p, "WpaFlags").unwrap_or(0),
                prop::<u32>(&p, "RsnFlags").unwrap_or(0),
            ),
            ssid: ssid.clone(),
        };
        match best.get(&ssid) {
            Some(existing) if existing.strength >= ap.strength => {}
            _ => {
                best.insert(ssid, ap);
            }
        }
    }
    let mut list: Vec<AccessPoint> = best.into_values().collect();
    list.sort_by(|a, b| b.strength.cmp(&a.strength).then(a.ssid.cmp(&b.ssid)));
    list
}

type ConnSettings = HashMap<String, HashMap<String, glib::Variant>>;

async fn connection_settings(conn: &gio::DBusConnection, path: &str) -> Option<ConnSettings> {
    let reply = dbus::call(conn, NM, path, CONNECTION, "GetSettings", None)
        .await
        .ok()?;
    reply.child_value(0).get::<ConnSettings>()
}

async fn saved(conn: &gio::DBusConnection) -> Vec<Saved> {
    let Ok(reply) = dbus::call(conn, NM, SETTINGS_PATH, SETTINGS, "ListConnections", None).await
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for path in dbus::strings(&reply.child_value(0)) {
        let Some(settings) = connection_settings(conn, &path).await else {
            continue;
        };
        let Some(c) = settings.get("connection") else {
            continue;
        };
        let kind = prop_str(c, "type");
        if kind != "802-11-wireless" && kind != "802-3-ethernet" {
            continue;
        }
        let ssid = settings
            .get("802-11-wireless")
            .and_then(|w| prop::<Vec<u8>>(w, "ssid"))
            .map(|b| ssid_of(&b));
        out.push(Saved {
            path,
            id: prop_str(c, "id"),
            ssid,
            autoconnect: prop::<bool>(c, "autoconnect").unwrap_or(true),
        });
    }
    out.sort_by_key(|a| a.id.to_lowercase());
    out
}

pub async fn overview(conn: &gio::DBusConnection) -> Overview {
    let root = dbus::get_all(conn, NM, NM_PATH, NM).await;
    if root.is_empty() {
        return Overview::default();
    }
    let mut devices = Vec::new();
    if let Ok(reply) = dbus::call(conn, NM, NM_PATH, NM, "GetDevices", None).await {
        for path in dbus::strings(&reply.child_value(0)) {
            let d = device(conn, &path).await;
            if d.is_wifi() || d.is_ethernet() {
                devices.push(d);
            }
        }
    }
    let mut networks = Vec::new();
    if let Some(wifi) = devices.iter().find(|d| d.is_wifi()) {
        networks = access_points(conn, &wifi.path).await;
    }
    Overview {
        running: true,
        wifi_enabled: prop::<bool>(&root, "WirelessEnabled").unwrap_or(false),
        devices,
        networks,
        saved: saved(conn).await,
    }
}

pub async fn set_wifi_enabled(conn: &gio::DBusConnection, on: bool) -> Result<(), glib::Error> {
    dbus::set(conn, NM, NM_PATH, NM, "WirelessEnabled", on.to_variant()).await
}

pub async fn scan(conn: &gio::DBusConnection, device: &str) {
    let options: HashMap<String, glib::Variant> = HashMap::new();
    let _ = dbus::call(
        conn,
        NM,
        device,
        WIRELESS,
        "RequestScan",
        Some((options,).to_variant()),
    )
    .await;
}

/// Join a network. A saved connection for the same name is activated; a
/// new one is created with `password` when the network needs one.
pub async fn connect(
    conn: &gio::DBusConnection,
    device: &str,
    ap: &AccessPoint,
    saved: Option<&Saved>,
    password: Option<&str>,
) -> Result<(), glib::Error> {
    let device_path = glib::variant::ObjectPath::try_from(device.to_owned())
        .map_err(|_| glib::Error::new(gio::IOErrorEnum::InvalidArgument, "bad device path"))?;
    let ap_path = glib::variant::ObjectPath::try_from(ap.path.clone())
        .map_err(|_| glib::Error::new(gio::IOErrorEnum::InvalidArgument, "bad network path"))?;
    if let (Some(saved), None) = (saved, password) {
        let conn_path = glib::variant::ObjectPath::try_from(saved.path.clone())
            .map_err(|_| glib::Error::new(gio::IOErrorEnum::InvalidArgument, "bad path"))?;
        return dbus::call(
            conn,
            NM,
            NM_PATH,
            NM,
            "ActivateConnection",
            Some((conn_path, device_path, ap_path).to_variant()),
        )
        .await
        .map(|_| ());
    }
    let mut settings: ConnSettings = HashMap::new();
    let mut wireless = HashMap::new();
    wireless.insert(
        "ssid".to_owned(),
        glib::Variant::array_from_fixed_array(ap.ssid.as_bytes()),
    );
    settings.insert("802-11-wireless".to_owned(), wireless);
    if let Some(password) = password {
        let mut sec = HashMap::new();
        let mgmt = if ap.security == Security::Sae {
            "sae"
        } else {
            "wpa-psk"
        };
        sec.insert("key-mgmt".to_owned(), mgmt.to_variant());
        sec.insert("psk".to_owned(), password.to_variant());
        settings.insert("802-11-wireless-security".to_owned(), sec);
    }
    if let Some(saved) = saved {
        let _ = forget(conn, &saved.path).await;
    }
    dbus::call(
        conn,
        NM,
        NM_PATH,
        NM,
        "AddAndActivateConnection",
        Some((settings, device_path, ap_path).to_variant()),
    )
    .await
    .map(|_| ())
}

pub async fn disconnect(conn: &gio::DBusConnection, device: &str) -> Result<(), glib::Error> {
    dbus::call(conn, NM, device, DEVICE, "Disconnect", None)
        .await
        .map(|_| ())
}

pub async fn forget(conn: &gio::DBusConnection, path: &str) -> Result<(), glib::Error> {
    dbus::call(conn, NM, path, CONNECTION, "Delete", None)
        .await
        .map(|_| ())
}

/// Turn joining automatically on or off. `Update` replaces the whole
/// connection, so the stored secrets are read first and sent back with it.
pub async fn set_autoconnect(
    conn: &gio::DBusConnection,
    path: &str,
    on: bool,
) -> Result<(), glib::Error> {
    let mut settings = connection_settings(conn, path)
        .await
        .ok_or_else(|| glib::Error::new(gio::IOErrorEnum::NotFound, "connection not found"))?;
    if settings.contains_key("802-11-wireless-security") {
        if let Ok(reply) = dbus::call(
            conn,
            NM,
            path,
            CONNECTION,
            "GetSecrets",
            Some(("802-11-wireless-security",).to_variant()),
        )
        .await
        {
            if let Some(secrets) = reply.child_value(0).get::<ConnSettings>() {
                for (group, values) in secrets {
                    settings.entry(group).or_default().extend(values);
                }
            }
        }
    }
    settings
        .entry("connection".to_owned())
        .or_default()
        .insert("autoconnect".to_owned(), on.to_variant());
    dbus::call(
        conn,
        NM,
        path,
        CONNECTION,
        "Update",
        Some((settings,).to_variant()),
    )
    .await
    .map(|_| ())
}

/// Any NetworkManager change: device states, scans, saved connections.
pub fn watch(conn: &gio::DBusConnection, f: impl Fn() + 'static) -> Vec<gio::SignalSubscription> {
    let f = std::rc::Rc::new(f);
    let g = f.clone();
    vec![
        dbus::on_properties_changed(conn, NM, None, move |_, _, _| f()),
        dbus::on_signal(conn, NM, None, None, move |_, name, _| {
            if matches!(
                name,
                "AccessPointAdded"
                    | "AccessPointRemoved"
                    | "NewConnection"
                    | "ConnectionRemoved"
                    | "DeviceAdded"
                    | "DeviceRemoved"
                    | "StateChanged"
            ) {
                g()
            }
        }),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_kinds() {
        assert_eq!(security(0, 0, 0), Security::Open);
        assert_eq!(security(1, 0, 0), Security::Psk);
        assert_eq!(security(1, 0, SEC_KEY_MGMT_PSK), Security::Psk);
        assert_eq!(security(1, 0, SEC_KEY_MGMT_SAE), Security::Sae);
        assert_eq!(
            security(1, 0, SEC_KEY_MGMT_SAE | SEC_KEY_MGMT_PSK),
            Security::Psk
        );
        assert_eq!(security(1, SEC_KEY_MGMT_802_1X, 0), Security::Enterprise);
    }
}
