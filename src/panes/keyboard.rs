//! Keyboard: key repeat, the layout, and the shortcut list. Repeat goes in
//! the user `rc.xml`, the layout in the user labwc `environment`; labwc
//! reloads both on SIGHUP.

use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::labwc::{self, seg};
use crate::i18n::tr;
use crate::widgets;

const RATE: &[labwc::Seg] = &[seg("keyboard"), seg("repeatRate")];
const DELAY: &[labwc::Seg] = &[seg("keyboard"), seg("repeatDelay")];
/// labwc's defaults.
const DEFAULT_RATE: f64 = 25.0;
const DEFAULT_DELAY: f64 = 600.0;
const LAYOUT_KEY: &str = "XKB_DEFAULT_LAYOUT";

fn read(path: &[labwc::Seg], default: f64) -> f64 {
    labwc::get_merged(path)
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let status = widgets::status_label();

    let rate = widgets::slider(5.0, 60.0, 1.0);
    rate.set_value(read(RATE, DEFAULT_RATE));
    // Long delays are on the left, as "Long" to "Short" reads.
    let delay = widgets::slider(150.0, 1000.0, 10.0);
    delay.set_inverted(true);
    delay.set_value(read(DELAY, DEFAULT_DELAY));
    let test = gtk::Entry::builder()
        .placeholder_text(tr("Type here to test"))
        .hexpand(true)
        .build();
    page.append(&widgets::group(
        Some(&tr("Key repeat")),
        &[
            widgets::slider_row(&tr("Key repeat rate"), &rate, &tr("Slow"), &tr("Fast")),
            widgets::slider_row(&tr("Delay until repeat"), &delay, &tr("Long"), &tr("Short")),
            widgets::plain_row(&test),
        ],
    ));

    // Write once the slider settles, so dragging does not reload labwc on
    // every step.
    let pending: Rc<std::cell::RefCell<Option<glib::SourceId>>> = Rc::default();
    let save = {
        let rate = rate.clone();
        let delay = delay.clone();
        let status = status.clone();
        let pending = pending.clone();
        Rc::new(move || {
            if let Some(id) = pending.take() {
                id.remove();
            }
            let rate = rate.clone();
            let delay = delay.clone();
            let status = status.clone();
            let pending2 = pending.clone();
            let id =
                glib::timeout_add_local_once(std::time::Duration::from_millis(400), move || {
                    pending2.replace(None);
                    let values = [
                        (RATE, format!("{}", rate.value().round() as i64)),
                        (DELAY, format!("{}", delay.value().round() as i64)),
                    ];
                    match labwc::write_user(&values) {
                        Ok(()) => widgets::set_status(&status, ""),
                        Err(e) => {
                            widgets::set_status(&status, &format!("{} {e}", tr("Could not save:")))
                        }
                    }
                });
            pending.replace(Some(id));
        })
    };
    let s = save.clone();
    rate.connect_value_changed(move |_| s());
    let s = save.clone();
    delay.connect_value_changed(move |_| s());

    let layouts = labwc::layouts();
    let current = labwc::env_get_merged(LAYOUT_KEY).unwrap_or_else(|| "us".to_owned());
    let first = current.split(',').next().unwrap_or("us").to_owned();
    let mut labels: Vec<String> = layouts.iter().map(|(_, d)| d.clone()).collect();
    let mut codes: Vec<String> = layouts.iter().map(|(c, _)| c.clone()).collect();
    if !codes.contains(&first) {
        labels.insert(0, first.clone());
        codes.insert(0, first.clone());
    }
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let model = gtk::StringList::new(&refs);
    let layout = gtk::DropDown::builder()
        .model(&model)
        .enable_search(true)
        .expression(gtk::PropertyExpression::new(
            gtk::StringObject::static_type(),
            gtk::Expression::NONE,
            "string",
        ))
        .build();
    if let Some(i) = codes.iter().position(|c| *c == first) {
        layout.set_selected(i as u32);
    }
    let st = status.clone();
    layout.connect_selected_notify(move |dd| {
        if let Some(code) = codes.get(dd.selected() as usize) {
            if let Err(e) = labwc::env_write_user(LAYOUT_KEY, code) {
                widgets::set_status(&st, &format!("{} {e}", tr("Could not save:")));
            }
        }
    });
    page.append(&widgets::group(
        Some(&tr("Input source")),
        &[widgets::row(&tr("Keyboard layout"), &layout)],
    ));
    page.append(&widgets::caption(&tr(
        "If the new layout does not take effect right away, log out and back in.",
    )));
    page.append(&status);

    let (frame, list) = widgets::list();
    for bind in labwc::keybinds_merged() {
        let key = gtk::Label::builder()
            .label(&bind.key)
            .css_classes(["dim-label", "monospace-log"])
            .build();
        list.append(&widgets::row(&bind.action, &key));
    }
    page.append(&widgets::heading(&tr("Keyboard shortcuts")));
    page.append(&frame);
    page.append(&widgets::caption(&tr(
        "labwc's built-in shortcuts, such as Alt+Tab to switch windows, also apply. Shortcuts are changed in ~/.config/labwc/rc.xml for now.",
    )));
    page.upcast()
}
