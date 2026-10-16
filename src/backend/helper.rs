//! Running `elavo-system-settings-helper` through pkexec. polkit asks for an
//! administrator password; the helper only accepts its fixed commands.
//! Output is passed back a line at a time so long updates show progress.

use gtk::prelude::*;
use gtk::{gio, glib};

pub const HELPER: &str = match option_env!("HELPER_PATH") {
    Some(path) => path,
    None => "/usr/libexec/elavo-system-settings-helper",
};

/// Run the helper with `args`. `line` gets each line of output. Returns
/// whether it exited successfully; a cancelled password prompt is a failure.
pub async fn run(args: &[&str], line: impl Fn(&str)) -> Result<(), String> {
    let mut argv: Vec<&std::ffi::OsStr> = vec!["pkexec".as_ref(), HELPER.as_ref()];
    argv.extend(args.iter().map(|a| std::ffi::OsStr::new(*a)));
    let process = gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_MERGE,
    )
    .map_err(|e| e.to_string())?;
    let mut last = String::new();
    if let Some(stdout) = process.stdout_pipe() {
        let reader = gio::DataInputStream::new(&stdout);
        while let Ok(Some(text)) = reader.read_line_utf8_future(glib::Priority::DEFAULT).await {
            let text = text.trim_end();
            if !text.is_empty() {
                last = text.to_owned();
            }
            line(text);
        }
    }
    process.wait_future().await.map_err(|e| e.to_string())?;
    match process.exit_status() {
        0 => Ok(()),
        code => Err(failure(code, &last)),
    }
}

/// What to tell the person when the helper did not succeed. `last` is the
/// last line pkexec or the helper printed.
fn failure(code: i32, last: &str) -> String {
    use crate::i18n::tr;
    // pkexec falls back to a terminal prompt when no polkit agent is
    // running, and a window has no terminal.
    if last.contains("authentication agent") {
        return tr(
            "No password prompt is available. The polkit agent is not running in this session.",
        );
    }
    let message = match code {
        126 | 127 => tr("The administrator password was not accepted."),
        _ => format!("{} ({code})", tr("The change failed")),
    };
    if last.is_empty() {
        message
    } else {
        format!("{message}\n\n{last}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explains_a_missing_agent() {
        let text = failure(
            127,
            "Error creating textual authentication agent: Error opening current controlling terminal",
        );
        assert!(text.contains("polkit agent"));
    }

    #[test]
    fn keeps_the_last_line() {
        let text = failure(1, "bootc: error: no network");
        assert!(text.contains("(1)"));
        assert!(text.ends_with("bootc: error: no network"));
    }
}
