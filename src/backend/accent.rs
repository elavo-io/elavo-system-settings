//! Accent color. The presets are the ones in `elavo-visuals`
//! `themes/Elavo/src/gtk-4.0/_elavo-accents.scss`. The choice is written two
//! ways: `~/.config/gtk-4.0/elavo-accent.css`, which the user `gtk.css`
//! imports for the Elavo theme, and `accent-color`, for libadwaita apps.
//! An empty CSS file means the theme default, blue.

use std::path::PathBuf;

use gtk::glib;

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

    /// The stylesheet for this preset. Blue is the theme's own default, so
    /// it is written as an empty file.
    pub fn css(&self) -> String {
        if *self == DEFAULT {
            return String::new();
        }
        format!(
            ":root {{ --accent-bg-color: {}; --accent-fg-color: {}; }}\n",
            self.bg, self.fg
        )
    }
}

pub fn path() -> PathBuf {
    glib::user_config_dir()
        .join("gtk-4.0")
        .join("elavo-accent.css")
}

/// The preset whose background color appears in `css`, or the default.
pub fn parse(css: &str) -> Preset {
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
    std::fs::read_to_string(path())
        .map(|css| parse(&css))
        .unwrap_or(DEFAULT)
}

pub fn apply(preset: Preset) -> std::io::Result<()> {
    super::keyfile::write_atomic(&path(), &preset.css())?;
    gsettings::set_string("accent-color", preset.gnome);
    Ok(())
}

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
    fn default_is_empty_file() {
        assert_eq!(DEFAULT.css(), "");
        assert_eq!(parse(""), DEFAULT);
    }

    #[test]
    fn round_trip_every_preset() {
        for preset in PRESETS {
            assert_eq!(parse(&preset.css()), preset, "{}", preset.id);
        }
    }

    #[test]
    fn css_shape() {
        let red = PRESETS.iter().find(|p| p.id == "red").unwrap();
        assert_eq!(
            red.css(),
            ":root { --accent-bg-color: #d93b36; --accent-fg-color: #ffffff; }\n"
        );
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
