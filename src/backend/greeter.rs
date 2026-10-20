//! Folio's login window. What it shows is in `/etc/elavo/greeter.conf`,
//! which only root can change, so writes go through the helper.

use gtk::glib;

pub const CONF: &str = "/etc/elavo/greeter.conf";

/// `list` (the people on this computer) or `name` (a name field and a
/// password field). A missing or unreadable file is the default, `list`.
pub fn login_window() -> &'static str {
    let file = glib::KeyFile::new();
    if file.load_from_file(CONF, glib::KeyFileFlags::NONE).is_err() {
        return "list";
    }
    parse_show(file.string("Login", "Show").ok().as_deref())
}

fn parse_show(value: Option<&str>) -> &'static str {
    match value.map(str::trim) {
        Some("name") => "name",
        _ => "list",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anything_but_name_is_the_list() {
        assert_eq!(parse_show(Some("name")), "name");
        assert_eq!(parse_show(Some(" name ")), "name");
        assert_eq!(parse_show(Some("list")), "list");
        assert_eq!(parse_show(Some("names")), "list");
        assert_eq!(parse_show(None), "list");
    }
}
