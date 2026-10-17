//! Desktop pictures. Dynamic wallpapers are directories of a light and a
//! dark image. A picture the person chose is a single file. The choice is
//! `~/.config/elavo/wallpaper.conf`, and `elavo-wallpaper.service` paints it
//! with swaybg, always scaled to cover the screen.

use std::path::{Path, PathBuf};

use super::dbus;
use super::keyfile;

pub const UNIT: &str = "elavo-wallpaper.service";
pub const FILE: &str = "wallpaper.conf";

const ROLES: &[&str] = &["jpg", "jpeg", "png", "webp"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Dynamic,
    Custom,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    Automatic,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub kind: Kind,
    pub id: String,
    pub custom: String,
    pub variant: Variant,
}

impl Default for Choice {
    fn default() -> Self {
        Self {
            kind: Kind::Dynamic,
            id: String::new(),
            custom: String::new(),
            variant: Variant::Automatic,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dynamic {
    pub id: String,
    pub light: Option<PathBuf>,
    pub dark: Option<PathBuf>,
    pub light_thumb: Option<PathBuf>,
    pub dark_thumb: Option<PathBuf>,
}

pub fn display_name(id: &str) -> String {
    let id = id.strip_prefix("dynamic-").unwrap_or(id);
    id.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn roots() -> Vec<PathBuf> {
    vec![
        glib_user_data().join("backgrounds/elavo"),
        PathBuf::from("/usr/share/backgrounds/elavo"),
    ]
}

fn glib_user_data() -> PathBuf {
    gtk::glib::user_data_dir()
}

/// Directories are searched in order. The first root that has an id wins,
/// so a copy under the home directory replaces the image's copy.
///
/// A wallpaper is a directory that holds a light picture, a dark picture, or
/// both. The file can be `light.jpg` or `blue-light.jpg`. A `thumb` file is
/// the picker preview for both versions. Directories named `source`, and
/// files with `source` in the name, are the artwork masters and are skipped.
/// A directory that is not itself a wallpaper is searched, so
/// `dynamic-wallpapers/dynamic-blue/` is found under the wallpaper root.
pub fn scan(roots: &[PathBuf]) -> Vec<Dynamic> {
    let mut found: Vec<Dynamic> = Vec::new();
    for root in roots {
        visit(root, &mut found);
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found
}

fn visit(dir: &Path, found: &mut Vec<Dynamic>) {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name.starts_with('.') || name == "source" {
        return;
    }
    let light = role(dir, "light");
    let dark = role(dir, "dark");
    if light.is_some() || dark.is_some() {
        if !found.iter().any(|item| item.id == name) {
            let shared = role(dir, "thumb");
            found.push(Dynamic {
                id: name,
                light,
                dark,
                light_thumb: role(dir, "light-thumb").or_else(|| shared.clone()),
                dark_thumb: role(dir, "dark-thumb").or(shared),
            });
        }
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.path().is_dir() {
            visit(&entry.path(), found);
        }
    }
}

pub fn catalog() -> Vec<Dynamic> {
    scan(&roots())
}

fn role(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = ROLES.iter().find_map(|ext| {
        let path = dir.join(format!("{name}.{ext}"));
        path.is_file().then_some(path)
    });
    if exact.is_some() {
        return exact;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return None;
    };
    let mut matches: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file() && stem_is_role(path, name))
        .collect();
    matches.sort();
    matches.into_iter().next()
}

fn stem_is_role(path: &Path, name: &str) -> bool {
    let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
        return false;
    };
    if stem.contains("source") {
        return false;
    }
    if name == "thumb" && (stem.ends_with("-light-thumb") || stem.ends_with("-dark-thumb")) {
        return false;
    }
    let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
        return false;
    };
    if !ROLES.contains(&ext) {
        return false;
    }
    stem == name
        || stem
            .strip_suffix(&format!("-{name}"))
            .is_some_and(|prefix| !prefix.is_empty())
}

pub fn wants_dark(variant: Variant, scheme_dark: bool) -> bool {
    match variant {
        Variant::Light => false,
        Variant::Dark => true,
        Variant::Automatic => scheme_dark,
    }
}

/// The file swaybg should show. A missing light or dark file uses the other.
pub fn picture(wallpaper: &Dynamic, variant: Variant, scheme_dark: bool) -> Option<PathBuf> {
    let dark = wants_dark(variant, scheme_dark);
    if dark {
        wallpaper.dark.clone().or_else(|| wallpaper.light.clone())
    } else {
        wallpaper.light.clone().or_else(|| wallpaper.dark.clone())
    }
}

/// The picker image. A thumbnail is preferred. Without one, the picture itself.
pub fn thumb(wallpaper: &Dynamic, variant: Variant, scheme_dark: bool) -> Option<PathBuf> {
    let dark = wants_dark(variant, scheme_dark);
    if dark {
        wallpaper
            .dark_thumb
            .clone()
            .or_else(|| wallpaper.light_thumb.clone())
    } else {
        wallpaper
            .light_thumb
            .clone()
            .or_else(|| wallpaper.dark_thumb.clone())
    }
    .or_else(|| picture(wallpaper, variant, scheme_dark))
}

pub fn parse(text: &str) -> Choice {
    let mut choice = Choice::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "Kind" => {
                choice.kind = if value == "custom" {
                    Kind::Custom
                } else {
                    Kind::Dynamic
                }
            }
            "Id" => choice.id = value.to_owned(),
            "Custom" => choice.custom = value.to_owned(),
            "Variant" => {
                choice.variant = match value {
                    "light" => Variant::Light,
                    "dark" => Variant::Dark,
                    _ => Variant::Automatic,
                }
            }
            _ => {}
        }
    }
    choice
}

pub fn format(choice: &Choice) -> String {
    let kind = match choice.kind {
        Kind::Dynamic => "dynamic",
        Kind::Custom => "custom",
    };
    let variant = match choice.variant {
        Variant::Automatic => "automatic",
        Variant::Light => "light",
        Variant::Dark => "dark",
    };
    format!(
        "[Wallpaper]\nKind={kind}\nId={}\nCustom={}\nVariant={variant}\n",
        choice.id, choice.custom
    )
}

pub fn load() -> Choice {
    std::fs::read_to_string(keyfile::path(FILE))
        .map(|text| parse(&text))
        .unwrap_or_default()
}

/// An empty dynamic id means the first installed wallpaper.
pub fn resolve_id(choice: &Choice, installed: &[Dynamic]) -> Choice {
    let mut choice = choice.clone();
    if choice.kind == Kind::Dynamic
        && (choice.id.is_empty() || !installed.iter().any(|item| item.id == choice.id))
    {
        choice.id = installed
            .first()
            .map(|item| item.id.clone())
            .unwrap_or_default();
    }
    choice
}

pub fn save(choice: &Choice) -> std::io::Result<()> {
    keyfile::write_atomic(&keyfile::path(FILE), &format(choice))?;
    dbus::user_unit("RestartUnit", UNIT);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("elavo-wallpaper-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, b"x").unwrap();
    }

    #[test]
    fn name_from_id() {
        assert_eq!(display_name("wavy-blue"), "Wavy Blue");
        assert_eq!(display_name("dynamic-blue"), "Blue");
    }

    #[test]
    fn nested_dynamic_wallpapers_use_one_thumbnail() {
        let root = temp_root("nested");
        touch(&root.join("dynamic-wallpapers/dynamic-blue/blue-light.jpg"));
        touch(&root.join("dynamic-wallpapers/dynamic-blue/blue-dark.jpg"));
        touch(&root.join("dynamic-wallpapers/dynamic-blue/blue-thumb.jpg"));
        touch(&root.join("source/blue-source.png"));
        let found = scan(std::slice::from_ref(&root));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "dynamic-blue");
        assert!(picture(&found[0], Variant::Automatic, true)
            .unwrap()
            .ends_with("blue-dark.jpg"));
        assert!(thumb(&found[0], Variant::Light, false)
            .unwrap()
            .ends_with("blue-thumb.jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn dark_falls_back_when_only_light_exists() {
        let root = temp_root("light-only");
        touch(&root.join("wavy-blue/light.jpg"));
        let found = scan(std::slice::from_ref(&root));
        assert_eq!(found.len(), 1);
        let pic = picture(&found[0], Variant::Dark, true).unwrap();
        assert!(pic.ends_with("light.jpg"));
        assert!(thumb(&found[0], Variant::Light, false)
            .unwrap()
            .ends_with("light.jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn automatic_follows_the_scheme_and_thumbs_are_separate() {
        let root = temp_root("pair");
        touch(&root.join("wavy-blue/light.jpg"));
        touch(&root.join("wavy-blue/dark.jpg"));
        touch(&root.join("wavy-blue/light-thumb.jpg"));
        touch(&root.join("wavy-blue/dark-thumb.jpg"));
        let found = scan(std::slice::from_ref(&root));
        let paper = &found[0];
        assert!(picture(paper, Variant::Automatic, false)
            .unwrap()
            .ends_with("light.jpg"));
        assert!(picture(paper, Variant::Automatic, true)
            .unwrap()
            .ends_with("dark.jpg"));
        assert!(thumb(paper, Variant::Automatic, true)
            .unwrap()
            .ends_with("dark-thumb.jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_shared_thumb_covers_both() {
        let root = temp_root("shared-thumb");
        touch(&root.join("wavy-blue/light.png"));
        touch(&root.join("wavy-blue/thumb.jpg"));
        let found = scan(std::slice::from_ref(&root));
        assert!(thumb(&found[0], Variant::Dark, true)
            .unwrap()
            .ends_with("thumb.jpg"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_home_copy_replaces_the_system_copy() {
        let system = temp_root("system");
        let home = temp_root("home");
        touch(&system.join("wavy-blue/light.jpg"));
        touch(&home.join("wavy-blue/light.jpg"));
        let found = scan(&[home.clone(), system.clone()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].light.as_ref().unwrap().starts_with(&home));
        let _ = std::fs::remove_dir_all(&system);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn source_files_are_not_wallpapers() {
        let root = temp_root("source");
        touch(&root.join("wavy-blue/source.png"));
        assert!(scan(std::slice::from_ref(&root)).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn choice_round_trip_keeps_a_path_with_spaces() {
        let text = format(&Choice {
            kind: Kind::Custom,
            id: "wavy-blue".to_owned(),
            custom: "/home/kevin/My Pictures/shore.jpg".to_owned(),
            variant: Variant::Dark,
        });
        let choice = parse(&text);
        assert_eq!(choice.kind, Kind::Custom);
        assert_eq!(choice.custom, "/home/kevin/My Pictures/shore.jpg");
        assert_eq!(choice.variant, Variant::Dark);
        assert_eq!(choice.id, "wavy-blue");
    }

    #[test]
    fn a_missing_id_becomes_the_first_wallpaper() {
        let root = temp_root("order");
        touch(&root.join("amber/light.jpg"));
        touch(&root.join("wavy-blue/light.jpg"));
        let found = scan(std::slice::from_ref(&root));
        let choice = resolve_id(&Choice::default(), &found);
        assert_eq!(choice.id, "amber");
        let _ = std::fs::remove_dir_all(&root);
    }
}
