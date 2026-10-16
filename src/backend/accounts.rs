//! Local accounts through AccountsService. It checks polkit itself, so
//! changing your own name needs nothing and adding a user asks for an
//! administrator.

use gtk::prelude::*;
use gtk::{gio, glib};

use super::dbus::{self, prop, prop_str};

const ACCOUNTS: &str = "org.freedesktop.Accounts";
const ACCOUNTS_PATH: &str = "/org/freedesktop/Accounts";
const USER: &str = "org.freedesktop.Accounts.User";

#[derive(Clone, Debug)]
pub struct User {
    pub path: String,
    pub uid: u64,
    pub name: String,
    pub real_name: String,
    pub admin: bool,
    pub icon: String,
}

impl User {
    pub fn display_name(&self) -> &str {
        if self.real_name.is_empty() {
            &self.name
        } else {
            &self.real_name
        }
    }

    pub fn initial(&self) -> String {
        self.display_name()
            .chars()
            .next()
            .map(|c| c.to_uppercase().collect())
            .unwrap_or_default()
    }

    pub fn is_current(&self) -> bool {
        self.uid == u64::from(unsafe { libc::getuid() })
    }
}

pub fn bus() -> Option<gio::DBusConnection> {
    dbus::system()
}

async fn user(conn: &gio::DBusConnection, path: &str) -> Option<User> {
    let p = dbus::get_all(conn, ACCOUNTS, path, USER).await;
    if p.is_empty() || prop::<bool>(&p, "SystemAccount").unwrap_or(false) {
        return None;
    }
    let icon = prop_str(&p, "IconFile");
    Some(User {
        path: path.to_owned(),
        uid: prop::<u64>(&p, "Uid").unwrap_or(0),
        name: prop_str(&p, "UserName"),
        real_name: prop_str(&p, "RealName"),
        admin: prop::<i32>(&p, "AccountType").unwrap_or(0) == 1,
        icon: if std::path::Path::new(&icon).is_file() {
            icon
        } else {
            String::new()
        },
    })
}

/// Every person account, the current one first. `None` when AccountsService
/// is not running.
pub async fn list(conn: &gio::DBusConnection) -> Option<Vec<User>> {
    let reply = dbus::call(
        conn,
        ACCOUNTS,
        ACCOUNTS_PATH,
        ACCOUNTS,
        "ListCachedUsers",
        None,
    )
    .await
    .ok()?;
    let mut paths = dbus::strings(&reply.child_value(0));
    // The cache can miss the current user when no one else has looked it up.
    if let Ok(me) = dbus::call(
        conn,
        ACCOUNTS,
        ACCOUNTS_PATH,
        ACCOUNTS,
        "FindUserById",
        Some((i64::from(unsafe { libc::getuid() }),).to_variant()),
    )
    .await
    {
        if let Some(path) = me.child_value(0).str() {
            if !paths.iter().any(|p| p == path) {
                paths.push(path.to_owned());
            }
        }
    }
    let mut users = Vec::new();
    for path in paths {
        if let Some(u) = user(conn, &path).await {
            users.push(u);
        }
    }
    users.sort_by(|a, b| {
        b.is_current().cmp(&a.is_current()).then(
            a.display_name()
                .to_lowercase()
                .cmp(&b.display_name().to_lowercase()),
        )
    });
    Some(users)
}

/// A login name: lowercase letters, digits, `-` and `_`, starting with a
/// letter, at most 32 characters.
pub fn valid_username(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_lowercase())
        && name.len() <= 32
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A login name suggested from a full name: "Ada Lovelace" to "ada".
pub fn suggest_username(full_name: &str) -> String {
    let first = full_name.split_whitespace().next().unwrap_or("");
    let s: String = first
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let s = s
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .to_owned();
    s.chars().take(32).collect()
}

pub fn hash_password(password: &str) -> Result<String, String> {
    sha_crypt::sha512_simple(password, &sha_crypt::Sha512Params::default())
        .map_err(|e| format!("{e:?}"))
}

pub async fn create(
    conn: &gio::DBusConnection,
    name: &str,
    real_name: &str,
    admin: bool,
    password: &str,
) -> Result<(), glib::Error> {
    let reply = dbus::call(
        conn,
        ACCOUNTS,
        ACCOUNTS_PATH,
        ACCOUNTS,
        "CreateUser",
        Some((name, real_name, i32::from(admin)).to_variant()),
    )
    .await?;
    let path = reply.child_value(0).str().unwrap_or("").to_owned();
    if !password.is_empty() && !path.is_empty() {
        set_password(conn, &path, password).await?;
    }
    Ok(())
}

pub async fn delete(
    conn: &gio::DBusConnection,
    uid: u64,
    remove_files: bool,
) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        ACCOUNTS,
        ACCOUNTS_PATH,
        ACCOUNTS,
        "DeleteUser",
        Some((uid as i64, remove_files).to_variant()),
    )
    .await
    .map(|_| ())
}

pub async fn set_real_name(
    conn: &gio::DBusConnection,
    path: &str,
    name: &str,
) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        ACCOUNTS,
        path,
        USER,
        "SetRealName",
        Some((name,).to_variant()),
    )
    .await
    .map(|_| ())
}

pub async fn set_admin(
    conn: &gio::DBusConnection,
    path: &str,
    admin: bool,
) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        ACCOUNTS,
        path,
        USER,
        "SetAccountType",
        Some((i32::from(admin),).to_variant()),
    )
    .await
    .map(|_| ())
}

pub async fn set_password(
    conn: &gio::DBusConnection,
    path: &str,
    password: &str,
) -> Result<(), glib::Error> {
    let hashed =
        hash_password(password).map_err(|e| glib::Error::new(gio::IOErrorEnum::Failed, &e))?;
    dbus::call(
        conn,
        ACCOUNTS,
        path,
        USER,
        "SetPassword",
        Some((hashed, "").to_variant()),
    )
    .await
    .map(|_| ())
}

pub async fn set_icon(
    conn: &gio::DBusConnection,
    path: &str,
    file: &str,
) -> Result<(), glib::Error> {
    dbus::call(
        conn,
        ACCOUNTS,
        path,
        USER,
        "SetIconFile",
        Some((file,).to_variant()),
    )
    .await
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usernames() {
        assert!(valid_username("ada"));
        assert!(valid_username("ada_l-2"));
        assert!(!valid_username("Ada"));
        assert!(!valid_username("2ada"));
        assert!(!valid_username(""));
        assert!(!valid_username("a b"));
        assert!(!valid_username(&"a".repeat(33)));
        assert_eq!(suggest_username("Ada Lovelace"), "ada");
        assert_eq!(suggest_username("  José "), "jos");
    }

    #[test]
    fn password_hash_is_sha512_crypt() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$6$"));
        assert!(sha_crypt::sha512_check("correct horse", &hash).is_ok());
    }
}
