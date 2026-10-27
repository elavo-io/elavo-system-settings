//! Applications: what happens when an app finishes installing from the
//! Applications folder or a downloaded file. The `elavo-applications`
//! service reads `applications.conf` each time, and its "Don’t ask again"
//! box writes the same key.

use gtk::prelude::*;

use crate::backend::keyfile::applications::{self, AfterInstall};
use crate::i18n::tr;
use crate::widgets;

fn choice(title: &str, subtitle: &str) -> (gtk::ListBoxRow, gtk::CheckButton) {
    let check = gtk::CheckButton::new();
    let row = widgets::row_with_subtitle(title, Some(subtitle), &check);
    (row, check)
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let (ask_row, ask) = choice(
        &tr("Ask every time"),
        &tr("Ask whether to open the app when it finishes installing."),
    );
    let (open_row, open) = choice(
        &tr("Open automatically"),
        &tr("Open the app as soon as it finishes installing."),
    );
    let (nothing_row, nothing) = choice(
        &tr("Don’t open"),
        &tr("Leave the app in Applications until you open it."),
    );
    open.set_group(Some(&ask));
    nothing.set_group(Some(&ask));
    let checks = [
        (ask, AfterInstall::Ask),
        (open, AfterInstall::Open),
        (nothing, AfterInstall::Nothing),
    ];
    let status = widgets::status_label();
    for (check, value) in checks.clone() {
        let status = status.clone();
        check.connect_toggled(move |c| {
            if c.is_active() {
                match applications::save(value) {
                    Ok(()) => widgets::set_status(&status, ""),
                    Err(e) => widgets::set_status(&status, &e.to_string()),
                }
            }
        });
    }
    // The service may have changed the file since this page was built.
    let sync = move || {
        let current = applications::load();
        for (check, value) in &checks {
            if *value == current && !check.is_active() {
                check.set_active(true);
            }
        }
    };
    sync();
    page.connect_map(move |_| sync());
    page.append(&widgets::group(
        Some(&tr("After an App Installs")),
        &[ask_row, open_row, nothing_row],
    ));
    page.append(&status);
    page.append(&widgets::caption(&tr(
        "Applies to apps you add to an Applications folder or install from a downloaded AppImage or Flatpak file.",
    )));
    page.upcast()
}
