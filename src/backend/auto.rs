//! Whether updates download and install on their own. Settings reads the
//! file; the helper writes it and the background update service follows it.
//! The helper compiles this file too, so it uses nothing from GTK.

pub const CONFIG: &str = "/etc/elavo/updates.conf";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auto {
    /// Nothing happens until someone presses a button.
    Off,
    /// Updates download, and wait for Install Now.
    Download,
    /// Updates download and finish installing at the next restart.
    Install,
}

impl Auto {
    pub fn arg(self) -> &'static str {
        match self {
            Auto::Off => "off",
            Auto::Download => "download",
            Auto::Install => "install",
        }
    }

    #[allow(dead_code)] // The helper's.
    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg {
            "off" => Some(Auto::Off),
            "download" => Some(Auto::Download),
            "install" => Some(Auto::Install),
            _ => None,
        }
    }

    pub fn from_switches(download: bool, install: bool) -> Self {
        match (download, install) {
            (false, _) => Auto::Off,
            (true, false) => Auto::Download,
            (true, true) => Auto::Install,
        }
    }

    pub fn downloads(self) -> bool {
        self != Auto::Off
    }

    pub fn installs(self) -> bool {
        self == Auto::Install
    }

    /// A missing key is on: a new install keeps itself up to date.
    pub fn parse(text: &str) -> Self {
        let mut download = true;
        let mut install = true;
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let on = matches!(value.trim(), "true" | "yes" | "1");
            match key.trim() {
                "Download" => download = on,
                "Install" => install = on,
                _ => {}
            }
        }
        Self::from_switches(download, install)
    }

    #[allow(dead_code)] // The helper's.
    pub fn render(self) -> String {
        format!(
            "# Written by System Settings.\n[Updates]\nDownload={}\nInstall={}\n",
            self.downloads(),
            self.installs()
        )
    }

    pub fn read() -> Self {
        std::fs::read_to_string(CONFIG)
            .map(|text| Self::parse(&text))
            .unwrap_or(Auto::Install)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_or_keys_mean_on() {
        assert_eq!(Auto::parse(""), Auto::Install);
        assert_eq!(Auto::parse("[Updates]\nInstall=false\n"), Auto::Download);
    }

    #[test]
    fn install_needs_download() {
        assert_eq!(
            Auto::parse("[Updates]\nDownload=false\nInstall=true\n"),
            Auto::Off
        );
        assert_eq!(Auto::from_switches(false, true), Auto::Off);
    }

    #[test]
    fn render_round_trips() {
        for mode in [Auto::Off, Auto::Download, Auto::Install] {
            assert_eq!(Auto::parse(&mode.render()), mode);
            assert_eq!(Auto::from_arg(mode.arg()), Some(mode));
        }
        assert_eq!(Auto::from_arg("on"), None);
    }
}
