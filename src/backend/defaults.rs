//! Default applications: which app opens a link or a kind of file.
//!
//! GIO's `AppInfo::set_as_default_for_type` writes `~/.config/mimeapps.list`,
//! which xdg-open (generic DE path), `gio open`, and xdg-desktop-portal's
//! OpenURI all read. Browsers that make themselves the default through
//! `xdg-settings` write the same file, so it is the one place a choice lives.
//! With `XDG_CURRENT_DESKTOP=elavo`, `~/.config/elavo-mimeapps.list` would be
//! looked up first and outrank them, so this module clears browser keys from
//! it rather than writing there. Image-wide defaults belong in a system
//! `mimeapps.list`, below the user's own.

use std::collections::HashSet;
use std::path::PathBuf;

use gtk::gio::prelude::*;
use gtk::{gio, glib};

use super::keyfile;

/// MIME types the web browser owns, matching `xdg-settings set
/// default-web-browser` on a generic desktop, plus `application/xhtml+xml`.
pub const BROWSER_TYPES: &[&str] = &[
    "x-scheme-handler/http",
    "x-scheme-handler/https",
    "text/html",
    "application/xhtml+xml",
    "x-scheme-handler/about",
    "x-scheme-handler/unknown",
];

/// The type used to list candidates and read the current default.
pub const BROWSER_PROBE: &str = "x-scheme-handler/https";

const DEFAULTS_GROUP: &str = "Default Applications";

/// A row that will gain a real picker once Elavo ships a handler app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ComingSoon {
    pub title: &'static str,
    pub probe: &'static str,
}

pub const COMING_SOON: &[ComingSoon] = &[
    ComingSoon {
        title: "Mail",
        probe: "x-scheme-handler/mailto",
    },
    ComingSoon {
        title: "Calendar",
        probe: "text/calendar",
    },
    ComingSoon {
        title: "Music",
        probe: "audio/mpeg",
    },
    ComingSoon {
        title: "Video",
        probe: "video/mp4",
    },
    ComingSoon {
        title: "Photos",
        probe: "image/jpeg",
    },
    ComingSoon {
        title: "Text",
        probe: "text/plain",
    },
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browser {
    pub id: String,
    pub name: String,
}

/// First component of `XDG_CURRENT_DESKTOP`, lowercased, for
/// `$desktop-mimeapps.list`. Empty or missing yields `None`.
pub fn desktop_mimeapps_name(current_desktop: &str) -> Option<String> {
    let first = current_desktop
        .split(':')
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    Some(format!("{}-mimeapps.list", first.to_ascii_lowercase()))
}

pub fn desktop_mimeapps_path() -> Option<PathBuf> {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").ok()?;
    let name = desktop_mimeapps_name(&desktop)?;
    Some(glib::user_config_dir().join(name))
}

/// When two apps share a display name, append a short hint from the id.
pub fn disambiguate_names(apps: &[(String, String)]) -> Vec<String> {
    let names: Vec<&str> = apps.iter().map(|(_, name)| name.as_str()).collect();
    apps.iter()
        .enumerate()
        .map(|(i, (id, name))| {
            let dup = names
                .iter()
                .enumerate()
                .any(|(j, other)| j != i && *other == name);
            if !dup {
                return name.clone();
            }
            let hint = name_hint(id);
            if hint.is_empty() {
                name.clone()
            } else {
                format!("{name} ({hint})")
            }
        })
        .collect()
}

fn name_hint(id: &str) -> String {
    let base = id.trim_end_matches(".desktop");
    if base.contains("flatpak") || base.starts_with("org.") || base.starts_with("com.") {
        if base.contains("appimage") {
            return "AppImage".into();
        }
        // Flatpak ids look like org.mozilla.firefox
        if base.matches('.').count() >= 2 {
            return "Flatpak".into();
        }
    }
    if base.contains("appimage") {
        return "AppImage".into();
    }
    String::new()
}

fn from_appinfo(app: &gio::AppInfo) -> Option<Browser> {
    if !app.should_show() {
        return None;
    }
    let id = app.id()?.to_string();
    if id.is_empty() {
        return None;
    }
    let name = app.display_name().to_string();
    let name = if name.is_empty() {
        app.name().to_string()
    } else {
        name
    };
    Some(Browser { id, name })
}

/// Installed apps that handle HTTPS, Flatpak exports included. Recommended
/// first, then any others `all_for_type` knows about.
pub fn list_browsers() -> Vec<Browser> {
    let mut apps: Vec<Browser> = Vec::new();
    let mut seen = HashSet::new();
    for app in gio::AppInfo::recommended_for_type(BROWSER_PROBE)
        .into_iter()
        .chain(gio::AppInfo::all_for_type(BROWSER_PROBE))
    {
        let Some(browser) = from_appinfo(&app) else {
            continue;
        };
        if !seen.insert(browser.id.clone()) {
            continue;
        }
        apps.push(browser);
    }
    let labeled = disambiguate_names(
        &apps
            .iter()
            .map(|b| (b.id.clone(), b.name.clone()))
            .collect::<Vec<_>>(),
    );
    for (browser, label) in apps.iter_mut().zip(labeled) {
        browser.name = label;
    }
    apps
}

pub fn current_browser_id() -> Option<String> {
    gio::AppInfo::default_for_type(BROWSER_PROBE, false)?
        .id()
        .map(|s| s.to_string())
}

fn app_by_id(id: &str) -> Option<gio::AppInfo> {
    gio::AppInfo::all_for_type(BROWSER_PROBE)
        .into_iter()
        .find(|a| a.id().as_deref() == Some(id))
}

/// Set the browser as default for every browser MIME type, then drop any
/// browser keys left in the desktop-specific mimeapps file so they can't
/// outrank this choice or one a browser makes later.
pub fn set_default_browser(id: &str) -> Result<(), String> {
    let app = app_by_id(id).ok_or_else(|| format!("No browser with id {id}"))?;
    for mime in BROWSER_TYPES {
        app.set_as_default_for_type(mime)
            .map_err(|e| e.to_string())?;
    }
    clear_desktop_mimeapps(BROWSER_TYPES)?;
    Ok(())
}

fn clear_desktop_mimeapps(types: &[&str]) -> Result<(), String> {
    let Some(path) = desktop_mimeapps_path() else {
        return Ok(());
    };
    let file = glib::KeyFile::new();
    if file
        .load_from_file(&path, glib::KeyFileFlags::KEEP_COMMENTS)
        .is_err()
    {
        return Ok(());
    }
    let mut changed = false;
    for mime in types {
        changed |= file.remove_key(DEFAULTS_GROUP, mime).is_ok();
    }
    if !changed {
        return Ok(());
    }
    if file.keys(DEFAULTS_GROUP).is_ok_and(|keys| keys.is_empty()) {
        let _ = file.remove_group(DEFAULTS_GROUP);
    }
    if file.groups().is_empty() {
        return std::fs::remove_file(&path).map_err(|e| e.to_string());
    }
    keyfile::write_atomic(&path, file.to_data().as_str()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_types_cover_http_stack() {
        for needed in [
            "x-scheme-handler/http",
            "x-scheme-handler/https",
            "text/html",
            "application/xhtml+xml",
        ] {
            assert!(BROWSER_TYPES.contains(&needed), "{needed}");
        }
        assert!(BROWSER_TYPES.contains(&"x-scheme-handler/about"));
        assert!(BROWSER_TYPES.contains(&"x-scheme-handler/unknown"));
        assert_eq!(BROWSER_PROBE, "x-scheme-handler/https");
    }

    #[test]
    fn desktop_mimeapps_uses_first_component() {
        assert_eq!(
            desktop_mimeapps_name("elavo").as_deref(),
            Some("elavo-mimeapps.list")
        );
        assert_eq!(
            desktop_mimeapps_name("elavo:GNOME").as_deref(),
            Some("elavo-mimeapps.list")
        );
        assert_eq!(
            desktop_mimeapps_name("Elavo").as_deref(),
            Some("elavo-mimeapps.list")
        );
        assert_eq!(desktop_mimeapps_name(""), None);
        assert_eq!(desktop_mimeapps_name("   "), None);
    }

    fn unique_ids(ids: impl IntoIterator<Item = String>) -> Vec<String> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for id in ids {
            if id.is_empty() || !seen.insert(id.clone()) {
                continue;
            }
            out.push(id);
        }
        out
    }

    #[test]
    fn unique_ids_keeps_order() {
        assert_eq!(
            unique_ids([
                "a.desktop".into(),
                "b.desktop".into(),
                "a.desktop".into(),
                "".into(),
                "c.desktop".into(),
            ]),
            vec!["a.desktop", "b.desktop", "c.desktop"]
        );
    }

    #[test]
    fn disambiguate_only_duplicates() {
        let apps = vec![
            ("org.mozilla.firefox.desktop".into(), "Firefox".into()),
            ("elavo-appimage-firefox.desktop".into(), "Firefox".into()),
            ("com.brave.Browser.desktop".into(), "Brave".into()),
        ];
        let labels = disambiguate_names(&apps);
        assert_eq!(labels[0], "Firefox (Flatpak)");
        assert_eq!(labels[1], "Firefox (AppImage)");
        assert_eq!(labels[2], "Brave");
    }

    #[test]
    fn coming_soon_rows_are_named() {
        let titles: Vec<&str> = COMING_SOON.iter().map(|c| c.title).collect();
        assert!(titles.contains(&"Mail"));
        assert!(titles.contains(&"Calendar"));
        assert!(titles.contains(&"Text"));
        assert!(!titles.iter().any(|t| t.is_empty()));
    }
}
