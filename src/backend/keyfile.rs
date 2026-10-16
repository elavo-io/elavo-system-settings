//! Key files under `$XDG_CONFIG_HOME/elavo/`, the folder the shell watches.
//! Writes are atomic, so a reader never sees half a file.

use std::path::PathBuf;

use gtk::glib;

pub fn config_dir() -> PathBuf {
    glib::user_config_dir().join("elavo")
}

pub fn path(name: &str) -> PathBuf {
    config_dir().join(name)
}

pub fn load(name: &str) -> glib::KeyFile {
    let file = glib::KeyFile::new();
    let _ = file.load_from_file(path(name), glib::KeyFileFlags::KEEP_COMMENTS);
    file
}

pub fn save(name: &str, file: &glib::KeyFile) -> std::io::Result<()> {
    write_atomic(&path(name), file.to_data().as_str())
}

pub fn write_atomic(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    glib::file_set_contents(path, contents.as_bytes()).map_err(std::io::Error::other)
}

pub fn integer(file: &glib::KeyFile, group: &str, key: &str) -> Option<i64> {
    file.int64(group, key).ok()
}

pub fn string(file: &glib::KeyFile, group: &str, key: &str) -> Option<String> {
    file.string(group, key).ok().map(|s| s.to_string())
}

/// `idle.conf`, read by the image's `elavo-idle` service. Seconds; 0 is never.
pub mod idle {
    pub const FILE: &str = "idle.conf";
    pub const GROUP: &str = "Idle";
    pub const DEFAULT_LOCK: i64 = 300;
    pub const DEFAULT_DISPLAY_OFF: i64 = 600;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Idle {
        pub lock_after: i64,
        pub display_off_after: i64,
    }

    impl Default for Idle {
        fn default() -> Self {
            Self {
                lock_after: DEFAULT_LOCK,
                display_off_after: DEFAULT_DISPLAY_OFF,
            }
        }
    }

    pub fn parse(file: &gtk::glib::KeyFile) -> Idle {
        let d = Idle::default();
        Idle {
            lock_after: super::integer(file, GROUP, "LockAfter")
                .filter(|v| *v >= 0)
                .unwrap_or(d.lock_after),
            display_off_after: super::integer(file, GROUP, "DisplayOffAfter")
                .filter(|v| *v >= 0)
                .unwrap_or(d.display_off_after),
        }
    }

    pub fn fill(file: &gtk::glib::KeyFile, idle: Idle) {
        file.set_int64(GROUP, "LockAfter", idle.lock_after);
        file.set_int64(GROUP, "DisplayOffAfter", idle.display_off_after);
    }

    pub fn load() -> Idle {
        parse(&super::load(FILE))
    }

    pub fn save(idle: Idle) -> std::io::Result<()> {
        let file = super::load(FILE);
        fill(&file, idle);
        super::save(FILE, &file)
    }
}

/// `lens.conf`: what the Applications view lists.
pub mod lens {
    pub const FILE: &str = "lens.conf";
    pub const GROUP: &str = "Lens";
    pub const KEY: &str = "Applications";

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Applications {
        /// The shared Applications folder and this account's.
        Folders,
        /// The folders plus every system desktop file.
        All,
    }

    impl Applications {
        pub fn as_str(self) -> &'static str {
            match self {
                Self::Folders => "folders",
                Self::All => "all",
            }
        }

        pub fn parse(s: &str) -> Self {
            if s.trim().eq_ignore_ascii_case("all") {
                Self::All
            } else {
                Self::Folders
            }
        }
    }

    pub fn load() -> Applications {
        super::string(&super::load(FILE), GROUP, KEY)
            .map(|s| Applications::parse(&s))
            .unwrap_or(Applications::Folders)
    }

    pub fn save(value: Applications) -> std::io::Result<()> {
        let file = super::load(FILE);
        file.set_string(GROUP, KEY, value.as_str());
        super::save(FILE, &file)
    }
}

/// This application's own state: the last pane and the window size.
pub mod state {
    pub const FILE: &str = "system-settings.conf";
    pub const GROUP: &str = "Window";

    pub fn last_pane() -> Option<String> {
        super::string(&super::load(FILE), GROUP, "LastPane")
    }

    pub fn size() -> Option<(i32, i32)> {
        let file = super::load(FILE);
        let w = super::integer(&file, GROUP, "Width")?;
        let h = super::integer(&file, GROUP, "Height")?;
        Some((w.clamp(640, 4000) as i32, h.clamp(480, 4000) as i32))
    }

    pub fn save(pane: &str, width: i32, height: i32) {
        let file = super::load(FILE);
        file.set_string(GROUP, "LastPane", pane);
        file.set_int64(GROUP, "Width", i64::from(width));
        file.set_int64(GROUP, "Height", i64::from(height));
        let _ = super::save(FILE, &file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_defaults_and_round_trip() {
        let file = glib::KeyFile::new();
        assert_eq!(idle::parse(&file), idle::Idle::default());
        idle::fill(
            &file,
            idle::Idle {
                lock_after: 120,
                display_off_after: 0,
            },
        );
        let text = file.to_data();
        assert!(text.contains("[Idle]"));
        assert!(text.contains("LockAfter=120"));
        assert!(text.contains("DisplayOffAfter=0"));
        let back = glib::KeyFile::new();
        back.load_from_data(&text, glib::KeyFileFlags::NONE)
            .unwrap();
        assert_eq!(idle::parse(&back).lock_after, 120);
        assert_eq!(idle::parse(&back).display_off_after, 0);
    }

    #[test]
    fn idle_rejects_negative() {
        let file = glib::KeyFile::new();
        file.set_int64(idle::GROUP, "LockAfter", -5);
        assert_eq!(idle::parse(&file).lock_after, idle::DEFAULT_LOCK);
    }

    #[test]
    fn lens_values() {
        use lens::Applications;
        assert_eq!(Applications::parse("all"), Applications::All);
        assert_eq!(Applications::parse("ALL "), Applications::All);
        assert_eq!(Applications::parse("folders"), Applications::Folders);
        assert_eq!(Applications::parse("anything"), Applications::Folders);
        assert_eq!(Applications::All.as_str(), "all");
    }
}
