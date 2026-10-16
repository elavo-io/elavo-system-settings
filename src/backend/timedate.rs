//! Time zone and network time through systemd-timedated, and the 12- or
//! 24-hour clock through `org.gnome.desktop.interface clock-format`, which
//! the panel clock follows.

use gtk::prelude::*;
use gtk::{gio, glib};

use super::dbus::{self, prop, prop_str};
use super::gsettings;

const TIMEDATE: &str = "org.freedesktop.timedate1";
const TIMEDATE_PATH: &str = "/org/freedesktop/timedate1";

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub available: bool,
    pub timezone: String,
    pub ntp: bool,
    pub can_ntp: bool,
}

pub fn bus() -> Option<gio::DBusConnection> {
    dbus::system()
}

pub async fn status(conn: &gio::DBusConnection) -> Status {
    let p = dbus::get_all(conn, TIMEDATE, TIMEDATE_PATH, TIMEDATE).await;
    Status {
        available: !p.is_empty(),
        timezone: prop_str(&p, "Timezone"),
        ntp: prop::<bool>(&p, "NTP").unwrap_or(false),
        can_ntp: prop::<bool>(&p, "CanNTP").unwrap_or(false),
    }
}

pub async fn timezones(conn: &gio::DBusConnection) -> Vec<String> {
    match dbus::call(
        conn,
        TIMEDATE,
        TIMEDATE_PATH,
        TIMEDATE,
        "ListTimezones",
        None,
    )
    .await
    {
        Ok(reply) => dbus::strings(&reply.child_value(0)),
        Err(_) => Vec::new(),
    }
}

pub async fn set_timezone(conn: &gio::DBusConnection, zone: &str) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        TIMEDATE,
        TIMEDATE_PATH,
        TIMEDATE,
        "SetTimezone",
        Some((zone, true).to_variant()),
    )
    .await
    .map(|_| ())
}

pub async fn set_ntp(conn: &gio::DBusConnection, on: bool) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        TIMEDATE,
        TIMEDATE_PATH,
        TIMEDATE,
        "SetNTP",
        Some((on, true).to_variant()),
    )
    .await
    .map(|_| ())
}

/// Set the clock to `when`, microseconds since the epoch in UTC.
pub async fn set_time(conn: &gio::DBusConnection, when_usec: i64) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        TIMEDATE,
        TIMEDATE_PATH,
        TIMEDATE,
        "SetTime",
        Some((when_usec, false, true).to_variant()),
    )
    .await
    .map(|_| ())
}

pub fn watch(conn: &gio::DBusConnection, f: impl Fn() + 'static) -> gio::SignalSubscription {
    dbus::on_properties_changed(conn, TIMEDATE, Some(TIMEDATE_PATH), move |_, _, _| f())
}

/// The schema's default is 24h, so an unset key follows the locale. The
/// panel clock reads it the same way.
pub fn uses_24h() -> bool {
    match gsettings::get_user_string("clock-format").as_deref() {
        Some("12h") => false,
        Some(_) => true,
        None => locale_uses_24h(),
    }
}

fn locale_uses_24h() -> bool {
    glib::DateTime::from_local(2000, 1, 1, 13, 0, 0.0)
        .and_then(|t| t.format("%X"))
        .map(|s| s.contains("13"))
        .unwrap_or(true)
}

pub fn set_24h(on: bool) -> bool {
    gsettings::set_string("clock-format", if on { "24h" } else { "12h" })
}

/// `America/Los_Angeles` to `Los Angeles (America)`.
pub fn zone_label(zone: &str) -> String {
    match zone.rsplit_once('/') {
        Some((region, city)) => {
            format!("{} ({})", city.replace('_', " "), region.replace('_', " "))
        }
        None => zone.replace('_', " "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zone_labels() {
        assert_eq!(zone_label("America/Los_Angeles"), "Los Angeles (America)");
        assert_eq!(
            zone_label("America/Argentina/Buenos_Aires"),
            "Buenos Aires (America/Argentina)"
        );
        assert_eq!(zone_label("UTC"), "UTC");
    }
}
