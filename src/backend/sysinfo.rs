//! What the About pane lists. Read from `/proc`, `/sys`, and `os-release`,
//! the same sources Folio's About Elavo window uses.

pub fn os_release_text() -> String {
    std::fs::read_to_string("/etc/os-release")
        .or_else(|_| std::fs::read_to_string("/usr/lib/os-release"))
        .unwrap_or_default()
}

pub fn os_release(key: &str) -> Option<String> {
    parse_os_release(&os_release_text(), key)
}

pub fn parse_os_release(text: &str, key: &str) -> Option<String> {
    text.lines()
        .find_map(|line| {
            let (k, v) = line.split_once('=')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"').trim_matches('\'').to_owned())
        })
        .filter(|v| !v.is_empty())
}

/// The edition shown after the version, or `None` on the standard image
/// and for unrecognized `VARIANT_ID` values. `VARIANT_ID` is preferred;
/// `VARIANT` is used only when it is already a known Elavo display name.
pub fn edition_label_from_os_release(text: &str) -> Option<&'static str> {
    if let Some(id) = parse_os_release(text, "VARIANT_ID") {
        return match id.as_str() {
            "nvidia-legacy" => Some("NVIDIA Legacy"),
            "nvidia-open" => Some("NVIDIA Open"),
            _ => None,
        };
    }
    match parse_os_release(text, "VARIANT").as_deref() {
        Some("NVIDIA Legacy") => Some("NVIDIA Legacy"),
        Some("NVIDIA Open") => Some("NVIDIA Open"),
        _ => None,
    }
}

/// The version, with the edition in parentheses when this is an NVIDIA
/// image. About prefixes this with the translated "Version" word.
pub fn version_suffix(text: &str) -> Option<String> {
    let version = parse_os_release(text, "VERSION").filter(|v| !v.is_empty())?;
    Some(match edition_label_from_os_release(text) {
        Some(edition) => format!("{version} ({edition})"),
        None => version,
    })
}

pub fn cpu() -> Option<String> {
    let text = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    let name = text.lines().find_map(|line| {
        let rest = line.strip_prefix("model name")?;
        let name = rest.trim_start_matches([' ', ':', '\t']);
        (!name.is_empty()).then(|| name.to_owned())
    })?;
    let cores = text.lines().filter(|l| l.starts_with("processor")).count();
    Some(if cores > 1 {
        format!("{name} ({cores} threads)")
    } else {
        name
    })
}

/// Total memory in kibibytes.
pub fn memory_kib() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    text.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

pub fn memory() -> Option<String> {
    memory_kib().map(|kb| format!("{} GB", (kb as f64 / 1_048_576.0).round() as u64))
}

pub fn kernel() -> Option<String> {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .ok()
        .map(|s| s.trim().to_owned())
}

/// `lspci -mm`: VGA, 3D, and display controllers, by marketing name.
pub fn gpus() -> Vec<String> {
    let Ok(output) = std::process::Command::new("lspci").arg("-mm").output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    parse_lspci(&String::from_utf8_lossy(&output.stdout))
}

pub fn parse_lspci(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    for line in text.lines() {
        let fields = quoted_fields(line);
        let class = fields.first().map(String::as_str).unwrap_or("");
        if !(class.contains("VGA") || class.contains("3D") || class.contains("Display")) {
            continue;
        }
        let Some(device) = fields.get(2).filter(|n| !n.is_empty()) else {
            continue;
        };
        let vendor = fields.get(1).map(|v| short_vendor(v)).unwrap_or_default();
        let device = marketing_name(device);
        names.push(if vendor.is_empty() || device.starts_with(&vendor) {
            device.to_owned()
        } else {
            format!("{vendor} {device}")
        });
    }
    names
}

fn short_vendor(vendor: &str) -> String {
    if vendor.contains("Intel") {
        "Intel".into()
    } else if vendor.contains("AMD") || vendor.contains("ATI") {
        "AMD".into()
    } else if vendor.contains("NVIDIA") {
        "NVIDIA".into()
    } else {
        vendor.split_whitespace().next().unwrap_or("").to_owned()
    }
}

fn marketing_name(name: &str) -> &str {
    name.rsplit_once('[')
        .and_then(|(_, rest)| rest.strip_suffix(']'))
        .filter(|inner| !inner.trim().is_empty())
        .map(str::trim)
        .unwrap_or(name)
}

fn quoted_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut rest = line;
    while let Some(start) = rest.find('"') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('"') else {
            break;
        };
        fields.push(rest[..end].to_owned());
        rest = &rest[end + 1..];
    }
    fields
}

/// Free and total bytes of the file system holding `path`.
pub fn storage(path: &str) -> Option<(u64, u64)> {
    let c = std::ffi::CString::new(path).ok()?;
    let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(c.as_ptr(), &mut s) } != 0 {
        return None;
    }
    let block = s.f_frsize;
    Some((s.f_bavail * block, s.f_blocks * block))
}

pub fn bytes_label(bytes: u64) -> String {
    let gb = bytes as f64 / 1e9;
    if gb >= 1000.0 {
        format!("{:.1} TB", gb / 1000.0)
    } else {
        format!("{} GB", gb.round() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lspci_names() {
        let text = "00:02.0 \"VGA compatible controller\" \"Intel Corporation\" \"WhiskeyLake-U GT2 [UHD Graphics 620]\" -r02 \"Dell\" \"Device 08e1\"\n00:1f.3 \"Audio device\" \"Intel Corporation\" \"Cannon Point\"\n01:00.0 \"3D controller\" \"NVIDIA Corporation\" \"GP108M [GeForce MX150]\"";
        assert_eq!(
            parse_lspci(text),
            vec![
                "Intel UHD Graphics 620".to_owned(),
                "NVIDIA GeForce MX150".to_owned()
            ]
        );
    }

    #[test]
    fn os_release_values() {
        let text = "NAME=\"Elavo\"\nVERSION_ID=44\nIMAGE_ID='elavo'\n";
        assert_eq!(parse_os_release(text, "NAME").as_deref(), Some("Elavo"));
        assert_eq!(parse_os_release(text, "VERSION_ID").as_deref(), Some("44"));
        assert_eq!(parse_os_release(text, "IMAGE_ID").as_deref(), Some("elavo"));
        assert_eq!(parse_os_release(text, "MISSING"), None);
    }

    #[test]
    fn version_suffix_adds_known_editions() {
        assert_eq!(
            version_suffix(
                "NAME=\"Elavo\"\nVERSION=\"0.3.4.1\"\nVARIANT=\"NVIDIA Legacy\"\nVARIANT_ID=nvidia-legacy\n"
            )
            .as_deref(),
            Some("0.3.4.1 (NVIDIA Legacy)")
        );
        // VARIANT_ID wins, even when VARIANT still says "NVIDIA".
        assert_eq!(
            version_suffix(
                "NAME=\"Elavo\"\nVERSION=\"0.3.4.1\"\nVARIANT=\"NVIDIA\"\nVARIANT_ID=nvidia-open\n"
            )
            .as_deref(),
            Some("0.3.4.1 (NVIDIA Open)")
        );
        assert_eq!(
            version_suffix("NAME=\"Elavo\"\nVERSION=\"0.3.4.1\"\n").as_deref(),
            Some("0.3.4.1")
        );
        assert_eq!(
            version_suffix(
                "NAME=\"Elavo\"\nVERSION=\"0.3.4.1\"\nVARIANT=\"Workstation\"\nVARIANT_ID=workstation\n"
            )
            .as_deref(),
            Some("0.3.4.1")
        );
        assert_eq!(
            version_suffix("NAME=\"Elavo\"\nVERSION=\"0.3.4.1\"\nVARIANT=\"NVIDIA Legacy\"\n")
                .as_deref(),
            Some("0.3.4.1 (NVIDIA Legacy)")
        );
        assert_eq!(
            version_suffix("NAME=\"Elavo\"\nVERSION=\"0.3.4.1\"\nVARIANT_ID=unknown\n").as_deref(),
            Some("0.3.4.1")
        );
        assert_eq!(
            version_suffix("NAME=\"Elavo\"\nVARIANT_ID=nvidia-open\n"),
            None
        );
        assert_eq!(
            edition_label_from_os_release("VARIANT_ID=nvidia-legacy\n"),
            Some("NVIDIA Legacy")
        );
        assert_eq!(
            edition_label_from_os_release("VARIANT_ID=nvidia-open\n"),
            Some("NVIDIA Open")
        );
        assert_eq!(edition_label_from_os_release("NAME=Elavo\n"), None);
    }

    #[test]
    fn byte_labels() {
        assert_eq!(bytes_label(512_000_000_000), "512 GB");
        assert_eq!(bytes_label(2_000_000_000_000), "2.0 TB");
    }
}
