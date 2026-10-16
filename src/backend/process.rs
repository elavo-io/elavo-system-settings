//! Finding this user's compositor and kanshi to ask them to reload. A
//! signal is the interface both programs document.

use std::os::unix::fs::MetadataExt;

/// Process ids whose `comm` is `name` and that belong to this user.
pub fn pids(name: &str) -> Vec<i32> {
    let uid = unsafe { libc::getuid() };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
            let meta = entry.metadata().ok()?;
            if meta.uid() != uid {
                return None;
            }
            let comm = std::fs::read_to_string(entry.path().join("comm")).ok()?;
            (comm.trim() == name).then_some(pid)
        })
        .collect()
}

pub fn signal(pid: i32, sig: i32) -> bool {
    unsafe { libc::kill(pid, sig) == 0 }
}

/// labwc reloads `rc.xml` and `environment` on SIGHUP. `LABWC_PID` is set
/// for programs labwc starts; Settings is usually started by the dock or
/// panel, which run as user services without it, so fall back to `/proc`.
pub fn reconfigure_labwc() -> bool {
    let mut targets: Vec<i32> = std::env::var("LABWC_PID")
        .ok()
        .and_then(|p| p.parse().ok())
        .into_iter()
        .collect();
    if targets.is_empty() {
        targets = pids("labwc");
    }
    hup_all(targets)
}

fn hup_all(pids: Vec<i32>) -> bool {
    let mut any = false;
    for pid in pids {
        any |= signal(pid, libc::SIGHUP);
    }
    any
}

/// kanshi re-reads its configuration on SIGHUP.
pub fn reload_kanshi() -> bool {
    hup_all(pids("kanshi"))
}
