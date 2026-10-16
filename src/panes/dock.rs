//! Desktop & Dock. The Dock section is Folio's own page: it reads and
//! writes `dock.conf`, and the dock follows that file, so there is no
//! other interface to the dock.

use gtk::prelude::*;

pub fn build() -> gtk::Widget {
    let page = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    page.append(&elavo_dock_settings::page());
    page.upcast()
}
