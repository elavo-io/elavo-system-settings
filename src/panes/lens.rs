//! Lens: what the Applications view and application search list. Lens
//! reads `lens.conf` each time it opens.

use gtk::glib;
use gtk::prelude::*;

use crate::backend::keyfile::lens::{self, Applications};
use crate::i18n::tr;
use crate::widgets;

fn choice(title: &str, subtitle: &str) -> (gtk::ListBoxRow, gtk::CheckButton) {
    let check = gtk::CheckButton::new();
    let row = widgets::row_with_subtitle(title, Some(subtitle), &check);
    (row, check)
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let (folders_row, folders) = choice(
        &tr("Applications folders"),
        &tr("The shared Applications folder and the one in your home folder."),
    );
    let (all_row, all) = choice(
        &tr("Also show all installed apps"),
        &tr("Adds every app the system has installed, including ones that are not in an Applications folder."),
    );
    all.set_group(Some(&folders));
    match lens::load() {
        Applications::Folders => folders.set_active(true),
        Applications::All => all.set_active(true),
    }
    let status = widgets::status_label();
    for (check, value) in [(&folders, Applications::Folders), (&all, Applications::All)] {
        let status = status.clone();
        check.connect_toggled(move |c| {
            if c.is_active() {
                match lens::save(value) {
                    Ok(()) => widgets::set_status(&status, ""),
                    Err(e) => widgets::set_status(&status, &e.to_string()),
                }
            }
        });
    }
    page.append(&widgets::group(
        Some(&tr("Applications in Lens")),
        &[folders_row, all_row],
    ));
    page.append(&status);
    if !glib::home_dir().join("Applications").is_dir() {
        page.append(&widgets::caption(&tr(
            "Your Applications folder has not been set up yet, so Lens shows every installed app for now.",
        )));
    }
    page.append(&widgets::caption(&tr(
        "Press Super+Space to open Lens. The change applies the next time it opens.",
    )));
    page.upcast()
}
