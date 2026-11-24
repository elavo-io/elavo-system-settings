//! Accounts through AccountsService. It checks polkit itself, so
//! changing your own name needs nothing and adding a user asks for an
//! administrator.
//!
//! Directory accounts (Active Directory, FreeIPA, through sssd) are not in
//! AccountsService's list. The ones that have used this computer are
//! added: a home in `/home`, or a wallpaper their session published for the
//! login window. They are marked `directory`; their name, password, and
//! administrator rights belong to the directory, not to this computer.

use gtk::prelude::*;
use gtk::{gio, glib};

use super::dbus::{self, prop, prop_str};

const ACCOUNTS: &str = "org.freedesktop.Accounts";
const ACCOUNTS_PATH: &str = "/org/freedesktop/Accounts";
const USER: &str = "org.freedesktop.Accounts.User";
const HOMES: &str = "/home";
/// Where sessions publish their wallpaper for the login window, by UID.
const PUBLISHED: &str = "/var/lib/elavo/greeter/wallpapers";
/// The image's directory settings. `AdminGroup=` lines name the directory
/// groups whose members administer this computer.
const DIRECTORY_CONF: &str = "/etc/elavo/directory.conf";

#[derive(Clone, Debug)]
pub struct User {
    pub path: String,
    pub uid: u64,
    pub name: String,
    pub real_name: String,
    pub admin: bool,
    pub icon: String,
    /// Not in `/etc/passwd`: the account comes from a directory.
    pub directory: bool,
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
    let name = prop_str(&p, "UserName");
    let directory = !local_names().contains(&name);
    // AccountsService only counts wheel. Directory administrators are
    // members of the groups in directory.conf.
    let admin =
        prop::<i32>(&p, "AccountType").unwrap_or(0) == 1 || (directory && in_admin_group(&name));
    Some(User {
        path: path.to_owned(),
        uid: prop::<u64>(&p, "Uid").unwrap_or(0),
        name,
        real_name: prop_str(&p, "RealName"),
        admin,
        icon: if std::path::Path::new(&icon).is_file() {
            icon
        } else {
            String::new()
        },
        directory,
    })
}

/// The `AdminGroup=` values in directory.conf's text.
fn admin_groups(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("AdminGroup="))
        .map(|group| group.trim().to_owned())
        .filter(|group| !group.is_empty())
        .collect()
}

/// Whether `name` is in one of directory.conf's administrator groups.
fn in_admin_group(name: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(DIRECTORY_CONF) else {
        return false;
    };
    let wanted: Vec<libc::gid_t> = admin_groups(&text)
        .iter()
        .filter_map(|group| gid_of(group))
        .collect();
    !wanted.is_empty() && groups_of(name).iter().any(|gid| wanted.contains(gid))
}

fn gid_of(group: &str) -> Option<libc::gid_t> {
    let group = std::ffi::CString::new(group).ok()?;
    let mut entry: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 64 * 1024];
    let status = unsafe {
        libc::getgrnam_r(
            group.as_ptr(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    (status == 0 && !result.is_null()).then_some(entry.gr_gid)
}

/// Every group `name` is in, from the system's databases (sssd included).
fn groups_of(name: &str) -> Vec<libc::gid_t> {
    let Ok(user) = std::ffi::CString::new(name) else {
        return Vec::new();
    };
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let status = unsafe {
        libc::getpwnam_r(
            user.as_ptr(),
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return Vec::new();
    }
    let mut count: libc::c_int = 64;
    loop {
        let mut groups = vec![0 as libc::gid_t; count as usize];
        let before = count;
        let found = unsafe {
            libc::getgrouplist(user.as_ptr(), entry.pw_gid, groups.as_mut_ptr(), &mut count)
        };
        if found >= 0 {
            groups.truncate(count.max(0) as usize);
            return groups;
        }
        if count <= before {
            return Vec::new();
        }
    }
}

/// Names in `/etc/passwd`.
fn local_names() -> Vec<String> {
    std::fs::read_to_string("/etc/passwd")
        .map(|text| passwd_names(&text))
        .unwrap_or_default()
}

fn passwd_names(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| line.split(':').next())
        .filter(|name| !name.is_empty() && !name.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Accounts that have used this computer without being in
/// AccountsService's list: folders in `/home`, and the UIDs that published
/// a wallpaper, as user names.
fn used_here() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(HOMES)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    let uids = std::fs::read_dir(PUBLISHED)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok());
    names.extend(uids.filter_map(name_for_uid));
    names.sort();
    names.dedup();
    names
}

/// The user name for `uid` from the system's user database (sssd
/// included).
fn name_for_uid(uid: u32) -> Option<String> {
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let status = unsafe {
        libc::getpwuid_r(
            uid,
            &mut entry,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() || entry.pw_name.is_null() {
        return None;
    }
    let name = unsafe { std::ffi::CStr::from_ptr(entry.pw_name) };
    Some(name.to_string_lossy().into_owned())
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
    let mut users: Vec<User> = Vec::new();
    for path in paths {
        if let Some(u) = user(conn, &path).await {
            users.push(u);
        }
    }
    for name in used_here() {
        if users.iter().any(|u| u.name == name) {
            continue;
        }
        let Ok(reply) = dbus::call(
            conn,
            ACCOUNTS,
            ACCOUNTS_PATH,
            ACCOUNTS,
            "FindUserByName",
            Some((name.as_str(),).to_variant()),
        )
        .await
        else {
            continue;
        };
        let Some(path) = reply.child_value(0).str().map(str::to_owned) else {
            continue;
        };
        if let Some(u) = user(conn, &path).await {
            if !users.iter().any(|known| known.uid == u.uid) {
                users.push(u);
            }
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

/// The logged-in account, when AccountsService answers.
pub async fn current(conn: &gio::DBusConnection) -> Option<User> {
    let reply = dbus::call(
        conn,
        ACCOUNTS,
        ACCOUNTS_PATH,
        ACCOUNTS,
        "FindUserById",
        Some((i64::from(unsafe { libc::getuid() }),).to_variant()),
    )
    .await
    .ok()?;
    user(conn, reply.child_value(0).str()?).await
}

/// AccountsService's picture for the logged-in user, when that file is
/// already on disk. The sidebar shows it before the bus answers.
pub fn local_icon() -> String {
    let path = format!(
        "/var/lib/AccountsService/icons/{}",
        glib::user_name().to_string_lossy()
    );
    if std::path::Path::new(&path).is_file() {
        path
    } else {
        String::new()
    }
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
    fn local_names_come_from_passwd() {
        let text = "root:x:0:0::/root:/bin/bash\nada:x:1000:1000:Ada:/home/ada:/bin/bash\n\n";
        assert_eq!(passwd_names(text), ["root", "ada"]);
    }

    #[test]
    fn admin_groups_come_from_directory_conf() {
        let text = "[Directory]\n# who administers\nAdminGroup=Elavo Admins\nAdminGroup = ignored\nAdminGroup=it-admins \nAllowGroup=Elavo Users\n";
        assert_eq!(admin_groups(text), ["Elavo Admins", "it-admins"]);
    }

    #[test]
    fn root_is_in_its_own_group() {
        assert!(groups_of("root").contains(&0));
        assert!(groups_of("elavo-no-such-user").is_empty());
        assert_eq!(gid_of("root"), Some(0));
    }

    /// For a joined test machine: the accounts the Users pane adds beside
    /// AccountsService's list, and whether each is a directory
    /// administrator. `cargo test -- --ignored --nocapture used_here`.
    #[test]
    #[ignore]
    fn print_accounts_used_here() {
        let local = local_names();
        for name in used_here() {
            let directory = !local.contains(&name);
            let admin = directory && in_admin_group(&name);
            println!("{name} directory={directory} admin={admin}");
        }
    }

    #[test]
    fn root_has_a_name() {
        assert_eq!(name_for_uid(0).as_deref(), Some("root"));
    }

    #[test]
    fn password_hash_is_sha512_crypt() {
        let hash = hash_password("correct horse").unwrap();
        assert!(hash.starts_with("$6$"));
        assert!(sha_crypt::sha512_check("correct horse", &hash).is_ok());
    }
}
