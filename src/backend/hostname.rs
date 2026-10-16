//! The computer's name and model through systemd-hostnamed.

use gtk::prelude::*;
use gtk::{gio, glib};

use super::dbus::{self, prop_str};

const HOSTNAME: &str = "org.freedesktop.hostname1";
const HOSTNAME_PATH: &str = "/org/freedesktop/hostname1";

#[derive(Clone, Debug, Default)]
pub struct Info {
    pub pretty: String,
    pub hostname: String,
    pub vendor: String,
    pub model: String,
}

pub async fn info(conn: &gio::DBusConnection) -> Info {
    let p = dbus::get_all(conn, HOSTNAME, HOSTNAME_PATH, HOSTNAME).await;
    let dmi = |name: &str| {
        std::fs::read_to_string(format!("/sys/class/dmi/id/{name}"))
            .map(|s| s.trim().to_owned())
            .unwrap_or_default()
    };
    let mut vendor = prop_str(&p, "HardwareVendor");
    let mut model = prop_str(&p, "HardwareModel");
    if vendor.is_empty() {
        vendor = dmi("sys_vendor");
    }
    if model.is_empty() {
        model = dmi("product_name");
    }
    Info {
        pretty: prop_str(&p, "PrettyHostname"),
        hostname: prop_str(&p, "Hostname"),
        vendor,
        model,
    }
}

/// "Kevin's Laptop" to `Kevins-Laptop`: what the network sees.
pub fn static_name(pretty: &str) -> String {
    let mut out = String::new();
    for c in pretty.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if (c == ' ' || c == '-' || c == '_' || c == '.')
            && !out.ends_with('-')
            && !out.is_empty()
        {
            out.push('-');
        }
    }
    let out = out.trim_end_matches('-');
    let out: String = out.chars().take(63).collect();
    if out.is_empty() {
        "elavo".to_owned()
    } else {
        out
    }
}

pub async fn set_name(conn: &gio::DBusConnection, pretty: &str) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        HOSTNAME,
        HOSTNAME_PATH,
        HOSTNAME,
        "SetPrettyHostname",
        Some((pretty, true).to_variant()),
    )
    .await?;
    dbus::call(
        conn,
        HOSTNAME,
        HOSTNAME_PATH,
        HOSTNAME,
        "SetStaticHostname",
        Some((static_name(pretty), true).to_variant()),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_names() {
        assert_eq!(static_name("Kevin's Laptop"), "Kevins-Laptop");
        assert_eq!(static_name("  my  pc  "), "my-pc");
        assert_eq!(static_name("!!!"), "elavo");
        assert_eq!(static_name(&"a".repeat(80)).len(), 63);
    }
}
