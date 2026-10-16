//! Power: idle timeouts (written to `idle.conf`, applied by restarting the
//! image's `elavo-idle` user service), the power profile from
//! power-profiles-daemon, and what closing the lid does.

use gtk::prelude::*;
use gtk::{gio, glib};

use super::dbus::{self, prop_str, Props};
use super::keyfile::idle;

const PROFILES: [(&str, &str); 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
    ),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];
const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";
pub const IDLE_UNIT: &str = "elavo-idle.service";

pub fn save_idle(value: idle::Idle) -> std::io::Result<()> {
    idle::save(value)?;
    dbus::user_unit("TryRestartUnit", IDLE_UNIT);
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub struct Profiles {
    pub service: String,
    pub path: String,
    pub active: String,
    pub available: Vec<String>,
}

pub async fn profiles(conn: &gio::DBusConnection) -> Option<Profiles> {
    for (name, path) in PROFILES {
        let p = dbus::get_all(conn, name, path, name).await;
        if p.is_empty() {
            continue;
        }
        let available = p
            .get("Profiles")
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.get::<Props>())
                    .map(|d| prop_str(&d, "Profile"))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        return Some(Profiles {
            service: name.to_owned(),
            path: path.to_owned(),
            active: prop_str(&p, "ActiveProfile"),
            available,
        });
    }
    None
}

pub async fn set_profile(
    conn: &gio::DBusConnection,
    p: &Profiles,
    profile: &str,
) -> Result<(), glib::Error> {
    dbus::set(
        conn,
        &p.service,
        &p.path,
        &p.service,
        "ActiveProfile",
        profile.to_variant(),
    )
    .await
}

pub fn profile_label(profile: &str) -> String {
    use crate::i18n::tr;
    match profile {
        "power-saver" => tr("Power Saver"),
        "balanced" => tr("Balanced"),
        "performance" => tr("Performance"),
        other => other.to_owned(),
    }
}

pub fn has_lid() -> bool {
    std::fs::read_dir("/proc/acpi/button/lid")
        .map(|mut d| d.next().is_some())
        .unwrap_or(false)
}

/// What logind does now when the lid closes.
pub async fn lid_action(conn: &gio::DBusConnection) -> String {
    dbus::get(conn, LOGIND, LOGIND_PATH, LOGIND_MANAGER, "HandleLidSwitch")
        .await
        .and_then(|v| v.str().map(str::to_owned))
        .unwrap_or_else(|| "suspend".to_owned())
}

pub async fn reboot(conn: &gio::DBusConnection) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        LOGIND,
        LOGIND_PATH,
        LOGIND_MANAGER,
        "Reboot",
        Some((true,).to_variant()),
    )
    .await
    .map(|_| ())
}
