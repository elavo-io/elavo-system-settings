//! Wallpaper: the dynamic pictures shipped with Elavo, and a picture the
//! person chooses. The picture always covers the screen. Automatic follows
//! the light or dark appearance.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use crate::backend::accent;
use crate::backend::wallpaper::{self, Dynamic, Kind, Variant};
use crate::i18n::tr;
use crate::widgets;

const VARIANTS: [Variant; 3] = [Variant::Automatic, Variant::Light, Variant::Dark];

fn variant_label(variant: Variant) -> String {
    match variant {
        Variant::Automatic => tr("Automatic"),
        Variant::Light => tr("Light"),
        Variant::Dark => tr("Dark"),
    }
}

fn scheme_dark() -> bool {
    accent::scheme() == accent::Scheme::Dark
}

fn picture_file(path: &std::path::Path) -> gtk::Picture {
    let picture = gtk::Picture::for_filename(path);
    picture.set_content_fit(gtk::ContentFit::Cover);
    picture.set_can_shrink(true);
    picture.set_overflow(gtk::Overflow::Hidden);
    picture.set_size_request(168, 94);
    picture.add_css_class("wallpaper-thumb");
    picture
}

fn card(label: &str, picture: &gtk::Picture) -> gtk::ToggleButton {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    content.append(picture);
    content.append(&gtk::Label::new(Some(label)));
    gtk::ToggleButton::builder()
        .child(&content)
        .css_classes(["wallpaper-card"])
        .build()
}

struct Shown {
    id: String,
    picture: gtk::Picture,
    paper: Option<Dynamic>,
    button: gtk::ToggleButton,
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let status = widgets::status_label();
    let installed = wallpaper::catalog();
    let choice = Rc::new(RefCell::new(wallpaper::resolve_id(
        &wallpaper::load(),
        &installed,
    )));
    let suppress = Rc::new(Cell::new(true));
    let shown: Rc<RefCell<Vec<Shown>>> = Rc::new(RefCell::new(Vec::new()));

    let labels: Vec<String> = VARIANTS.iter().copied().map(variant_label).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let variant = gtk::DropDown::from_strings(&refs);
    let current = choice.borrow().variant;
    let pos = VARIANTS
        .iter()
        .position(|item| *item == current)
        .unwrap_or(0);
    variant.set_selected(pos as u32);

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .column_spacing(12)
        .row_spacing(12)
        .homogeneous(true)
        .max_children_per_line(4)
        .build();

    let mut leader: Option<gtk::ToggleButton> = None;
    let dark = scheme_dark();
    let selected_variant = choice.borrow().variant;
    for paper in &installed {
        let Some(path) = wallpaper::thumb(paper, selected_variant, dark) else {
            continue;
        };
        let picture = picture_file(&path);
        let button = card(&wallpaper::display_name(&paper.id), &picture);
        if let Some(leader) = &leader {
            button.set_group(Some(leader));
        } else {
            leader = Some(button.clone());
        }
        flow.append(&button);
        shown.borrow_mut().push(Shown {
            id: paper.id.clone(),
            picture,
            paper: Some(paper.clone()),
            button,
        });
    }

    let custom_picture = gtk::Picture::new();
    custom_picture.set_content_fit(gtk::ContentFit::Cover);
    custom_picture.set_can_shrink(true);
    custom_picture.set_overflow(gtk::Overflow::Hidden);
    custom_picture.set_size_request(168, 94);
    custom_picture.add_css_class("wallpaper-thumb");
    let custom_name = gtk::Label::new(Some(&tr("Your picture")));
    let custom_button = card_with_label(&custom_picture, &custom_name);
    if let Some(leader) = &leader {
        custom_button.set_group(Some(leader));
    }
    let custom_path = choice.borrow().custom.clone();
    if let Some(name) = file_name(&custom_path) {
        custom_picture.set_filename(Some(&custom_path));
        custom_name.set_label(&name);
        custom_button.set_visible(true);
    } else {
        custom_button.set_visible(false);
    }

    let your = gtk::Box::builder().spacing(12).build();
    your.append(&custom_button);
    let choose = gtk::Button::with_label(&tr("Choose Picture…"));
    choose.set_valign(gtk::Align::Center);
    your.append(&choose);

    let refresh_thumbs = {
        let shown = shown.clone();
        let variant = variant.clone();
        move || {
            let variant = VARIANTS[variant.selected() as usize % VARIANTS.len()];
            let dark = scheme_dark();
            for item in shown.borrow().iter() {
                if let Some(paper) = &item.paper {
                    if let Some(path) = wallpaper::thumb(paper, variant, dark) {
                        item.picture.set_filename(Some(path));
                    }
                }
            }
        }
    };

    let persist = {
        let choice = choice.clone();
        let status = status.clone();
        let variant = variant.clone();
        let shown = shown.clone();
        let suppress = suppress.clone();
        Rc::new(move || {
            if suppress.get() {
                return;
            }
            let mut next = choice.borrow().clone();
            next.variant = VARIANTS[variant.selected() as usize % VARIANTS.len()];
            if next.kind == Kind::Dynamic {
                if let Some(item) = shown
                    .borrow()
                    .iter()
                    .find(|item| item.button.is_active() && item.paper.is_some())
                {
                    next.id = item.id.clone();
                }
            }
            *choice.borrow_mut() = next.clone();
            match wallpaper::save(&next) {
                Ok(()) => widgets::set_status(&status, ""),
                Err(e) => widgets::set_status(&status, &format!("{} {e}", tr("Could not save:"))),
            }
        })
    };

    {
        let persist = persist.clone();
        let choice = choice.clone();
        custom_button.connect_toggled(move |button| {
            if button.is_active() {
                choice.borrow_mut().kind = Kind::Custom;
                persist();
            }
        });
    }
    let dynamic_buttons: Vec<_> = shown
        .borrow()
        .iter()
        .map(|item| item.button.clone())
        .collect();
    for button in dynamic_buttons {
        let persist = persist.clone();
        let choice = choice.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                choice.borrow_mut().kind = Kind::Dynamic;
                persist();
            }
        });
    }
    if let Some(settings) = crate::backend::gsettings::interface("color-scheme") {
        let refresh_thumbs = refresh_thumbs.clone();
        let held = settings.clone();
        settings.connect_changed(Some("color-scheme"), move |_, _| {
            let _held = &held;
            refresh_thumbs();
        });
    }

    let on_variant = {
        let refresh_thumbs = refresh_thumbs.clone();
        let persist = persist.clone();
        move || {
            refresh_thumbs();
            persist();
        }
    };
    variant.connect_selected_notify(move |_| on_variant());

    {
        let choice = choice.clone();
        let custom_button = custom_button.clone();
        let custom_picture = custom_picture.clone();
        let custom_name = custom_name.clone();
        let persist = persist.clone();
        let suppress = suppress.clone();
        choose.connect_clicked(move |button| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title(&tr("Choose Picture"));
            dialog.set_modal(true);
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(&tr("Pictures")));
            filter.add_pixbuf_formats();
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            let parent = button
                .root()
                .and_then(|root| root.downcast::<gtk::Window>().ok());
            let choice = choice.clone();
            let custom_button = custom_button.clone();
            let custom_picture = custom_picture.clone();
            let custom_name = custom_name.clone();
            let persist = persist.clone();
            let suppress = suppress.clone();
            dialog.open(
                parent.as_ref(),
                None::<&gtk::gio::Cancellable>,
                move |result| {
                    let Ok(file) = result else {
                        return;
                    };
                    let Some(path) = file.path() else {
                        return;
                    };
                    let Some(path_str) = path.to_str() else {
                        return;
                    };
                    if path_str.contains('\n') {
                        return;
                    }
                    custom_picture.set_filename(Some(&path));
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        custom_name.set_label(name);
                    }
                    custom_button.set_visible(true);
                    choice.borrow_mut().kind = Kind::Custom;
                    choice.borrow_mut().custom = path_str.to_owned();
                    suppress.set(false);
                    if custom_button.is_active() {
                        persist();
                    } else {
                        custom_button.set_active(true);
                    }
                },
            );
        });
    }

    suppress.set(true);
    let (kind, id) = {
        let current = choice.borrow();
        (current.kind, current.id.clone())
    };
    match kind {
        Kind::Custom if custom_button.is_visible() => custom_button.set_active(true),
        _ => {
            let button = shown
                .borrow()
                .iter()
                .find(|item| item.id == id)
                .map(|item| item.button.clone());
            if let Some(button) = button {
                button.set_active(true);
            }
        }
    }
    suppress.set(false);

    if installed.is_empty() {
        page.append(&widgets::caption(&tr(
            "No dynamic wallpapers are installed.",
        )));
    } else {
        page.append(&widgets::group(
            Some(&tr("Dynamic wallpaper")),
            &[
                widgets::plain_row(&flow),
                widgets::row_with_subtitle(
                    &tr("Appearance"),
                    Some(&tr(
                        "Automatic uses the light or dark picture that matches Appearance.",
                    )),
                    &variant,
                ),
            ],
        ));
    }
    page.append(&widgets::group(
        Some(&tr("Your pictures")),
        &[widgets::plain_row(&your)],
    ));
    page.append(&widgets::caption(&tr(
        "A picture you choose stays as it is. It does not switch with Appearance.",
    )));
    page.append(&status);
    page.upcast()
}

fn card_with_label(picture: &gtk::Picture, label: &gtk::Label) -> gtk::ToggleButton {
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .build();
    content.append(picture);
    content.append(label);
    gtk::ToggleButton::builder()
        .child(&content)
        .css_classes(["wallpaper-card"])
        .build()
}

fn file_name(path: &str) -> Option<String> {
    if path.is_empty() || !std::path::Path::new(path).is_file() {
        return None;
    }
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}
