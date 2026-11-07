//! Hardware the Drivers wizard can see. Read from sysfs. Nothing here loads
//! a module or writes a file; the helper does that.

use std::cell::Cell;
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

/// Broadcom's license, shipped with the driver. The wizard shows this file
/// and does not replace it with text of its own.
pub const BROADCOM_LICENSE: &str = "/usr/share/licenses/broadcom-wl/LICENSE.txt";

/// Opt-in only. Unset means the live computer. A name that is not one of
/// the scenarios below is rejected, and nothing then falls through to a
/// real driver change.
pub const SIMULATE_ENV: &str = "ELAVO_SETTINGS_SIMULATE_DRIVERS";

/// What a setup operation finished as. `Ready` is only a working wireless
/// interface. `Failed` is everything else, including a partial change, so
/// the wizard must not describe it as unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SetupOutcome {
    Ready,
    Restart,
    Missing,
    Blocked,
    Failed(String),
    /// Nothing to install. Explanations only.
    Explained,
}

/// How the Wi-Fi pane should talk about a missing device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WifiBanner {
    None,
    /// Proprietary or built-in driver, with a Set Up action.
    Available(Gap),
    /// Firmware that is not in the image. No install action.
    Firmware(WifiChip),
    /// The simulation name was not recognized.
    Rejected,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scenario {
    /// BCM4360, then a verified wireless interface.
    Proprietary,
    /// BCM43602, including its module option, then a verified interface.
    BuiltIn,
    /// BCM4364. Explanation only.
    Firmware,
    /// No gap, and a Wi-Fi device is already present.
    None,
    /// Same detection as `Proprietary`, named for the success result.
    Ready,
    Restart,
    Missing,
    Blocked,
    Failed,
    /// A proprietary Wi-Fi chip, NVIDIA graphics, and an Apple T2.
    Several,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Live,
    Scenario(Scenario),
    Rejected,
}

impl Mode {
    /// Live setup, reboot, and NetworkManager changes. Simulation never
    /// allows them, and neither does an unknown scenario name.
    pub fn allows_system_changes(self) -> bool {
        matches!(self, Mode::Live)
    }
}

impl Scenario {
    fn gaps(self) -> Vec<Gap> {
        let wifi = |id: &str| {
            Gap::Wifi(WifiChip {
                id: id.to_owned(),
                name: chip_name(id),
                kind: wifi_kind(id).expect("scenario chip"),
            })
        };
        match self {
            Scenario::Proprietary
            | Scenario::Ready
            | Scenario::Restart
            | Scenario::Missing
            | Scenario::Blocked
            | Scenario::Failed => vec![wifi("43a0")],
            Scenario::BuiltIn => vec![wifi("43ba")],
            Scenario::Firmware => vec![wifi("4464")],
            Scenario::None => Vec::new(),
            Scenario::Several => vec![wifi("43a0"), Gap::Nvidia, Gap::T2],
        }
    }

    fn wifi_device_present(self) -> bool {
        matches!(self, Scenario::None)
    }

    fn outcome(self, gap: &Gap) -> SetupOutcome {
        let actionable = matches!(
            gap,
            Gap::Wifi(WifiChip {
                kind: WifiKind::Proprietary | WifiKind::BuiltIn,
                ..
            })
        );
        if !actionable {
            return SetupOutcome::Explained;
        }
        match self {
            Scenario::Restart => SetupOutcome::Restart,
            Scenario::Missing => SetupOutcome::Missing,
            Scenario::Blocked => SetupOutcome::Blocked,
            Scenario::Failed => SetupOutcome::Failed("The driver was not turned on.".to_owned()),
            Scenario::Firmware | Scenario::None => SetupOutcome::Explained,
            Scenario::Proprietary | Scenario::BuiltIn | Scenario::Ready | Scenario::Several => {
                SetupOutcome::Ready
            }
        }
    }
}

pub fn mode_from(value: Option<&str>) -> Mode {
    match value {
        None => Mode::Live,
        Some(name) => parse_scenario(name)
            .map(Mode::Scenario)
            .unwrap_or(Mode::Rejected),
    }
}

pub fn mode() -> Mode {
    match std::env::var(SIMULATE_ENV) {
        Err(_) => Mode::Live,
        Ok(value) => mode_from(Some(value.trim())),
    }
}

fn parse_scenario(name: &str) -> Option<Scenario> {
    Some(match name {
        "proprietary" => Scenario::Proprietary,
        "builtin" => Scenario::BuiltIn,
        "firmware" => Scenario::Firmware,
        "none" => Scenario::None,
        "ready" => Scenario::Ready,
        "restart" => Scenario::Restart,
        "missing" => Scenario::Missing,
        "blocked" => Scenario::Blocked,
        "failed" => Scenario::Failed,
        "several" => Scenario::Several,
        _ => return None,
    })
}

/// The chips to show. A simulation never reads this computer's PCI bus.
/// An unknown scenario shows nothing rather than the live hardware.
pub fn scan() -> Vec<Gap> {
    match mode() {
        Mode::Live => {
            let nvidia = Path::new("/sys/module/nvidia").exists();
            gaps(&pci_devices(), nvidia)
        }
        Mode::Scenario(scenario) => scenario.gaps(),
        Mode::Rejected => Vec::new(),
    }
}

/// `None` only in live mode, where the caller may run the helper. Every
/// other mode returns an outcome and must not touch the system.
pub fn simulated_setup(gap: &Gap) -> Option<SetupOutcome> {
    match mode() {
        Mode::Live => None,
        other => Some(setup_for(other, gap)),
    }
}

fn setup_for(mode: Mode, gap: &Gap) -> SetupOutcome {
    match mode {
        Mode::Live => SetupOutcome::Failed(String::new()),
        Mode::Rejected => SetupOutcome::Failed(
            "This simulation name is not recognized, so nothing will change.".to_owned(),
        ),
        Mode::Scenario(scenario) => scenario.outcome(gap),
    }
}

/// Map a helper status word. `verified` is a wireless interface seen after
/// the module loaded. Without that, `loaded` is a restart, not success.
pub fn outcome_from_status(status: &str, verified: bool, error: Option<&str>) -> SetupOutcome {
    match status {
        "missing" => SetupOutcome::Missing,
        "refused" => SetupOutcome::Blocked,
        "reboot" => SetupOutcome::Restart,
        "loaded" if verified => SetupOutcome::Ready,
        "loaded" => SetupOutcome::Restart,
        _ => {
            let detail = error.filter(|text| !text.is_empty()).map(str::to_owned);
            SetupOutcome::Failed(
                detail.unwrap_or_else(|| "The driver was not turned on.".to_owned()),
            )
        }
    }
}

pub fn wifi_banner(nm_has_wifi: bool) -> WifiBanner {
    wifi_banner_for(mode(), nm_has_wifi)
}

thread_local! {
    /// A simulated setup that verified Wi-Fi. The notice then leaves, the
    /// same way it leaves once NetworkManager sees a device.
    static SIMULATED_WIFI_READY: Cell<bool> = const { Cell::new(false) };
}

/// Remember a simulated, verified Wi-Fi result so the notice can refresh.
pub fn mark_simulated_wifi_ready() {
    SIMULATED_WIFI_READY.with(|ready| ready.set(true));
}

fn wifi_banner_for(mode: Mode, nm_has_wifi: bool) -> WifiBanner {
    match mode {
        Mode::Rejected => WifiBanner::Rejected,
        Mode::Live => {
            if nm_has_wifi {
                return WifiBanner::None;
            }
            let nvidia = Path::new("/sys/module/nvidia").exists();
            banner_from(&gaps(&pci_devices(), nvidia))
        }
        Mode::Scenario(scenario) => {
            let ready = SIMULATED_WIFI_READY.with(|ready| ready.get());
            if scenario.wifi_device_present() || ready {
                return WifiBanner::None;
            }
            banner_from(&scenario.gaps())
        }
    }
}

fn banner_from(found: &[Gap]) -> WifiBanner {
    if let Some(gap) = found.iter().find(|gap| gap.is_actionable_wifi()).cloned() {
        return WifiBanner::Available(gap);
    }
    if let Some(Gap::Wifi(chip)) = found
        .iter()
        .find(|gap| matches!(gap, Gap::Wifi(chip) if chip.kind == WifiKind::Firmware))
    {
        return WifiBanner::Firmware(chip.clone());
    }
    WifiBanner::None
}

impl Gap {
    /// A Wi-Fi driver this image can turn on. Firmware is not one of these.
    pub fn is_actionable_wifi(&self) -> bool {
        matches!(
            self,
            Gap::Wifi(WifiChip {
                kind: WifiKind::Proprietary | WifiKind::BuiltIn,
                ..
            })
        )
    }

    /// The as-is disclosure is only for Broadcom's proprietary driver.
    pub fn shows_as_is_disclosure(&self) -> bool {
        matches!(
            self,
            Gap::Wifi(WifiChip {
                kind: WifiKind::Proprietary,
                ..
            })
        )
    }
}

/// The license file's own text, or `None` when it is missing or empty.
/// Nothing is substituted.
pub fn license_text_at(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

pub fn license_text() -> Option<String> {
    license_text_at(Path::new(BROADCOM_LICENSE))
}

thread_local! {
    static ON_CHANGE: std::cell::RefCell<Option<Box<dyn Fn()>>> = const { std::cell::RefCell::new(None) };
}

/// The Wi-Fi pane asks to be told when a setup finishes, so its notice can
/// leave once a device appears.
pub fn on_driver_change(update: impl Fn() + 'static) {
    ON_CHANGE.with(|slot| *slot.borrow_mut() = Some(Box::new(update)));
}

pub fn notify_driver_change() {
    ON_CHANGE.with(|slot| {
        if let Some(update) = slot.borrow().as_ref() {
            update();
        }
    });
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

    fn chip(id: &str) -> Gap {
        Gap::Wifi(WifiChip {
            id: id.to_owned(),
            name: chip_name(id),
            kind: wifi_kind(id).unwrap(),
        })
    }

    #[test]
    fn an_unset_simulation_is_live_and_an_unknown_name_is_rejected() {
        assert_eq!(mode_from(None), Mode::Live);
        assert!(mode_from(None).allows_system_changes());
        assert_eq!(mode_from(Some("")), Mode::Rejected);
        assert_eq!(mode_from(Some("nope")), Mode::Rejected);
        assert!(!Mode::Rejected.allows_system_changes());
    }

    #[test]
    fn every_named_scenario_blocks_live_changes() {
        for name in [
            "proprietary",
            "builtin",
            "firmware",
            "none",
            "ready",
            "restart",
            "missing",
            "blocked",
            "failed",
            "several",
        ] {
            let mode = mode_from(Some(name));
            assert!(matches!(mode, Mode::Scenario(_)), "{name}");
            assert!(!mode.allows_system_changes(), "{name}");
            let Mode::Scenario(scenario) = mode else {
                continue;
            };
            let found = scenario.gaps();
            assert!(simulated_setup_is_closed(mode, &found));
        }
    }

    fn simulated_setup_is_closed(mode: Mode, found: &[Gap]) -> bool {
        if found.is_empty() {
            return setup_for(mode, &chip("43a0")) != SetupOutcome::Failed(String::new());
        }
        found.iter().all(|gap| {
            let outcome = setup_for(mode, gap);
            !matches!(outcome, SetupOutcome::Failed(text) if text.is_empty())
        })
    }

    #[test]
    fn a_rejected_simulation_never_asks_for_a_live_setup() {
        let outcome = setup_for(Mode::Rejected, &chip("43a0"));
        assert!(matches!(outcome, SetupOutcome::Failed(text) if text.contains("not recognized")));
    }

    #[test]
    fn scenarios_detect_the_hardware_they_name() {
        assert_eq!(Scenario::Proprietary.gaps(), vec![chip("43a0")]);
        assert!(Scenario::Proprietary.gaps()[0].is_actionable_wifi());
        assert!(Scenario::Proprietary.gaps()[0].shows_as_is_disclosure());
        assert!(!Scenario::BuiltIn.gaps()[0].shows_as_is_disclosure());
        assert!(!Scenario::Firmware.gaps()[0].shows_as_is_disclosure());
        let builtin = Scenario::BuiltIn.gaps();
        match &builtin[0] {
            Gap::Wifi(found) => {
                assert_eq!(found.kind, WifiKind::BuiltIn);
                assert!(found.needs_option());
            }
            other => panic!("{other:?}"),
        }
        match &Scenario::Firmware.gaps()[0] {
            Gap::Wifi(found) => assert_eq!(found.kind, WifiKind::Firmware),
            other => panic!("{other:?}"),
        }
        assert!(Scenario::None.gaps().is_empty());
        assert_eq!(Scenario::Several.gaps().len(), 3);
    }

    #[test]
    fn the_banner_follows_the_gap_and_ignores_live_wifi_while_simulating() {
        assert!(matches!(
            wifi_banner_for(Mode::Scenario(Scenario::Proprietary), true),
            WifiBanner::Available(_)
        ));
        assert!(matches!(
            wifi_banner_for(Mode::Scenario(Scenario::BuiltIn), false),
            WifiBanner::Available(_)
        ));
        assert!(matches!(
            wifi_banner_for(Mode::Scenario(Scenario::Firmware), false),
            WifiBanner::Firmware(_)
        ));
        assert_eq!(
            wifi_banner_for(Mode::Scenario(Scenario::None), false),
            WifiBanner::None
        );
        assert_eq!(wifi_banner_for(Mode::Rejected, false), WifiBanner::Rejected);
        assert_eq!(wifi_banner_for(Mode::Live, true), WifiBanner::None);
        mark_simulated_wifi_ready();
        assert_eq!(
            wifi_banner_for(Mode::Scenario(Scenario::Proprietary), false),
            WifiBanner::None
        );
        SIMULATED_WIFI_READY.with(|ready| ready.set(false));
        assert!(matches!(
            wifi_banner_for(Mode::Scenario(Scenario::Restart), true),
            WifiBanner::Available(_)
        ));
        assert_eq!(banner_from(&[]), WifiBanner::None);
        assert!(matches!(
            banner_from(&[chip("43a0")]),
            WifiBanner::Available(_)
        ));
    }

    #[test]
    fn setup_results_match_the_scenario_and_success_needs_a_wireless_interface() {
        let wifi = chip("43a0");
        assert_eq!(Scenario::Ready.outcome(&wifi), SetupOutcome::Ready);
        assert_eq!(Scenario::Restart.outcome(&wifi), SetupOutcome::Restart);
        assert_eq!(Scenario::Missing.outcome(&wifi), SetupOutcome::Missing);
        assert_eq!(Scenario::Blocked.outcome(&wifi), SetupOutcome::Blocked);
        match Scenario::Failed.outcome(&wifi) {
            SetupOutcome::Failed(text) => {
                assert!(!text.to_lowercase().contains("nothing was changed"))
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(
            Scenario::Firmware.outcome(&chip("4464")),
            SetupOutcome::Explained
        );
        assert_eq!(
            outcome_from_status("loaded", true, None),
            SetupOutcome::Ready
        );
        assert_eq!(
            outcome_from_status("loaded", false, None),
            SetupOutcome::Restart
        );
        assert_eq!(
            outcome_from_status("missing", false, Some("missing")),
            SetupOutcome::Missing
        );
        assert_eq!(
            outcome_from_status("refused", false, Some("refused")),
            SetupOutcome::Blocked
        );
        assert_eq!(
            outcome_from_status("reboot", false, None),
            SetupOutcome::Restart
        );
        match outcome_from_status("", false, Some("modprobe: busy")) {
            SetupOutcome::Failed(text) => {
                assert!(text.contains("modprobe: busy"));
                assert!(!text.to_lowercase().contains("nothing was changed"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_missing_license_file_is_not_replaced() {
        assert!(license_text_at(Path::new("/no/such/broadcom-wl-license.txt")).is_none());
        let path = std::env::temp_dir().join("elavo-license-read-test.txt");
        std::fs::write(&path, "notice stays with the file\n").unwrap();
        assert_eq!(
            license_text_at(&path).as_deref(),
            Some("notice stays with the file")
        );
        let _ = std::fs::remove_file(&path);
        assert!(license_text_at(Path::new("/no/such/empty")).is_none());
    }
}
