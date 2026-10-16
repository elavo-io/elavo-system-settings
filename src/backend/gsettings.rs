//! `org.gnome.desktop.interface`, shared with GTK and libadwaita apps. The
//! schema is looked up first, because `gio::Settings::new` aborts the
//! process on a missing schema or key.

use gtk::gio;
use gtk::prelude::*;

pub const INTERFACE: &str = "org.gnome.desktop.interface";

pub fn settings(schema: &str, key: &str) -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    let found = source.lookup(schema, true)?;
    if !found.has_key(key) {
        return None;
    }
    Some(gio::Settings::new(schema))
}

pub fn interface(key: &str) -> Option<gio::Settings> {
    settings(INTERFACE, key)
}

pub fn get_string(key: &str) -> Option<String> {
    interface(key).map(|s| s.string(key).to_string())
}

/// The value the person set, or `None` while the key is at its default.
pub fn get_user_string(key: &str) -> Option<String> {
    interface(key)?.user_value(key)?.str().map(str::to_owned)
}

/// Sets a string key when the schema has it. Enum keys are strings too.
pub fn set_string(key: &str, value: &str) -> bool {
    match interface(key) {
        Some(s) => s.set_string(key, value).is_ok(),
        None => false,
    }
}
