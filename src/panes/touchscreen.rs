//! Touchscreen controls. The sidebar shows this pane only when a
//! touchscreen is connected. The enlarged window buttons are on unless
//! `touch.conf` turns them off. labwc reads that file on the next tap.
//!
//! The on-screen keyboard settings go in `onscreen-keyboard.conf`, which
//! `elavo-keyboard` watches and applies at once.

use gtk::glib;
use gtk::prelude::*;

use crate::backend::keyfile;
use crate::backend::keyfile::onscreen_keyboard::{self as osk, Mode, Size};
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
    page.append(&keyboard_group(&status));
    page.append(&status);
    page.upcast()
}

fn mode_label(mode: Mode) -> String {
    match mode {
        Mode::Automatic => tr("Automatic"),
        Mode::Always => tr("Always"),
        Mode::Never => tr("Never"),
    }
}

/// Says when the chosen mode shows the keyboard.
fn mode_subtitle(mode: Mode) -> String {
    match mode {
        Mode::Automatic => tr("When a text field is tapped and no keyboard is in use"),
        Mode::Always => tr("Whenever a text field is selected"),
        Mode::Never => tr("Never shown"),
    }
}

fn size_label(size: Size) -> String {
    match size {
        Size::Compact => tr("Compact"),
        Size::Standard => tr("Standard"),
        Size::Large => tr("Large"),
    }
}

fn dropdown(labels: &[String], selected: usize) -> gtk::DropDown {
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let dd = gtk::DropDown::builder()
        .model(&gtk::StringList::new(&refs))
        .build();
    dd.set_selected(selected as u32);
    dd
}

fn keyboard_group(status: &gtk::Label) -> gtk::Box {
    let mode = osk::mode();
    let modes: Vec<String> = Mode::ALL.iter().map(|m| mode_label(*m)).collect();
    let mode_dd = dropdown(
        &modes,
        Mode::ALL.iter().position(|m| *m == mode).unwrap_or(0),
    );
    let mode_row = widgets::row_with_subtitle(
        &tr("On-Screen Keyboard"),
        Some(&mode_subtitle(mode)),
        &mode_dd,
    );
    let subtitle = subtitle_label(&mode_row);

    let size = osk::size();
    let sizes: Vec<String> = Size::ALL.iter().map(|s| size_label(*s)).collect();
    let size_dd = dropdown(
        &sizes,
        Size::ALL.iter().position(|s| *s == size).unwrap_or(1),
    );
    let size_row = widgets::row(&tr("Keyboard size"), &size_dd);
    size_row.set_sensitive(mode != Mode::Never);

    let st = status.clone();
    let size_row_for_mode = size_row.clone();
    mode_dd.connect_selected_notify(move |dd| {
        let Some(mode) = Mode::ALL.get(dd.selected() as usize).copied() else {
            return;
        };
        if let Some(label) = &subtitle {
            label.set_label(&mode_subtitle(mode));
        }
        size_row_for_mode.set_sensitive(mode != Mode::Never);
        save_result(&st, osk::set_mode(mode));
    });
    let st = status.clone();
    size_dd.connect_selected_notify(move |dd| {
        if let Some(size) = Size::ALL.get(dd.selected() as usize).copied() {
            save_result(&st, osk::set_size(size));
        }
    });

    widgets::group(None, &[mode_row, size_row])
}

fn save_result(status: &gtk::Label, result: std::io::Result<()>) {
    match result {
        Ok(()) => widgets::set_status(status, ""),
        Err(e) => widgets::set_status(status, &format!("{} {e}", tr("Could not save:"))),
    }
}

/// The subtitle label `widgets::row_with_subtitle` made, so it can follow
/// the dropdown.
fn subtitle_label(row: &gtk::ListBoxRow) -> Option<gtk::Label> {
    let content = row.child()?;
    let title_box = content.first_child()?;
    let title = title_box.first_child()?;
    title.next_sibling()?.downcast::<gtk::Label>().ok()
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
