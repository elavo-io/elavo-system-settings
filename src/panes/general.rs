//! General: the hub for settings about the computer as a whole. Each row
//! opens one of its pages; Back returns here.

use gtk::prelude::*;

use crate::i18n::tr;
use crate::widgets;

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    page.append(&widgets::hub_header(
        "preferences-system-symbolic",
        "graphite",
        &tr("General"),
        &tr("Manage your overall setup and preferences for this computer, such as software updates, storage, and the date and time."),
    ));
    let children = super::children("general");
    let mut groups: Vec<u8> = children.iter().map(|p| p.group).collect();
    groups.dedup();
    for group in groups {
        let rows: Vec<gtk::ListBoxRow> = children
            .iter()
            .filter(|p| p.group == group)
            .map(|p| widgets::nav_row(p.icon, p.color, &(p.title)(), None, Some(p.id)))
            .collect();
        page.append(&widgets::group(None, &rows));
    }
    page.upcast()
}
