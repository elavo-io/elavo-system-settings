//! Panes whose backend belongs to the shell and is not built yet. They hold
//! their place in the sidebar so the layout does not change when they land.

use crate::i18n::tr;
use crate::widgets;

pub fn wallpaper() -> gtk::Widget {
    widgets::placeholder(
        "preferences-desktop-wallpaper-symbolic",
        &tr("Wallpaper"),
        &tr("Choosing a desktop picture comes in a later release, when the desktop draws its own background."),
    )
}

pub fn screensaver() -> gtk::Widget {
    widgets::placeholder(
        "preferences-desktop-screensaver-symbolic",
        &tr("Screen Saver"),
        &tr("Screen savers come in a later release. To choose when the screen locks and turns off, use Power."),
    )
}
