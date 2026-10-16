//! Mouse and Trackpad, written to the user `rc.xml` `<libinput>` section:
//! the `non-touch` category for mice and `touchpad` for trackpads. A
//! value the person has not set falls back to the system file's `default`
//! device, which is where the image turns natural scrolling on.

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

fn read(category: &str, key: &str) -> Option<String> {
    labwc::get_merged(&path(category, key)).or_else(|| labwc::get_merged(&path("default", key)))
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
