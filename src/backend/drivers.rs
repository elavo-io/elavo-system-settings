//! Hardware the Drivers wizard can see. Read from sysfs. Nothing here loads
//! a module or writes a file; the helper does that.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

mod nvidia_gpus {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/backend/nvidia_gpus.rs"
    ));
}

/// NVIDIA's supported-gpus list, shipped with the driver on the NVIDIA
/// editions. The embedded table is used when this file is not present,
/// which is the case on the base image.
pub const NVIDIA_SUPPORTED_GPUS: &str = "/usr/share/doc/nvidia-driver/supported-gpus.json";

/// EFI Secure Boot state. The first four bytes are attributes; the fifth
/// is 1 when Secure Boot is on, 0 when it is off. Readable without root.
pub const SECURE_BOOT_EFIVAR: &str =
    "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

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

/// Which NVIDIA edition a GPU needs, or why it cannot use one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NvidiaSupport {
    /// Turing and newer. Image name `elavo-nvidia-open`.
    Open,
    /// Maxwell, Pascal, or Volta. Image name `elavo-nvidia-legacy`.
    Legacy,
    /// Older than Maxwell. Stay on the base image.
    Unsupported,
}

/// The NVIDIA image a GPU should run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NvidiaEdition {
    Open,
    Legacy,
}

impl NvidiaEdition {
    /// The image repository name, without a registry or tag.
    pub fn image_name(self) -> &'static str {
        match self {
            NvidiaEdition::Open => "elavo-nvidia-open",
            NvidiaEdition::Legacy => "elavo-nvidia-legacy",
        }
    }
}

impl From<NvidiaEdition> for NvidiaSupport {
    fn from(edition: NvidiaEdition) -> Self {
        match edition {
            NvidiaEdition::Open => NvidiaSupport::Open,
            NvidiaEdition::Legacy => NvidiaSupport::Legacy,
        }
    }
}

/// Which Elavo image this computer is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElavoImage {
    Base,
    NvidiaOpen,
    NvidiaLegacy,
}

impl ElavoImage {
    fn edition(self) -> Option<NvidiaEdition> {
        match self {
            ElavoImage::Base => None,
            ElavoImage::NvidiaOpen => Some(NvidiaEdition::Open),
            ElavoImage::NvidiaLegacy => Some(NvidiaEdition::Legacy),
        }
    }
}

/// What the Drivers wizard should do for NVIDIA graphics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NvidiaGap {
    /// Switch to this edition, keeping the current tag.
    Switch {
        edition: NvidiaEdition,
        id: String,
        from: ElavoImage,
    },
    /// The driver would not load because Secure Boot is on.
    SecureBoot {
        edition: NvidiaEdition,
        id: String,
        /// Already on this edition, but the module did not load.
        current: bool,
    },
    /// Already on the right edition, but the nvidia module is not loaded.
    NotLoaded { edition: NvidiaEdition, id: String },
    /// The GPU is older than the current NVIDIA driver supports.
    Unsupported { id: String },
    /// The GPU is not in NVIDIA's supported-gpus list.
    Unknown { id: String },
}

impl NvidiaGap {
    pub fn is_actionable(&self) -> bool {
        matches!(self, NvidiaGap::Switch { .. })
    }

    pub fn id(&self) -> &str {
        match self {
            NvidiaGap::Switch { id, .. }
            | NvidiaGap::SecureBoot { id, .. }
            | NvidiaGap::NotLoaded { id, .. }
            | NvidiaGap::Unsupported { id }
            | NvidiaGap::Unknown { id } => id,
        }
    }
}

/// What `gaps` needs besides the PCI device list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanCtx {
    pub nvidia_module: bool,
    pub image: ElavoImage,
    pub secure_boot: bool,
}

impl ScanCtx {
    #[cfg(test)]
    pub fn base() -> Self {
        Self {
            nvidia_module: false,
            image: ElavoImage::Base,
            secure_boot: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Gap {
    Wifi(WifiChip),
    Nvidia(NvidiaGap),
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
pub fn gaps(devices: &[PciDevice], ctx: ScanCtx) -> Vec<Gap> {
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
    if let Some(gap) = nvidia_gap(devices, ctx) {
        out.push(Gap::Nvidia(gap));
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
    /// Pascal NVIDIA graphics on the base image. Offers a switch to legacy.
    Nvidia,
    /// Turing NVIDIA graphics on the base image. Offers a switch to open.
    NvidiaOpen,
    /// Pascal NVIDIA graphics, but Secure Boot is on.
    NvidiaSecureBoot,
    /// A Kepler GPU that the current NVIDIA driver does not support.
    NvidiaUnsupported,
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
            Scenario::Several => vec![wifi("43a0"), nvidia_switch_gap(), Gap::T2],
            Scenario::Nvidia => vec![nvidia_switch_gap()],
            Scenario::NvidiaOpen => vec![nvidia_open_switch_gap()],
            Scenario::NvidiaSecureBoot => vec![nvidia_secure_boot_gap()],
            Scenario::NvidiaUnsupported => vec![Gap::Nvidia(NvidiaGap::Unsupported {
                id: "1180".to_owned(),
            })],
        }
    }

    fn wifi_device_present(self) -> bool {
        matches!(self, Scenario::None)
    }

    fn outcome(self, gap: &Gap) -> SetupOutcome {
        if let Gap::Nvidia(state) = gap {
            if !state.is_actionable() {
                return SetupOutcome::Explained;
            }
            return match self {
                Scenario::Restart => SetupOutcome::Restart,
                Scenario::Missing => SetupOutcome::Missing,
                Scenario::Blocked => SetupOutcome::Blocked,
                Scenario::Failed => {
                    SetupOutcome::Failed("The NVIDIA edition was not switched.".to_owned())
                }
                Scenario::Firmware | Scenario::None | Scenario::NvidiaUnsupported => {
                    SetupOutcome::Explained
                }
                _ => SetupOutcome::Restart,
            };
        }
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
            Scenario::Firmware
            | Scenario::None
            | Scenario::Nvidia
            | Scenario::NvidiaOpen
            | Scenario::NvidiaSecureBoot
            | Scenario::NvidiaUnsupported => SetupOutcome::Explained,
            Scenario::Proprietary | Scenario::BuiltIn | Scenario::Ready | Scenario::Several => {
                SetupOutcome::Ready
            }
        }
    }
}

fn nvidia_switch_gap() -> Gap {
    Gap::Nvidia(NvidiaGap::Switch {
        edition: NvidiaEdition::Legacy,
        id: "1d10".to_owned(),
        from: ElavoImage::Base,
    })
}

fn nvidia_open_switch_gap() -> Gap {
    Gap::Nvidia(NvidiaGap::Switch {
        edition: NvidiaEdition::Open,
        id: "1f82".to_owned(),
        from: ElavoImage::Base,
    })
}

fn nvidia_secure_boot_gap() -> Gap {
    Gap::Nvidia(NvidiaGap::SecureBoot {
        edition: NvidiaEdition::Legacy,
        id: "1d10".to_owned(),
        current: false,
    })
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
        "nvidia" => Scenario::Nvidia,
        "nvidia-open" => Scenario::NvidiaOpen,
        "nvidia-secureboot" => Scenario::NvidiaSecureBoot,
        "nvidia-unsupported" => Scenario::NvidiaUnsupported,
        _ => return None,
    })
}

/// The chips to show. A simulation never reads this computer's PCI bus.
/// An unknown scenario shows nothing rather than the live hardware.
pub fn scan() -> Vec<Gap> {
    match mode() {
        Mode::Live => gaps(&pci_devices(), live_scan_ctx()),
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
            banner_from(&gaps(&pci_devices(), live_scan_ctx()))
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

    /// A switch to an NVIDIA edition this image can start.
    pub fn is_actionable_nvidia(&self) -> bool {
        matches!(self, Gap::Nvidia(state) if state.is_actionable())
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

fn live_scan_ctx() -> ScanCtx {
    ScanCtx {
        nvidia_module: Path::new("/sys/module/nvidia").exists(),
        image: elavo_image_from_os_release_path(Path::new("/usr/lib/os-release")),
        secure_boot: secure_boot_enabled(),
    }
}

/// Secure Boot as the firmware reports it. Missing or unreadable is off,
/// which covers BIOS systems and a missing efivar. No root needed.
pub fn secure_boot_enabled() -> bool {
    secure_boot_enabled_at(Path::new(SECURE_BOOT_EFIVAR))
}

pub fn secure_boot_enabled_at(path: &Path) -> bool {
    let Ok(data) = std::fs::read(path) else {
        return false;
    };
    data.get(4).copied() == Some(1)
}

pub fn elavo_image_from_os_release_path(path: &Path) -> ElavoImage {
    std::fs::read_to_string(path)
        .map(|text| elavo_image_from_os_release(&text))
        .unwrap_or(ElavoImage::Base)
}

/// `VARIANT_ID` from os-release. Only `nvidia-open` and `nvidia-legacy`
/// are NVIDIA editions; anything else is the base image.
pub fn elavo_image_from_os_release(text: &str) -> ElavoImage {
    for line in text.lines() {
        let Some(value) = line.strip_prefix("VARIANT_ID=") else {
            continue;
        };
        let value = value.trim().trim_matches('"');
        return match value {
            "nvidia-open" => ElavoImage::NvidiaOpen,
            "nvidia-legacy" => ElavoImage::NvidiaLegacy,
            _ => ElavoImage::Base,
        };
    }
    ElavoImage::Base
}

/// The last path component of an image reference, without a tag or digest.
#[allow(dead_code)]
pub fn image_basename(image: &str) -> Option<&str> {
    let image = image.split('@').next()?;
    let last = image.rsplit('/').next()?;
    let name = last.split(':').next()?;
    (!name.is_empty()).then_some(name)
}

#[allow(dead_code)]
pub fn elavo_image_from_reference(image: &str) -> Option<ElavoImage> {
    match image_basename(image)? {
        "elavo" => Some(ElavoImage::Base),
        "elavo-nvidia-open" => Some(ElavoImage::NvidiaOpen),
        "elavo-nvidia-legacy" => Some(ElavoImage::NvidiaLegacy),
        // The unpublished single-image name, before the two editions.
        "elavo-nvidia" => None,
        _ => None,
    }
}

const ELAVO_SWITCHABLE: &[&str] = &[
    "elavo",
    "elavo-nvidia",
    "elavo-nvidia-open",
    "elavo-nvidia-legacy",
];

fn rewrite_elavo_image(image: &str, dest: &str) -> Option<String> {
    let image = image.split('@').next()?;
    let (repo, last) = image.rsplit_once('/')?;
    let (name, tag) = match last.split_once(':') {
        Some((name, tag)) => (name, Some(tag)),
        None => (last, None),
    };
    if !ELAVO_SWITCHABLE.contains(&name) {
        return None;
    }
    Some(match tag {
        Some(tag) => format!("{repo}/{dest}:{tag}"),
        None => format!("{repo}/{dest}"),
    })
}

/// `ghcr.io/elavo-io/elavo:latest` to `ghcr.io/elavo-io/elavo-nvidia-legacy:latest`.
/// Accepts the base image and either NVIDIA edition, and keeps the tag.
#[allow(dead_code)]
pub fn nvidia_image_target(image: &str, edition: NvidiaEdition) -> Option<String> {
    rewrite_elavo_image(image, edition.image_name())
}

/// `ghcr.io/elavo-io/elavo-nvidia-legacy:latest` to `ghcr.io/elavo-io/elavo:latest`.
/// Target for switching back to the standard Elavo image, keeping the tag.
#[allow(dead_code)]
pub fn base_image_target(image: &str) -> Option<String> {
    let name = image_basename(image)?;
    if name == "elavo" || !ELAVO_SWITCHABLE.contains(&name) {
        return None;
    }
    rewrite_elavo_image(image, "elavo")
}

pub fn embedded_nvidia_support(id: &str) -> Option<NvidiaSupport> {
    let id = id.trim().trim_start_matches("0x").to_ascii_lowercase();
    if nvidia_gpus::OPEN.binary_search(&id.as_str()).is_ok() {
        Some(NvidiaSupport::Open)
    } else if nvidia_gpus::LEGACY.binary_search(&id.as_str()).is_ok() {
        Some(NvidiaSupport::Legacy)
    } else if nvidia_gpus::UNSUPPORTED.binary_search(&id.as_str()).is_ok() {
        Some(NvidiaSupport::Unsupported)
    } else {
        None
    }
}

/// Parse NVIDIA's supported-gpus.json and classify each unique device id.
pub fn parse_supported_gpus_json(text: &str) -> Option<BTreeMap<String, NvidiaSupport>> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let chips = value.get("chips")?.as_array()?;
    let mut map = BTreeMap::new();
    for chip in chips {
        let devid = chip
            .get("devid")?
            .as_str()?
            .trim_start_matches("0x")
            .to_ascii_lowercase();
        if devid.is_empty() {
            continue;
        }
        let support = if chip.get("legacybranch").and_then(|b| b.as_str()).is_some() {
            NvidiaSupport::Unsupported
        } else if chip
            .get("features")
            .and_then(|f| f.as_array())
            .is_some_and(|features| {
                features
                    .iter()
                    .any(|item| item.as_str() == Some("kernelopen"))
            })
        {
            NvidiaSupport::Open
        } else {
            NvidiaSupport::Legacy
        };
        map.insert(devid, support);
    }
    Some(map)
}

fn installed_nvidia_table() -> Option<&'static BTreeMap<String, NvidiaSupport>> {
    static TABLE: OnceLock<Option<BTreeMap<String, NvidiaSupport>>> = OnceLock::new();
    TABLE
        .get_or_init(|| {
            let text = std::fs::read_to_string(NVIDIA_SUPPORTED_GPUS).ok()?;
            parse_supported_gpus_json(&text)
        })
        .as_ref()
}

/// Prefer the driver copy on this computer when it is present, so an
/// NVIDIA image can stay current. Fall back to the embedded table.
pub fn nvidia_support(id: &str) -> Option<NvidiaSupport> {
    let id = id.trim().trim_start_matches("0x").to_ascii_lowercase();
    if let Some(table) = installed_nvidia_table() {
        return table.get(&id).copied();
    }
    embedded_nvidia_support(&id)
}

fn nvidia_display_gpus(devices: &[PciDevice]) -> Vec<&PciDevice> {
    devices
        .iter()
        .filter(|device| device.vendor == NVIDIA && is_display(&device.class))
        .collect()
}

fn nvidia_gap(devices: &[PciDevice], ctx: ScanCtx) -> Option<NvidiaGap> {
    let gpus = nvidia_display_gpus(devices);
    if gpus.is_empty() {
        return None;
    }
    let mut editions: Vec<(NvidiaEdition, String)> = Vec::new();
    let mut unsupported = Vec::new();
    let mut unknown = Vec::new();
    for gpu in gpus {
        match nvidia_support(&gpu.device) {
            Some(NvidiaSupport::Open) => editions.push((NvidiaEdition::Open, gpu.device.clone())),
            Some(NvidiaSupport::Legacy) => {
                editions.push((NvidiaEdition::Legacy, gpu.device.clone()))
            }
            Some(NvidiaSupport::Unsupported) => unsupported.push(gpu.device.clone()),
            None => unknown.push(gpu.device.clone()),
        }
    }
    if editions.is_empty() {
        if let Some(id) = unsupported.into_iter().next() {
            return Some(NvidiaGap::Unsupported { id });
        }
        return unknown
            .into_iter()
            .next()
            .map(|id| NvidiaGap::Unknown { id });
    }
    let edition = editions[0].0;
    if editions.iter().any(|(other, _)| *other != edition) {
        return Some(NvidiaGap::Unknown {
            id: editions[0].1.clone(),
        });
    }
    let id = editions[0].1.clone();
    let already = ctx.image.edition() == Some(edition);
    if already {
        if ctx.nvidia_module {
            return None;
        }
        if ctx.secure_boot {
            return Some(NvidiaGap::SecureBoot {
                edition,
                id,
                current: true,
            });
        }
        return Some(NvidiaGap::NotLoaded { edition, id });
    }
    if ctx.secure_boot {
        return Some(NvidiaGap::SecureBoot {
            edition,
            id,
            current: false,
        });
    }
    Some(NvidiaGap::Switch {
        edition,
        id,
        from: ctx.image,
    })
}

/// The edition `switch-nvidia` should rebase to, or a reason it should not.
#[allow(dead_code)]
pub fn needed_nvidia_edition(devices: &[PciDevice]) -> Result<NvidiaEdition, String> {
    match nvidia_gap(
        devices,
        ScanCtx {
            nvidia_module: false,
            image: ElavoImage::Base,
            secure_boot: false,
        },
    ) {
        Some(NvidiaGap::Switch { edition, .. }) => Ok(edition),
        Some(NvidiaGap::Unsupported { id }) => Err(format!(
            "NVIDIA GPU 10de:{id} is not supported by the current driver"
        )),
        Some(NvidiaGap::Unknown { id }) => Err(format!(
            "NVIDIA GPU 10de:{id} is not in the supported-gpus list"
        )),
        Some(NvidiaGap::SecureBoot { .. } | NvidiaGap::NotLoaded { .. }) | None => {
            Err("this computer has no NVIDIA graphics Elavo can switch to".to_owned())
        }
    }
}

/// Live PCI devices, for the helper. The wizard uses `scan` instead.
pub fn pci_devices() -> Vec<PciDevice> {
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
        let found = gaps(&[dev(BROADCOM, "43a0", "028000", false)], ScanCtx::base());
        let chip = wifi(&found);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].id, "43a0");
        assert_eq!(chip[0].name, "BCM4360");
        assert_eq!(chip[0].kind, WifiKind::Proprietary);
        assert!(!chip[0].needs_option());
    }

    #[test]
    fn a_working_interface_is_not_listed() {
        let found = gaps(&[dev(BROADCOM, "43a0", "028000", true)], ScanCtx::base());
        assert!(wifi(&found).is_empty());
    }

    #[test]
    fn newer_macs_stay_on_the_built_in_driver() {
        let found = gaps(
            &[
                dev(BROADCOM, "43a3", "028000", false),
                dev(BROADCOM, "43ba", "028000", false),
            ],
            ScanCtx::base(),
        );
        let chip = wifi(&found);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].kind, WifiKind::BuiltIn);
        assert_eq!(chip[0].id, "43a3");

        let only_43602 = gaps(&[dev(BROADCOM, "43ba", "028000", false)], ScanCtx::base());
        let chip = wifi(&only_43602);
        assert_eq!(chip[0].id, "43ba");
        assert!(chip[0].needs_option());
    }

    #[test]
    fn t2_wifi_is_explained_and_not_the_proprietary_driver() {
        let found = gaps(&[dev(BROADCOM, "4464", "028000", false)], ScanCtx::base());
        let chip = wifi(&found);
        assert_eq!(chip.len(), 1);
        assert_eq!(chip[0].kind, WifiKind::Firmware);
        assert_eq!(chip[0].name, "BCM4364");
    }

    #[test]
    fn nvidia_is_a_separate_case_from_its_audio_function() {
        let gpu = gaps(&[dev(NVIDIA, "1c8d", "030200", false)], ScanCtx::base());
        assert_eq!(
            gpu,
            vec![Gap::Nvidia(NvidiaGap::Switch {
                edition: NvidiaEdition::Legacy,
                id: "1c8d".to_owned(),
                from: ElavoImage::Base,
            })]
        );
        // The module being loaded on the base image does not hide a needed
        // switch; already-on is decided by the image, not the module.
        let with_module = gaps(
            &[dev(NVIDIA, "1c8d", "030000", false)],
            ScanCtx {
                nvidia_module: true,
                ..ScanCtx::base()
            },
        );
        assert_eq!(with_module.len(), 1);
        let audio = gaps(&[dev(NVIDIA, "10f0", "040300", false)], ScanCtx::base());
        assert!(audio.is_empty());
    }

    #[test]
    fn t2_and_facetime_are_explanations() {
        let found = gaps(
            &[
                dev(APPLE, "1801", "060400", false),
                dev(BROADCOM, "1570", "048000", false),
            ],
            ScanCtx::base(),
        );
        assert_eq!(found, vec![Gap::T2, Gap::Facetime]);
    }

    #[test]
    fn an_unknown_broadcom_chip_is_ignored() {
        let found = gaps(&[dev(BROADCOM, "9999", "028000", false)], ScanCtx::base());
        assert!(found.is_empty());
    }

    #[test]
    fn the_embedded_ids_match_nvidia_classification() {
        assert_eq!(embedded_nvidia_support("1d10"), Some(NvidiaSupport::Legacy));
        assert_eq!(embedded_nvidia_support("1c8d"), Some(NvidiaSupport::Legacy));
        assert_eq!(embedded_nvidia_support("1b80"), Some(NvidiaSupport::Legacy));
        assert_eq!(embedded_nvidia_support("1d81"), Some(NvidiaSupport::Legacy));
        assert_eq!(embedded_nvidia_support("1380"), Some(NvidiaSupport::Legacy));
        assert_eq!(embedded_nvidia_support("1f82"), Some(NvidiaSupport::Open));
        assert_eq!(embedded_nvidia_support("2184"), Some(NvidiaSupport::Open));
        assert_eq!(embedded_nvidia_support("2206"), Some(NvidiaSupport::Open));
        assert_eq!(embedded_nvidia_support("2684"), Some(NvidiaSupport::Open));
        assert_eq!(
            embedded_nvidia_support("1180"),
            Some(NvidiaSupport::Unsupported)
        );
        assert_eq!(
            embedded_nvidia_support("0x1D10"),
            Some(NvidiaSupport::Legacy)
        );
        assert_eq!(embedded_nvidia_support("ffff"), None);
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
            "nvidia",
            "nvidia-open",
            "nvidia-secureboot",
            "nvidia-unsupported",
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
    fn mx150_on_the_base_image_offers_the_legacy_edition() {
        let found = gaps(&[dev(NVIDIA, "1d10", "030200", false)], ScanCtx::base());
        assert_eq!(
            found,
            vec![Gap::Nvidia(NvidiaGap::Switch {
                edition: NvidiaEdition::Legacy,
                id: "1d10".to_owned(),
                from: ElavoImage::Base,
            })]
        );
        assert!(found[0].is_actionable_nvidia());
    }

    #[test]
    fn already_on_the_legacy_edition_is_not_listed() {
        let found = gaps(
            &[dev(NVIDIA, "1d10", "030200", false)],
            ScanCtx {
                nvidia_module: true,
                image: ElavoImage::NvidiaLegacy,
                secure_boot: false,
            },
        );
        assert!(found.is_empty());
    }

    #[test]
    fn the_wrong_nvidia_edition_is_offered_even_if_the_module_is_loaded() {
        let found = gaps(
            &[dev(NVIDIA, "1d10", "030200", false)],
            ScanCtx {
                nvidia_module: true,
                image: ElavoImage::NvidiaOpen,
                secure_boot: false,
            },
        );
        match &found[..] {
            [Gap::Nvidia(NvidiaGap::Switch {
                edition: NvidiaEdition::Legacy,
                id,
                from: ElavoImage::NvidiaOpen,
            })] => assert_eq!(id, "1d10"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn secure_boot_blocks_the_switch() {
        let found = gaps(
            &[dev(NVIDIA, "1d10", "030200", false)],
            ScanCtx {
                secure_boot: true,
                ..ScanCtx::base()
            },
        );
        match &found[..] {
            [Gap::Nvidia(NvidiaGap::SecureBoot {
                edition: NvidiaEdition::Legacy,
                id,
                current: false,
            })] => {
                assert_eq!(id, "1d10");
                assert!(!found[0].is_actionable_nvidia());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn secure_boot_on_the_right_edition_is_explained_if_the_module_is_missing() {
        let found = gaps(
            &[dev(NVIDIA, "1d10", "030200", false)],
            ScanCtx {
                nvidia_module: false,
                image: ElavoImage::NvidiaLegacy,
                secure_boot: true,
            },
        );
        match &found[..] {
            [Gap::Nvidia(NvidiaGap::SecureBoot { current: true, .. })] => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_turing_gpu_offers_the_open_edition() {
        let found = gaps(&[dev(NVIDIA, "1f82", "030000", false)], ScanCtx::base());
        match &found[..] {
            [Gap::Nvidia(NvidiaGap::Switch {
                edition: NvidiaEdition::Open,
                id,
                from: ElavoImage::Base,
            })] => assert_eq!(id, "1f82"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn kepler_is_explained_and_not_offered() {
        let found = gaps(&[dev(NVIDIA, "1180", "030000", false)], ScanCtx::base());
        assert_eq!(
            found,
            vec![Gap::Nvidia(NvidiaGap::Unsupported {
                id: "1180".to_owned()
            })]
        );
        assert!(!found[0].is_actionable_nvidia());
    }

    #[test]
    fn an_unknown_nvidia_id_is_not_guessed() {
        let found = gaps(&[dev(NVIDIA, "ffff", "030000", false)], ScanCtx::base());
        assert_eq!(
            found,
            vec![Gap::Nvidia(NvidiaGap::Unknown {
                id: "ffff".to_owned()
            })]
        );
    }

    #[test]
    fn nvidia_image_targets_keep_the_tag() {
        assert_eq!(
            nvidia_image_target("ghcr.io/elavo-io/elavo:latest", NvidiaEdition::Legacy).as_deref(),
            Some("ghcr.io/elavo-io/elavo-nvidia-legacy:latest")
        );
        assert_eq!(
            nvidia_image_target("ghcr.io/elavo-io/elavo:stable", NvidiaEdition::Open).as_deref(),
            Some("ghcr.io/elavo-io/elavo-nvidia-open:stable")
        );
        assert_eq!(
            nvidia_image_target(
                "ghcr.io/elavo-io/elavo-nvidia-open:latest",
                NvidiaEdition::Legacy
            )
            .as_deref(),
            Some("ghcr.io/elavo-io/elavo-nvidia-legacy:latest")
        );
        assert_eq!(
            nvidia_image_target("ghcr.io/elavo-io/elavo@sha256:abc", NvidiaEdition::Open)
                .as_deref(),
            Some("ghcr.io/elavo-io/elavo-nvidia-open")
        );
        assert_eq!(
            nvidia_image_target("ghcr.io/elavo-io/other:latest", NvidiaEdition::Open),
            None
        );
        assert_eq!(
            base_image_target("ghcr.io/elavo-io/elavo-nvidia-legacy:latest").as_deref(),
            Some("ghcr.io/elavo-io/elavo:latest")
        );
        assert_eq!(
            base_image_target("ghcr.io/elavo-io/elavo-nvidia-open:stable").as_deref(),
            Some("ghcr.io/elavo-io/elavo:stable")
        );
        assert_eq!(base_image_target("ghcr.io/elavo-io/elavo:latest"), None);
        assert_eq!(
            elavo_image_from_reference(
                "ostree-unverified-registry:ghcr.io/elavo-io/elavo-nvidia-legacy:latest"
            ),
            Some(ElavoImage::NvidiaLegacy)
        );
        assert_eq!(
            elavo_image_from_os_release("NAME=Elavo\nVARIANT_ID=nvidia-legacy\n"),
            ElavoImage::NvidiaLegacy
        );
        assert_eq!(
            elavo_image_from_os_release("NAME=Elavo\nVARIANT_ID=nvidia-open\n"),
            ElavoImage::NvidiaOpen
        );
        assert_eq!(
            elavo_image_from_os_release("NAME=Elavo\n"),
            ElavoImage::Base
        );
    }

    #[test]
    fn secure_boot_reads_the_efivar_payload() {
        let dir = std::env::temp_dir();
        let off = dir.join("elavo-sb-off");
        let on = dir.join("elavo-sb-on");
        std::fs::write(&off, [6u8, 0, 0, 0, 0]).unwrap();
        std::fs::write(&on, [6u8, 0, 0, 0, 1]).unwrap();
        assert!(!secure_boot_enabled_at(&off));
        assert!(secure_boot_enabled_at(&on));
        assert!(!secure_boot_enabled_at(Path::new("/no/such/secure-boot")));
        let _ = std::fs::remove_file(&off);
        let _ = std::fs::remove_file(&on);
    }

    #[test]
    fn supported_gpus_json_uses_kernelopen_and_legacybranch() {
        let json = r#"{"chips":[
            {"devid":"0x1D10","name":"GeForce MX150","features":[]},
            {"devid":"0x1F82","name":"GeForce GTX 1650","features":["kernelopen"]},
            {"devid":"0x1180","name":"GeForce GTX 680","legacybranch":"470.xx","features":[]}
        ]}"#;
        let map = parse_supported_gpus_json(json).unwrap();
        assert_eq!(map.get("1d10"), Some(&NvidiaSupport::Legacy));
        assert_eq!(map.get("1f82"), Some(&NvidiaSupport::Open));
        assert_eq!(map.get("1180"), Some(&NvidiaSupport::Unsupported));
    }

    #[test]
    fn embedded_table_matches_installed_json_when_present() {
        let Ok(text) = std::fs::read_to_string(NVIDIA_SUPPORTED_GPUS) else {
            return;
        };
        let parsed = parse_supported_gpus_json(&text).expect("supported-gpus.json");
        for (id, support) in &parsed {
            assert_eq!(embedded_nvidia_support(id), Some(*support), "{id}");
        }
        for &id in nvidia_gpus::OPEN
            .iter()
            .chain(nvidia_gpus::LEGACY)
            .chain(nvidia_gpus::UNSUPPORTED)
        {
            assert!(parsed.contains_key(id), "{id}");
        }
        assert_eq!(
            parsed.len(),
            nvidia_gpus::OPEN.len() + nvidia_gpus::LEGACY.len() + nvidia_gpus::UNSUPPORTED.len()
        );
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
