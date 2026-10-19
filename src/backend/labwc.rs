//! labwc configuration. The session runs `labwc --merge-config`, so the
//! system files in `/etc/xdg/labwc` stay in effect and the user's
//! `~/.config/labwc` holds only what the person changed. Settings edits the
//! user files in place: it changes the elements it owns and leaves every
//! other byte (keybinds someone added by hand, comments) alone.

use std::path::PathBuf;

use gtk::glib;

pub const SYSTEM_DIR: &str = "/etc/xdg/labwc";
const EMPTY: &str = "<?xml version=\"1.0\"?>\n<labwc_config>\n</labwc_config>\n";
const ROOT: &str = "labwc_config";

pub fn user_dir() -> PathBuf {
    glib::user_config_dir().join("labwc")
}

pub fn user_rc() -> PathBuf {
    user_dir().join("rc.xml")
}

pub fn system_rc() -> PathBuf {
    PathBuf::from(SYSTEM_DIR).join("rc.xml")
}

/// One step of a path below `<labwc_config>`, with an optional attribute
/// the element must carry.
#[derive(Clone, Copy, Debug)]
pub struct Seg<'a> {
    pub name: &'a str,
    pub attr: Option<(&'a str, &'a str)>,
}

pub const fn seg(name: &str) -> Seg<'_> {
    Seg { name, attr: None }
}

pub const fn seg_with<'a>(name: &'a str, key: &'a str, value: &'a str) -> Seg<'a> {
    Seg {
        name,
        attr: Some((key, value)),
    }
}

fn matches(node: &roxmltree::Node, seg: &Seg) -> bool {
    node.is_element()
        && node.tag_name().name().eq_ignore_ascii_case(seg.name)
        && seg
            .attr
            .is_none_or(|(k, v)| node.attribute(k).is_some_and(|a| a.eq_ignore_ascii_case(v)))
}

fn find<'a, 'i>(
    doc: &'a roxmltree::Document<'i>,
    path: &[Seg],
) -> (roxmltree::Node<'a, 'i>, usize) {
    let mut node = doc.root_element();
    for (i, seg) in path.iter().enumerate() {
        // The last match wins, as it does when labwc reads the file.
        match node.children().rfind(|c| matches(c, seg)) {
            Some(child) => node = child,
            None => return (node, i),
        }
    }
    (node, path.len())
}

/// The text of the element at `path`, if the document has it.
pub fn get(doc: &str, path: &[Seg]) -> Option<String> {
    let doc = roxmltree::Document::parse(doc).ok()?;
    if !doc.root_element().has_tag_name(ROOT) {
        return None;
    }
    let (node, depth) = find(&doc, path);
    (depth == path.len()).then(|| node.text().unwrap_or("").trim().to_owned())
}

/// Get a value from the user file first, then the system file.
pub fn get_merged(path: &[Seg]) -> Option<String> {
    let user = std::fs::read_to_string(user_rc()).ok();
    let system = std::fs::read_to_string(system_rc()).ok();
    user.as_deref()
        .and_then(|d| get(d, path))
        .or_else(|| system.as_deref().and_then(|d| get(d, path)))
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The byte offset just past the `>` that closes the start tag at the
/// beginning of `text`, skipping `>` inside quoted attribute values.
fn start_tag_end(text: &str) -> Option<usize> {
    let mut quote = None;
    for (i, c) in text.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), c) if c == q => quote = None,
            (None, '>') => return Some(i + 1),
            _ => {}
        }
    }
    None
}

fn open_tag(seg: &Seg) -> String {
    match seg.attr {
        Some((k, v)) => format!("<{} {}=\"{}\">", seg.name, k, escape(v)),
        None => format!("<{}>", seg.name),
    }
}

fn snippet(path: &[Seg], value: &str, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    match path {
        [] => String::new(),
        [leaf] => format!(
            "{indent}{}{}</{}>",
            open_tag(leaf),
            escape(value),
            leaf.name
        ),
        [first, rest @ ..] => format!(
            "{indent}{}\n{}\n{indent}</{}>",
            open_tag(first),
            snippet(rest, value, depth + 1),
            first.name
        ),
    }
}

/// Set the element at `path` to `value`, creating the elements that are
/// missing. Returns the new document.
pub fn set(doc: &str, path: &[Seg], value: &str) -> Result<String, String> {
    let source = if doc.trim().is_empty() { EMPTY } else { doc };
    let parsed = roxmltree::Document::parse(source).map_err(|e| e.to_string())?;
    let root = parsed.root_element();
    if !root.has_tag_name(ROOT) {
        return Err(format!("the root element is not <{ROOT}>"));
    }
    let (node, depth) = find(&parsed, path);
    let range = node.range();
    let text = &source[range.clone()];
    let self_closing = text.ends_with("/>");

    if depth == path.len() {
        let leaf = path.last().ok_or("empty path")?;
        let replacement = if self_closing {
            let open = text[..text.len() - 2].trim_end();
            format!("{open}>{}</{}>", escape(value), leaf.name)
        } else {
            let open_end = start_tag_end(text).ok_or("unterminated tag")?;
            let close_start = text.rfind("</").ok_or("missing end tag")?;
            format!(
                "{}{}{}",
                &text[..open_end],
                escape(value),
                &text[close_start..]
            )
        };
        return Ok(format!(
            "{}{}{}",
            &source[..range.start],
            replacement,
            &source[range.end..]
        ));
    }

    let node_indent = "  ".repeat(depth);
    let insert = snippet(&path[depth..], value, depth + 1);
    if self_closing {
        let open = text[..text.len() - 2].trim_end();
        let name = node.tag_name().name();
        let replacement = format!("{open}>\n{insert}\n{node_indent}</{name}>");
        return Ok(format!(
            "{}{}{}",
            &source[..range.start],
            replacement,
            &source[range.end..]
        ));
    }
    let close_start = range.start + text.rfind("</").ok_or("missing end tag")?;
    let before = &source[..close_start];
    let line_start = before.trim_end_matches([' ', '\t']);
    if line_start.ends_with('\n') {
        Ok(format!(
            "{line_start}{insert}\n{node_indent}{}",
            &source[close_start..]
        ))
    } else {
        Ok(format!(
            "{before}\n{insert}\n{node_indent}{}",
            &source[close_start..]
        ))
    }
}

/// Set several values in the user `rc.xml`, then ask labwc to reload.
pub fn write_user(values: &[(&[Seg], String)]) -> Result<(), String> {
    let path = user_rc();
    let mut doc = std::fs::read_to_string(&path).unwrap_or_default();
    for (segs, value) in values {
        doc = set(&doc, segs, value)?;
    }
    super::keyfile::write_atomic(&path, &doc).map_err(|e| e.to_string())?;
    super::process::reconfigure_labwc();
    Ok(())
}

// ----- environment -----

pub fn user_environment() -> PathBuf {
    user_dir().join("environment")
}

pub fn env_get(text: &str, key: &str) -> Option<String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| k.trim() == key)
        .map(|(_, v)| v.trim().to_owned())
        .next_back()
}

/// Replace `key` in an environment file, or append it. An empty value
/// removes the line.
pub fn env_set(text: &str, key: &str, value: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut done = false;
    for line in text.lines() {
        let is_key = line
            .split_once('=')
            .is_some_and(|(k, _)| k.trim() == key && !line.trim_start().starts_with('#'));
        if is_key {
            if !done && !value.is_empty() {
                out.push(format!("{key}={value}"));
            }
            done = true;
        } else {
            out.push(line.to_owned());
        }
    }
    if !done && !value.is_empty() {
        out.push(format!("{key}={value}"));
    }
    let mut text = out.join("\n");
    if !text.is_empty() {
        text.push('\n');
    }
    text
}

pub fn env_get_merged(key: &str) -> Option<String> {
    let user = std::fs::read_to_string(user_environment()).unwrap_or_default();
    let system =
        std::fs::read_to_string(PathBuf::from(SYSTEM_DIR).join("environment")).unwrap_or_default();
    env_get(&user, key).or_else(|| env_get(&system, key))
}

pub fn env_write_user(key: &str, value: &str) -> std::io::Result<()> {
    let path = user_environment();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    super::keyfile::write_atomic(&path, &env_set(&text, key, value))?;
    super::process::reconfigure_labwc();
    Ok(())
}

// ----- keyboard shortcuts -----

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keybind {
    pub key: String,
    pub action: String,
}

/// `W-S-s` to `Super+Shift+S`.
pub fn pretty_key(key: &str) -> String {
    let parts: Vec<&str> = key.split('-').collect();
    let mut out = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let last = i + 1 == parts.len();
        let name = match (*part, last) {
            ("W", false) => "Super".to_owned(),
            ("C", false) => "Ctrl".to_owned(),
            ("A", false) => "Alt".to_owned(),
            ("S", false) => "Shift".to_owned(),
            ("M", false) => "Meta".to_owned(),
            (p, _) if p.len() == 1 => p.to_ascii_uppercase(),
            (p, _) => p.to_owned(),
        };
        out.push(name);
    }
    out.join("+")
}

fn describe(action: &roxmltree::Node) -> String {
    let name = action.attribute("name").unwrap_or("");
    let command = action.attribute("command").unwrap_or("");
    if name.eq_ignore_ascii_case("Execute") {
        let known = [
            ("elavo-lens --toggle", "Open Lens"),
            ("elavo-lens", "Open Lens"),
            ("foot", "Open Terminal"),
            ("elavo-grim-region", "Screenshot of a region"),
            ("elavo-dock --activate", "Open dock item"),
            ("elavo-switcher --window-next", "Next window of this application"),
            ("elavo-switcher --window-previous", "Previous window of this application"),
            ("elavo-switcher --previous", "Previous application"),
            ("elavo-switcher --next", "Next application"),
            ("elavo-switcher --scoped", "Open the app switcher"),
            ("elavo-system-settings", "Open System Settings"),
        ];
        for (prefix, label) in known {
            if command.ends_with(prefix) || command.contains(prefix) {
                if prefix == "elavo-dock --activate" {
                    let n = command.rsplit(' ').next().unwrap_or("");
                    return format!("{label} {n}");
                }
                return label.to_owned();
            }
        }
        return format!("Run {command}");
    }
    if name.eq_ignore_ascii_case("GoToDesktop") {
        return match action.attribute("to") {
            Some("left") => "Previous desktop".into(),
            Some("right") => "Next desktop".into(),
            Some(n) => format!("Go to desktop {n}"),
            None => "Go to desktop".into(),
        };
    }
    if name.eq_ignore_ascii_case("If") || name.is_empty() {
        if let Some(inner) = action
            .descendants()
            .skip(1)
            .find(|n| n.has_tag_name("action"))
        {
            return describe(&inner);
        }
    }
    let mut words = String::new();
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            words.push(' ');
            words.push(c.to_ascii_lowercase());
        } else {
            words.push(c);
        }
    }
    words
}

pub fn keybinds(doc: &str) -> Vec<Keybind> {
    let Ok(doc) = roxmltree::Document::parse(doc) else {
        return Vec::new();
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("keybind"))
        .filter_map(|n| {
            let key = n.attribute("key")?;
            let action = n.children().find(|c| c.has_tag_name("action"))?;
            Some(Keybind {
                key: pretty_key(key),
                action: describe(&action),
            })
        })
        .collect()
}

/// System keybinds, then the user's; a user binding replaces a system one
/// on the same key.
pub fn keybinds_merged() -> Vec<Keybind> {
    let system = std::fs::read_to_string(system_rc()).unwrap_or_default();
    let user = std::fs::read_to_string(user_rc()).unwrap_or_default();
    let mut all = keybinds(&system);
    for bind in keybinds(&user) {
        all.retain(|b| b.key != bind.key);
        all.push(bind);
    }
    all
}

// ----- keyboard layouts -----

/// `(code, description)` pairs from the XKB rules list.
pub fn layouts() -> Vec<(String, String)> {
    let text = std::fs::read_to_string("/usr/share/X11/xkb/rules/evdev.lst")
        .or_else(|_| std::fs::read_to_string("/usr/share/xkeyboard-config-2/rules/evdev.lst"))
        .unwrap_or_default();
    parse_layouts(&text)
}

pub fn parse_layouts(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut in_layouts = false;
    for line in text.lines() {
        if let Some(section) = line.strip_prefix('!') {
            in_layouts = section.trim() == "layout";
            continue;
        }
        if !in_layouts {
            continue;
        }
        let line = line.trim();
        if let Some((code, desc)) = line.split_once(char::is_whitespace) {
            out.push((code.to_owned(), desc.trim().to_owned()));
        }
    }
    out.sort_by_key(|a| a.1.to_lowercase());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: &[Seg] = &[seg("keyboard"), seg("repeatRate")];
    const TOUCH_TAP: &[Seg] = &[
        seg("libinput"),
        seg_with("device", "category", "touchpad"),
        seg("tap"),
    ];

    #[test]
    fn creates_file_from_nothing() {
        let out = set("", RATE, "30").unwrap();
        assert_eq!(get(&out, RATE).as_deref(), Some("30"));
        assert!(out.starts_with("<?xml"));
        assert!(out.contains("<keyboard>\n    <repeatRate>30</repeatRate>\n  </keyboard>"));
    }

    #[test]
    fn updates_in_place_and_keeps_the_rest() {
        let doc = "<?xml version=\"1.0\"?>\n<labwc_config>\n  <!-- mine -->\n  <keyboard>\n    <keybind key=\"W-x\"><action name=\"Close\"/></keybind>\n    <repeatRate>25</repeatRate>\n  </keyboard>\n</labwc_config>\n";
        let out = set(doc, RATE, "40").unwrap();
        assert_eq!(get(&out, RATE).as_deref(), Some("40"));
        assert!(out.contains("<!-- mine -->"));
        assert!(out.contains("<keybind key=\"W-x\"><action name=\"Close\"/></keybind>"));
        assert_eq!(out.len(), doc.len());
    }

    #[test]
    fn adds_to_existing_parent() {
        let doc = "<labwc_config>\n  <keyboard>\n    <keybind key=\"W-x\"><action name=\"Close\"/></keybind>\n  </keyboard>\n</labwc_config>\n";
        let out = set(doc, RATE, "33").unwrap();
        assert_eq!(get(&out, RATE).as_deref(), Some("33"));
        assert!(out.contains("    <repeatRate>33</repeatRate>\n  </keyboard>"));
        assert!(roxmltree::Document::parse(&out).is_ok());
    }

    #[test]
    fn expands_self_closing_parent() {
        let doc = "<labwc_config>\n  <libinput />\n</labwc_config>";
        let out = set(doc, TOUCH_TAP, "yes").unwrap();
        assert_eq!(get(&out, TOUCH_TAP).as_deref(), Some("yes"));
        assert!(roxmltree::Document::parse(&out).is_ok());
    }

    #[test]
    fn self_closing_leaf() {
        let doc = "<labwc_config><keyboard><repeatRate/></keyboard></labwc_config>";
        let out = set(doc, RATE, "20").unwrap();
        assert_eq!(get(&out, RATE).as_deref(), Some("20"));
    }

    #[test]
    fn attribute_selects_device() {
        let mut doc = set("", TOUCH_TAP, "yes").unwrap();
        let mouse: &[Seg] = &[
            seg("libinput"),
            seg_with("device", "category", "non-touch"),
            seg("naturalScroll"),
        ];
        doc = set(&doc, mouse, "no").unwrap();
        doc = set(&doc, TOUCH_TAP, "no").unwrap();
        assert_eq!(get(&doc, TOUCH_TAP).as_deref(), Some("no"));
        assert_eq!(get(&doc, mouse).as_deref(), Some("no"));
        assert_eq!(doc.matches("<libinput>").count(), 1);
        assert_eq!(doc.matches("category=\"touchpad\"").count(), 1);
    }

    #[test]
    fn escapes_values() {
        let path: &[Seg] = &[seg("theme"), seg("name")];
        let out = set("", path, "A&B").unwrap();
        assert!(out.contains("A&amp;B"));
        assert_eq!(get(&out, path).as_deref(), Some("A&B"));
    }

    #[test]
    fn rejects_other_roots() {
        assert!(set("<openbox_config/>", RATE, "1").is_err());
    }

    #[test]
    fn environment_edit() {
        let text = "XCURSOR_SIZE=24\n# XKB_DEFAULT_LAYOUT=fr\nXKB_DEFAULT_LAYOUT=us\n";
        let out = env_set(text, "XKB_DEFAULT_LAYOUT", "de");
        assert_eq!(
            out,
            "XCURSOR_SIZE=24\n# XKB_DEFAULT_LAYOUT=fr\nXKB_DEFAULT_LAYOUT=de\n"
        );
        assert_eq!(env_get(&out, "XKB_DEFAULT_LAYOUT").as_deref(), Some("de"));
        let removed = env_set(&out, "XKB_DEFAULT_LAYOUT", "");
        assert_eq!(env_get(&removed, "XKB_DEFAULT_LAYOUT"), None);
        assert_eq!(env_set("", "A", "1"), "A=1\n");
    }

    #[test]
    fn keybind_list() {
        let doc = r#"<labwc_config><keyboard>
            <keybind key="W-Space"><action name="Execute" command="elavo-lens --toggle"/></keybind>
            <keybind key="W-C-Left"><action name="GoToDesktop" to="left" wrap="yes"/></keybind>
            <keybind key="W-S-s"><action name="Execute" command="/usr/libexec/elavo-grim-region"/></keybind>
            <keybind key="A-F4"><action name="Close"/></keybind>
            <keybind key="W-2"><action name="Execute" command="elavo-dock --activate 2"/></keybind>
            <keybind key="A-Tab"><action name="Execute" command="elavo-switcher --next"/></keybind>
        </keyboard></labwc_config>"#;
        let binds = keybinds(doc);
        assert_eq!(binds[0].key, "Super+Space");
        assert_eq!(binds[0].action, "Open Lens");
        assert_eq!(binds[1].key, "Super+Ctrl+Left");
        assert_eq!(binds[1].action, "Previous desktop");
        assert_eq!(binds[2].key, "Super+Shift+S");
        assert_eq!(binds[2].action, "Screenshot of a region");
        assert_eq!(binds[3].action, "Close");
        assert_eq!(binds[4].action, "Open dock item 2");
        assert_eq!(binds[5].action, "Next application");
    }

    #[test]
    fn layout_list() {
        let text = "! model\n  pc105 Generic\n\n! layout\n  us              English (US)\n  de              German\n\n! variant\n  dvorak us: English (Dvorak)\n";
        let layouts = parse_layouts(text);
        assert_eq!(
            layouts,
            vec![
                ("us".to_owned(), "English (US)".to_owned()),
                ("de".to_owned(), "German".to_owned())
            ]
        );
    }
}
