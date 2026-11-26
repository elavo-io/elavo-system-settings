//! Startup Disk: rEFInd default boot target and menu visibility.
//!
//! Settings writes are carried out by `elavo-system-settings-helper` into
//! `/var/lib/elavo/refind.conf`, then `/usr/libexec/elavo-refind sync` updates
//! `EFI/refind/elavo-entries.conf` and `/run/elavo/refind-state`.
//! The helper compiles this file too, so it uses nothing from GTK.

use std::fs;
use std::path::Path;
use std::process::Command;

pub const CONF: &str = "/var/lib/elavo/refind.conf";
pub const FALLBACK_CONF: &str = "/etc/elavo/refind.conf";
pub const STATE: &str = "/run/elavo/refind-state";
pub const ELAVO_REFIND: &str = "/usr/libexec/elavo-refind";
pub const REFIND_EFI: &str = "/usr/share/rEFInd/refind/refind_x64.efi";
pub const BLS_DIR: &str = "/boot/loader/entries";
pub const EFI_DIR: &str = "/sys/firmware/efi";

/// Maximum length of an `other:<id>` identifier.
const MAX_OTHER_ID: usize = 64;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DefaultTarget {
    Current,
    Previous,
    Recovery,
    Other(String),
}

impl DefaultTarget {
    pub fn as_arg(&self) -> String {
        match self {
            DefaultTarget::Current => "current".to_owned(),
            DefaultTarget::Previous => "previous".to_owned(),
            DefaultTarget::Recovery => "recovery".to_owned(),
            DefaultTarget::Other(id) => format!("other:{id}"),
        }
    }

    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg.trim() {
            "current" => Some(DefaultTarget::Current),
            "previous" => Some(DefaultTarget::Previous),
            "recovery" => Some(DefaultTarget::Recovery),
            rest => {
                let id = rest.strip_prefix("other:")?;
                valid_other_id(id).then(|| DefaultTarget::Other(id.to_owned()))
            }
        }
    }
}

pub fn valid_other_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_OTHER_ID
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuVisibility {
    Auto,
    Hidden,
    Always,
}

impl MenuVisibility {
    pub fn as_arg(self) -> &'static str {
        match self {
            MenuVisibility::Auto => "auto",
            MenuVisibility::Hidden => "hidden",
            MenuVisibility::Always => "always",
        }
    }

    pub fn from_arg(arg: &str) -> Option<Self> {
        match arg.trim() {
            "auto" => Some(MenuVisibility::Auto),
            "hidden" => Some(MenuVisibility::Hidden),
            "always" => Some(MenuVisibility::Always),
            _ => None,
        }
    }

    /// Whether the boot menu shows without holding a key.
    pub fn is_always_shown(self, has_other_oses: bool) -> bool {
        match self {
            MenuVisibility::Always => true,
            MenuVisibility::Hidden => false,
            MenuVisibility::Auto => has_other_oses,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub default: DefaultTarget,
    pub menu: MenuVisibility,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default: DefaultTarget::Current,
            menu: MenuVisibility::Auto,
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Self {
        let mut cfg = Self::default();
        let mut in_startup = true;
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            if line.starts_with('[') && line.ends_with(']') {
                let sec = line[1..line.len() - 1].trim();
                in_startup = sec == "Startup";
                continue;
            }
            if !in_startup {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "Default" => {
                    if let Some(target) = DefaultTarget::from_arg(value) {
                        cfg.default = target;
                    }
                }
                "Menu" => {
                    if let Some(menu) = MenuVisibility::from_arg(value) {
                        cfg.menu = menu;
                    }
                }
                _ => {}
            }
        }
        cfg
    }

    #[allow(dead_code)]
    pub fn format(&self) -> String {
        format!(
            "# Written by System Settings.\n[Startup]\nDefault={}\nMenu={}\n",
            self.default.as_arg(),
            self.menu.as_arg(),
        )
    }

    pub fn read() -> Self {
        fs::read_to_string(CONF)
            .or_else(|_| fs::read_to_string(FALLBACK_CONF))
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlsEntry {
    pub title: String,
    pub version: u64,
}

/// Parse one BLS `.conf` file from `/boot/loader/entries`.
pub fn parse_bls(text: &str) -> Option<BlsEntry> {
    let mut title = None;
    let mut version = 0u64;
    for raw in text.lines() {
        let line = raw.trim();
        if let Some(rest) = line.strip_prefix("title") {
            let val = rest.trim_start();
            if !val.is_empty() && rest.starts_with(char::is_whitespace) {
                title = Some(strip_ostree_suffix(val).to_owned());
            }
        } else if let Some(rest) = line.strip_prefix("version") {
            if rest.starts_with(char::is_whitespace) {
                if let Ok(v) = rest.trim().parse::<u64>() {
                    version = v;
                }
            }
        }
    }
    Some(BlsEntry {
        title: title.unwrap_or_else(|| "Elavo".to_owned()),
        version,
    })
}

fn strip_ostree_suffix(title: &str) -> &str {
    let t = title.trim();
    if let Some((prefix, rest)) = t.rsplit_once(" (ostree:") {
        if let Some(digits) = rest.strip_suffix(')') {
            if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                return prefix.trim_end();
            }
        }
    }
    t
}

/// Split `"Elavo 0.3.4.5"` into `("Elavo", "0.3.4.5")`.
pub fn split_name_version(title: &str) -> (String, String) {
    let t = title.trim();
    if let Some((name, ver)) = t.rsplit_once(' ') {
        if ver.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return (name.trim().to_owned(), ver.trim().to_owned());
        }
    }
    (t.to_owned(), String::new())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OtherOs {
    pub id: DefaultTarget,
    pub label: String,
    pub disk: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RefindState {
    pub installed: Option<bool>,
    pub secure_boot: bool,
    pub boot_entry: bool,
    pub esp_label: String,
    pub others: Vec<OtherOs>,
}

impl RefindState {
    pub fn parse(text: &str) -> Self {
        let mut state = Self::default();
        for raw in text.lines() {
            let line = raw.trim();
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k.trim() {
                "installed" => state.installed = Some(v.trim() == "1"),
                "secure_boot" => state.secure_boot = v.trim() == "1",
                "boot_entry" => state.boot_entry = v.trim() == "1",
                "esp_label" => state.esp_label = v.trim().to_owned(),
                "other" => {
                    let parts: Vec<&str> = v.split('\t').collect();
                    if parts.len() >= 2 {
                        if let Some(id) = DefaultTarget::from_arg(parts[0]) {
                            let label = parts[1].trim().to_owned();
                            let disk = parts
                                .get(2)
                                .map(|s| s.trim())
                                .filter(|s| !s.is_empty())
                                .unwrap_or("EFI")
                                .to_owned();
                            state.others.push(OtherOs { id, label, disk });
                        }
                    }
                }
                _ => {}
            }
        }
        state
    }

    pub fn read() -> Option<Self> {
        fs::read_to_string(STATE)
            .ok()
            .map(|text| Self::parse(&text))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Available,
    NotUefi,
    SecureBootEnabled,
    RefindNotInstalled,
}

pub fn detect_availability_with(
    efi_present: bool,
    secure_boot: bool,
    refind_in_image: bool,
    state: Option<&RefindState>,
) -> Availability {
    if !efi_present {
        return Availability::NotUefi;
    }
    if secure_boot || state.is_some_and(|s| s.secure_boot) {
        return Availability::SecureBootEnabled;
    }
    if !refind_in_image {
        return Availability::RefindNotInstalled;
    }
    if let Some(s) = state {
        if s.installed == Some(false) && !s.boot_entry {
            return Availability::RefindNotInstalled;
        }
    }
    Availability::Available
}

pub fn detect_availability(secure_boot: bool, state: Option<&RefindState>) -> Availability {
    let efi_present = Path::new(EFI_DIR).is_dir();
    let refind_in_image = Path::new(ELAVO_REFIND).is_file() && Path::new(REFIND_EFI).is_file();
    detect_availability_with(efi_present, secure_boot, refind_in_image, state)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupOption {
    pub target: DefaultTarget,
    /// Primary tile heading, e.g. `"Elavo"`, `"Recovery (GRUB)"`, `"Windows"`.
    pub name: String,
    /// Secondary tile subtitle, e.g. `"0.3.4.5"`, `"0.3.4.4 (Previous)"`.
    pub detail: String,
    /// Full name used in the footer summary.
    pub summary_name: String,
    /// Disk/volume label shown in `You have selected X on the disk "Y".`.
    pub disk_name: String,
}

/// Read `/boot/loader/entries/*.conf` sorted newest first (highest BLS `version`).
pub fn read_bls_entries() -> Vec<BlsEntry> {
    let Ok(rd) = fs::read_dir(BLS_DIR) else {
        return Vec::new();
    };
    let mut entries = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("conf") {
            continue;
        }
        if let Ok(text) = fs::read_to_string(&path) {
            if let Some(parsed) = parse_bls(&text) {
                entries.push(parsed);
            }
        }
    }
    entries.sort_by(|a, b| b.version.cmp(&a.version));
    entries
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiskLabels {
    pub system: String,
    pub esp: String,
}

pub fn parse_lsblk_labels(text: &str) -> DiskLabels {
    let mut system = None;
    let mut esp = None;
    for line in text.lines() {
        let label = extract_quoted_field(line, "LABEL").unwrap_or_default();
        let partlabel = extract_quoted_field(line, "PARTLABEL").unwrap_or_default();
        let mount = extract_quoted_field(line, "MOUNTPOINT").unwrap_or_default();
        if matches!(mount.as_str(), "/" | "/sysroot") {
            let candidate = non_generic_label(&label)
                .or_else(|| non_generic_label(&partlabel))
                .unwrap_or("Elavo");
            system = Some(candidate.to_owned());
        } else if mount == "/boot/efi" {
            let candidate = first_non_empty(&[&label, &partlabel]).unwrap_or("EFI");
            esp = Some(candidate.to_owned());
        }
    }
    DiskLabels {
        system: system.unwrap_or_else(|| "Elavo".to_owned()),
        esp: esp.unwrap_or_else(|| "EFI".to_owned()),
    }
}

fn non_generic_label(label: &str) -> Option<&str> {
    let t = label.trim();
    if t.is_empty() || matches!(t.to_ascii_lowercase().as_str(), "root" | "fedora" | "sysroot") {
        None
    } else {
        Some(t)
    }
}

fn first_non_empty<'a>(items: &[&'a str]) -> Option<&'a str> {
    items.iter().map(|s| s.trim()).find(|s| !s.is_empty())
}

fn extract_quoted_field(line: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let idx = line.find(&needle)?;
    let rest = &line[idx + needle.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}

pub fn read_disk_labels(state: Option<&RefindState>) -> DiskLabels {
    let mut labels = Command::new("lsblk")
        .args(["-n", "-P", "-o", "LABEL,PARTLABEL,MOUNTPOINT"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| parse_lsblk_labels(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_else(|| DiskLabels {
            system: "Elavo".to_owned(),
            esp: "EFI".to_owned(),
        });
    if let Some(s) = state {
        if !s.esp_label.trim().is_empty() {
            labels.esp = s.esp_label.trim().to_owned();
        }
    }
    labels
}

pub fn build_options(
    bls: &[BlsEntry],
    state: Option<&RefindState>,
    labels: &DiskLabels,
    previous_suffix: &str,
    recovery_label: &str,
    recovery_detail: &str,
    other_detail: &str,
) -> Vec<StartupOption> {
    let mut out = Vec::new();
    if let Some(curr) = bls.first() {
        let (name, ver) = split_name_version(&curr.title);
        out.push(StartupOption {
            target: DefaultTarget::Current,
            name,
            detail: ver,
            summary_name: curr.title.clone(),
            disk_name: labels.system.clone(),
        });
    } else {
        out.push(StartupOption {
            target: DefaultTarget::Current,
            name: "Elavo".to_owned(),
            detail: String::new(),
            summary_name: "Elavo".to_owned(),
            disk_name: labels.system.clone(),
        });
    }

    if let Some(prev) = bls.get(1) {
        let (name, ver) = split_name_version(&prev.title);
        let detail = if ver.is_empty() {
            previous_suffix.to_owned()
        } else {
            format!("{ver} ({previous_suffix})")
        };
        out.push(StartupOption {
            target: DefaultTarget::Previous,
            name,
            detail,
            summary_name: format!("{} ({previous_suffix})", prev.title),
            disk_name: labels.system.clone(),
        });
    }

    if let Some(s) = state {
        for other in &s.others {
            out.push(StartupOption {
                target: other.id.clone(),
                name: other.label.clone(),
                detail: other_detail.to_owned(),
                summary_name: other.label.clone(),
                disk_name: other.disk.clone(),
            });
        }
    }

    out.push(StartupOption {
        target: DefaultTarget::Recovery,
        name: recovery_label.to_owned(),
        detail: recovery_detail.to_owned(),
        summary_name: recovery_label.to_owned(),
        disk_name: labels.esp.clone(),
    });

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_target_round_trip_and_validation() {
        assert_eq!(
            DefaultTarget::from_arg("current"),
            Some(DefaultTarget::Current)
        );
        assert_eq!(
            DefaultTarget::from_arg("previous"),
            Some(DefaultTarget::Previous)
        );
        assert_eq!(
            DefaultTarget::from_arg("recovery"),
            Some(DefaultTarget::Recovery)
        );
        assert_eq!(
            DefaultTarget::from_arg("other:Microsoft"),
            Some(DefaultTarget::Other("Microsoft".into()))
        );
        assert_eq!(
            DefaultTarget::from_arg("other:pop-os_24.04"),
            Some(DefaultTarget::Other("pop-os_24.04".into()))
        );

        assert_eq!(DefaultTarget::from_arg("other:"), None);
        assert_eq!(DefaultTarget::from_arg("other:../etc"), None);
        assert_eq!(DefaultTarget::from_arg("other:win dow"), None);
        assert_eq!(DefaultTarget::from_arg("other:a\nb"), None);
        assert_eq!(DefaultTarget::from_arg("anything"), None);
        assert_eq!(
            DefaultTarget::Other("Microsoft".into()).as_arg(),
            "other:Microsoft"
        );
    }

    #[test]
    fn menu_visibility_round_trip() {
        assert_eq!(MenuVisibility::from_arg("auto"), Some(MenuVisibility::Auto));
        assert_eq!(
            MenuVisibility::from_arg("hidden"),
            Some(MenuVisibility::Hidden)
        );
        assert_eq!(
            MenuVisibility::from_arg("always"),
            Some(MenuVisibility::Always)
        );
        assert_eq!(MenuVisibility::from_arg("never"), None);

        assert!(!MenuVisibility::Auto.is_always_shown(false));
        assert!(MenuVisibility::Auto.is_always_shown(true));
        assert!(!MenuVisibility::Hidden.is_always_shown(true));
        assert!(MenuVisibility::Always.is_always_shown(false));
    }

    #[test]
    fn config_parse_and_format() {
        let cfg = Config::parse(
            "# comment\n[Startup]\nDefault=previous\nMenu=always\n[Other]\nDefault=recovery\n",
        );
        assert_eq!(cfg.default, DefaultTarget::Previous);
        assert_eq!(cfg.menu, MenuVisibility::Always);
        assert_eq!(
            cfg.format(),
            "# Written by System Settings.\n[Startup]\nDefault=previous\nMenu=always\n"
        );

        let fallback = Config::parse("Default=bad/value\nMenu=invalid\n");
        assert_eq!(fallback, Config::default());
    }

    #[test]
    fn bls_parsing_strips_ostree_index() {
        let bls = "\
title Elavo 0.3.4.5 (ostree:0)
version 2
options root=UUID=1234 rw
linux /boot/vmlinuz
";
        let parsed = parse_bls(bls).unwrap();
        assert_eq!(parsed.title, "Elavo 0.3.4.5");
        assert_eq!(parsed.version, 2);
        assert_eq!(
            split_name_version(&parsed.title),
            ("Elavo".to_owned(), "0.3.4.5".to_owned())
        );
    }

    #[test]
    fn state_parsing_and_availability() {
        let state = RefindState::parse(
            "installed=1\nsecure_boot=0\nboot_entry=1\nesp_label=EFI-SYSTEM\nother=other:Microsoft\tWindows\tWindows SSD\n",
        );
        assert_eq!(state.installed, Some(true));
        assert!(!state.secure_boot);
        assert!(state.boot_entry);
        assert_eq!(state.esp_label, "EFI-SYSTEM");
        assert_eq!(state.others.len(), 1);
        assert_eq!(state.others[0].label, "Windows");
        assert_eq!(state.others[0].disk, "Windows SSD");

        assert_eq!(
            detect_availability_with(false, false, true, Some(&state)),
            Availability::NotUefi
        );
        assert_eq!(
            detect_availability_with(true, true, true, Some(&state)),
            Availability::SecureBootEnabled
        );
        assert_eq!(
            detect_availability_with(true, false, false, Some(&state)),
            Availability::RefindNotInstalled
        );
        assert_eq!(
            detect_availability_with(true, false, true, Some(&state)),
            Availability::Available
        );
    }

    #[test]
    fn lsblk_and_options_building() {
        let lsblk = r#"
LABEL="EFI-SYSTEM" PARTLABEL="EFI-SYSTEM" MOUNTPOINT="/boot/efi"
LABEL="boot" PARTLABEL="boot" MOUNTPOINT="/boot"
LABEL="root" PARTLABEL="root" MOUNTPOINT="/sysroot"
"#;
        let labels = parse_lsblk_labels(lsblk);
        assert_eq!(labels.system, "Elavo");
        assert_eq!(labels.esp, "EFI-SYSTEM");

        let bls = vec![
            BlsEntry {
                title: "Elavo 0.3.4.5".into(),
                version: 2,
            },
            BlsEntry {
                title: "Elavo 0.3.4.4".into(),
                version: 1,
            },
        ];
        let state = RefindState::parse("installed=1\nother=other:Microsoft\tWindows\tOSDisk\n");
        let opts = build_options(
            &bls,
            Some(&state),
            &labels,
            "Previous",
            "Recovery (GRUB)",
            "GRUB fallback",
            "EFI Boot",
        );
        assert_eq!(opts.len(), 4);
        assert_eq!(opts[0].target, DefaultTarget::Current);
        assert_eq!(opts[0].detail, "0.3.4.5");
        assert_eq!(opts[1].target, DefaultTarget::Previous);
        assert_eq!(opts[1].detail, "0.3.4.4 (Previous)");
        assert_eq!(opts[2].target, DefaultTarget::Other("Microsoft".into()));
        assert_eq!(opts[2].disk_name, "OSDisk");
        assert_eq!(opts[3].target, DefaultTarget::Recovery);
        assert_eq!(opts[3].disk_name, "EFI-SYSTEM");
    }
}
