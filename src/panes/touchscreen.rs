//! Touchscreen controls. The sidebar shows this pane only when a
//! touchscreen is connected. The enlarged window buttons are on unless
//! `touch.conf` turns them off. labwc reads that file on the next tap.

use gtk::glib;
use gtk::prelude::*;

use crate::backend::keyfile;
use crate::i18n::tr;
use crate::widgets;

/// A touchscreen reports `INPUT_PROP_DIRECT` (bit 1 of `B: PROP=`). A
/// stylus does too, and is not a touchscreen. A name that says
/// touchscreen counts even when the property bit is missing.
pub fn present_in(text: &str) -> bool {
    text.split("\n\n").any(is_touchscreen)
}

fn is_touchscreen(block: &str) -> bool {
    let mut name = "";
    let mut direct = false;
    for line in block.lines() {
        if let Some(rest) = line.strip_prefix("N: Name=") {
            name = rest.trim().trim_matches('"');
        } else if let Some(rest) = line.strip_prefix("B: PROP=") {
            let bits = u64::from_str_radix(rest.trim(), 16).unwrap_or(0);
            direct = (bits & 0x2) != 0;
        }
    }
    let lower = name.to_ascii_lowercase();
    if lower.contains("stylus") || lower.contains(" pen") || lower.ends_with("pen") {
        return false;
    }
    direct
        || lower.contains("touchscreen")
        || lower.contains("touch screen")
        || lower.contains("touch-screen")
}

pub fn present() -> bool {
    std::fs::read_to_string("/proc/bus/input/devices")
        .map(|text| present_in(&text))
        .unwrap_or(false)
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let status = widgets::status_label();
    let (row, switch) = widgets::switch_row(
        &tr("Enlarged window buttons"),
        Some(&tr(
            "Tap close, minimize, or maximize to show larger buttons.",
        )),
    );
    switch.set_active(keyfile::touch::enlarged_buttons());
    let status_for_switch = status.clone();
    switch.connect_state_set(move |_, on| {
        match keyfile::touch::set_enlarged_buttons(on) {
            Ok(()) => widgets::set_status(&status_for_switch, ""),
            Err(e) => widgets::set_status(
                &status_for_switch,
                &format!("{} {e}", tr("Could not save:")),
            ),
        }
        glib::Propagation::Proceed
    });
    page.append(&widgets::group(None, &[row]));
    page.append(&status);
    page.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICES: &str = "\
N: Name=\"ELAN2097:00 04F3:2783\"
B: PROP=2

N: Name=\"DELL0895:00 04F3:30B6 Touchpad\"
B: PROP=5

N: Name=\"ELAN2097:00 04F3:2783 Stylus\"
B: PROP=2

N: Name=\"ImPS/2 Logitech Wheel Mouse\"
B: PROP=1
";

    #[test]
    fn a_direct_digitizer_is_a_touchscreen() {
        assert!(present_in(DEVICES));
        assert!(is_touchscreen(
            "N: Name=\"ELAN2097:00 04F3:2783\"\nB: PROP=2\n"
        ));
    }

    #[test]
    fn a_trackpad_stylus_or_mouse_is_not() {
        assert!(!is_touchscreen(
            "N: Name=\"DELL0895:00 04F3:30B6 Touchpad\"\nB: PROP=5\n"
        ));
        assert!(!is_touchscreen(
            "N: Name=\"ELAN2097:00 04F3:2783 Stylus\"\nB: PROP=2\n"
        ));
        assert!(!is_touchscreen(
            "N: Name=\"ImPS/2 Logitech Wheel Mouse\"\nB: PROP=1\n"
        ));
        assert!(!present_in(
            "N: Name=\"AT Translated Set 2 keyboard\"\nB: PROP=0\n"
        ));
    }

    #[test]
    fn a_named_touchscreen_counts_without_the_property() {
        assert!(is_touchscreen(
            "N: Name=\"Goodix Touchscreen\"\nB: PROP=0\n"
        ));
        assert!(is_touchscreen("N: Name=\"Touch Screen\"\n"));
    }
}
