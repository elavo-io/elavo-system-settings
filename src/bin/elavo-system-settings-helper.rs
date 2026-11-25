//! The privileged half of System Settings, run as root through pkexec.
//!
//! It accepts a fixed set of commands and nothing else: no paths, no image
//! names, no free text. polkit decides who may run each one (see
//! `io.elavo.SystemSettings.policy`); this program only checks the
//! arguments and does the one thing asked.
//!
//! ```text
//! elavo-system-settings-helper upgrade
//! elavo-system-settings-helper upgrade-anyway
//! elavo-system-settings-helper download
//! elavo-system-settings-helper install
//! elavo-system-settings-helper auto off|download|install
//! elavo-system-settings-helper auto-run
//! elavo-system-settings-helper switch stable|latest
//! elavo-system-settings-helper rollback
//! elavo-system-settings-helper lid suspend|lock|ignore|poweroff
//! elavo-system-settings-helper login-window list|name
//! elavo-system-settings-helper wifi-driver broadcom|brcmfmac
//! elavo-system-settings-helper switch-nvidia
//! elavo-system-settings-helper switch-elavo
//! elavo-system-settings-helper domain-admins [GROUP...]
//! ```
//!
//! `auto-run` is what `elavo-system-update.service` runs as root on a
//! timer. It does what `/etc/elavo/updates.conf` says, so turning automatic
//! updates on asks for a password once and never again.
//!
//! `domain-admins` takes directory group names, the one free-text argument:
//! each must be a group the system can look up, and it only ever becomes an
//! `AdminGroup=` line in `/etc/elavo/directory.conf`, which
//! `elavo-directory apply` then turns into sudo, polkit, and Applications
//! folder rights.

#[allow(dead_code)]
#[path = "../backend/auto.rs"]
mod auto;

#[allow(dead_code)]
#[path = "../backend/drivers.rs"]
mod drivers;

#[allow(dead_code)]
#[path = "../backend/layered.rs"]
mod layered;

use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::path::Path;
use std::process::{Command as Process, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

const LID_DROPIN_DIR: &str = "/etc/systemd/logind.conf.d";
const LID_DROPIN: &str = "/etc/systemd/logind.conf.d/50-elavo-lid.conf";
const GREETER_DIR: &str = "/etc/elavo";
const GREETER_CONF: &str = "/etc/elavo/greeter.conf";
const ELAVO_DIR: &str = "/etc/elavo";
const DIRECTORY_CONF: &str = "/etc/elavo/directory.conf";
const ELAVO_DIRECTORY: &str = "/usr/libexec/elavo-directory";
const KEYTAB: &str = "/etc/krb5.keytab";
/// More administrator groups than anyone needs, and a bound on argv.
const MAX_ADMIN_GROUPS: usize = 16;
const STAGED_LOCKED: &str = "/run/ostree/staged-deployment-locked";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Download and install at the next restart.
    Upgrade,
    /// Remove layered packages, then download and install the update.
    UpgradeAnyway,
    /// Download only; the update waits for `install`.
    Download,
    /// Let a downloaded update install at the next restart.
    Install,
    Auto(auto::Auto),
    AutoRun,
    Switch(Channel),
    Rollback,
    Lid(Lid),
    LoginWindow(LoginWindow),
    /// Load Broadcom's `wl`, or turn on the built-in `brcmfmac` driver.
    WifiDriver(WifiDriver),
    /// `bootc switch` to `elavo-nvidia-open` or `elavo-nvidia-legacy` on
    /// the same tag, chosen from the GPU's PCI id.
    SwitchNvidia,
    /// `bootc switch` to `elavo` on the same tag, from an NVIDIA edition.
    SwitchElavo,
    /// The directory groups whose members administer this computer.
    DomainAdmins(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WifiDriver {
    Broadcom,
    Brcmfmac,
}

/// What Folio's login window shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoginWindow {
    List,
    Name,
}

impl LoginWindow {
    fn value(self) -> &'static str {
        match self {
            LoginWindow::List => "list",
            LoginWindow::Name => "name",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    Stable,
    Latest,
}

impl Channel {
    fn tag(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Latest => "latest",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lid {
    Suspend,
    Lock,
    Ignore,
    Poweroff,
}

impl Lid {
    fn value(self) -> &'static str {
        match self {
            Lid::Suspend => "suspend",
            Lid::Lock => "lock",
            Lid::Ignore => "ignore",
            Lid::Poweroff => "poweroff",
        }
    }
}

fn parse(args: &[String]) -> Result<Command, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["upgrade"] => Ok(Command::Upgrade),
        ["upgrade-anyway"] => Ok(Command::UpgradeAnyway),
        ["download"] => Ok(Command::Download),
        ["install"] => Ok(Command::Install),
        ["auto", mode] => auto::Auto::from_arg(mode)
            .map(Command::Auto)
            .ok_or_else(|| format!("unsupported command: auto {mode}")),
        ["auto-run"] => Ok(Command::AutoRun),
        ["rollback"] => Ok(Command::Rollback),
        ["switch", "stable"] => Ok(Command::Switch(Channel::Stable)),
        ["switch", "latest"] => Ok(Command::Switch(Channel::Latest)),
        ["lid", "suspend"] => Ok(Command::Lid(Lid::Suspend)),
        ["lid", "lock"] => Ok(Command::Lid(Lid::Lock)),
        ["lid", "ignore"] => Ok(Command::Lid(Lid::Ignore)),
        ["lid", "poweroff"] => Ok(Command::Lid(Lid::Poweroff)),
        ["login-window", "list"] => Ok(Command::LoginWindow(LoginWindow::List)),
        ["login-window", "name"] => Ok(Command::LoginWindow(LoginWindow::Name)),
        ["wifi-driver", "broadcom"] => Ok(Command::WifiDriver(WifiDriver::Broadcom)),
        ["wifi-driver", "brcmfmac"] => Ok(Command::WifiDriver(WifiDriver::Brcmfmac)),
        ["switch-nvidia"] => Ok(Command::SwitchNvidia),
        ["switch-elavo"] => Ok(Command::SwitchElavo),
        ["domain-admins", groups @ ..] => {
            if groups.len() > MAX_ADMIN_GROUPS {
                return Err("too many groups".to_owned());
            }
            let mut out: Vec<String> = Vec::new();
            for group in groups {
                if !valid_group_name(group) {
                    return Err(format!("not a group name: {group:?}"));
                }
                if !out.iter().any(|g| g.eq_ignore_ascii_case(group)) {
                    out.push((*group).to_owned());
                }
            }
            Ok(Command::DomainAdmins(out))
        }
        _ => Err(format!("unsupported command: {}", args.join(" "))),
    }
}

/// `ghcr.io/elavo-io/elavo:stable` to `ghcr.io/elavo-io/elavo-nvidia-legacy:stable`.
fn nvidia_target(image: &str, edition: drivers::NvidiaEdition) -> Option<String> {
    drivers::nvidia_image_target(image, edition)
}

/// Swap the tag of `image` for `tag`, dropping any digest, and keep the
/// image name so a channel switch stays on `elavo`, `elavo-nvidia-open`,
/// or `elavo-nvidia-legacy`.
/// `ghcr.io/o/elavo:stable` to `ghcr.io/o/elavo:latest`.
fn retag(image: &str, tag: &str) -> Option<String> {
    let image = image.split('@').next()?;
    let (repo, last) = image.rsplit_once('/').unwrap_or(("", image));
    let name = last.split(':').next()?;
    if name.is_empty() {
        return None;
    }
    Some(if repo.is_empty() {
        format!("{name}:{tag}")
    } else {
        format!("{repo}/{name}:{tag}")
    })
}

fn lid_dropin(lid: Lid) -> String {
    format!(
        "# Written by System Settings.\n[Login]\nHandleLidSwitch={0}\nHandleLidSwitchExternalPower={0}\n",
        lid.value()
    )
}

fn greeter_conf(show: LoginWindow) -> String {
    format!(
        "# Written by System Settings.\n[Login]\nShow={}\n",
        show.value()
    )
}

/// A directory group name that can sit on one `AdminGroup=` line: no
/// control characters, no edge spaces, not a comment.
fn valid_group_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 256
        && name.trim() == name
        && !name.starts_with('#')
        && !name.starts_with(';')
        && !name.chars().any(char::is_control)
}

fn group_exists(name: &str) -> bool {
    let Ok(group) = std::ffi::CString::new(name) else {
        return false;
    };
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
    status == 0 && !result.is_null()
}

/// directory.conf with its `AdminGroup=` lines replaced by `groups`. The
/// new lines go where the old ones were, else under the commented example,
/// else at the end of `[Directory]`. Everything else stays as it was.
fn with_admin_groups(text: &str, groups: &[String]) -> String {
    let is_setting = |line: &str| {
        line.trim_start()
            .strip_prefix("AdminGroup")
            .is_some_and(|rest| rest.trim_start().starts_with('='))
    };
    let lines: Vec<&str> = text.lines().collect();
    let anchor = lines
        .iter()
        .position(|l| is_setting(l))
        .map(|i| (i, false))
        .or_else(|| {
            lines
                .iter()
                .position(|l| l.trim_start().starts_with("#AdminGroup"))
                .map(|i| (i, true))
        })
        .or_else(|| {
            let header = lines.iter().position(|l| l.trim() == "[Directory]")?;
            let end = lines[header + 1..]
                .iter()
                .position(|l| l.trim_start().starts_with('['))
                .map(|n| header + n)
                .unwrap_or(lines.len() - 1);
            Some((end, true))
        });
    let new_lines: Vec<String> = groups.iter().map(|g| format!("AdminGroup={g}")).collect();
    let mut out: Vec<String> = Vec::new();
    match anchor {
        None => {
            out.extend(lines.iter().map(|l| (*l).to_owned()));
            if !out.is_empty() {
                out.push(String::new());
            }
            out.push("[Directory]".to_owned());
            out.extend(new_lines);
        }
        Some((at, after)) => {
            for (i, line) in lines.iter().enumerate() {
                if i == at && !after {
                    out.extend(new_lines.iter().cloned());
                }
                if !is_setting(line) {
                    out.push((*line).to_owned());
                }
                if i == at && after {
                    out.extend(new_lines.iter().cloned());
                }
            }
        }
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

fn set_admin_groups(groups: &[String]) -> Result<(), String> {
    if let Some(missing) = groups.iter().find(|g| !group_exists(g)) {
        return Err(format!("no group named \"{missing}\" was found"));
    }
    let text = std::fs::read_to_string(DIRECTORY_CONF).unwrap_or_default();
    write_conf(DIRECTORY_CONF, &with_admin_groups(&text, groups))?;
    // Applied now when joined; otherwise the next join applies it.
    if Path::new(KEYTAB).exists() && Path::new(ELAVO_DIRECTORY).exists() {
        run({
            let mut p = tool(ELAVO_DIRECTORY);
            p.arg("apply");
            p
        })?;
    }
    Ok(())
}

fn tool(program: &str) -> Process {
    let mut p = Process::new(program);
    p.env_clear()
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("LANG", "C.UTF-8");
    p
}

fn run(mut p: Process) -> Result<(), String> {
    p.stdout(Stdio::piped());
    p.stderr(Stdio::piped());
    let child = p.spawn().map_err(|e| e.to_string())?;
    wait_forwarding(child, None)
}

fn emit_line(out: &Mutex<std::io::Stdout>, line: &str) {
    if let Ok(mut o) = out.lock() {
        let _ = writeln!(o, "{line}");
        let _ = o.flush();
    }
}

/// `--quiet` hides bootc's own redraws. `--progress-fd` is the pipe bootc
/// writes JSON progress to. Call this before the image name: that argument
/// is positional.
fn attach_progress(p: &mut Process) -> Result<(i32, i32), String> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
        return Err("could not watch the download".into());
    }
    p.arg("--quiet");
    p.arg("--progress-fd");
    p.arg(fds[1].to_string());
    Ok((fds[0], fds[1]))
}

/// Each progress event is one `ELAVO_PROGRESS ` line on stdout, so Software
/// Update can draw a bar. The write end is inherited by bootc and closed
/// here afterwards.
fn run_with_progress(mut p: Process, (read_fd, write_fd): (i32, i32)) -> Result<(), String> {
    p.stdout(Stdio::piped());
    p.stderr(Stdio::piped());
    let spawned = p.spawn();
    if spawned.is_err() {
        unsafe {
            libc::close(read_fd);
            libc::close(write_fd);
        }
    }
    let child = spawned.map_err(|e| e.to_string())?;
    unsafe { libc::close(write_fd) };
    wait_forwarding(child, Some(read_fd))
}

fn capture_lines(
    pipe: impl std::io::Read + Send + 'static,
    out: &Arc<Mutex<std::io::Stdout>>,
    captured: &Arc<Mutex<String>>,
) {
    for line in BufReader::new(pipe).lines() {
        let Ok(line) = line else { break };
        emit_line(out, &line);
        if let Ok(mut log) = captured.lock() {
            if !log.is_empty() {
                log.push('\n');
            }
            log.push_str(&line);
        }
    }
}

/// Forwards stdout and stderr a line at a time. On failure, returns the
/// tool's own output instead of only the exit status.
fn wait_forwarding(mut child: std::process::Child, progress_fd: Option<i32>) -> Result<(), String> {
    let out = Arc::new(Mutex::new(std::io::stdout()));
    let captured = Arc::new(Mutex::new(String::new()));
    let progress = progress_fd.map(|read_fd| {
        let out = Arc::clone(&out);
        thread::spawn(move || {
            let file = unsafe { std::fs::File::from_raw_fd(read_fd) };
            for line in BufReader::new(file).lines() {
                let Ok(line) = line else { break };
                if !line.is_empty() {
                    emit_line(&out, &format!("ELAVO_PROGRESS {line}"));
                }
            }
        })
    });
    let mut copies = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let out = Arc::clone(&out);
        let captured = Arc::clone(&captured);
        copies.push(thread::spawn(move || {
            capture_lines(stdout, &out, &captured);
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let out = Arc::clone(&out);
        let captured = Arc::clone(&captured);
        copies.push(thread::spawn(move || {
            capture_lines(stderr, &out, &captured);
        }));
    }
    let status = child.wait();
    if let Some(progress) = progress {
        let _ = progress.join();
    }
    for copy in copies {
        let _ = copy.join();
    }
    let status = status.map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        let text = captured
            .lock()
            .ok()
            .map(|s| s.trim().to_owned())
            .unwrap_or_default();
        if text.is_empty() {
            Err(format!("{status}"))
        } else {
            Err(text)
        }
    }
}

struct Booted {
    image: String,
    transport: String,
    signed: bool,
}

fn booted() -> Result<Booted, String> {
    let out = tool("bootc")
        .args(["status", "--json"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("bootc status failed".into());
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let image = &json["status"]["booted"]["image"]["image"];
    let name = image["image"].as_str().ok_or("no booted image")?.to_owned();
    Ok(Booted {
        image: name,
        transport: image["transport"].as_str().unwrap_or("registry").to_owned(),
        signed: !image["signature"].is_null(),
    })
}

fn bootc_upgrade(extra: &[&str]) -> Result<(), String> {
    let mut p = tool("bootc");
    p.arg("upgrade");
    p.args(extra);
    let fds = attach_progress(&mut p)?;
    run_with_progress(p, fds)
}

fn install_downloaded() -> Result<(), String> {
    run({
        let mut p = tool("bootc");
        p.args(["upgrade", "--from-downloaded"]);
        p
    })
}

fn write_atomically(dir: &str, path: &str, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

fn write_conf(path: &str, text: &str) -> Result<(), String> {
    let dir = Path::new(path)
        .parent()
        .and_then(|dir| dir.to_str())
        .ok_or("no directory")?;
    write_atomically(dir, path, text)
}

fn modprobe(args: &[&str]) -> Result<String, String> {
    let output = tool("modprobe")
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    text.push_str(&String::from_utf8_lossy(&output.stdout));
    if output.status.success() {
        Ok(text)
    } else {
        Err(text)
    }
}

fn module_missing(err: &str) -> bool {
    err.to_ascii_lowercase().contains("not found")
}

fn module_refused(err: &str) -> bool {
    let err = err.to_ascii_lowercase();
    err.contains("key was rejected")
        || err.contains("required key not available")
        || err.contains("lockdown")
        || err.contains("module verification failed")
}

/// A status word on stdout is what the wizard reads. `loaded` and `reboot`
/// are success. `missing` and `refused` leave the disk unchanged.
fn driver_status(status: &str) -> Result<(), String> {
    println!("{status}");
    match status {
        "loaded" | "reboot" => Ok(()),
        other => Err(other.to_owned()),
    }
}

fn wifi_broadcom() -> Result<(), String> {
    if let Err(err) = modprobe(&["--dry-run", "wl"]) {
        if module_missing(&err) {
            return driver_status("missing");
        }
    }
    // Persist the blacklist only after the kernel has accepted the module,
    // or when the card is busy and a restart is what finishes the job.
    // A lockdown refusal must not leave the built-in driver blacklisted.
    for module in ["brcmfmac", "brcmsmac", "b43", "b43legacy", "ssb", "bcma"] {
        let _ = modprobe(&["-r", module]);
    }
    match modprobe(&["wl"]) {
        Ok(_) => {
            write_conf(drivers::WL_LOAD_PATH, drivers::wl_load_conf())?;
            write_conf(drivers::WL_BLACKLIST_PATH, drivers::wl_blacklist_conf())?;
            driver_status("loaded")
        }
        Err(err) if module_refused(&err) || module_missing(&err) => {
            driver_status(if module_missing(&err) {
                "missing"
            } else {
                "refused"
            })
        }
        Err(_) => {
            write_conf(drivers::WL_LOAD_PATH, drivers::wl_load_conf())?;
            write_conf(drivers::WL_BLACKLIST_PATH, drivers::wl_blacklist_conf())?;
            driver_status("reboot")
        }
    }
}

fn wifi_brcmfmac() -> Result<(), String> {
    if drivers::needs_brcmfmac_option() {
        write_conf(
            drivers::BRCMFMAC_OPTION_PATH,
            drivers::brcmfmac_option_conf(),
        )?;
        if modprobe(&["-r", "brcmfmac"]).is_err() {
            return driver_status("reboot");
        }
    }
    match modprobe(&["brcmfmac"]) {
        Ok(_) => driver_status("loaded"),
        Err(_) => driver_status("reboot"),
    }
}

/// How to drop layered changes and install the update.
#[derive(Debug, PartialEq, Eq)]
enum AnywayPlan {
    /// One rpm-ostree transaction: upgrade and uninstall these packages.
    UpgradeUninstall(Vec<String>),
    /// Overrides or unsafe names: reset, then `bootc upgrade`.
    ResetThenUpgrade,
    /// Nothing is layered; just upgrade.
    Upgrade,
}

fn anyway_plan(layered: &layered::Layered) -> AnywayPlan {
    if layered.is_empty() {
        return AnywayPlan::Upgrade;
    }
    if layered.needs_reset() {
        return AnywayPlan::ResetThenUpgrade;
    }
    let all = layered.uninstall_packages();
    let pkgs: Vec<String> = all
        .iter()
        .filter(|p| layered::is_safe_package(p))
        .cloned()
        .collect();
    if !pkgs.is_empty() && pkgs.len() == all.len() {
        AnywayPlan::UpgradeUninstall(pkgs)
    } else {
        AnywayPlan::ResetThenUpgrade
    }
}

fn rpm_ostree_status_json() -> Result<String, String> {
    let out = tool("rpm-ostree")
        .args(["status", "--json"])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("rpm-ostree status failed".into());
    }
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

fn upgrade_anyway() -> Result<(), String> {
    let layered = layered::parse_booted(&rpm_ostree_status_json()?);
    match anyway_plan(&layered) {
        AnywayPlan::Upgrade => {
            if Path::new(STAGED_LOCKED).exists() {
                install_downloaded()?;
            }
            bootc_upgrade(&[])
        }
        AnywayPlan::UpgradeUninstall(pkgs) => {
            println!("Removing extra software and updating…");
            let mut p = tool("rpm-ostree");
            p.arg("upgrade");
            for pkg in pkgs {
                p.arg(format!("--uninstall={pkg}"));
            }
            run(p)
        }
        AnywayPlan::ResetThenUpgrade => {
            println!("Removing extra software…");
            run({
                let mut p = tool("rpm-ostree");
                p.arg("reset");
                p
            })?;
            if Path::new(STAGED_LOCKED).exists() {
                install_downloaded()?;
            }
            bootc_upgrade(&[])
        }
    }
}

fn execute(command: Command) -> Result<(), String> {
    match command {
        Command::Upgrade => {
            // An update held back by a download-only run would otherwise
            // stay held back.
            if Path::new(STAGED_LOCKED).exists() {
                install_downloaded()?;
            }
            bootc_upgrade(&[])
        }
        Command::UpgradeAnyway => upgrade_anyway(),
        Command::Download => bootc_upgrade(&["--download-only"]),
        Command::Install => install_downloaded(),
        Command::Auto(mode) => write_atomically(ELAVO_DIR, auto::CONFIG, &mode.render()),
        Command::AutoRun => match auto::Auto::read() {
            auto::Auto::Off => {
                println!("Automatic updates are off.");
                Ok(())
            }
            auto::Auto::Download => bootc_upgrade(&["--download-only"]),
            auto::Auto::Install => {
                if Path::new(STAGED_LOCKED).exists() {
                    install_downloaded()?;
                }
                bootc_upgrade(&[])
            }
        },
        Command::Rollback => run({
            let mut p = tool("bootc");
            p.arg("rollback");
            p
        }),
        Command::Switch(channel) => {
            let current = booted()?;
            let target = retag(&current.image, channel.tag())
                .ok_or("the booted image name is not understood")?;
            if target == current.image {
                println!("Already on {target}");
                return Ok(());
            }
            println!("Switching to {target}");
            let mut p = tool("bootc");
            p.args(["switch", "--transport", &current.transport]);
            if current.signed {
                p.arg("--enforce-container-sigpolicy");
            }
            let fds = attach_progress(&mut p)?;
            p.arg(&target);
            run_with_progress(p, fds)
        }
        Command::Lid(lid) => {
            std::fs::create_dir_all(LID_DROPIN_DIR).map_err(|e| e.to_string())?;
            let tmp = format!("{LID_DROPIN}.tmp");
            std::fs::write(&tmp, lid_dropin(lid)).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, LID_DROPIN).map_err(|e| e.to_string())?;
            // logind rereads its configuration on SIGHUP.
            run({
                let mut p = tool("systemctl");
                p.args(["kill", "--signal=HUP", "systemd-logind.service"]);
                p
            })
        }
        Command::LoginWindow(show) => {
            // The greeter reads this each time it starts.
            std::fs::create_dir_all(GREETER_DIR).map_err(|e| e.to_string())?;
            let tmp = format!("{GREETER_CONF}.tmp");
            std::fs::write(&tmp, greeter_conf(show)).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, GREETER_CONF).map_err(|e| e.to_string())
        }
        Command::WifiDriver(WifiDriver::Broadcom) => wifi_broadcom(),
        Command::WifiDriver(WifiDriver::Brcmfmac) => wifi_brcmfmac(),
        Command::SwitchNvidia => {
            if drivers::secure_boot_enabled() {
                return Err(
                    "Secure Boot is on, so the NVIDIA driver would not load. Nothing was changed."
                        .to_owned(),
                );
            }
            let edition = drivers::needed_nvidia_edition(&drivers::pci_devices())?;
            let current = booted()?;
            let target =
                nvidia_target(&current.image, edition).ok_or("the booted image is not elavo")?;
            if target == current.image {
                println!("already");
                println!("Already on {target}");
                return Ok(());
            }
            println!("reboot");
            println!("Switching to {target}");
            let mut p = tool("bootc");
            p.args(["switch", "--transport", &current.transport]);
            if current.signed {
                p.arg("--enforce-container-sigpolicy");
            }
            let fds = attach_progress(&mut p)?;
            p.arg(&target);
            run_with_progress(p, fds)
        }
        Command::DomainAdmins(groups) => set_admin_groups(&groups),
        Command::SwitchElavo => {
            let current = booted()?;
            let target = drivers::base_image_target(&current.image)
                .ok_or("the booted image is not an NVIDIA edition of elavo")?;
            if target == current.image {
                println!("already");
                println!("Already on {target}");
                return Ok(());
            }
            println!("reboot");
            println!("Switching to {target}");
            let mut p = tool("bootc");
            p.args(["switch", "--transport", &current.transport]);
            if current.signed {
                p.arg("--enforce-container-sigpolicy");
            }
            let fds = attach_progress(&mut p)?;
            p.arg(&target);
            run_with_progress(p, fds)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = match parse(&args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("elavo-system-settings-helper: {e}");
            return ExitCode::from(2);
        }
    };
    if unsafe { libc::geteuid() } != 0 {
        eprintln!("elavo-system-settings-helper: must be run through pkexec");
        return ExitCode::from(2);
    }
    if !Path::new("/run/ostree-booted").exists()
        && !matches!(
            command,
            Command::Lid(_) | Command::LoginWindow(_) | Command::Auto(_) | Command::DomainAdmins(_)
        )
    {
        eprintln!(
            "elavo-system-settings-helper: this computer is not running an image-based system"
        );
        return ExitCode::from(3);
    }
    match execute(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            // The command's own output was already forwarded. Only add a
            // line when the error is a status with no tool text.
            if !e.contains('\n') {
                eprintln!("elavo-system-settings-helper: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn accepts_the_fixed_commands() {
        assert_eq!(parse(&args(&["upgrade"])), Ok(Command::Upgrade));
        assert_eq!(
            parse(&args(&["upgrade-anyway"])),
            Ok(Command::UpgradeAnyway)
        );
        assert_eq!(parse(&args(&["download"])), Ok(Command::Download));
        assert_eq!(parse(&args(&["install"])), Ok(Command::Install));
        assert_eq!(parse(&args(&["auto-run"])), Ok(Command::AutoRun));
        assert_eq!(
            parse(&args(&["auto", "download"])),
            Ok(Command::Auto(auto::Auto::Download))
        );
        assert_eq!(parse(&args(&["rollback"])), Ok(Command::Rollback));
        assert_eq!(
            parse(&args(&["switch", "latest"])),
            Ok(Command::Switch(Channel::Latest))
        );
        assert_eq!(
            parse(&args(&["lid", "ignore"])),
            Ok(Command::Lid(Lid::Ignore))
        );
        assert_eq!(
            parse(&args(&["login-window", "name"])),
            Ok(Command::LoginWindow(LoginWindow::Name))
        );
        assert_eq!(
            parse(&args(&["wifi-driver", "broadcom"])),
            Ok(Command::WifiDriver(WifiDriver::Broadcom))
        );
        assert_eq!(
            parse(&args(&["wifi-driver", "brcmfmac"])),
            Ok(Command::WifiDriver(WifiDriver::Brcmfmac))
        );
        assert_eq!(parse(&args(&["switch-nvidia"])), Ok(Command::SwitchNvidia));
        assert_eq!(parse(&args(&["switch-elavo"])), Ok(Command::SwitchElavo));
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            &[][..],
            &["status"],
            &["check"],
            &["auto"],
            &["auto", "on"],
            &["auto", "install", "extra"],
            &["auto-run", "now"],
            &["download", "--apply"],
            &["switch"],
            &["switch", "ghcr.io/evil/image:latest"],
            &["switch", "stable", "extra"],
            &["lid", "hibernate"],
            &["lid", "suspend\nHandlePowerKey=ignore"],
            &["login-window"],
            &["login-window", "names"],
            &["login-window", "list\nShow=name"],
            &["upgrade", "--apply"],
            &["upgrade-anyway", "uxplay"],
            &["wifi-driver"],
            &["wifi-driver", "wl"],
            &["wifi-driver", "broadcom", "extra"],
            &["switch-nvidia", "elavo-nvidia"],
            &["switch-elavo", "elavo"],
            &["switch-elavo", "ghcr.io/elavo-io/elavo:latest"],
        ] {
            assert!(parse(&args(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn retags_the_image() {
        assert_eq!(
            retag("ghcr.io/elavo-io/elavo:stable", "latest").as_deref(),
            Some("ghcr.io/elavo-io/elavo:latest")
        );
        assert_eq!(
            retag("ghcr.io/elavo-io/elavo@sha256:abc", "stable").as_deref(),
            Some("ghcr.io/elavo-io/elavo:stable")
        );
        assert_eq!(
            retag("localhost:5000/elavo", "stable").as_deref(),
            Some("localhost:5000/elavo:stable")
        );
        assert_eq!(
            retag("elavo:latest", "stable").as_deref(),
            Some("elavo:stable")
        );
        // Channel switches keep the image name, including NVIDIA editions.
        // Beta is included so a later channel would stay on the same edition.
        let editions = ["elavo", "elavo-nvidia-open", "elavo-nvidia-legacy"];
        let channels = ["latest", "stable", "beta"];
        for edition in editions {
            for from in channels {
                for to in channels {
                    let source = format!("ghcr.io/elavo-io/{edition}:{from}");
                    let want = format!("ghcr.io/elavo-io/{edition}:{to}");
                    assert_eq!(
                        retag(&source, to).as_deref(),
                        Some(want.as_str()),
                        "{source} -> {to}"
                    );
                }
            }
            assert_eq!(
                retag(&format!("ghcr.io/elavo-io/{edition}@sha256:abc"), "latest").as_deref(),
                Some(format!("ghcr.io/elavo-io/{edition}:latest").as_str()),
                "{edition} digest"
            );
        }
    }

    #[test]
    fn nvidia_target_keeps_the_tag() {
        assert_eq!(
            nvidia_target(
                "ghcr.io/elavo-io/elavo:stable",
                drivers::NvidiaEdition::Legacy
            )
            .as_deref(),
            Some("ghcr.io/elavo-io/elavo-nvidia-legacy:stable")
        );
        assert_eq!(
            nvidia_target(
                "ghcr.io/elavo-io/elavo:latest",
                drivers::NvidiaEdition::Open
            )
            .as_deref(),
            Some("ghcr.io/elavo-io/elavo-nvidia-open:latest")
        );
        assert_eq!(
            nvidia_target(
                "ghcr.io/elavo-io/elavo@sha256:abc",
                drivers::NvidiaEdition::Open
            )
            .as_deref(),
            Some("ghcr.io/elavo-io/elavo-nvidia-open")
        );
        assert_eq!(
            nvidia_target(
                "ghcr.io/elavo-io/elavo-nvidia-legacy:latest",
                drivers::NvidiaEdition::Legacy
            )
            .as_deref(),
            Some("ghcr.io/elavo-io/elavo-nvidia-legacy:latest")
        );
        assert_eq!(
            nvidia_target(
                "ghcr.io/elavo-io/other:stable",
                drivers::NvidiaEdition::Open
            ),
            None
        );
    }

    #[test]
    fn a_refused_module_is_not_a_missing_one() {
        assert!(module_missing("modprobe: FATAL: Module wl not found"));
        assert!(!module_refused("modprobe: FATAL: Module wl not found"));
        assert!(module_refused(
            "Lockdown: modprobe: unsigned module loading is restricted"
        ));
        assert!(module_refused("Key was rejected by service"));
        assert!(!module_missing("Device or resource busy"));
    }

    #[test]
    fn lid_dropin_sets_both_keys() {
        let text = lid_dropin(Lid::Lock);
        assert!(text.contains("[Login]\n"));
        assert!(text.contains("HandleLidSwitch=lock\n"));
        assert!(text.contains("HandleLidSwitchExternalPower=lock\n"));
    }

    #[test]
    fn greeter_conf_sets_show() {
        let text = greeter_conf(LoginWindow::List);
        assert!(text.contains("[Login]\nShow=list\n"));
        let text = greeter_conf(LoginWindow::Name);
        assert!(text.contains("[Login]\nShow=name\n"));
    }

    #[test]
    fn anyway_plan_uses_one_uninstall_transaction() {
        let layered = layered::Layered {
            packages: vec!["uxplay".into()],
            ..Default::default()
        };
        assert_eq!(
            anyway_plan(&layered),
            AnywayPlan::UpgradeUninstall(vec!["uxplay".into()])
        );
        let layered = layered::Layered {
            packages: vec!["uxplay".into(), "foo".into()],
            ..Default::default()
        };
        assert_eq!(
            anyway_plan(&layered),
            AnywayPlan::UpgradeUninstall(vec!["uxplay".into(), "foo".into()])
        );
        let layered = layered::Layered {
            packages: vec!["uxplay".into(), "foo".into()],
            removals: vec!["firefox".into()],
            ..Default::default()
        };
        assert_eq!(anyway_plan(&layered), AnywayPlan::ResetThenUpgrade);
        assert_eq!(
            anyway_plan(&layered::Layered::default()),
            AnywayPlan::Upgrade
        );
        let bad = layered::Layered {
            packages: vec!["--oops".into()],
            ..Default::default()
        };
        assert_eq!(anyway_plan(&bad), AnywayPlan::ResetThenUpgrade);
    }

    #[test]
    fn domain_admins_arguments() {
        assert_eq!(
            parse(&args(&["domain-admins"])),
            Ok(Command::DomainAdmins(vec![]))
        );
        assert_eq!(
            parse(&args(&[
                "domain-admins",
                "Domain Admins",
                "domain admins",
                "ops"
            ])),
            Ok(Command::DomainAdmins(vec![
                "Domain Admins".to_owned(),
                "ops".to_owned()
            ]))
        );
        assert!(parse(&args(&["domain-admins", "a\nAdminGroup=x"])).is_err());
        assert!(parse(&args(&["domain-admins", " padded"])).is_err());
        assert!(parse(&args(&["domain-admins", "#x"])).is_err());
        assert!(parse(&args(&["domain-admins", ""])).is_err());
        let many: Vec<String> = (0..17).map(|n| format!("g{n}")).collect();
        let mut list = vec!["domain-admins".to_owned()];
        list.extend(many);
        assert!(parse(&list).is_err());
    }

    #[test]
    fn admin_group_lines_replace_in_place() {
        let image = "# c\n[Directory]\n# Example: AdminGroup=Domain Admins\n#AdminGroup=\n# more\nShortNames=true\n";
        let set = with_admin_groups(image, &["Domain Admins".to_owned(), "ops".to_owned()]);
        assert_eq!(
            set,
            "# c\n[Directory]\n# Example: AdminGroup=Domain Admins\n#AdminGroup=\nAdminGroup=Domain Admins\nAdminGroup=ops\n# more\nShortNames=true\n"
        );
        let again = with_admin_groups(&set, &["admins".to_owned()]);
        assert_eq!(
            again,
            "# c\n[Directory]\n# Example: AdminGroup=Domain Admins\n#AdminGroup=\nAdminGroup=admins\n# more\nShortNames=true\n"
        );
        let cleared = with_admin_groups(&again, &[]);
        assert!(!cleared.lines().any(|l| l.starts_with("AdminGroup")));
        assert!(cleared.contains("ShortNames=true"));
        assert_eq!(
            with_admin_groups("", &["g".to_owned()]),
            "[Directory]\nAdminGroup=g\n"
        );
        assert_eq!(
            with_admin_groups("[Directory]\nShortNames=true\n", &["g".to_owned()]),
            "[Directory]\nShortNames=true\nAdminGroup=g\n"
        );
    }
}
