//! Every pane, in sidebar order. A pane is built the first time it is
//! shown and then kept, so going back to it is instant and its D-Bus
//! subscriptions keep it current.
//!
//! Panes with a `parent` are not in the sidebar: they open from their
//! parent's page (General opens About, Software Update, and the rest), and
//! the parent stays highlighted while they are shown.

mod about;
pub mod appearance;
mod bluetooth;
mod datetime;
mod displays;
mod dock;
mod general;
mod keyboard;
mod lens;
mod lockscreen;
mod mouse;
mod network;
mod placeholders;
mod power;
mod sound;
mod storage;
mod updates;
mod users;

use crate::i18n::tr;

pub struct Pane {
    /// The `--pane` name and the stack page name.
    pub id: &'static str,
    pub title: fn() -> String,
    pub icon: &'static str,
    /// The sidebar tile color, a class in `style.css`.
    pub color: &'static str,
    /// Extra words the sidebar search matches.
    pub keywords: &'static [&'static str],
    /// Panes with the same group sit together in the sidebar.
    pub group: u8,
    /// The hub page this pane opens from, instead of the sidebar.
    pub parent: Option<&'static str>,
    pub build: fn() -> gtk::Widget,
}

pub fn all() -> Vec<Pane> {
    vec![
        // Connections
        Pane {
            id: "wifi",
            title: || tr("Wi-Fi"),
            icon: "network-wireless-symbolic",
            color: "blue",
            keywords: &[
                "wifi",
                "wireless",
                "internet",
                "password",
                "join",
                "known networks",
            ],
            group: 0,
            parent: None,
            build: network::build_wifi,
        },
        Pane {
            id: "bluetooth",
            title: || tr("Bluetooth"),
            icon: "bluetooth-active-symbolic",
            color: "blue",
            keywords: &["pair", "headphones", "keyboard", "mouse", "devices"],
            group: 0,
            parent: None,
            build: bluetooth::build,
        },
        Pane {
            id: "network",
            title: || tr("Network"),
            icon: "network-workgroup-symbolic",
            color: "blue",
            keywords: &["ethernet", "wired", "ip", "router", "internet"],
            group: 0,
            parent: None,
            build: network::build_network,
        },
        Pane {
            id: "sound",
            title: || tr("Sound"),
            icon: "audio-volume-high-symbolic",
            color: "pink",
            keywords: &[
                "volume",
                "speaker",
                "microphone",
                "input",
                "output",
                "mute",
                "audio",
            ],
            group: 1,
            parent: None,
            build: sound::build,
        },
        // The system as a whole
        Pane {
            id: "general",
            title: || tr("General"),
            icon: "preferences-system-symbolic",
            color: "graphite",
            keywords: &[],
            group: 2,
            parent: None,
            build: general::build,
        },
        Pane {
            id: "appearance",
            title: || tr("Appearance"),
            icon: "preferences-desktop-appearance-symbolic",
            color: "dark",
            keywords: &["dark", "light", "accent", "color", "colour", "theme"],
            group: 2,
            parent: None,
            build: appearance::build,
        },
        Pane {
            id: "lens",
            title: || tr("Lens"),
            icon: "system-search-symbolic",
            color: "graphite",
            keywords: &["search", "launcher", "applications", "folders"],
            group: 2,
            parent: None,
            build: lens::build,
        },
        // The desktop
        Pane {
            id: "dock",
            title: || tr("Desktop & Dock"),
            icon: "user-desktop-symbolic",
            color: "dark",
            keywords: &[
                "dock",
                "magnification",
                "autohide",
                "hide",
                "position",
                "recent",
            ],
            group: 3,
            parent: None,
            build: dock::build,
        },
        Pane {
            id: "displays",
            title: || tr("Displays"),
            icon: "video-display-symbolic",
            color: "blue",
            keywords: &[
                "monitor",
                "resolution",
                "scale",
                "hidpi",
                "rotate",
                "refresh",
                "arrange",
            ],
            group: 3,
            parent: None,
            build: displays::build,
        },
        Pane {
            id: "wallpaper",
            title: || tr("Wallpaper"),
            icon: "preferences-desktop-wallpaper-symbolic",
            color: "teal",
            keywords: &["background", "desktop picture"],
            group: 3,
            parent: None,
            build: placeholders::wallpaper,
        },
        Pane {
            id: "screensaver",
            title: || tr("Screen Saver"),
            icon: "preferences-desktop-screensaver-symbolic",
            color: "indigo",
            keywords: &["idle"],
            group: 3,
            parent: None,
            build: placeholders::screensaver,
        },
        Pane {
            id: "power",
            title: power::title,
            icon: "battery-good-symbolic",
            color: "green",
            keywords: &[
                "battery",
                "energy",
                "sleep",
                "lid",
                "power mode",
                "performance",
            ],
            group: 3,
            parent: None,
            build: power::build,
        },
        // People and security
        Pane {
            id: "lockscreen",
            title: || tr("Lock Screen"),
            icon: "system-lock-screen-symbolic",
            color: "dark",
            keywords: &["lock", "idle", "inactive", "display off", "password"],
            group: 4,
            parent: None,
            build: lockscreen::build,
        },
        Pane {
            id: "users",
            title: || tr("Users & Groups"),
            icon: "system-users-symbolic",
            color: "blue",
            keywords: &["account", "password", "picture", "admin", "login"],
            group: 4,
            parent: None,
            build: users::build,
        },
        // Input
        Pane {
            id: "keyboard",
            title: || tr("Keyboard"),
            icon: "input-keyboard-symbolic",
            color: "graphite",
            keywords: &["repeat", "layout", "shortcuts", "keys", "input source"],
            group: 5,
            parent: None,
            build: keyboard::build,
        },
        Pane {
            id: "mouse",
            title: || tr("Mouse"),
            icon: "input-mouse-symbolic",
            color: "graphite",
            keywords: &["scroll", "natural", "pointer", "tracking", "speed"],
            group: 5,
            parent: None,
            build: mouse::build_mouse,
        },
        Pane {
            id: "trackpad",
            title: || tr("Trackpad"),
            icon: "input-touchpad-symbolic",
            color: "graphite",
            keywords: &["touchpad", "scroll", "natural", "tap", "click", "tracking"],
            group: 5,
            parent: None,
            build: mouse::build_trackpad,
        },
        // Opened from General
        Pane {
            id: "about",
            title: || tr("About"),
            icon: "computer-symbolic",
            color: "graphite",
            keywords: &[
                "version",
                "computer name",
                "hostname",
                "memory",
                "processor",
                "model",
            ],
            group: 0,
            parent: Some("general"),
            build: about::build,
        },
        Pane {
            id: "updates",
            title: || tr("Software Update"),
            icon: "software-update-available-symbolic",
            color: "graphite",
            keywords: &[
                "upgrade", "image", "channel", "stable", "latest", "rollback", "restart",
            ],
            group: 0,
            parent: Some("general"),
            build: updates::build,
        },
        Pane {
            id: "storage",
            title: || tr("Storage"),
            icon: "drive-harddisk-symbolic",
            color: "graphite",
            keywords: &["disk", "space", "free", "available"],
            group: 0,
            parent: Some("general"),
            build: storage::build,
        },
        Pane {
            id: "datetime",
            title: || tr("Date & Time"),
            icon: "preferences-system-time-symbolic",
            color: "blue",
            keywords: &["clock", "timezone", "time zone", "24-hour", "network time"],
            group: 1,
            parent: Some("general"),
            build: datetime::build,
        },
    ]
}

/// The panes a hub page opens, in its order.
pub fn children(parent: &str) -> Vec<Pane> {
    all()
        .into_iter()
        .filter(|p| p.parent == Some(parent))
        .collect()
}

pub const DEFAULT: &str = "general";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_parents_exist() {
        let panes = all();
        let ids: Vec<&str> = panes.iter().map(|p| p.id).collect();
        for (i, id) in ids.iter().enumerate() {
            assert!(!ids[i + 1..].contains(id), "{id} twice");
        }
        for p in &panes {
            if let Some(parent) = p.parent {
                let parent = panes.iter().find(|q| q.id == parent).expect(parent);
                assert!(parent.parent.is_none(), "{} nests too deep", p.id);
            }
        }
        assert!(ids.contains(&DEFAULT));
    }
}
