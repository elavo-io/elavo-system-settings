//! About: the system's name and version, what this computer is, and its
//! name on the network.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::{dbus, hostname, sysinfo};
use crate::i18n::tr;
use crate::widgets;

const VERIFY_URL: &str = "https://github.com/elavo-io/elavo#elavo";

fn version() -> String {
    let base = sysinfo::os_release("VERSION").unwrap_or_default();
    match sysinfo::os_release("OSTREE_VERSION") {
        Some(build) if !build.is_empty() => format!("{} {base} ({build})", tr("Version")),
        _ if !base.is_empty() => format!("{} {base}", tr("Version")),
        _ => String::new(),
    }
}

fn name_row(conn: Option<gio::DBusConnection>) -> gtk::ListBoxRow {
    let entry = gtk::Entry::builder()
        .width_chars(22)
        .valign(gtk::Align::Center)
        .build();
    let caption = gtk::Label::builder()
        .css_classes(["dim-label", "caption"])
        .xalign(0.0)
        .build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    text.append(&gtk::Label::builder().label(tr("Name")).xalign(0.0).build());
    text.append(&caption);
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&text);
    content.append(&entry);
    let row = widgets::plain_row(&content);

    let Some(conn) = conn else {
        entry.set_sensitive(false);
        return row;
    };
    let saved = Rc::new(std::cell::RefCell::new(String::new()));
    {
        let conn = conn.clone();
        let entry = entry.clone();
        let caption = caption.clone();
        let saved = saved.clone();
        glib::spawn_future_local(async move {
            let info = hostname::info(&conn).await;
            let pretty = if info.pretty.is_empty() {
                info.hostname.clone()
            } else {
                info.pretty
            };
            entry.set_text(&pretty);
            saved.replace(pretty);
            caption.set_label(&format!("{} {}", tr("On the network:"), info.hostname));
        });
    }
    let commit = Rc::new(move |entry: &gtk::Entry| {
        let name = entry.text().trim().to_owned();
        if name.is_empty() || *saved.borrow() == name {
            return;
        }
        let conn = conn.clone();
        let entry = entry.clone();
        let caption = caption.clone();
        let saved = saved.clone();
        glib::spawn_future_local(async move {
            match hostname::set_name(&conn, &name).await {
                Ok(()) => {
                    saved.replace(name.clone());
                    caption.set_label(&format!(
                        "{} {}",
                        tr("On the network:"),
                        hostname::static_name(&name)
                    ));
                }
                Err(e) => {
                    entry.set_text(&saved.borrow());
                    widgets::show_error(
                        &entry,
                        &tr("Could not rename this computer."),
                        &widgets::error_text(&e),
                    );
                }
            }
        });
    });
    let c = commit.clone();
    entry.connect_activate(move |e| c(e));
    let focus = gtk::EventControllerFocus::new();
    let e = entry.clone();
    focus.connect_leave(move |_| commit(&e));
    entry.add_controller(focus);
    row
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();

    let logo = gtk::Image::builder()
        .resource("/io/elavo/SystemSettings/about-sun.svg")
        .pixel_size(96)
        .halign(gtk::Align::Center)
        .margin_top(12)
        .build();
    let name = gtk::Label::builder()
        .label(sysinfo::os_release("NAME").unwrap_or_else(|| "Elavo".to_owned()))
        .css_classes(["about-name"])
        .build();
    let ver = gtk::Label::builder()
        .label(version())
        .css_classes(["about-version"])
        .selectable(true)
        .build();
    page.append(&logo);
    page.append(&name);
    page.append(&ver);

    let conn = dbus::system();
    let model_label = widgets::value_row(&tr("Model"), "");
    let mut rows = vec![name_row(conn.clone()), model_label.0.clone()];
    if let Some(cpu) = sysinfo::cpu() {
        rows.push(widgets::value_row(&tr("Processor"), &cpu).0);
    }
    for gpu in sysinfo::gpus() {
        rows.push(widgets::value_row(&tr("Graphics"), &gpu).0);
    }
    if let Some(mem) = sysinfo::memory() {
        rows.push(widgets::value_row(&tr("Memory"), &mem).0);
    }
    if let Some((free, total)) = sysinfo::storage("/var") {
        rows.push(
            widgets::value_row(
                &tr("Storage"),
                &format!(
                    "{} {} {}",
                    sysinfo::bytes_label(free),
                    tr("available of"),
                    sysinfo::bytes_label(total)
                ),
            )
            .0,
        );
    }
    if let Some(kernel) = sysinfo::kernel() {
        rows.push(widgets::value_row(&tr("Kernel"), &kernel).0);
    }
    page.append(&widgets::group(None, &rows));

    match conn {
        Some(conn) => {
            let (row, label) = model_label;
            glib::spawn_future_local(async move {
                let info = hostname::info(&conn).await;
                let model = format!("{} {}", info.vendor, info.model).trim().to_owned();
                if model.is_empty() {
                    row.set_visible(false);
                } else {
                    label.set_label(&model);
                }
            });
        }
        None => model_label.0.set_visible(false),
    }

    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::Center)
        .build();
    let update = gtk::Button::with_label(&tr("Software Update…"));
    update.set_action_name(Some("app.show-pane"));
    update.set_action_target_value(Some(&"updates".to_variant()));
    let verify = gtk::Button::with_label(&tr("Verify This System…"));
    verify.connect_clicked(|b| {
        let launcher = gtk::UriLauncher::new(VERIFY_URL);
        launcher.launch(
            widgets::parent_window(b).as_ref(),
            gio::Cancellable::NONE,
            |_| {},
        );
    });
    buttons.append(&update);
    buttons.append(&verify);
    page.append(&buttons);
    page.append(&widgets::caption(&tr(
        "Elavo is free software built on Fedora. Every image is signed; Verify This System explains how to check yours.",
    )));
    page.upcast()
}
