//! The Default Apps section of the Applications page: pick the web browser
//! that opens links, with placeholders for Mail and the other handlers until
//! Elavo ships them.

use std::cell::Cell;
use std::rc::Rc;

use gtk::prelude::*;

use crate::backend::defaults::{self, Browser, COMING_SOON};
use crate::i18n::tr;
use crate::widgets;

/// Append the Default Apps group, its status line and caption to `page`.
pub fn append_to(page: &gtk::Box) {
    let status = widgets::status_label();

    let browsers = defaults::list_browsers();
    let browser_row = browser_row(&browsers, &status);
    let mut rows = vec![browser_row];
    for entry in COMING_SOON {
        rows.push(coming_soon_row(&tr(entry.title)));
    }

    page.append(&widgets::group(Some(&tr("Default Apps")), &rows));
    page.append(&status);
    page.append(&widgets::caption(&tr(
        "The web browser opens links from other apps. Mail and the other rows will unlock when Elavo includes apps for them.",
    )));
}

fn browser_row(browsers: &[Browser], status: &gtk::Label) -> gtk::ListBoxRow {
    if browsers.is_empty() {
        let label = gtk::Label::builder()
            .label(tr("No web browsers found"))
            .css_classes(["dim-label"])
            .build();
        return widgets::row(&tr("Web Browser"), &label);
    }

    let labels: Vec<String> = browsers.iter().map(|b| b.name.clone()).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&refs);
    dropdown.set_hexpand(false);

    let current = defaults::current_browser_id();
    let selected = browsers
        .iter()
        .position(|b| Some(b.id.as_str()) == current.as_deref())
        .unwrap_or(0);
    dropdown.set_selected(selected as u32);

    let ids: Rc<Vec<String>> = Rc::new(browsers.iter().map(|b| b.id.clone()).collect());
    let last = Rc::new(Cell::new(selected as u32));
    let reverting = Rc::new(Cell::new(false));
    let status = status.clone();
    {
        let ids = Rc::clone(&ids);
        let last = Rc::clone(&last);
        let reverting = Rc::clone(&reverting);
        let status = status.clone();
        dropdown.connect_selected_notify(move |dd| {
            if reverting.get() {
                return;
            }
            let index = dd.selected();
            if index == last.get() {
                return;
            }
            let Some(id) = ids.get(index as usize).cloned() else {
                return;
            };
            match defaults::set_default_browser(&id) {
                Ok(()) => {
                    last.set(index);
                    widgets::set_status(&status, "");
                }
                Err(e) => {
                    reverting.set(true);
                    dd.set_selected(last.get());
                    reverting.set(false);
                    widgets::set_status(
                        &status,
                        &format!("{} {e}", tr("Could not set the default browser:")),
                    );
                }
            }
        });
    }

    // Refresh the selection when returning to the page, in case something
    // else changed mimeapps.list while we were away.
    {
        let dd = dropdown.clone();
        let ids = Rc::clone(&ids);
        let last = Rc::clone(&last);
        let reverting = Rc::clone(&reverting);
        dropdown.connect_map(move |_| {
            let current = defaults::current_browser_id();
            if let Some(i) = ids
                .iter()
                .position(|id| Some(id.as_str()) == current.as_deref())
            {
                if dd.selected() != i as u32 {
                    reverting.set(true);
                    dd.set_selected(i as u32);
                    reverting.set(false);
                    last.set(i as u32);
                }
            }
        });
    }

    widgets::row_with_subtitle(
        &tr("Web Browser"),
        Some(&tr("Opens links and HTML files.")),
        &dropdown,
    )
}

fn coming_soon_row(title: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(tr("Coming soon"))
        .css_classes(["dim-label"])
        .build();
    let row = widgets::row(title, &label);
    row.set_sensitive(false);
    row
}
