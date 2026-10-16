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
}
