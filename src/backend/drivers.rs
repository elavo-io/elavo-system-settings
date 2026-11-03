//! Hardware the Drivers wizard can see. Read from sysfs. Nothing here loads
//! a module or writes a file; the helper does that.

use std::path::Path;

/// `elavo-nvidia` is specified and not published. The wizard explains it
/// and does not offer to switch until this returns true, in the change that
/// publishes the image.
pub fn nvidia_image_published() -> bool {
    false
}

// The helper writes these. This binary only shows the wizard.
#[allow(dead_code)]
pub const WL_LOAD_PATH: &str = "/etc/modules-load.d/elavo-broadcom-wl.conf";
#[allow(dead_code)]
pub const WL_BLACKLIST_PATH: &str = "/etc/modprobe.d/elavo-broadcom-wl.conf";
#[allow(dead_code)]
pub const BRCMFMAC_OPTION_PATH: &str = "/etc/modprobe.d/elavo-brcmfmac.conf";

const BROADCOM: &str = "14e4";
const NVIDIA: &str = "10de";
const APPLE: &str = "106b";

/// PCI device ids the proprietary `wl` driver runs, and the kernel driver
/// does not. BCM4360 (`43a0`) is the 2013 MacBook Air.
const WL_IDS: &[&str] = &[
    "4311", "4312", "4315", "4328", "4329", "432a", "432b", "432c", "432d", "4331", "4353", "4358",
    "4359", "4365", "43a0", "43b1", "4727",
];
/// 2015–2017 Intel Macs. The built-in `brcmfmac` driver runs these.
const BRCMFMAC_IDS: &[&str] = &["43a3", "43ba"];
/// 2018–2020 T2 Macs. `brcmfmac` needs Apple firmware that is not shipped.
const APPLE_FIRMWARE_ID: &str = "4464";
/// FaceTime HD camera. Its firmware is taken from macOS and is not shipped.
const FACETIME_ID: &str = "1570";
/// Apple T2 bridge.
const T2_IDS: &[&str] = &["1801", "1802"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PciDevice {
    pub vendor: String,
    pub device: String,
    /// Class code without the `0x` prefix, lowercase. Display is `03…`.
    pub class: String,
    pub wireless: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WifiKind {
    /// Broadcom's closed-source `wl` module.
    Proprietary,
    /// The kernel's `brcmfmac`, which may need a module option.
    BuiltIn,
    /// The kernel driver needs firmware that is not in the image.
    Firmware,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiChip {
    pub id: String,
    pub name: &'static str,
    pub kind: WifiKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gap {
    Wifi(WifiChip),
    Nvidia,
    T2,
    Facetime,
}

impl WifiChip {
    /// The BCM43602 needs a module option. The other built-in chips do not.
    pub fn needs_option(&self) -> bool {
        self.kind == WifiKind::BuiltIn && self.id == "43ba"
    }
}

pub fn chip_name(id: &str) -> &'static str {
    match id {
        "4311" | "4312" => "BCM4311",
        "4315" => "BCM4312",
        "4328" | "4329" | "432a" => "BCM4321",
        "432b" | "432c" | "432d" => "BCM4322",
        "4331" => "BCM4331",
        "4353" => "BCM43224",
        "4358" => "BCM43227",
        "4359" => "BCM43228",
        "4365" => "BCM43142",
        "43a0" => "BCM4360",
        "43a3" => "BCM4350",
        "43b1" => "BCM4352",
        "43ba" => "BCM43602",
        "4464" => "BCM4364",
        "4727" => "BCM4313",
        _ => "Broadcom",
    }
}

fn wifi_kind(id: &str) -> Option<WifiKind> {
    if WL_IDS.contains(&id) {
        Some(WifiKind::Proprietary)
    } else if BRCMFMAC_IDS.contains(&id) {
        Some(WifiKind::BuiltIn)
    } else if id == APPLE_FIRMWARE_ID {
        Some(WifiKind::Firmware)
    } else {
        None
    }
}

fn is_display(class: &str) -> bool {
    class.starts_with("03")
}

/// What the wizard should list. A Wi-Fi chip that already has a wireless
/// interface is left out. The proprietary driver is never offered for a
/// chip `brcmfmac` can run.
pub fn gaps(devices: &[PciDevice], nvidia_module: bool) -> Vec<Gap> {
    let mut out = Vec::new();
    let mut saw_proprietary = false;
    let mut saw_builtin = false;
    let mut saw_firmware = false;
    for device in devices {
        if device.vendor != BROADCOM || device.wireless {
            continue;
        }
        let Some(kind) = wifi_kind(&device.device) else {
            continue;
        };
        let seen = match kind {
            WifiKind::Proprietary => &mut saw_proprietary,
            WifiKind::BuiltIn => &mut saw_builtin,
            WifiKind::Firmware => &mut saw_firmware,
        };
        if *seen {
            continue;
        }
        *seen = true;
        out.push(Gap::Wifi(WifiChip {
            id: device.device.clone(),
            name: chip_name(&device.device),
            kind,
        }));
    }
    out.sort_by_key(|gap| match gap {
        Gap::Wifi(chip) => match chip.kind {
            WifiKind::Proprietary => 0,
            WifiKind::BuiltIn => 1,
            WifiKind::Firmware => 2,
        },
        _ => 3,
    });
    if !nvidia_module
        && devices
            .iter()
            .any(|device| device.vendor == NVIDIA && is_display(&device.class))
    {
        out.push(Gap::Nvidia);
    }
    if devices
        .iter()
        .any(|device| device.vendor == APPLE && T2_IDS.contains(&device.device.as_str()))
    {
        out.push(Gap::T2);
    }
    if devices
        .iter()
        .any(|device| device.vendor == BROADCOM && device.device == FACETIME_ID)
    {
        out.push(Gap::Facetime);
    }
    out
}

#[allow(dead_code)] // The helper writes this file.
pub fn wl_load_conf() -> &'static str {
    "# Written by System Settings. Loads Broadcom's wl driver.\nwl\n"
}

#[allow(dead_code)] // The helper writes this file.
pub fn wl_blacklist_conf() -> &'static str {
    "# Written by System Settings. Keeps the in-kernel Broadcom drivers off\n\
     # this card so wl can use it.\n\
     blacklist b43\n\
     blacklist b43legacy\n\
     blacklist ssb\n\
     blacklist bcma\n\
     blacklist brcmsmac\n\
     blacklist brcmfmac\n"
}

#[allow(dead_code)] // The helper writes this file.
pub fn brcmfmac_option_conf() -> &'static str {
    "# Written by System Settings. The BCM43602 needs this option.\n\
     options brcmfmac feature_disable=0x82000\n"
}

/// The chips on this computer, and whether NVIDIA's module is loaded.
pub fn scan() -> Vec<Gap> {
    let nvidia = Path::new("/sys/module/nvidia").exists();
    gaps(&pci_devices(), nvidia)
}

pub fn chip_is_wireless(id: &str) -> bool {
    pci_devices()
        .iter()
        .any(|device| device.vendor == BROADCOM && device.device == id && device.wireless)
}

/// True when a BCM43602 with no working interface is present. The helper
/// writes the module option only then.
#[allow(dead_code)] // The helper decides whether to write the option.
pub fn needs_brcmfmac_option() -> bool {
    pci_devices()
        .iter()
        .any(|device| device.vendor == BROADCOM && device.device == "43ba" && !device.wireless)
}

fn pci_devices() -> Vec<PciDevice> {
    let Ok(entries) = std::fs::read_dir("/sys/bus/pci/devices") else {
        return Vec::new();
    };
    let mut devices = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(vendor) = read_hex(&path.join("vendor")) else {
            continue;
        };
        let Some(device) = read_hex(&path.join("device")) else {
            continue;
        };
        let class = read_hex(&path.join("class")).unwrap_or_default();
        devices.push(PciDevice {
            vendor,
            device,
            class,
            wireless: has_wireless(&path),
        });
    }
    devices
}

fn read_hex(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim().trim_start_matches("0x").to_ascii_lowercase();
    (!text.is_empty()).then_some(text)
}

fn has_wireless(device: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(device.join("net")) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let class = Path::new("/sys/class/net").join(entry.file_name());
        class.join("wireless").exists() || class.join("phy80211").exists()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(vendor: &str, device: &str, class: &str, wireless: bool) -> PciDevice {
        PciDevice {
            vendor: vendor.to_owned(),
            device: device.to_owned(),
            class: class.to_owned(),
            wireless,
        }
    }

    fn wifi(gaps: &[Gap]) -> Vec<&WifiChip> {
        gaps.iter()
            .filter_map(|gap| match gap {
                Gap::Wifi(chip) => Some(chip),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn air_2013_needs_the_proprietary_driver() {
        let found = gaps(&[dev(BROADCOM, "43a0", "028000", false)], false);
        let chip = wifi(&found);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].id, "43a0");
        assert_eq!(chip[0].name, "BCM4360");
        assert_eq!(chip[0].kind, WifiKind::Proprietary);
        assert!(!chip[0].needs_option());
    }

    #[test]
    fn a_working_interface_is_not_listed() {
        let found = gaps(&[dev(BROADCOM, "43a0", "028000", true)], false);
        assert!(wifi(&found).is_empty());
    }

    #[test]
    fn newer_macs_stay_on_the_built_in_driver() {
        let found = gaps(
            &[
                dev(BROADCOM, "43a3", "028000", false),
                dev(BROADCOM, "43ba", "028000", false),
            ],
            false,
        );
        let chip = wifi(&found);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].kind, WifiKind::BuiltIn);
        assert_eq!(chip[0].id, "43a3");

        let only_43602 = gaps(&[dev(BROADCOM, "43ba", "028000", false)], false);
        let chip = wifi(&only_43602);
        assert_eq!(chip[0].id, "43ba");
        assert!(chip[0].needs_option());
    }

    #[test]
    fn t2_wifi_is_explained_and_not_the_proprietary_driver() {
        let found = gaps(&[dev(BROADCOM, "4464", "028000", false)], false);
        let chip = wifi(&found);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].kind, WifiKind::Firmware);
        assert_eq!(chip[0].name, "BCM4364");
    }

    #[test]
    fn nvidia_is_a_separate_case_from_its_audio_function() {
        let gpu = gaps(&[dev(NVIDIA, "1c8d", "030200", false)], false);
        assert_eq!(gpu, vec![Gap::Nvidia]);
        let with_module = gaps(&[dev(NVIDIA, "1c8d", "030000", false)], true);
        assert!(with_module.is_empty());
        let audio = gaps(&[dev(NVIDIA, "10f0", "040300", false)], false);
        assert!(audio.is_empty());
    }

    #[test]
    fn t2_and_facetime_are_explanations() {
        let found = gaps(
            &[
                dev(APPLE, "1801", "060400", false),
                dev(BROADCOM, "1570", "048000", false),
            ],
            false,
        );
        assert_eq!(found, vec![Gap::T2, Gap::Facetime]);
    }

    #[test]
    fn an_unknown_broadcom_chip_is_ignored() {
        let found = gaps(&[dev(BROADCOM, "9999", "028000", false)], false);
        assert!(found.is_empty());
    }

    #[test]
    fn nvidia_image_is_not_published() {
        assert!(!nvidia_image_published());
    }

    #[test]
    fn config_files_do_what_the_wizard_says() {
        assert!(wl_load_conf().contains("\nwl\n"));
        for module in ["b43", "b43legacy", "ssb", "bcma", "brcmsmac", "brcmfmac"] {
            assert!(
                wl_blacklist_conf().contains(&format!("blacklist {module}\n")),
                "{module}"
            );
        }
        assert!(!wl_blacklist_conf().contains("install wl"));
        assert!(brcmfmac_option_conf().contains("options brcmfmac feature_disable=0x82000\n"));
    }
}
