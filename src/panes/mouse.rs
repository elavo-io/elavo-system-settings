//! Mouse and Trackpad, written to the user `rc.xml` `<libinput>` section:
//! the `non-touch` category for mice and `touchpad` for trackpads. A
//! value the person has not set falls back to the system file's `default`
//! device, which is where the image turns natural scrolling and two-finger
//! right-click on.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::labwc::{self, seg, seg_with, Seg};
use crate::i18n::tr;
use crate::widgets;

fn path<'a>(category: &'a str, key: &'a str) -> [Seg<'a>; 3] {
    [
        seg("libinput"),
        seg_with("device", "category", category),
        seg(key),
    ]
}

/// labwc `<clickMethod>`. A two-finger click is the right-click. Clicking
/// the bottom-right corner is the other choice. A clickpad uses one method.
const CLICK_FINGER: &str = "clickfinger";
const CLICK_CORNER: &str = "buttonAreas";

fn read(category: &str, key: &str) -> Option<String> {
    if category == "touchpad" && key == "clickMethod" {
        return Some(read_click_method());
    }
    labwc::get_merged(&path(category, key)).or_else(|| labwc::get_merged(&path("default", key)))
}

/// The click method actually in effect. A touchpad block that omits
/// `<clickMethod>` uses libinput's corner click; it does not inherit the
/// `default` device, because labwc treats the block as a whole.
fn read_click_method() -> String {
    let touchpad = labwc::get_merged(&path("touchpad", "clickMethod"));
    let block = keys("touchpad").iter().any(|(key, _)| {
        *key != "clickMethod" && labwc::get_merged(&path("touchpad", key)).is_some()
    });
    let default_device = labwc::get_merged(&path("default", "clickMethod"));
    effective_click_method(touchpad.as_deref(), block, default_device.as_deref()).to_owned()
}

fn effective_click_method(
    touchpad: Option<&str>,
    touchpad_block: bool,
    default_device: Option<&str>,
) -> &'static str {
    match touchpad {
        Some(CLICK_CORNER) => CLICK_CORNER,
        Some(CLICK_FINGER) => CLICK_FINGER,
        Some(_) => CLICK_FINGER,
        None if touchpad_block => CLICK_CORNER,
        None => match default_device {
            Some(CLICK_CORNER) => CLICK_CORNER,
            _ => CLICK_FINGER,
        },
    }
}

fn read_bool(category: &str, key: &str, default: bool) -> bool {
    match read(category, key).as_deref() {
        Some(v) => matches!(
            v.to_ascii_lowercase().as_str(),
            "yes" | "true" | "1" | "enabled"
        ),
        None => default,
    }
}

fn read_speed(category: &str) -> f64 {
    read(category, "pointerSpeed")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

fn yes(on: bool) -> String {
    if on { "yes" } else { "no" }.to_owned()
}

fn keys(category: &str) -> &'static [(&'static str, &'static str)] {
    if category == "touchpad" {
        &[
            ("pointerSpeed", "0.00"),
            ("naturalScroll", "yes"),
            ("tap", "yes"),
            ("clickMethod", CLICK_FINGER),
            ("disableWhileTyping", "yes"),
        ]
    } else {
        &[("pointerSpeed", "0.00"), ("naturalScroll", "yes")]
    }
}

/// labwc takes a category's block as a whole: a key missing from it gets
/// libinput's default, not the `default` device's. So every key Settings
/// manages is written together, with the values in effect now.
fn write(status: &gtk::Label, category: &'static str, key: &'static str, value: String) {
    let paths: Vec<([Seg; 3], String)> = keys(category)
        .iter()
        .map(|(k, default)| {
            let v = if *k == key {
                value.clone()
            } else {
                read(category, k).unwrap_or_else(|| (*default).to_owned())
            };
            (path(category, k), v)
        })
        .collect();
    let values: Vec<(&[Seg], String)> = paths.iter().map(|(p, v)| (&p[..], v.clone())).collect();
    match labwc::write_user(&values) {
        Ok(()) => widgets::set_status(status, ""),
        Err(e) => widgets::set_status(status, &format!("{} {e}", tr("Could not save:"))),
    }
}

fn speed_row(status: &gtk::Label, category: &'static str) -> gtk::ListBoxRow {
    let scale = widgets::slider(-1.0, 1.0, 0.05);
    scale.set_value(read_speed(category));
    let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
    let status = status.clone();
    scale.connect_value_changed(move |sc| {
        if let Some(id) = pending.take() {
            id.remove();
        }
        let value = format!("{:.2}", sc.value());
        let status = status.clone();
        let pending2 = pending.clone();
        let id = glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
            pending2.replace(None);
            write(&status, category, "pointerSpeed", value);
        });
        pending.replace(Some(id));
    });
    widgets::slider_row(&tr("Tracking speed"), &scale, &tr("Slow"), &tr("Fast"))
}

fn switch_row(
    status: &gtk::Label,
    title: &str,
    subtitle: Option<&str>,
    category: &'static str,
    key: &'static str,
    default: bool,
) -> gtk::ListBoxRow {
    let (row, switch) = widgets::switch_row(title, subtitle);
    switch.set_active(read_bool(category, key, default));
    let status = status.clone();
    switch.connect_state_set(move |_, on| {
        write(&status, category, key, yes(on));
        glib::Propagation::Proceed
    });
    row
}

fn secondary_click_row(status: &gtk::Label) -> gtk::ListBoxRow {
    let labels = [
        tr("Click with two fingers"),
        tr("Click the bottom-right corner"),
    ];
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&refs);
    let corner = read("touchpad", "clickMethod").as_deref() == Some(CLICK_CORNER);
    dropdown.set_selected(u32::from(corner));
    let status = status.clone();
    dropdown.connect_selected_notify(move |dropdown| {
        let value = if dropdown.selected() == 1 {
            CLICK_CORNER
        } else {
            CLICK_FINGER
        };
        write(&status, "touchpad", "clickMethod", value.to_owned());
    });
    widgets::row_with_subtitle(
        &tr("Secondary click"),
        Some(&tr("Right-click on the trackpad.")),
        &dropdown,
    )
}

fn has_touchpad() -> bool {
    std::fs::read_to_string("/proc/bus/input/devices")
        .map(|t| {
            t.lines().filter(|l| l.starts_with("N: Name=")).any(|l| {
                let l = l.to_ascii_lowercase();
                l.contains("touchpad") || l.contains("trackpad")
            })
        })
        .unwrap_or(true)
}

pub fn build_mouse() -> gtk::Widget {
    let page = widgets::page();
    let status = widgets::status_label();
    page.append(&widgets::group(
        None,
        &[
            speed_row(&status, "non-touch"),
            switch_row(
                &status,
                &tr("Natural scrolling"),
                Some(&tr("Content tracks the movement of the wheel.")),
                "non-touch",
                "naturalScroll",
                true,
            ),
        ],
    ));
    page.append(&status);
    page.upcast()
}

pub fn build_trackpad() -> gtk::Widget {
    let page = widgets::page();
    let status = widgets::status_label();
    page.append(&widgets::group(
        Some(&tr("Point & Click")),
        &[
            speed_row(&status, "touchpad"),
            switch_row(
                &status,
                &tr("Tap to click"),
                Some(&tr("Tap with one finger.")),
                "touchpad",
                "tap",
                true,
            ),
            secondary_click_row(&status),
            switch_row(
                &status,
                &tr("Ignore the trackpad while typing"),
                None,
                "touchpad",
                "disableWhileTyping",
                true,
            ),
        ],
    ));
    page.append(&widgets::group(
        Some(&tr("Scroll")),
        &[switch_row(
            &status,
            &tr("Natural scrolling"),
            Some(&tr("Content tracks finger movement.")),
            "touchpad",
            "naturalScroll",
            true,
        )],
    ));
    if !has_touchpad() {
        page.append(&widgets::caption(&tr(
            "No trackpad is connected. These settings apply when one is.",
        )));
    }
    page.append(&status);
    page.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_finger_click_is_the_default_right_click() {
        assert_eq!(effective_click_method(None, false, None), CLICK_FINGER);
        assert_eq!(
            effective_click_method(None, false, Some(CLICK_FINGER)),
            CLICK_FINGER
        );
        assert_eq!(
            effective_click_method(Some(CLICK_FINGER), false, Some(CLICK_CORNER)),
            CLICK_FINGER
        );
        assert!(keys("touchpad").contains(&("clickMethod", CLICK_FINGER)));
        assert!(!keys("non-touch")
            .iter()
            .any(|(key, _)| *key == "clickMethod"));
    }

    #[test]
    fn bottom_right_stays_available_and_an_old_block_keeps_it() {
        assert_eq!(
            effective_click_method(Some(CLICK_CORNER), false, Some(CLICK_FINGER)),
            CLICK_CORNER
        );
        assert_eq!(
            effective_click_method(None, true, Some(CLICK_FINGER)),
            CLICK_CORNER
        );
        assert_eq!(
            effective_click_method(Some("none"), false, None),
            CLICK_FINGER
        );
    }
}
