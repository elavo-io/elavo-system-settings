//! Who may sign in on a FreeIPA client. FreeIPA keeps that on the server as
//! HBAC rules, so Settings keeps one rule for this computer,
//! `elavo-<hostname>`, through the `ipa` command with a directory
//! administrator's ticket. The ticket lives in a private credential cache
//! under the runtime directory and is destroyed when the session ends; the
//! password goes to `kinit` on its standard input.

use std::cell::RefCell;
use std::path::PathBuf;

use gtk::{gio, glib};

/// The rule's state on the server.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rule {
    pub exists: bool,
    pub everyone: bool,
    pub groups: Vec<String>,
}

/// A directory administrator's ticket, for as long as the pane needs it.
pub struct Session {
    ccache: PathBuf,
    alive: RefCell<bool>,
}

/// `elavo-` and this computer's short name: one rule per computer.
pub fn rule_name(hostname: &str) -> String {
    let short = hostname.split('.').next().unwrap_or(hostname);
    let clean: String = short
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    format!("elavo-{clean}")
}

/// FreeIPA group names are letters, digits, `_`, `-`, `.`, and a final `$`.
pub fn valid_group(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !name.starts_with('-')
        && name.chars().enumerate().all(|(i, c)| {
            c.is_ascii_alphanumeric() || "_-.".contains(c) || (c == '$' && i == name.len() - 1)
        })
}

async fn run(
    program: &str,
    args: &[&str],
    ccache: &std::path::Path,
    input: Option<&str>,
) -> Result<String, String> {
    let launcher = gio::SubprocessLauncher::new(
        gio::SubprocessFlags::STDIN_PIPE
            | gio::SubprocessFlags::STDOUT_PIPE
            | gio::SubprocessFlags::STDERR_MERGE,
    );
    launcher.setenv("KRB5CCNAME", format!("FILE:{}", ccache.display()), true);
    launcher.setenv("LANG", "C.UTF-8", true);
    launcher.setenv("LC_ALL", "C.UTF-8", true);
    let mut argv: Vec<&std::ffi::OsStr> = vec![program.as_ref()];
    argv.extend(args.iter().map(std::ffi::OsStr::new));
    let process = launcher.spawn(&argv).map_err(|e| e.to_string())?;
    let (out, _) = process
        .communicate_utf8_future(input.map(|s| s.to_owned()))
        .await
        .map_err(|e| e.to_string())?;
    let out = out.map(|s| s.to_string()).unwrap_or_default();
    if process.is_successful() {
        Ok(out)
    } else {
        Err(out.trim().to_owned())
    }
}

const CCACHE_PREFIX: &str = "elavo-settings-ipa-";

/// The process that made a credential cache, from its file name.
fn ccache_owner(file_name: &str) -> Option<u32> {
    file_name
        .strip_prefix(CCACHE_PREFIX)?
        .split('-')
        .next()?
        .parse()
        .ok()
}

/// Remove caches left by a Settings that was killed before it could
/// destroy its ticket.
fn sweep_stale_ccaches(dir: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(ccache_owner) else {
            continue;
        };
        if pid != std::process::id() && !std::path::Path::new(&format!("/proc/{pid}")).exists() {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A private, empty credential cache file in the runtime directory.
fn new_ccache() -> Result<PathBuf, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = glib::user_runtime_dir();
    sweep_stale_ccaches(&dir);
    for n in 0..100u32 {
        let path = dir.join(format!(
            "{CCACHE_PREFIX}{}-{}-{n}",
            std::process::id(),
            glib::monotonic_time()
        ));
        if std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .is_ok()
        {
            return Ok(path);
        }
    }
    Err("no credential cache could be made".to_owned())
}

impl Session {
    /// Get a ticket for `admin` in `realm`.
    pub async fn open(admin: &str, realm: &str, password: &str) -> Result<Session, String> {
        let ccache = new_ccache()?;
        let principal = if admin.contains('@') {
            admin.to_owned()
        } else {
            format!("{admin}@{realm}")
        };
        let input = format!("{password}\n");
        if let Err(e) = run("kinit", &[&principal], &ccache, Some(&input)).await {
            let _ = std::fs::remove_file(&ccache);
            return Err(kinit_error(&e));
        }
        Ok(Session {
            ccache,
            alive: RefCell::new(true),
        })
    }

    async fn ipa(&self, args: &[&str]) -> Result<String, String> {
        run("ipa", args, &self.ccache, None).await
    }

    /// The rule for this computer, and whether FreeIPA's `allow_all` rule
    /// (which lets everyone in everywhere) is still on.
    pub async fn rule(&self, name: &str) -> Result<(Rule, bool), String> {
        let rule = match self.ipa(&["hbacrule-show", name]).await {
            Ok(text) => parse_rule(&text),
            Err(e) if e.contains("not found") => Rule::default(),
            Err(e) => return Err(e),
        };
        let allow_all = match self.ipa(&["hbacrule-show", "allow_all"]).await {
            Ok(text) => field(&text, "Enabled")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
            Err(_) => false,
        };
        Ok((rule, allow_all))
    }

    /// Make the rule for this computer let `groups` (or everyone) sign in
    /// to every service on `host`.
    pub async fn set_rule(
        &self,
        name: &str,
        host: &str,
        everyone: bool,
        groups: &[String],
    ) -> Result<(), String> {
        let (current, _) = self.rule(name).await?;
        if !current.exists {
            self.ipa(&[
                "hbacrule-add",
                name,
                "--servicecat=all",
                "--desc=Who can sign in on this computer, set in Elavo Settings",
            ])
            .await?;
        }
        if let Err(e) = self
            .ipa(&["hbacrule-add-host", name, &format!("--hosts={host}")])
            .await
        {
            if !e.contains("already a member") {
                return Err(e);
            }
        }
        let remove: Vec<&String> = current
            .groups
            .iter()
            .filter(|g| everyone || !groups.iter().any(|n| n.eq_ignore_ascii_case(g)))
            .collect();
        if !remove.is_empty() {
            let mut args = vec!["hbacrule-remove-user".to_owned(), name.to_owned()];
            args.extend(remove.iter().map(|g| format!("--groups={g}")));
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            self.ipa(&args).await?;
        }
        if everyone != current.everyone {
            let value = if everyone {
                "--usercat=all"
            } else {
                "--usercat="
            };
            self.ipa(&["hbacrule-mod", name, value]).await?;
        }
        if !everyone {
            let add: Vec<&String> = groups
                .iter()
                .filter(|g| !current.groups.iter().any(|n| n.eq_ignore_ascii_case(g)))
                .collect();
            if !add.is_empty() {
                let mut args = vec!["hbacrule-add-user".to_owned(), name.to_owned()];
                args.extend(add.iter().map(|g| format!("--groups={g}")));
                let args: Vec<&str> = args.iter().map(String::as_str).collect();
                self.ipa(&args).await?;
            }
        }
        Ok(())
    }

    /// Remove the rule, after leaving with the computer's account removed.
    pub async fn delete_rule(&self, name: &str) -> Result<(), String> {
        match self.ipa(&["hbacrule-del", name]).await {
            Err(e) if !e.contains("not found") => Err(e),
            _ => Ok(()),
        }
    }

    /// Destroy the ticket now rather than when the session is dropped.
    pub fn close(&self) {
        if self.alive.replace(false) {
            let _ = std::process::Command::new("kdestroy")
                .arg("-q")
                .env("KRB5CCNAME", format!("FILE:{}", self.ccache.display()))
                .status();
            let _ = std::fs::remove_file(&self.ccache);
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

fn kinit_error(text: &str) -> String {
    use crate::i18n::tr;
    if text.contains("Password incorrect") || text.contains("Preauthentication failed") {
        tr("The directory administrator's name or password was not accepted.")
    } else if text.contains("not found in Kerberos database") {
        tr("The directory has no administrator with that name.")
    } else if text.contains("Cannot contact any KDC") || text.contains("Cannot find KDC") {
        tr("The directory could not be reached.")
    } else {
        text.to_owned()
    }
}

/// `  Name: value` from `ipa` output.
fn field<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.lines().find_map(|line| {
        let (key, value) = line.trim().split_once(':')?;
        (key.trim() == name).then(|| value.trim())
    })
}

fn parse_rule(text: &str) -> Rule {
    let groups = field(text, "User Groups")
        .map(|v| {
            v.split(',')
                .map(|g| g.trim().to_owned())
                .filter(|g| !g.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Rule {
        exists: field(text, "Rule name").is_some(),
        everyone: field(text, "User category")
            .map(|v| v.eq_ignore_ascii_case("all"))
            .unwrap_or(false),
        groups,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ccache_owners() {
        assert_eq!(
            ccache_owner("elavo-settings-ipa-18133-65205631404-0"),
            Some(18133)
        );
        assert_eq!(ccache_owner("elavo-settings-ipa-x-1-0"), None);
        assert_eq!(ccache_owner("krb5cc_1000"), None);
    }

    #[test]
    fn rule_names() {
        assert_eq!(rule_name("elavo-ipa.ipa.elavo.test"), "elavo-elavo-ipa");
        assert_eq!(rule_name("Eve"), "elavo-eve");
        assert_eq!(rule_name("kev's pc"), "elavo-kev-s-pc");
    }

    #[test]
    fn group_names() {
        assert!(valid_group("elavo-users"));
        assert!(valid_group("admins"));
        assert!(!valid_group("Domain Admins"));
        assert!(!valid_group("a,b"));
        assert!(!valid_group("-x"));
        assert!(!valid_group(""));
    }

    #[test]
    fn reads_ipa_output() {
        let text = "  Rule name: elavo-eve\n  User category: \n  Service category: all\n  Description: x\n  Enabled: True\n  User Groups: elavo-users, admins\n  Hosts: eve.ipa.elavo.test\n";
        let rule = parse_rule(text);
        assert!(rule.exists);
        assert!(!rule.everyone);
        assert_eq!(rule.groups, vec!["elavo-users", "admins"]);
        let all = parse_rule("  Rule name: r\n  User category: all\n");
        assert!(all.everyone && all.groups.is_empty());
        assert_eq!(field("  Enabled: True\n", "Enabled"), Some("True"));
    }
}
