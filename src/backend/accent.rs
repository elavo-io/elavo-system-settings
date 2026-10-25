//! Accent color. The presets are the ones in `elavo-visuals`
//! `themes/Elavo/src/gtk-4.0/_elavo-accents.scss`. Blue is the Elavo theme.
//!
//! Apps that load `gtk-theme` read the color from `Elavo-user`. `Elavo-mirror`
//! is a symlink to that directory, so selecting the other name reloads the
//! sheet without a dark step. libadwaita apps, including Files, force their
//! theme to `Adwaita-empty` and never load that sheet. They import
//! `elavo-accent.css` instead. A high-contrast change makes GTK re-read that
//! import. The light colors stay as they are. A dark theme later gets its
//! own pair.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::{ObjectExt, SettingsExt};

use super::gsettings;
use crate::i18n::tr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub id: &'static str,
    pub bg: &'static str,
    pub fg: &'static str,
    /// The `org.gnome.desktop.interface accent-color` value.
    pub gnome: &'static str,
}

pub const PRESETS: [Preset; 8] = [
    Preset {
        id: "blue",
        bg: "#0860f2",
        fg: "#ffffff",
        gnome: "blue",
    },
    Preset {
        id: "purple",
        bg: "#8a4fd8",
        fg: "#ffffff",
        gnome: "purple",
    },
    Preset {
        id: "pink",
        bg: "#d6337f",
        fg: "#ffffff",
        gnome: "pink",
    },
    Preset {
        id: "red",
        bg: "#d93b36",
        fg: "#ffffff",
        gnome: "red",
    },
    Preset {
        id: "orange",
        bg: "#c95a06",
        fg: "#ffffff",
        gnome: "orange",
    },
    Preset {
        id: "yellow",
        bg: "#f2c230",
        fg: "#1c1c1c",
        gnome: "yellow",
    },
    Preset {
        id: "green",
        bg: "#258a3b",
        fg: "#ffffff",
        gnome: "green",
    },
    Preset {
        id: "graphite",
        bg: "#6e6e73",
        fg: "#ffffff",
        gnome: "slate",
    },
];

pub const DEFAULT: Preset = PRESETS[0];

const DEFAULT_THEME: &str = "Elavo";
const USER_THEME: &str = "Elavo-user";
/// The same directory as [`USER_THEME`], under a second name.
const MIRROR_THEME: &str = "Elavo-mirror";
const A11Y_INTERFACE: &str = "org.gnome.desktop.a11y.interface";
/// GTK's contrast values. Off is 1, high contrast is 2.
const CONTRAST_OFF: i32 = 1;
const CONTRAST_ON: i32 = 2;
/// After this process has observed the temporary contrast, other apps need
/// this long to re-read the accent import.
const STEP_SETTLE: Duration = Duration::from_millis(200);
const STEP_POLL: Duration = Duration::from_millis(40);
const STEP_GIVE_UP: Duration = Duration::from_millis(500);

static ACCENT_EPOCH: AtomicU64 = AtomicU64::new(0);
static CONTRAST_EPOCH: AtomicU64 = AtomicU64::new(0);
/// Set while a refresh has high contrast away from the person's choice.
static CONTRAST_AWAY: AtomicBool = AtomicBool::new(false);
static HOME_IS_CONTRAST: AtomicBool = AtomicBool::new(false);

/// The accent file for blue. A named color is inserted above the fallback.
const ACCENT_POINTER: &str = "\
:root,
toolbarview {
  --accent-bg-color: var(--elavo-user-accent, var(--elavo-accent-blue));
  --accent-fg-color: var(--elavo-user-accent-fg, var(--elavo-accent-blue-fg));
}
";

impl Preset {
    pub fn label(&self) -> String {
        match self.id {
            "blue" => tr("Blue"),
            "purple" => tr("Purple"),
            "pink" => tr("Pink"),
            "red" => tr("Red"),
            "orange" => tr("Orange"),
            "yellow" => tr("Yellow"),
            "green" => tr("Green"),
            _ => tr("Graphite"),
        }
    }
}

pub fn path() -> PathBuf {
    glib::user_config_dir()
        .join("gtk-4.0")
        .join("elavo-accent.css")
}

/// The preset whose background color appears in an older accent file, or
/// the default.
#[cfg(test)]
fn parse(css: &str) -> Preset {
    let css = css.to_ascii_lowercase();
    let Some(at) = css.find("--accent-bg-color") else {
        return DEFAULT;
    };
    let value = css[at..]
        .split_once(':')
        .map(|(_, rest)| {
            rest.split([';', '}'])
                .next()
                .unwrap_or("")
                .trim()
                .to_owned()
        })
        .unwrap_or_default();
    PRESETS
        .iter()
        .copied()
        .find(|p| p.bg == value)
        .unwrap_or(DEFAULT)
}

pub fn current() -> Preset {
    gsettings::get_string("accent-color")
        .as_deref()
        .and_then(|name| PRESETS.iter().copied().find(|preset| preset.gnome == name))
        .unwrap_or(DEFAULT)
}

/// What every app imports. Blue leaves the variable unset, so the fallback
/// is the theme default. Any other color names the preset here, because
/// libadwaita apps do not load the theme that also names it.
fn accent_css(preset: Preset) -> String {
    if preset == DEFAULT {
        return ACCENT_POINTER.to_owned();
    }
    format!(
        ":root,\ntoolbarview {{\n  --elavo-user-accent: var(--elavo-accent-{id});\n  --elavo-user-accent-fg: var(--elavo-accent-{id}-fg);\n  --accent-bg-color: var(--elavo-user-accent, var(--elavo-accent-blue));\n  --accent-fg-color: var(--elavo-user-accent-fg, var(--elavo-accent-blue-fg));\n}}\n",
        id = preset.id,
    )
}

pub fn apply(preset: Preset) -> std::io::Result<()> {
    let epoch = ACCENT_EPOCH.fetch_add(1, Ordering::Relaxed) + 1;
    super::keyfile::write_atomic(&path(), &accent_css(preset))?;
    gsettings::set_string("accent-color", preset.gnome);
    if preset == DEFAULT {
        set_named_theme(DEFAULT_THEME);
    } else {
        write_user_theme(preset)?;
        ensure_mirror()?;
        let next = match gsettings::get_string("gtk-theme").as_deref() {
            Some(USER_THEME) => MIRROR_THEME,
            _ => USER_THEME,
        };
        set_named_theme(next);
    }
    refresh_libadwaita(epoch);
    Ok(())
}

/// Turn high contrast away and back. libadwaita apps re-read the accent
/// import when contrast changes, and their light colors stay put.
fn refresh_libadwaita(epoch: u64) {
    let contrast_epoch = CONTRAST_EPOCH.load(Ordering::Relaxed);
    if !CONTRAST_AWAY.load(Ordering::Relaxed) {
        HOME_IS_CONTRAST.store(high_contrast().unwrap_or(false), Ordering::Relaxed);
        CONTRAST_AWAY.store(true, Ordering::Relaxed);
    }
    step_contrast(epoch, contrast_epoch, 0);
}

fn step_contrast(epoch: u64, contrast_epoch: u64, index: usize) {
    if ACCENT_EPOCH.load(Ordering::Relaxed) != epoch
        || CONTRAST_EPOCH.load(Ordering::Relaxed) != contrast_epoch
        || index >= 2
    {
        return;
    }
    let home = HOME_IS_CONTRAST.load(Ordering::Relaxed);
    let away = index == 0;
    let on = if away { !home } else { home };
    let expected = if on { CONTRAST_ON } else { CONTRAST_OFF };
    set_high_contrast(on);
    let started = Instant::now();
    glib::timeout_add_local(STEP_POLL, move || {
        if ACCENT_EPOCH.load(Ordering::Relaxed) != epoch
            || CONTRAST_EPOCH.load(Ordering::Relaxed) != contrast_epoch
        {
            return glib::ControlFlow::Break;
        }
        let observed = interface_contrast() == Some(expected);
        if !observed && started.elapsed() < STEP_GIVE_UP {
            return glib::ControlFlow::Continue;
        }
        glib::timeout_add_local_once(STEP_SETTLE, move || {
            if index == 0 {
                step_contrast(epoch, contrast_epoch, 1);
            } else {
                CONTRAST_AWAY.store(false, Ordering::Relaxed);
            }
        });
        glib::ControlFlow::Break
    });
}

fn high_contrast() -> Option<bool> {
    gsettings::settings(A11Y_INTERFACE, "high-contrast")
        .map(|settings| settings.boolean("high-contrast"))
}

fn set_high_contrast(on: bool) {
    if let Some(settings) = gsettings::settings(A11Y_INTERFACE, "high-contrast") {
        let _ = settings.set_boolean("high-contrast", on);
    }
}

fn interface_contrast() -> Option<i32> {
    gtk::Settings::default()?
        .property_value("gtk-interface-contrast")
        .get()
        .ok()
}

/// `gtk.css` and `gtk-dark.css` sit beside each other. GTK loads the dark
/// sheet instead of the light one when the color scheme is dark.
fn dark_import(import_url: &str) -> String {
    match import_url.rsplit_once("gtk.css") {
        Some((head, tail)) => format!("{head}gtk-dark.css{tail}"),
        None => import_url.to_string(),
    }
}

/// GTK4 sheet: import Elavo, then publish the accent the user stylesheet
/// points at. The stylesheet outranks this file, so the color cannot be
/// assigned to `--accent-bg-color` here.
fn user_theme_gtk4(preset: Preset, import_url: &str) -> String {
    format!(
        "@import url(\"{import_url}\");\n\n:root {{\n  --elavo-user-accent: var(--elavo-accent-{id});\n  --elavo-user-accent-fg: var(--elavo-accent-{id}-fg);\n}}\n",
        id = preset.id,
    )
}

/// GTK3 has no variables. Import Elavo and redefine the named accent colors.
fn user_theme_gtk3(preset: Preset, import_url: &str) -> String {
    format!(
        "@import url(\"{import_url}\");\n\n@define-color theme_selected_bg_color {bg};\n@define-color theme_selected_fg_color {fg};\n@define-color theme_unfocused_selected_bg_color {bg};\n@define-color theme_unfocused_selected_fg_color {fg};\n@define-color accent_color {bg};\n",
        bg = preset.bg,
        fg = preset.fg,
    )
}

fn themes_dir() -> PathBuf {
    glib::user_data_dir().join("themes")
}

fn write_user_theme(preset: Preset) -> std::io::Result<()> {
    let root = themes_dir().join(USER_THEME);
    let gtk4 = elavo_import("4.0").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the Elavo theme was not found",
        )
    })?;
    let gtk3 = elavo_import("3.0").ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the Elavo theme was not found",
        )
    })?;
    super::keyfile::write_atomic(
        &root.join("gtk-4.0/gtk.css"),
        &user_theme_gtk4(preset, &gtk4),
    )?;
    super::keyfile::write_atomic(
        &root.join("gtk-4.0/gtk-dark.css"),
        &user_theme_gtk4(preset, &dark_import(&gtk4)),
    )?;
    super::keyfile::write_atomic(
        &root.join("gtk-3.0/gtk.css"),
        &user_theme_gtk3(preset, &gtk3),
    )?;
    super::keyfile::write_atomic(
        &root.join("gtk-3.0/gtk-dark.css"),
        &user_theme_gtk3(preset, &dark_import(&gtk3)),
    )?;
    super::keyfile::write_atomic(&root.join("index.theme"), INDEX_THEME)
}

/// `Elavo-mirror` and `Elavo-user` are one directory. GTK follows the link
/// when it opens `gtk.css`, and a new theme name makes it read the file again.
fn ensure_mirror() -> std::io::Result<()> {
    let mirror = themes_dir().join(MIRROR_THEME);
    match std::fs::symlink_metadata(&mirror) {
        Ok(meta) => {
            if meta.file_type().is_symlink()
                && std::fs::read_link(&mirror).ok().as_deref() == Some(Path::new(USER_THEME))
            {
                return Ok(());
            }
            if meta.is_dir() {
                std::fs::remove_dir_all(&mirror)?;
            } else {
                std::fs::remove_file(&mirror)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    std::os::unix::fs::symlink(USER_THEME, mirror)
}

/// The Elavo sheet this user theme imports. The copy under the user's data
/// directory is the one Flatpak can read, and it is what GTK loads for the
/// name Elavo when it is present.
fn elavo_import(version: &str) -> Option<String> {
    let rel = format!("gtk-{version}/gtk.css");
    if themes_dir().join("Elavo").join(&rel).is_file() {
        // gtk.css lives in gtk-4.0/, so the sibling theme is two levels up.
        return Some(format!("../../Elavo/gtk-{version}/gtk.css"));
    }
    let system = PathBuf::from(format!("/usr/share/themes/Elavo/{rel}"));
    if system.is_file() {
        return glib::filename_to_uri(&system, None)
            .ok()
            .map(|uri| uri.to_string());
    }
    None
}

fn set_named_theme(name: &str) {
    gsettings::set_string("gtk-theme", name);
    for version in ["gtk-3.0", "gtk-4.0"] {
        let path = glib::user_config_dir().join(version).join("settings.ini");
        let _ = write_theme_name(&path, name);
    }
}

fn write_theme_name(path: &std::path::Path, name: &str) -> std::io::Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();
    let mut found = false;
    for line in &mut lines {
        let rest = line.trim().strip_prefix("gtk-theme-name");
        if rest.is_some_and(|rest| rest.trim_start().starts_with('=')) {
            *line = format!("gtk-theme-name={name}");
            found = true;
        }
    }
    if !found {
        if !lines.iter().any(|line| line.trim() == "[Settings]") {
            lines.insert(0, "[Settings]".to_owned());
        }
        lines.push(format!("gtk-theme-name={name}"));
    }
    let mut text = lines.join("\n");
    if !text.ends_with('\n') {
        text.push('\n');
    }
    super::keyfile::write_atomic(path, &text)
}

const INDEX_THEME: &str = "\
[Desktop Entry]
Type=X-GNOME-Metatheme
Name=Elavo
Comment=Elavo with the accent chosen in Settings
Encoding=UTF-8

[X-GNOME-Metatheme]
GtkTheme=Elavo-user
MetacityTheme=Elavo
IconTheme=Elavo
CursorTheme=Adwaita
ButtonLayout=close,minimize,maximize:
";

/// The color scheme the person picked. Light is the Elavo default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scheme {
    Light,
    Dark,
}

pub fn scheme() -> Scheme {
    match gsettings::get_string("color-scheme").as_deref() {
        Some("prefer-dark") => Scheme::Dark,
        _ => Scheme::Light,
    }
}

pub fn set_scheme(scheme: Scheme) {
    let value = match scheme {
        Scheme::Light => "prefer-light",
        Scheme::Dark => "prefer-dark",
    };
    gsettings::set_string("color-scheme", value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pointer_does_not_pin_a_color() {
        let red = PRESETS.iter().find(|preset| preset.id == "red").unwrap();
        let named = accent_css(*red);
        assert!(named.contains("--elavo-user-accent: var(--elavo-accent-red);"));
        assert!(named.contains("toolbarview"));
        assert!(!named.contains('#'));
        assert!(!accent_css(DEFAULT).contains("--elavo-accent-red"));
        assert_eq!(parse(""), DEFAULT);
        assert_eq!(parse(&named), DEFAULT);
    }

    #[test]
    fn old_hex_file_still_parses() {
        for preset in PRESETS {
            let css = format!(":root {{ --accent-bg-color: {}; }}", preset.bg);
            assert_eq!(parse(&css), preset, "{}", preset.id);
        }
    }

    #[test]
    fn user_theme_overrides_only_the_accent() {
        let pink = PRESETS.iter().find(|p| p.id == "pink").unwrap();
        let gtk4 = user_theme_gtk4(*pink, "../../Elavo/gtk-4.0/gtk.css");
        assert!(gtk4.starts_with("@import url(\"../../Elavo/gtk-4.0/gtk.css\");\n"));
        assert!(gtk4.contains("--elavo-user-accent: var(--elavo-accent-pink);"));
        assert!(gtk4.contains("--elavo-user-accent-fg: var(--elavo-accent-pink-fg);"));
        assert!(!gtk4.contains("--accent-bg-color"));
        assert!(!gtk4.contains(pink.bg));
        let gtk4_dark = user_theme_gtk4(*pink, &dark_import("../../Elavo/gtk-4.0/gtk.css"));
        assert!(gtk4_dark.starts_with("@import url(\"../../Elavo/gtk-4.0/gtk-dark.css\");\n"));
        assert!(gtk4_dark.contains("--elavo-user-accent: var(--elavo-accent-pink);"));
        let gtk3 = user_theme_gtk3(*pink, "../Elavo/gtk-3.0/gtk.css");
        assert!(gtk3.contains("@define-color theme_selected_bg_color #d6337f;"));
        assert!(gtk3.contains("@define-color accent_color #d6337f;"));
        assert!(gtk3.contains("@define-color theme_selected_fg_color #ffffff;"));
    }

    #[test]
    fn graphite_maps_to_slate() {
        let graphite = PRESETS.iter().find(|p| p.id == "graphite").unwrap();
        assert_eq!(graphite.gnome, "slate");
    }

    #[test]
    fn unknown_color_falls_back() {
        assert_eq!(parse(":root { --accent-bg-color: #123456; }"), DEFAULT);
    }
}
