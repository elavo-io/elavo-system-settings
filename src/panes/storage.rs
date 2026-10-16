//! Storage: how full the disk is. `/var` holds applications, documents,
//! and settings, and shares its filesystem with the system image.

use gtk::prelude::*;

use crate::backend::sysinfo;
use crate::i18n::tr;
use crate::widgets;

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let Some((free, total)) = sysinfo::storage("/var").filter(|(_, t)| *t > 0) else {
        return widgets::placeholder(
            "drive-harddisk-symbolic",
            &tr("Storage"),
            &tr("The disk's size could not be read."),
        );
    };
    let used = total.saturating_sub(free);

    let name = gtk::Label::builder()
        .label(sysinfo::os_release("NAME").unwrap_or_else(|| "Elavo".to_owned()))
        .xalign(0.0)
        .hexpand(true)
        .css_classes(["heading"])
        .build();
    let summary = gtk::Label::builder()
        .label(format!(
            "{} {} {} {}",
            sysinfo::bytes_label(used),
            tr("of"),
            sysinfo::bytes_label(total),
            tr("used"),
        ))
        .css_classes(["dim-label"])
        .build();
    let top = gtk::Box::builder().spacing(8).build();
    top.append(&widgets::tile(
        "drive-harddisk-symbolic",
        "graphite",
        "small",
    ));
    top.append(&name);
    top.append(&summary);

    let bar = gtk::LevelBar::builder()
        .min_value(0.0)
        .max_value(1.0)
        .value(used as f64 / total as f64)
        .css_classes(["storage-bar"])
        .build();
    // Turn the bar's color to a warning only when the disk is nearly full.
    bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
    bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
    bar.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));
    bar.add_offset_value("normal", 0.9);
    bar.add_offset_value("nearly-full", 1.0);
    let available = widgets::caption(&format!(
        "{} {}",
        sysinfo::bytes_label(free),
        tr("available")
    ));

    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    card.append(&top);
    card.append(&bar);
    card.append(&available);
    page.append(&widgets::group(None, &[widgets::plain_row(&card)]));

    page.append(&widgets::group(
        None,
        &[widgets::nav_row(
            "software-update-available-symbolic",
            "graphite",
            &tr("Previous system version"),
            Some(&tr("Kept for going back")),
            Some("updates"),
        )],
    ));
    page.append(&widgets::caption(&tr(
        "This includes the system. Its previous version stays installed until the next update, so you can go back to it from Software Update.",
    )));
    page.upcast()
}
