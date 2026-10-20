//! Lock Screen: how long the computer waits before locking and before
//! turning the display off, and what the login window shows. The times are
//! written to `idle.conf`, and the idle service is restarted so they apply
//! right away. The login window is set for everyone through the helper.

use std::cell::Cell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::keyfile::idle::{self, Idle};
use crate::backend::{greeter, helper, power};
use crate::i18n::tr;
use crate::widgets;

/// The helper's `login-window` values, in dropdown order.
const LOGIN_WINDOW: [&str; 2] = ["list", "name"];

/// Seconds; 0 is never.
const TIMES: [i64; 9] = [60, 120, 300, 600, 900, 1800, 3600, 7200, 0];

fn time_label(seconds: i64) -> String {
    match seconds {
        0 => tr("Never"),
        s if s < 3600 => {
            let m = s / 60;
            if m == 1 {
                tr("For 1 minute")
            } else {
                format!("{} {m} {}", tr("For"), tr("minutes"))
            }
        }
        s => {
            let h = s / 3600;
            if h == 1 {
                tr("For 1 hour")
            } else {
                format!("{} {h} {}", tr("For"), tr("hours"))
            }
        }
    }
}

fn time_dropdown(current: i64) -> gtk::DropDown {
    let labels: Vec<String> = TIMES.iter().map(|t| time_label(*t)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let dd = gtk::DropDown::from_strings(&refs);
    let pos = TIMES
        .iter()
        .position(|t| *t == current)
        .or_else(|| TIMES.iter().position(|t| *t >= current && *t != 0))
        .unwrap_or(2);
    dd.set_selected(pos as u32);
    dd
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let status = widgets::status_label();
    let current = idle::load();

    let lock = time_dropdown(current.lock_after);
    let off = time_dropdown(current.display_off_after);
    let save = {
        let lock = lock.clone();
        let off = off.clone();
        let status = status.clone();
        Rc::new(move || {
            let value = Idle {
                lock_after: TIMES[lock.selected() as usize % TIMES.len()],
                display_off_after: TIMES[off.selected() as usize % TIMES.len()],
            };
            match power::save_idle(value) {
                Ok(()) => widgets::set_status(&status, ""),
                Err(e) => widgets::set_status(&status, &format!("{} {e}", tr("Could not save:"))),
            }
        })
    };
    let s = save.clone();
    lock.connect_selected_notify(move |_| s());
    let s = save.clone();
    off.connect_selected_notify(move |_| s());
    page.append(&widgets::group(
        None,
        &[
            widgets::row(&tr("Lock the screen when inactive"), &lock),
            widgets::row(&tr("Turn the display off when inactive"), &off),
        ],
    ));
    page.append(&widgets::caption(&tr(
        "The screen also locks before the computer goes to sleep. Your password unlocks it.",
    )));
    page.append(&widgets::group(
        Some(&tr("Login Window")),
        &[widgets::row(
            &tr("Login window shows"),
            &login_window_dropdown(),
        )],
    ));
    page.append(&widgets::caption(&tr(
        "This applies to everyone who uses this computer.",
    )));
    page.append(&status);
    page.upcast()
}

fn login_window_dropdown() -> gtk::DropDown {
    let dd = gtk::DropDown::from_strings(&[&tr("List of users"), &tr("Name and password")]);
    let current = greeter::login_window();
    let selected = LOGIN_WINDOW.iter().position(|v| *v == current).unwrap_or(0) as u32;
    dd.set_selected(selected);
    let last = Rc::new(Cell::new(selected));
    let reverting = Rc::new(Cell::new(false));
    dd.connect_selected_notify(move |dd| {
        if reverting.get() {
            return;
        }
        let Some(value) = LOGIN_WINDOW.get(dd.selected() as usize) else {
            return;
        };
        let dd = dd.clone();
        let last = last.clone();
        let reverting = reverting.clone();
        glib::spawn_future_local(async move {
            match helper::run(&["login-window", value], |_| {}).await {
                Ok(()) => last.set(dd.selected()),
                Err(e) => {
                    reverting.set(true);
                    dd.set_selected(last.get());
                    reverting.set(false);
                    widgets::show_error(
                        &dd,
                        &tr("Could not change what the login window shows."),
                        &e,
                    );
                }
            }
        });
    });
    dd
}
