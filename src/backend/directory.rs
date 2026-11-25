//! This computer's side of a directory join: `/etc/elavo/directory.conf`
//! and what `elavo-directory` has applied. The image runs
//! `elavo-directory` after every join and leave; Settings only reads its
//! state and changes the administrator groups through the helper.

use super::helper;

pub const CONF: &str = "/etc/elavo/directory.conf";
const STATE: &str = "/var/lib/elavo/directory";

/// Whether the image has the directory support at all.
pub fn supported() -> bool {
    std::path::Path::new("/usr/libexec/elavo-directory").exists()
}

/// Whether `elavo-directory` finished setting up after the join.
pub fn applied() -> bool {
    std::path::Path::new(STATE).join("applied").exists()
}

/// `AdminGroup=` lines.
pub fn admin_groups() -> Vec<String> {
    std::fs::read_to_string(CONF)
        .map(|text| super::accounts::admin_groups(&text))
        .unwrap_or_default()
}

/// Whether directory accounts sign in with short names (`ada`).
pub fn short_names() -> bool {
    let text = std::fs::read_to_string(CONF).unwrap_or_default();
    short_names_in(&text)
}

fn short_names_in(text: &str) -> bool {
    text.lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("ShortNames="))
        .map(|v| !v.trim().eq_ignore_ascii_case("false"))
        .unwrap_or(true)
}

/// Whether the system can look up `group` (sssd included).
pub fn group_exists(group: &str) -> bool {
    super::accounts::gid_of(group).is_some()
}

/// Replace the administrator groups. Asks for an administrator password.
pub async fn set_admin_groups(groups: &[String]) -> Result<(), String> {
    let mut args = vec!["domain-admins"];
    args.extend(groups.iter().map(String::as_str));
    helper::run(&args, |_| {}).await
}

/// Wait up to `seconds` for `elavo-directory` to reach `want`.
pub async fn wait_applied(want: bool, seconds: u32) -> bool {
    for _ in 0..seconds * 2 {
        if applied() == want {
            return true;
        }
        gtk::glib::timeout_future(std::time::Duration::from_millis(500)).await;
    }
    applied() == want
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_default_on() {
        assert!(short_names_in(""));
        assert!(short_names_in("ShortNames=true\n"));
        assert!(!short_names_in("ShortNames=false\n"));
        assert!(!short_names_in("ShortNames=true\nShortNames=False\n"));
    }
}
