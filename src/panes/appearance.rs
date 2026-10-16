//! Appearance: light or dark, and the accent color.

use std::cell::RefCell;

use gtk::gdk;
use gtk::prelude::*;

use crate::backend::accent::{self, Preset, Scheme, PRESETS};
use crate::i18n::tr;
use crate::widgets;

thread_local! {
    static LIVE: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

fn live_css(preset: Preset) -> String {
    format!(
        ":root {{ --accent-bg-color: {}; --accent-fg-color: {}; }}\n",
        preset.bg, preset.fg
    )
}

/// The user `gtk.css` is read once at startup. This provider sits above it
/// so a new accent shows in this window at once.
pub fn install_live_accent(display: &gdk::Display) {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&live_css(accent::current()));
    gtk::style_context_add_provider_for_display(
        display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_USER + 1,
    );
    let swatches = gtk::CssProvider::new();
    let css: String = PRESETS
        .iter()
        .map(|p| {
            format!(
                ".accent-swatch.accent-{} {{ background-color: {}; }}\n",
                p.id, p.bg
            )
        })
        .collect();
    swatches.load_from_string(&css);
    gtk::style_context_add_provider_for_display(
        display,
        &swatches,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    LIVE.with(|l| l.replace(Some(provider)));
    apply_scheme_here(accent::scheme());
}

fn apply_scheme_here(scheme: Scheme) {
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(scheme == Scheme::Dark);
    }
}

fn scheme_card(label: &str, class: &str) -> gtk::ToggleButton {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    content.append(
        &gtk::Box::builder()
            .css_classes(["appearance-thumb", class])
            .build(),
    );
    content.append(&gtk::Label::new(Some(label)));
    gtk::ToggleButton::builder()
        .child(&content)
        .css_classes(["appearance-card"])
        .build()
}

fn preview() -> gtk::Box {
    let row = gtk::Box::builder()
        .spacing(16)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    row.append(
        &gtk::Button::builder()
            .label(tr("Button"))
            .css_classes(["suggested-action"])
            .valign(gtk::Align::Center)
            .build(),
    );
    let switch = gtk::Switch::builder()
        .active(true)
        .valign(gtk::Align::Center)
        .build();
    row.append(&switch);
    row.append(
        &gtk::CheckButton::builder()
            .label(tr("Checked"))
            .active(true)
            .build(),
    );
    let scale = widgets::slider(0.0, 1.0, 0.01);
    scale.set_value(0.6);
    scale.set_width_request(140);
    row.append(&scale);
    row
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();

    let light = scheme_card(&tr("Light"), "light");
    let dark = scheme_card(&tr("Dark"), "dark");
    dark.set_group(Some(&light));
    match accent::scheme() {
        Scheme::Light => light.set_active(true),
        Scheme::Dark => dark.set_active(true),
    }
    for (button, scheme) in [(&light, Scheme::Light), (&dark, Scheme::Dark)] {
        button.connect_toggled(move |b| {
            if b.is_active() {
                accent::set_scheme(scheme);
                apply_scheme_here(scheme);
            }
        });
    }
    let cards = gtk::Box::builder().spacing(12).build();
    cards.append(&light);
    cards.append(&dark);
    let scheme_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(6)
        .margin_bottom(6)
        .build();
    scheme_box.append(&cards);
    scheme_box.append(&widgets::caption(&tr(
        "The dark Elavo theme is not finished yet. Apps built with libadwaita turn dark now; the others stay light until it ships.",
    )));
    page.append(&widgets::group(
        Some(&tr("Appearance")),
        &[widgets::plain_row(&scheme_box)],
    ));

    let current = accent::current();
    let swatches = gtk::Box::builder().spacing(10).build();
    let name = gtk::Label::builder()
        .label(current.label())
        .css_classes(["dim-label"])
        .build();
    let mut first: Option<gtk::ToggleButton> = None;
    for preset in PRESETS {
        let class = format!("accent-{}", preset.id);
        let button = gtk::ToggleButton::builder()
            .css_classes(["accent-swatch", class.as_str()])
            .tooltip_text(preset.label())
            .valign(gtk::Align::Center)
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&preset.label())]);
        if let Some(first) = &first {
            button.set_group(Some(first));
        } else {
            first = Some(button.clone());
        }
        button.set_active(preset == current);
        let name = name.clone();
        button.connect_toggled(move |b| {
            if !b.is_active() {
                return;
            }
            name.set_label(&preset.label());
            if let Err(e) = accent::apply(preset) {
                widgets::show_error(
                    b,
                    &tr("The accent color could not be saved."),
                    &e.to_string(),
                );
                return;
            }
            LIVE.with(|l| {
                if let Some(p) = l.borrow().as_ref() {
                    p.load_from_string(&live_css(preset));
                }
            });
        });
        swatches.append(&button);
    }
    let accent_row = gtk::Box::builder().spacing(12).build();
    accent_row.append(&swatches);
    accent_row.append(&name);
    page.append(&widgets::group(
        Some(&tr("Accent color")),
        &[
            widgets::row(&tr("Color"), &accent_row),
            widgets::plain_row(&preview()),
        ],
    ));
    page.append(&widgets::caption(&tr(
        "The color changes in open applications.",
    )));
    page.upcast()
}
