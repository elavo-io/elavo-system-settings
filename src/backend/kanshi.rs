//! The kanshi profile for the current set of displays. kanshi applies it on
//! login and whenever the same displays are connected again. Profiles that
//! Settings did not write are kept as they are.

use std::path::PathBuf;

use gtk::glib;

#[derive(Clone, Debug, PartialEq)]
pub struct ProfileOutput {
    /// `Make Model Serial`, or the connector name when the display does not
    /// report a make and model.
    pub criteria: String,
    pub enabled: bool,
    pub width: i32,
    pub height: i32,
    /// Millihertz; 0 leaves the refresh rate to the display.
    pub refresh: i32,
    pub x: i32,
    pub y: i32,
    pub scale: f64,
    pub transform: &'static str,
}

pub fn path() -> PathBuf {
    glib::user_config_dir().join("kanshi").join("config")
}

pub fn criteria(name: &str, make: &str, model: &str, serial: &str) -> String {
    if make.is_empty() || model.is_empty() {
        return name.to_owned();
    }
    let unknown = |s: &str| {
        if s.is_empty() {
            "Unknown".to_owned()
        } else {
            s.to_owned()
        }
    };
    format!("{} {} {}", make, model, unknown(serial))
}

/// A name that is the same whenever the same displays are connected.
pub fn profile_name(outputs: &[ProfileOutput]) -> String {
    let mut ids: Vec<String> = outputs
        .iter()
        .map(|o| {
            o.criteria
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect()
        })
        .collect();
    ids.sort();
    format!("elavo-{}", ids.join("-"))
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn render(outputs: &[ProfileOutput]) -> String {
    let mut out = format!("profile {} {{\n", profile_name(outputs));
    for o in outputs {
        if !o.enabled {
            out.push_str(&format!("    output {} disable\n", quote(&o.criteria)));
            continue;
        }
        let mode = if o.refresh > 0 {
            format!(
                "{}x{}@{:.3}Hz",
                o.width,
                o.height,
                f64::from(o.refresh) / 1000.0
            )
        } else {
            format!("{}x{}", o.width, o.height)
        };
        out.push_str(&format!(
            "    output {} enable mode {} position {},{} scale {} transform {}\n",
            quote(&o.criteria),
            mode,
            o.x,
            o.y,
            trim_float(o.scale),
            o.transform
        ));
    }
    out.push_str("}\n");
    out
}

fn trim_float(v: f64) -> String {
    let s = format!("{v:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Remove the top-level `profile <name> { ... }` block, if present.
fn remove_profile(text: &str, name: &str) -> String {
    let mut out = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        let is_ours = trimmed
            .strip_prefix("profile")
            .map(|rest| rest.trim_start())
            .is_some_and(|rest| {
                rest.strip_prefix(name)
                    .is_some_and(|after| after.trim_start().starts_with('{'))
            });
        if !is_ours {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let mut depth = line.matches('{').count() as i32 - line.matches('}').count() as i32;
        while depth > 0 {
            let Some(next) = lines.next() else {
                break;
            };
            depth += next.matches('{').count() as i32 - next.matches('}').count() as i32;
        }
        while lines.peek().is_some_and(|l| l.trim().is_empty()) {
            lines.next();
        }
    }
    out
}

/// Replace this display set's profile in `existing`.
pub fn merge(existing: &str, outputs: &[ProfileOutput]) -> String {
    let mut text = remove_profile(existing, &profile_name(outputs));
    while text.ends_with("\n\n") {
        text.pop();
    }
    if !text.trim().is_empty() {
        text.push('\n');
    } else {
        text.clear();
    }
    text.push_str(&render(outputs));
    text
}

pub fn save(outputs: &[ProfileOutput]) -> std::io::Result<()> {
    let path = path();
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    super::keyfile::write_atomic(&path, &merge(&existing, outputs))?;
    // The image only starts kanshi once a profile exists, so the first save
    // starts it.
    if !super::process::reload_kanshi() {
        super::dbus::user_unit("StartUnit", "kanshi.service");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn laptop() -> ProfileOutput {
        ProfileOutput {
            criteria: criteria("eDP-1", "BOE", "0x095F", ""),
            enabled: true,
            width: 2256,
            height: 1504,
            refresh: 59999,
            x: 0,
            y: 0,
            scale: 1.5,
            transform: "normal",
        }
    }

    fn monitor() -> ProfileOutput {
        ProfileOutput {
            criteria: criteria("HDMI-A-1", "", "", ""),
            enabled: false,
            width: 0,
            height: 0,
            refresh: 0,
            x: 0,
            y: 0,
            scale: 1.0,
            transform: "normal",
        }
    }

    #[test]
    fn criteria_rules() {
        assert_eq!(criteria("eDP-1", "BOE", "X", ""), "BOE X Unknown");
        assert_eq!(criteria("eDP-1", "BOE", "X", "123"), "BOE X 123");
        assert_eq!(criteria("HDMI-A-1", "", "", ""), "HDMI-A-1");
    }

    #[test]
    fn renders_profile() {
        let text = render(&[laptop(), monitor()]);
        assert_eq!(
            text,
            "profile elavo-BOE_0x095F_Unknown-HDMI_A_1 {\n    output \"BOE 0x095F Unknown\" enable mode 2256x1504@59.999Hz position 0,0 scale 1.5 transform normal\n    output \"HDMI-A-1\" disable\n}\n"
        );
    }

    #[test]
    fn name_is_order_independent() {
        assert_eq!(
            profile_name(&[laptop(), monitor()]),
            profile_name(&[monitor(), laptop()])
        );
    }

    #[test]
    fn merge_replaces_only_ours() {
        let mine = render(&[laptop()]);
        let other = "profile docked {\n    output eDP-1 disable\n}\n";
        let existing = format!("{other}\n{mine}");
        let mut changed = laptop();
        changed.scale = 2.0;
        let merged = merge(&existing, &[changed]);
        assert!(merged.starts_with(other));
        assert!(merged.contains("scale 2 "));
        assert!(!merged.contains("scale 1.5"));
        assert_eq!(merged.matches("profile elavo-").count(), 1);
    }

    #[test]
    fn merge_into_empty() {
        assert_eq!(merge("", &[laptop()]), render(&[laptop()]));
    }
}
