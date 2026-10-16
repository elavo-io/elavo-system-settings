//! Battery (Energy on a computer without one): the power mode and what
//! closing the lid does. When the screen locks and turns off is in Lock
//! Screen.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::helper;
use crate::backend::{dbus, power};
use crate::i18n::tr;
use crate::widgets;

const LID: [&str; 4] = ["suspend", "lock", "ignore", "poweroff"];

fn has_battery() -> bool {
    std::fs::read_dir("/sys/class/power_supply")
        .map(|dir| {
            dir.flatten().any(|e| {
                std::fs::read_to_string(e.path().join("type")).is_ok_and(|t| t.trim() == "Battery")
            })
        })
        .unwrap_or(false)
}

pub fn title() -> String {
    if has_battery() {
        tr("Battery")
    } else {
        tr("Energy")
    }
}

fn lid_label(action: &str) -> String {
    match action {
        "suspend" => tr("Sleep"),
        "lock" => tr("Lock the screen"),
        "ignore" => tr("Do nothing"),
        "poweroff" => tr("Shut down"),
        other => other.to_owned(),
    }
}

/// Charge in percent and the kernel's status word, for the first battery.
fn battery_level() -> Option<(u32, String)> {
    let dir = std::fs::read_dir("/sys/class/power_supply").ok()?;
    for e in dir.flatten() {
        let path = e.path();
        let read = |name: &str| {
            std::fs::read_to_string(path.join(name))
                .map(|s| s.trim().to_owned())
                .ok()
        };
        if read("type").as_deref() != Some("Battery") {
            continue;
        }
        let capacity = read("capacity")?.parse().ok()?;
        return Some((capacity, read("status").unwrap_or_default()));
    }
    None
}

fn battery_row() -> Option<gtk::ListBoxRow> {
    let (level, state) = battery_level()?;
    let bar = gtk::LevelBar::builder()
        .min_value(0.0)
        .max_value(100.0)
        .value(f64::from(level))
        .width_request(160)
        .valign(gtk::Align::Center)
        .build();
    let state = match state.as_str() {
        "Charging" => tr("Charging"),
        "Full" => tr("Fully charged"),
        "Not charging" => tr("Not charging"),
        _ => tr("On battery"),
    };
    let label = gtk::Label::builder()
        .label(format!("{level}% · {state}"))
        .css_classes(["dim-label"])
        .build();
    let right = gtk::Box::builder().spacing(10).build();
    right.append(&label);
    right.append(&bar);
    Some(widgets::row(&tr("Battery level"), &right))
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    if let Some(row) = battery_row() {
        page.append(&widgets::group(None, &[row]));
    }
    let profile_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .visible(false)
        .build();
    let lid_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .visible(false)
        .build();
    page.append(&profile_box);
    page.append(&lid_box);
    page.append(&widgets::group(
        None,
        &[widgets::nav_row(
            "system-lock-screen-symbolic",
            "dark",
            &tr("Lock Screen"),
            Some(&tr("When the display turns off")),
            Some("lockscreen"),
        )],
    ));

    if let Some(conn) = dbus::system() {
        let conn2 = conn.clone();
        let profile_box2 = profile_box.clone();
        glib::spawn_future_local(async move {
            let Some(profiles) = power::profiles(&conn2).await else {
                return;
            };
            if profiles.available.is_empty() {
                return;
            }
            let labels: Vec<String> = profiles
                .available
                .iter()
                .map(|p| power::profile_label(p))
                .collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            let dd = gtk::DropDown::from_strings(&refs);
            if let Some(i) = profiles
                .available
                .iter()
                .position(|p| *p == profiles.active)
            {
                dd.set_selected(i as u32);
            }
            let profiles = Rc::new(profiles);
            dd.connect_selected_notify(move |dd| {
                let Some(name) = profiles.available.get(dd.selected() as usize).cloned() else {
                    return;
                };
                let conn = conn2.clone();
                let profiles = profiles.clone();
                let dd = dd.clone();
                glib::spawn_future_local(async move {
                    if let Err(e) = power::set_profile(&conn, &profiles, &name).await {
                        widgets::show_error(
                            &dd,
                            &tr("Could not change the power mode."),
                            &widgets::error_text(&e),
                        );
                    }
                });
            });
            profile_box2.append(&widgets::group(
                Some(&tr("Power mode")),
                &[widgets::row_with_subtitle(
                    &tr("Mode"),
                    Some(&tr(
                        "Power Saver lowers performance to make the battery last longer.",
                    )),
                    &dd,
                )],
            ));
            profile_box2.set_visible(true);
        });

        if power::has_lid() {
            let lid_box2 = lid_box.clone();
            glib::spawn_future_local(async move {
                let current = power::lid_action(&conn).await;
                let mut actions: Vec<String> = LID.iter().map(|s| s.to_string()).collect();
                if !actions.contains(&current) {
                    actions.push(current.clone());
                }
                let labels: Vec<String> = actions.iter().map(|a| lid_label(a)).collect();
                let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let dd = gtk::DropDown::from_strings(&refs);
                let selected = actions.iter().position(|a| *a == current).unwrap_or(0);
                dd.set_selected(selected as u32);
                let last = Rc::new(Cell::new(selected as u32));
                let reverting = Rc::new(Cell::new(false));
                dd.connect_selected_notify(move |dd| {
                    if reverting.get() {
                        return;
                    }
                    let Some(action) = LID.get(dd.selected() as usize) else {
                        return;
                    };
                    let dd = dd.clone();
                    let last = last.clone();
                    let reverting = reverting.clone();
                    glib::spawn_future_local(async move {
                        match helper::run(&["lid", action], |_| {}).await {
                            Ok(()) => last.set(dd.selected()),
                            Err(e) => {
                                reverting.set(true);
                                dd.set_selected(last.get());
                                reverting.set(false);
                                widgets::show_error(
                                    &dd,
                                    &tr("Could not change what closing the lid does."),
                                    &e,
                                );
                            }
                        }
                    });
                });
                lid_box2.append(&widgets::group(
                    Some(&tr("Lid")),
                    &[widgets::row(&tr("When the lid is closed"), &dd)],
                ));
                lid_box2.set_visible(true);
            });
        }
    }
    page.upcast()
}
