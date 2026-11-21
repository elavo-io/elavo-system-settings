//! General → Sharing: LocalSend's settings in `localsend.conf` (shared with
//! Elavo Files, which receives), the PIN in the keyring, and the password
//! check before a saved PIN is shown.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::PathBuf;

use gtk::prelude::*;
use gtk::glib;

use super::{dbus, keyfile, secret};

pub const LOCALSEND_FILE: &str = "localsend.conf";
const GROUP: &str = "LocalSend";
const FAVORITES: &str = "Favorites";
/// The keyring entry Files reads the PIN from.
pub const PIN_SECRET: &str = "localsend-pin";
/// The polkit action asked before a saved PIN is shown.
pub const REVEAL_ACTION: &str = "io.elavo.Sharing.reveal-secret";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveFrom {
    Off,
    Favorites,
    Everyone,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favorite {
    pub fingerprint: String,
    pub alias: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalSend {
    pub receive_from: ReceiveFrom,
    pub auto_accept_favorites: bool,
    pub require_pin: bool,
    pub save_folder: PathBuf,
    pub favorites: Vec<Favorite>,
}

pub fn downloads() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Downloads)
        .unwrap_or_else(|| glib::home_dir().join("Downloads"))
}

/// The defaults match Files': receive from everyone, ask, auto-accept
/// favorites, Downloads, no PIN unless an older Files left one.
pub fn parse_localsend(f: &glib::KeyFile) -> LocalSend {
    let b = |k: &str, def: bool| f.boolean(GROUP, k).unwrap_or(def);
    let legacy_pin = f
        .string(GROUP, "PIN")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let receive_from = if !b("Receive", true) {
        ReceiveFrom::Off
    } else if b("FavoritesOnly", false) {
        ReceiveFrom::Favorites
    } else {
        ReceiveFrom::Everyone
    };
    let favorites = f
        .keys(FAVORITES)
        .map(|keys| {
            keys.iter()
                .map(|k| Favorite {
                    fingerprint: k.to_string(),
                    alias: f
                        .string(FAVORITES, k)
                        .map(|a| a.to_string())
                        .unwrap_or_default(),
                })
                .collect()
        })
        .unwrap_or_default();
    LocalSend {
        receive_from,
        auto_accept_favorites: b("AutoAcceptFavorites", true),
        require_pin: b("RequirePIN", legacy_pin),
        save_folder: f
            .string(GROUP, "SaveFolder")
            .ok()
            .map(|s| PathBuf::from(s.as_str()))
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(downloads),
        favorites,
    }
}

pub fn localsend() -> LocalSend {
    parse_localsend(&keyfile::load(LOCALSEND_FILE))
}

/// Change some keys and keep the rest, Files' included.
fn edit(f: impl FnOnce(&glib::KeyFile)) -> Result<(), String> {
    let file = keyfile::load(LOCALSEND_FILE);
    f(&file);
    keyfile::save(LOCALSEND_FILE, &file).map_err(|e| e.to_string())
}

pub fn set_receive_from(r: ReceiveFrom) -> Result<(), String> {
    edit(|f| {
        f.set_boolean(GROUP, "Receive", r != ReceiveFrom::Off);
        f.set_boolean(GROUP, "FavoritesOnly", r == ReceiveFrom::Favorites);
    })
}

pub fn set_auto_accept(on: bool) -> Result<(), String> {
    edit(|f| f.set_boolean(GROUP, "AutoAcceptFavorites", on))
}

pub fn set_require_pin(on: bool) -> Result<(), String> {
    edit(|f| f.set_boolean(GROUP, "RequirePIN", on))
}

pub fn set_save_folder(path: &std::path::Path) -> Result<(), String> {
    edit(|f| f.set_string(GROUP, "SaveFolder", &path.to_string_lossy()))
}

pub fn remove_favorite(fingerprint: &str) -> Result<(), String> {
    edit(|f| {
        let _ = f.remove_key(FAVORITES, fingerprint);
    })
}

/// The PIN, from the keyring, or from a `PIN=` an older Files left.
pub fn pin() -> Result<Option<String>, String> {
    match secret::lookup(PIN_SECRET)? {
        Some(pin) => Ok(Some(pin)),
        None => Ok(keyfile::load(LOCALSEND_FILE)
            .string(GROUP, "PIN")
            .ok()
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())),
    }
}

/// Keep `pin` in the keyring, turn Require PIN on, and touch the file so
/// Files reads the new PIN at once. A `PIN=` in the file is removed.
pub fn set_pin(pin: &str) -> Result<(), String> {
    secret::store(PIN_SECRET, "LocalSend PIN", pin)?;
    edit(|f| {
        let _ = f.remove_key(GROUP, "PIN");
        f.set_boolean(GROUP, "RequirePIN", true);
        f.set_int64(GROUP, "PINChanged", glib::real_time() / 1_000_000);
    })
}

/// A PIN: 1 to 16 digits or letters, as LocalSend accepts anything typed.
pub fn pin_problem(pin: &str) -> Option<&'static str> {
    if pin.trim().is_empty() {
        Some("Type a PIN.")
    } else if pin.chars().count() > 32 {
        Some("The PIN is too long.")
    } else if pin.chars().any(char::is_whitespace) {
        Some("The PIN can\u{2019}t contain spaces.")
    } else {
        None
    }
}

// The password check before showing a saved secret

thread_local! {
    /// Once the password was given, saved secrets stay visible until
    /// System Settings quits.
    static REVEALED: Cell<bool> = const { Cell::new(false) };
}

pub fn revealed() -> bool {
    REVEALED.with(Cell::get)
}

/// This process's start time, as polkit identifies a process.
fn start_time() -> Option<u64> {
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after = stat.rsplit_once(')')?.1;
    // Field 22 overall; after the name, fields start at 3.
    after.split_whitespace().nth(19)?.parse().ok()
}

/// Ask polkit, which has Elavo's agent ask for this person's own password
/// (`auth_self`). True once they gave it, here or earlier in this run.
pub async fn authorize_reveal() -> Result<bool, String> {
    if revealed() {
        return Ok(true);
    }
    let conn = dbus::system().ok_or("no system bus")?;
    let subject: HashMap<String, glib::Variant> = HashMap::from([
        ("pid".to_owned(), std::process::id().to_variant()),
        (
            "start-time".to_owned(),
            start_time().unwrap_or(0).to_variant(),
        ),
    ]);
    let details: HashMap<String, String> = HashMap::new();
    let reply = dbus::call(
        &conn,
        "org.freedesktop.PolicyKit1",
        "/org/freedesktop/PolicyKit1/Authority",
        "org.freedesktop.PolicyKit1.Authority",
        "CheckAuthorization",
        Some(
            (
                ("unix-process", subject),
                REVEAL_ACTION,
                details,
                1u32, // AllowUserInteraction
                "",
            )
                .to_variant(),
        ),
    )
    .await
    .map_err(|e| {
        if e.message().contains("not registered") {
            format!("{REVEAL_ACTION} is not installed (io.elavo.Sharing.policy).")
        } else {
            e.message().to_owned()
        }
    })?;
    let authorized = reply
        .child_value(0)
        .child_value(0)
        .get::<bool>()
        .unwrap_or(false);
    if authorized {
        REVEALED.with(|r| r.set(true));
    }
    Ok(authorized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_files_writes() {
        let f = glib::KeyFile::new();
        let d = parse_localsend(&f);
        assert_eq!(d.receive_from, ReceiveFrom::Everyone);
        assert!(d.auto_accept_favorites && !d.require_pin);
        f.set_boolean(GROUP, "Receive", true);
        f.set_boolean(GROUP, "FavoritesOnly", true);
        f.set_string(GROUP, "PIN", "1234");
        f.set_string(FAVORITES, "AB12", "Phone");
        let c = parse_localsend(&f);
        assert_eq!(c.receive_from, ReceiveFrom::Favorites);
        assert!(c.require_pin);
        assert_eq!(c.favorites[0].alias, "Phone");
        f.set_boolean(GROUP, "Receive", false);
        f.set_boolean(GROUP, "RequirePIN", false);
        let c = parse_localsend(&f);
        assert_eq!(c.receive_from, ReceiveFrom::Off);
        assert!(!c.require_pin);
    }

    #[test]
    fn finds_this_process_start_time() {
        assert!(start_time().is_some_and(|t| t > 0));
    }

    #[test]
    fn pins() {
        assert!(pin_problem("").is_some());
        assert!(pin_problem("12 34").is_some());
        assert!(pin_problem("1234").is_none());
    }
}
