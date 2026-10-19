//! App Switcher. The page is Folio's: it reads and writes `switcher.conf`,
//! and the switcher follows that file.

pub fn build() -> gtk::Widget {
    elavo_switcher_settings::page()
}
