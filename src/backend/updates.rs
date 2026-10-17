//! Image updates. The deployments are read from `rpm-ostree status --json`,
//! which needs no password; checking, downloading, switching channel, and
//! rolling back go through the helper, which calls `bootc`.

use gtk::gio;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Deployment {
    /// `ghcr.io/elavo-io/elavo:stable`, without the `ostree-...:` transport.
    pub image: String,
    pub digest: String,
    pub version: String,
    pub timestamp: i64,
    pub booted: bool,
    pub staged: bool,
}

impl Deployment {
    pub fn channel(&self) -> Option<&str> {
        channel_of(&self.image)
    }

    /// The last 12 hex digits of the digest, the length people compare.
    pub fn short_digest(&self) -> String {
        let hex = self.digest.strip_prefix("sha256:").unwrap_or(&self.digest);
        hex.chars()
            .rev()
            .take(12)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub booted: Option<Deployment>,
    pub staged: Option<Deployment>,
    pub rollback: Option<Deployment>,
}

/// `ostree-image-signed:docker://ghcr.io/o/elavo:stable` to
/// `ghcr.io/o/elavo:stable`.
pub fn image_name(reference: &str) -> String {
    let mut rest = reference;
    if let Some(after) = rest.strip_prefix("ostree-") {
        rest = after.split_once(':').map(|(_, r)| r).unwrap_or(after);
    }
    for transport in ["docker://", "registry:"] {
        if let Some(after) = rest.strip_prefix(transport) {
            rest = after;
        }
    }
    rest.to_owned()
}

/// The tag, when it is one of the two channels.
pub fn channel_of(image: &str) -> Option<&str> {
    let last = image.rsplit('/').next()?;
    let (_, tag) = last.split_once(':')?;
    matches!(tag, "stable" | "latest").then_some(tag)
}

pub fn parse(json: &str) -> Status {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Status::default();
    };
    let Some(list) = value.get("deployments").and_then(|d| d.as_array()) else {
        return Status::default();
    };
    let mut status = Status::default();
    for d in list {
        let s = |key: &str| d.get(key).and_then(|v| v.as_str()).unwrap_or("").to_owned();
        let b = |key: &str| d.get(key).and_then(|v| v.as_bool()).unwrap_or(false);
        let dep = Deployment {
            image: image_name(&s("container-image-reference")),
            digest: s("container-image-reference-digest"),
            version: s("version"),
            timestamp: d.get("timestamp").and_then(|v| v.as_i64()).unwrap_or(0),
            booted: b("booted"),
            staged: b("staged"),
        };
        if dep.booted {
            status.booted = Some(dep);
        } else if dep.staged || (status.booted.is_none() && status.staged.is_none()) {
            // Deployments are listed newest first: one ahead of the booted
            // one is waiting for a restart.
            status.staged = Some(dep);
        } else if status.rollback.is_none() {
            status.rollback = Some(dep);
        }
    }
    status
}

const REVISION_FILE: &str = "/usr/lib/elavo-image-revision";
const PROGRESS_PREFIX: &str = "ELAVO_PROGRESS ";

/// Bytes transferred and the bytes still to fetch. `bytes_total` is the
/// download, not the whole image: bootc has already subtracted what this
/// computer has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Download {
    pub bytes: u64,
    pub bytes_total: u64,
}

/// One bootc progress line, when it carries a known download size.
pub fn download_progress(line: &str) -> Option<Download> {
    let json = line.strip_prefix(PROGRESS_PREFIX)?;
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    if value.get("type").and_then(|t| t.as_str()) != Some("ProgressBytes") {
        return None;
    }
    let bytes_total = value.get("bytesTotal").and_then(|n| n.as_u64())?;
    if bytes_total == 0 {
        return None;
    }
    let bytes = value.get("bytes").and_then(|n| n.as_u64()).unwrap_or(0);
    Some(Download {
        bytes: bytes.min(bytes_total),
        bytes_total,
    })
}

/// `840 MB`. Decimal, the sizes people compare with a download.
pub fn format_bytes(bytes: u64) -> String {
    const THOUSAND: f64 = 1000.0;
    let n = bytes as f64;
    if n >= THOUSAND.powi(3) {
        format!("{:.1} GB", n / THOUSAND.powi(3))
    } else if n >= THOUSAND.powi(2) {
        format!("{:.0} MB", n / THOUSAND.powi(2))
    } else if n >= THOUSAND {
        format!("{:.0} KB", n / THOUSAND)
    } else {
        format!("{bytes} B")
    }
}

/// Seconds left, once a rate exists. The first two seconds are too short
/// to trust.
pub fn seconds_left(elapsed_secs: f64, bytes: u64, total: u64) -> Option<u64> {
    if elapsed_secs < 2.0 || bytes == 0 || bytes >= total {
        return None;
    }
    let rate = bytes as f64 / elapsed_secs;
    Some(((total - bytes) as f64 / rate).round() as u64)
}

pub fn format_remaining(seconds: u64) -> String {
    if seconds < 90 {
        format!("{} {}", seconds.max(1), crate::i18n::tr("seconds left"))
    } else {
        format!(
            "{} {}",
            seconds.div_ceil(60),
            crate::i18n::tr("minutes left")
        )
    }
}

/// The commit this running image was built from.
pub fn booted_revision() -> Option<String> {
    let text = std::fs::read_to_string(REVISION_FILE).ok()?;
    let revision = text.trim();
    revision_sha(revision).map(str::to_owned)
}

fn revision_sha(revision: &str) -> Option<&str> {
    let sha = revision.strip_suffix("-dirty").unwrap_or(revision);
    (sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit())).then_some(sha)
}

/// The same image pinned to a digest, so a staged build is not confused
/// with wherever its tag has moved since.
pub fn at_digest(image: &str, digest: &str) -> String {
    if digest.is_empty() {
        return image.to_owned();
    }
    let name = image.split('@').next().unwrap_or(image);
    let name = name.rsplit_once(':').map(|(name, _)| name).unwrap_or(name);
    format!("{name}@{digest}")
}

/// `ghcr.io/elavo-io/elavo:latest` to `ghcr.io/elavo-io/elavo:stable`.
pub fn with_tag(image: &str, tag: &str) -> Option<String> {
    let image = image.split('@').next()?;
    let (repo, last) = image.rsplit_once('/').unwrap_or(("", image));
    let name = last.split(':').next()?;
    if name.is_empty() {
        return None;
    }
    Some(if repo.is_empty() {
        format!("{name}:{tag}")
    } else {
        format!("{repo}/{name}:{tag}")
    })
}

/// `ghcr.io/elavo-io/elavo:latest` to `elavo-io/elavo`.
fn github_repo(image: &str) -> Option<String> {
    let rest = image.strip_prefix("ghcr.io/")?;
    let (org, name) = rest.split_once('/')?;
    let name = name.split([':', '@']).next()?;
    (!org.is_empty() && !name.is_empty()).then(|| format!("{org}/{name}"))
}

async fn command_text(argv: &[&str]) -> Option<String> {
    let argv: Vec<&std::ffi::OsStr> = argv.iter().map(|a| std::ffi::OsStr::new(*a)).collect();
    let process = gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    )
    .ok()?;
    let (stdout, _) = process.communicate_utf8_future(None).await.ok()?;
    process
        .is_successful()
        .then(|| stdout.unwrap_or_default().to_string())
}

/// The commit recorded on a published image. Absent on images built
/// before that label existed.
pub async fn image_revision(image: &str) -> Option<String> {
    let reference = format!("docker://{image}");
    let text = command_text(&["skopeo", "inspect", "--no-tags", &reference]).await?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let revision = value
        .pointer("/Labels/org.opencontainers.image.revision")
        .and_then(|v| v.as_str())?;
    revision_sha(revision).map(str::to_owned)
}

pub async fn tag_exists(image: &str) -> bool {
    let reference = format!("docker://{image}");
    command_text(&["skopeo", "inspect", "--no-tags", &reference])
        .await
        .is_some()
}

/// Commit subjects between the running image and `to`, newest last as
/// GitHub returns them. Empty when either revision is missing.
pub fn format_notes(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    let commits = value.get("commits")?.as_array()?;
    let subjects: Vec<&str> = commits
        .iter()
        .filter_map(|c| c.pointer("/commit/message").and_then(|m| m.as_str()))
        .filter_map(|m| m.lines().next().map(str::trim))
        .filter(|s| !s.is_empty())
        .collect();
    if subjects.is_empty() {
        return None;
    }
    const SHOWN: usize = 12;
    let mut lines: Vec<String> = subjects
        .iter()
        .take(SHOWN)
        .map(|s| (*s).to_owned())
        .collect();
    if subjects.len() > SHOWN {
        lines.push(format!(
            "{} {}",
            subjects.len() - SHOWN,
            crate::i18n::tr("more changes")
        ));
    }
    Some(lines.join("\n"))
}

/// What's new in `image` since this computer's build.
pub async fn changelog(image: &str) -> Option<String> {
    let from = booted_revision()?;
    let to = image_revision(image).await?;
    if from == to {
        return None;
    }
    let repo = github_repo(image)?;
    let url = format!("https://api.github.com/repos/{repo}/compare/{from}...{to}");
    let text = command_text(&[
        "curl",
        "-fsSL",
        "-H",
        "Accept: application/vnd.github+json",
        "-H",
        "User-Agent: elavo-system-settings",
        &url,
    ])
    .await?;
    format_notes(&text)
}

pub async fn status() -> Result<Status, String> {
    let process = gio::Subprocess::newv(
        &["rpm-ostree".as_ref(), "status".as_ref(), "--json".as_ref()],
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    )
    .map_err(|e| e.to_string())?;
    let (stdout, _) = process
        .communicate_utf8_future(None)
        .await
        .map_err(|e| e.to_string())?;
    if !process.is_successful() {
        return Err(crate::i18n::tr(
            "This computer is not running an Elavo image.",
        ));
    }
    Ok(parse(stdout.as_deref().unwrap_or("")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "deployments": [
        { "booted": false, "staged": true, "version": "44.20261016",
          "container-image-reference": "ostree-image-signed:docker://ghcr.io/elavo-io/elavo:stable",
          "container-image-reference-digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa0123456789ab",
          "timestamp": 1760600000 },
        { "booted": true, "version": "44.20261010",
          "container-image-reference": "ostree-image-signed:docker://ghcr.io/elavo-io/elavo:stable",
          "container-image-reference-digest": "sha256:bbbb",
          "timestamp": 1760000000 },
        { "booted": false, "version": "44.20261001",
          "container-image-reference": "ostree-unverified-registry:ghcr.io/elavo-io/elavo:latest",
          "container-image-reference-digest": "sha256:cccc" }
      ]
    }"#;

    #[test]
    fn parses_three_deployments() {
        let s = parse(SAMPLE);
        let booted = s.booted.unwrap();
        assert_eq!(booted.image, "ghcr.io/elavo-io/elavo:stable");
        assert_eq!(booted.channel(), Some("stable"));
        let staged = s.staged.unwrap();
        assert_eq!(staged.version, "44.20261016");
        assert_eq!(staged.short_digest(), "0123456789ab");
        let rollback = s.rollback.unwrap();
        assert_eq!(rollback.image, "ghcr.io/elavo-io/elavo:latest");
        assert_eq!(rollback.channel(), Some("latest"));
    }

    #[test]
    fn image_names() {
        assert_eq!(
            image_name("ghcr.io/o/elavo:stable"),
            "ghcr.io/o/elavo:stable"
        );
        assert_eq!(
            image_name("ostree-unverified-registry:ghcr.io/o/elavo:latest"),
            "ghcr.io/o/elavo:latest"
        );
        assert_eq!(channel_of("ghcr.io/o/elavo:42"), None);
        assert_eq!(channel_of("localhost:5000/elavo"), None);
    }

    #[test]
    fn bad_json() {
        assert_eq!(parse("nope"), Status::default());
    }

    #[test]
    fn progress_line_is_the_download() {
        let line = r#"ELAVO_PROGRESS {"type":"ProgressBytes","bytes":500,"bytesTotal":2000,"bytesCached":0}"#;
        let got = download_progress(line).unwrap();
        assert_eq!(got.bytes, 500);
        assert_eq!(got.bytes_total, 2000);
        assert!(download_progress("No changes in ghcr.io/elavo").is_none());
        assert!(
            download_progress(r#"ELAVO_PROGRESS {"type":"Start","version":"0.1.0"}"#).is_none()
        );
        assert!(download_progress(
            r#"ELAVO_PROGRESS {"type":"ProgressBytes","bytes":1,"bytesTotal":0}"#
        )
        .is_none());
    }

    #[test]
    fn bytes_and_time() {
        assert_eq!(format_bytes(840_000_000), "840 MB");
        assert_eq!(format_bytes(1_500_000_000), "1.5 GB");
        assert_eq!(seconds_left(1.0, 100, 1000), None);
        assert_eq!(seconds_left(2.0, 500, 1000), Some(2));
        assert_eq!(seconds_left(4.0, 1000, 1000), None);
    }

    #[test]
    fn notes_are_commit_subjects() {
        let json = r#"{
            "commits": [
                {"commit": {"message": "First change\n\nThe body."}},
                {"commit": {"message": "Second change"}}
            ]
        }"#;
        assert_eq!(
            format_notes(json).as_deref(),
            Some("First change\nSecond change")
        );
        assert_eq!(format_notes("{}"), None);
    }

    #[test]
    fn tag_swap() {
        assert_eq!(
            with_tag("ghcr.io/elavo-io/elavo:latest", "stable").as_deref(),
            Some("ghcr.io/elavo-io/elavo:stable")
        );
        assert_eq!(
            at_digest(
                "ghcr.io/elavo-io/elavo:stable",
                "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            ),
            "ghcr.io/elavo-io/elavo@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }
}
