//! Extra packages and other changes layered on an image, read from
//! `rpm-ostree status --json`. The privileged helper includes this file
//! too, so it has no GTK types.

/// Packages, local overlays, replacements, and removals on a deployment.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layered {
    pub packages: Vec<String>,
    pub local_packages: Vec<String>,
    pub local_fileoverrides: Vec<String>,
    pub replacements: Vec<String>,
    pub removals: Vec<String>,
}

impl Layered {
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
            && self.local_packages.is_empty()
            && self.local_fileoverrides.is_empty()
            && self.replacements.is_empty()
            && self.removals.is_empty()
    }

    /// Names to show the person, packages first, unique, in stable order.
    pub fn names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for group in [
            &self.packages,
            &self.local_packages,
            &self.local_fileoverrides,
            &self.replacements,
            &self.removals,
        ] {
            for name in group {
                if !name.is_empty() && !names.iter().any(|n| n == name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    /// Overlay and local packages `rpm-ostree upgrade --uninstall=` can drop.
    #[allow(dead_code)]
    pub fn uninstall_packages(&self) -> Vec<String> {
        let mut names = Vec::new();
        for group in [&self.packages, &self.local_packages] {
            for name in group {
                if !name.is_empty() && !names.iter().any(|n| n == name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    /// Replacements, removals, and file overrides need `rpm-ostree reset`.
    #[allow(dead_code)]
    pub fn needs_reset(&self) -> bool {
        !self.replacements.is_empty()
            || !self.removals.is_empty()
            || !self.local_fileoverrides.is_empty()
    }
}

/// Layered changes on the booted deployment.
#[allow(dead_code)]
pub fn parse_booted(json: &str) -> Layered {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
        return Layered::default();
    };
    let Some(list) = value.get("deployments").and_then(|d| d.as_array()) else {
        return Layered::default();
    };
    let Some(booted) = list
        .iter()
        .find(|d| d.get("booted").and_then(|v| v.as_bool()).unwrap_or(false))
    else {
        return Layered::default();
    };
    parse_deployment(booted)
}

pub fn parse_deployment(d: &serde_json::Value) -> Layered {
    Layered {
        packages: prefer(d, "requested-packages", "packages"),
        local_packages: strings(d.get("requested-local-packages")),
        local_fileoverrides: strings(d.get("requested-local-fileoverride-packages")),
        replacements: prefer(
            d,
            "requested-base-local-replacements",
            "base-local-replacements",
        ),
        removals: prefer(d, "requested-base-removals", "base-removals"),
    }
}

fn prefer(d: &serde_json::Value, first: &str, fallback: &str) -> Vec<String> {
    let from_first = strings(d.get(first));
    if from_first.is_empty() {
        strings(d.get(fallback))
    } else {
        from_first
    }
}

fn strings(value: Option<&serde_json::Value>) -> Vec<String> {
    let Some(arr) = value.and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in arr {
        if let Some(s) = item.as_str() {
            if !s.is_empty() {
                out.push(s.to_owned());
            }
        } else if let Some(pair) = item.as_array() {
            // rpm-ostree records a replacement as [from, to].
            if let Some(s) = pair
                .iter()
                .find_map(|x| x.as_str())
                .filter(|s| !s.is_empty())
            {
                out.push(s.to_owned());
            }
        }
    }
    out
}

/// Package names safe to pass as `rpm-ostree --uninstall=`. Rejects flags.
#[allow(dead_code)]
pub fn is_safe_package(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-' | ':' | '~'))
}

pub fn error_mentions_layers(text: &str) -> bool {
    let t = text.to_ascii_lowercase();
    t.contains("layered")
        || t.contains("overlayed")
        || t.contains("overlay package")
        || t.contains("local rpm-ostree modifications")
        || t.contains("rpm-ostree reset")
}

/// Join names as "uxplay", "uxplay and foo", or "uxplay, foo, and bar".
pub fn join_names(names: &[String], and: &str) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [a, b] => format!("{a} {and} {b}"),
        _ => {
            let (last, rest) = names.split_last().unwrap();
            format!("{}, {and} {last}", rest.join(", "))
        }
    }
}

/// Templates for the error dialog body. `one` and `many` include `{}`.
pub struct ErrorCopy<'a> {
    pub one: &'a str,
    pub many: &'a str,
    pub none: &'a str,
    pub reason: &'a str,
    pub and: &'a str,
}

pub fn error_detail(names: &[String], copy: ErrorCopy<'_>) -> String {
    let intro = match names {
        [] => copy.none.to_owned(),
        [name] => copy.one.replacen("{}", name, 1),
        _ => copy.many.replacen("{}", &join_names(names, copy.and), 1),
    };
    format!("{} {}", intro, copy.reason)
}

/// Templates for the confirmation body. `one` includes `{}`.
pub struct ConfirmCopy<'a> {
    pub one: &'a str,
    pub many_intro: &'a str,
    pub many_end: &'a str,
    pub none: &'a str,
}

pub fn confirm_detail(names: &[String], copy: ConfirmCopy<'_>) -> String {
    match names {
        [] => copy.none.to_owned(),
        [name] => copy.one.replacen("{}", name, 1),
        _ => format!(
            "{}\n\n{}\n\n{}",
            copy.many_intro,
            names.join("\n"),
            copy.many_end
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAYERED: &str = r#"{
      "deployments": [
        {
          "booted": true,
          "requested-packages": ["uxplay"],
          "packages": ["uxplay"],
          "requested-local-packages": [],
          "requested-local-fileoverride-packages": [],
          "requested-base-local-replacements": [],
          "requested-base-removals": []
        },
        { "booted": false, "packages": ["other"] }
      ]
    }"#;

    const CHANGES: &str = r#"{
      "deployments": [
        {
          "booted": true,
          "requested-packages": ["uxplay", "foo"],
          "requested-local-packages": ["bar-1.0-1.x86_64"],
          "requested-local-fileoverride-packages": ["local.rpm"],
          "requested-base-local-replacements": [
            ["mesa-24.0-1.x86_64", "mesa-24.1-1.x86_64"]
          ],
          "requested-base-removals": ["firefox"]
        }
      ]
    }"#;

    fn error_copy() -> ErrorCopy<'static> {
        ErrorCopy {
            one: "{} is installed on top of Elavo, so the update could not be applied.",
            many: "These packages are installed on top of Elavo, so the update could not be applied: {}.",
            none: "Extra software installed on top of Elavo stopped the update from applying.",
            reason: "Elavo updates the whole system image at once, and extra packages added later have to be removed first.",
            and: "and",
        }
    }

    fn confirm_copy() -> ConfirmCopy<'static> {
        ConfirmCopy {
            one: "You will lose access to {}. The update installs now and starts after a restart.",
            many_intro: "You will lose access to these apps:",
            many_end: "The update installs now and starts after a restart.",
            none: "This will remove the extra software installed on top of Elavo, then install the update. The update starts after a restart.",
        }
    }

    #[test]
    fn parses_uxplay_from_the_booted_deployment() {
        let layered = parse_booted(LAYERED);
        assert_eq!(layered.packages, ["uxplay"]);
        assert_eq!(layered.names(), ["uxplay"]);
        assert!(!layered.needs_reset());
        assert_eq!(layered.uninstall_packages(), ["uxplay"]);
        assert!(parse_booted("nope").is_empty());
    }

    #[test]
    fn parses_local_packages_overrides_and_removals() {
        let layered = parse_booted(CHANGES);
        assert_eq!(layered.packages, ["uxplay", "foo"]);
        assert_eq!(layered.local_packages, ["bar-1.0-1.x86_64"]);
        assert_eq!(layered.local_fileoverrides, ["local.rpm"]);
        assert_eq!(layered.replacements, ["mesa-24.0-1.x86_64"]);
        assert_eq!(layered.removals, ["firefox"]);
        assert!(layered.needs_reset());
        assert_eq!(
            layered.names(),
            [
                "uxplay",
                "foo",
                "bar-1.0-1.x86_64",
                "local.rpm",
                "mesa-24.0-1.x86_64",
                "firefox"
            ]
        );
        assert_eq!(
            layered.uninstall_packages(),
            ["uxplay", "foo", "bar-1.0-1.x86_64"]
        );
    }

    #[test]
    fn joins_names_with_and() {
        assert_eq!(join_names(&[], "and"), "");
        assert_eq!(join_names(&["uxplay".into()], "and"), "uxplay");
        assert_eq!(
            join_names(&["uxplay".into(), "foo".into()], "and"),
            "uxplay and foo"
        );
        assert_eq!(
            join_names(&["uxplay".into(), "foo".into(), "bar".into()], "and"),
            "uxplay, foo, and bar"
        );
    }

    #[test]
    fn error_and_confirm_copy() {
        let one = error_detail(&["uxplay".into()], error_copy());
        assert_eq!(
            one,
            "uxplay is installed on top of Elavo, so the update could not be applied. Elavo updates the whole system image at once, and extra packages added later have to be removed first."
        );
        let many = error_detail(&["uxplay".into(), "foo".into()], error_copy());
        assert_eq!(
            many,
            "These packages are installed on top of Elavo, so the update could not be applied: uxplay and foo. Elavo updates the whole system image at once, and extra packages added later have to be removed first."
        );
        let none = error_detail(&[], error_copy());
        assert!(none.starts_with("Extra software installed on top of Elavo"));
        assert_eq!(
            confirm_detail(&["uxplay".into()], confirm_copy()),
            "You will lose access to uxplay. The update installs now and starts after a restart."
        );
        assert_eq!(
            confirm_detail(&["uxplay".into(), "foo".into()], confirm_copy()),
            "You will lose access to these apps:\n\nuxplay\nfoo\n\nThe update installs now and starts after a restart."
        );
    }

    #[test]
    fn detects_bootc_layer_errors() {
        assert!(error_mentions_layers(
            "Deployment contains local rpm-ostree modifications; cannot upgrade via bootc. You can run `rpm-ostree reset` to undo the modifications."
        ));
        assert!(!error_mentions_layers("bootc: error: no network"));
    }

    #[test]
    fn rejects_unsafe_package_names() {
        assert!(is_safe_package("uxplay"));
        assert!(is_safe_package("bar-1.0-1.x86_64"));
        assert!(!is_safe_package(""));
        assert!(!is_safe_package("--uninstall=evil"));
        assert!(!is_safe_package("-foo"));
        assert!(!is_safe_package("foo bar"));
        assert!(!is_safe_package("foo;rm"));
    }
}
