//! The privileged half of System Settings, run as root through pkexec.
//!
//! It accepts a fixed set of commands and nothing else: no paths, no image
//! names, no free text. polkit decides who may run each one (see
//! `io.elavo.SystemSettings.policy`); this program only checks the
//! arguments and does the one thing asked.
//!
//! ```text
//! elavo-system-settings-helper check
//! elavo-system-settings-helper upgrade
//! elavo-system-settings-helper switch stable|latest
//! elavo-system-settings-helper rollback
//! elavo-system-settings-helper lid suspend|lock|ignore|poweroff
//! ```

use std::io::{BufRead, BufReader, Write};
use std::os::fd::FromRawFd;
use std::path::Path;
use std::process::{Command as Process, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;

const LID_DROPIN_DIR: &str = "/etc/systemd/logind.conf.d";
const LID_DROPIN: &str = "/etc/systemd/logind.conf.d/50-elavo-lid.conf";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Check,
    Upgrade,
    Switch(Channel),
    Rollback,
    Lid(Lid),
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
        ["check"] => Ok(Command::Check),
        ["upgrade"] => Ok(Command::Upgrade),
        ["rollback"] => Ok(Command::Rollback),
        ["switch", "stable"] => Ok(Command::Switch(Channel::Stable)),
        ["switch", "latest"] => Ok(Command::Switch(Channel::Latest)),
        ["lid", "suspend"] => Ok(Command::Lid(Lid::Suspend)),
        ["lid", "lock"] => Ok(Command::Lid(Lid::Lock)),
        ["lid", "ignore"] => Ok(Command::Lid(Lid::Ignore)),
        ["lid", "poweroff"] => Ok(Command::Lid(Lid::Poweroff)),
        _ => Err(format!("unsupported command: {}", args.join(" "))),
    }
}

/// Swap the tag of `image` for `tag`, dropping any digest:
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

fn tool(program: &str) -> Process {
    let mut p = Process::new(program);
    p.env_clear()
        .env("PATH", "/usr/sbin:/usr/bin")
        .env("LANG", "C.UTF-8");
    p
}

fn run(mut p: Process) -> Result<(), String> {
    let status = p.status().map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{status}"))
    }
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
    let mut child = spawned.map_err(|e| e.to_string())?;
    unsafe { libc::close(write_fd) };

    let out = Arc::new(Mutex::new(std::io::stdout()));
    let progress = {
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
    };
    let mut copies = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let out = Arc::clone(&out);
        copies.push(thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                emit_line(&out, &line);
            }
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let out = Arc::clone(&out);
        copies.push(thread::spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                emit_line(&out, &line);
            }
        }));
    }
    let status = child.wait();
    let _ = progress.join();
    for copy in copies {
        let _ = copy.join();
    }
    let status = status.map_err(|e| e.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{status}"))
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

fn execute(command: Command) -> Result<(), String> {
    match command {
        Command::Check => run({
            let mut p = tool("bootc");
            p.args(["upgrade", "--check"]);
            p
        }),
        Command::Upgrade => {
            let mut p = tool("bootc");
            p.arg("upgrade");
            let fds = attach_progress(&mut p)?;
            run_with_progress(p, fds)
        }
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
    if !Path::new("/run/ostree-booted").exists() && !matches!(command, Command::Lid(_)) {
        eprintln!(
            "elavo-system-settings-helper: this computer is not running an image-based system"
        );
        return ExitCode::from(3);
    }
    match execute(command) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("elavo-system-settings-helper: {e}");
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
        assert_eq!(parse(&args(&["check"])), Ok(Command::Check));
        assert_eq!(parse(&args(&["upgrade"])), Ok(Command::Upgrade));
        assert_eq!(parse(&args(&["rollback"])), Ok(Command::Rollback));
        assert_eq!(
            parse(&args(&["switch", "latest"])),
            Ok(Command::Switch(Channel::Latest))
        );
        assert_eq!(
            parse(&args(&["lid", "ignore"])),
            Ok(Command::Lid(Lid::Ignore))
        );
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            &[][..],
            &["status"],
            &["check", "--now"],
            &["switch"],
            &["switch", "ghcr.io/evil/image:latest"],
            &["switch", "stable", "extra"],
            &["lid", "hibernate"],
            &["lid", "suspend\nHandlePowerKey=ignore"],
            &["upgrade", "--apply"],
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
    }

    #[test]
    fn lid_dropin_sets_both_keys() {
        let text = lid_dropin(Lid::Lock);
        assert!(text.contains("[Login]\n"));
        assert!(text.contains("HandleLidSwitch=lock\n"));
        assert!(text.contains("HandleLidSwitchExternalPower=lock\n"));
    }
}
