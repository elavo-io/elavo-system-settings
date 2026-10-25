//! Wallpaper: dynamic pictures in one horizontal row, landscape pictures
//! in another, and a picture the person chooses. The picture always covers
//! the screen. Automatic follows the light or dark appearance, and only a
//! dynamic picture switches.

use std::cell::{Cell, RefCell};
use std::path::Path;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

use crate::backend::accent;
use crate::backend::wallpaper::{self, Dynamic, Kind, Variant};
use crate::i18n::tr;
use crate::widgets;

const VARIANTS: [Variant; 3] = [Variant::Automatic, Variant::Light, Variant::Dark];
const THUMB_W: i32 = 128;
const THUMB_H: i32 = 72;

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

/// A thumbnail that stays `THUMB_W` by `THUMB_H`. `GtkPicture` would report
/// the file's pixel size and stretch the row.
mod thumb {
    use gtk::subclass::prelude::*;

    use super::*;

    mod imp {
        use gtk::subclass::prelude::*;

        use super::*;

        #[derive(Default)]
        pub struct Thumb {
            pub texture: RefCell<Option<gdk::Texture>>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for Thumb {
            const NAME: &'static str = "ElavoWallpaperThumb";
            type Type = super::Thumb;
            type ParentType = gtk::Widget;
        }

        impl ObjectImpl for Thumb {}

        impl WidgetImpl for Thumb {
            fn measure(
                &self,
                orientation: gtk::Orientation,
                _for_size: i32,
            ) -> (i32, i32, i32, i32) {
                let size = if orientation == gtk::Orientation::Horizontal {
                    THUMB_W
                } else {
                    THUMB_H
                };
                (size, size, -1, -1)
            }

            fn snapshot(&self, snapshot: &gtk::Snapshot) {
                let Some(texture) = self.texture.borrow().clone() else {
                    return;
                };
                let widget = self.obj();
                let (w, h) = (widget.width() as f32, widget.height() as f32);
                if w <= 0.0 || h <= 0.0 || texture.width() <= 0 || texture.height() <= 0 {
                    return;
                }
                let bounds = graphene::Rect::new(0.0, 0.0, w, h);
                snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, 6.0));
                let (x, y, cw, ch) = cover(texture.width() as f32, texture.height() as f32, w, h);
                snapshot.append_texture(&texture, &graphene::Rect::new(x, y, cw, ch));
                snapshot.pop();
            }
        }
    }

    glib::wrapper! {
        pub struct Thumb(ObjectSubclass<imp::Thumb>)
            @extends gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl Thumb {
        pub fn new(path: Option<&Path>) -> Self {
            let thumb: Self = glib::Object::builder()
                .property("accessible-role", gtk::AccessibleRole::Img)
                .build();
            thumb.set_size_request(THUMB_W, THUMB_H);
            thumb.set_halign(gtk::Align::Center);
            thumb.set_valign(gtk::Align::Center);
            thumb.set_hexpand(false);
            thumb.set_vexpand(false);
            thumb.set_overflow(gtk::Overflow::Hidden);
            thumb.add_css_class("wallpaper-thumb");
            if let Some(path) = path {
                thumb.set_file(path);
            }
            thumb
        }

        pub fn set_file(&self, path: &Path) {
            let texture = gdk::Texture::from_filename(path).ok();
            self.imp().texture.replace(texture);
            self.queue_draw();
        }
    }
}

/// Where `texture` sits so it covers `width` by `height`, centered.
fn cover(texture_w: f32, texture_h: f32, width: f32, height: f32) -> (f32, f32, f32, f32) {
    let scale = (width / texture_w).max(height / texture_h);
    let (cw, ch) = (texture_w * scale, texture_h * scale);
    ((width - cw) / 2.0, (height - ch) / 2.0, cw, ch)
}

fn style_caption(label: &gtk::Label) {
    label.set_xalign(0.5);
    label.set_justify(gtk::Justification::Center);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    label.set_max_width_chars(14);
    label.set_width_request(THUMB_W);
}

fn card(label: &str, picture: &thumb::Thumb) -> gtk::ToggleButton {
    let caption = gtk::Label::new(Some(label));
    card_with_label(picture, &caption)
}

fn card_with_label(picture: &thumb::Thumb, label: &gtk::Label) -> gtk::ToggleButton {
    style_caption(label);
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .halign(gtk::Align::Center)
        .build();
    content.append(picture);
    content.append(label);
    gtk::ToggleButton::builder()
        .child(&content)
        .css_classes(["wallpaper-card"])
        .hexpand(false)
        .vexpand(false)
        .build()
}

/// A single row that scrolls sideways. The pane is itself in a vertical
/// scroller, and a wrapping grid was stacking these pictures one per line.
fn gallery(buttons: &[gtk::ToggleButton]) -> gtk::ScrolledWindow {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(2)
        .margin_bottom(2)
        .margin_start(2)
        .margin_end(2)
        .halign(gtk::Align::Start)
        .build();
    for button in buttons {
        row.append(button);
    }
    let scroll = gtk::ScrolledWindow::builder()
        .child(&row)
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .propagate_natural_width(false)
        .hexpand(true)
        .css_classes(["wallpaper-strip"])
        .build();
    scroll.set_has_frame(false);
    scroll.set_overlay_scrolling(true);
    scroll
}

fn join_group(leader: &mut Option<gtk::ToggleButton>, button: &gtk::ToggleButton) {
    if let Some(leader) = leader {
        button.set_group(Some(leader));
    } else {
        *leader = Some(button.clone());
    }
}

struct Shown {
    id: String,
    kind: Kind,
    picture: thumb::Thumb,
    paper: Option<Dynamic>,
    button: gtk::ToggleButton,
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let status = widgets::status_label();
    let installed = wallpaper::catalog();
    let landscapes = wallpaper::landscapes();
    let choice = Rc::new(RefCell::new(wallpaper::resolve_id(
        &wallpaper::load(),
        &installed,
        &landscapes,
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

    let mut leader: Option<gtk::ToggleButton> = None;
    let mut dynamic_buttons: Vec<gtk::ToggleButton> = Vec::new();
    let dark = scheme_dark();
    let selected_variant = choice.borrow().variant;
    for paper in &installed {
        let Some(path) = wallpaper::thumb(paper, selected_variant, dark) else {
            continue;
        };
        let picture = thumb::Thumb::new(Some(&path));
        let button = card(&wallpaper::display_name(&paper.id), &picture);
        join_group(&mut leader, &button);
        dynamic_buttons.push(button.clone());
        shown.borrow_mut().push(Shown {
            id: paper.id.clone(),
            kind: Kind::Dynamic,
            picture,
            paper: Some(paper.clone()),
            button,
        });
    }
    let dynamic_gallery = gallery(&dynamic_buttons);

    let mut landscape_buttons: Vec<gtk::ToggleButton> = Vec::new();
    for paper in &landscapes {
        let picture = thumb::Thumb::new(Some(&paper.path));
        let button = card(&wallpaper::landscape_title(&paper.id), &picture);
        if let Some(credit) = wallpaper::landscape_credit(&paper.id) {
            button.set_tooltip_text(Some(&credit));
        }
        join_group(&mut leader, &button);
        landscape_buttons.push(button.clone());
        shown.borrow_mut().push(Shown {
            id: paper.id.clone(),
            kind: Kind::Landscape,
            picture,
            paper: None,
            button,
        });
    }
    let landscape_gallery = gallery(&landscape_buttons);

    let custom_picture = thumb::Thumb::new(None);
    let custom_name = gtk::Label::new(Some(&tr("Your picture")));
    let custom_button = card_with_label(&custom_picture, &custom_name);
    join_group(&mut leader, &custom_button);
    let custom_path = choice.borrow().custom.clone();
    if let Some(name) = file_name(&custom_path) {
        custom_picture.set_file(Path::new(&custom_path));
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
                        item.picture.set_file(&path);
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
            let active = shown
                .borrow()
                .iter()
                .find(|item| item.button.is_active())
                .map(|item| (item.kind, item.id.clone()));
            if next.kind != Kind::Custom {
                if let Some((kind, id)) = active {
                    next.kind = kind;
                    next.id = id;
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
    let catalog_buttons: Vec<_> = shown
        .borrow()
        .iter()
        .map(|item| (item.kind, item.id.clone(), item.button.clone()))
        .collect();
    for (kind, id, button) in catalog_buttons {
        let persist = persist.clone();
        let choice = choice.clone();
        button.connect_toggled(move |button| {
            if button.is_active() {
                let mut current = choice.borrow().clone();
                current.kind = kind;
                current.id = id.clone();
                *choice.borrow_mut() = current;
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
                    custom_picture.set_file(&path);
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
        Kind::Landscape => {
            let button = shown
                .borrow()
                .iter()
                .find(|item| item.kind == Kind::Landscape && item.id == id)
                .map(|item| item.button.clone());
            if let Some(button) = button {
                button.set_active(true);
            }
        }
        Kind::Dynamic | Kind::Custom => {
            let button = shown
                .borrow()
                .iter()
                .find(|item| item.kind == Kind::Dynamic && item.id == id)
                .map(|item| item.button.clone());
            if let Some(button) = button {
                button.set_active(true);
            }
        }
    }
    suppress.set(false);

    if installed.is_empty() && landscapes.is_empty() {
        page.append(&widgets::caption(&tr(
            "No dynamic wallpapers are installed.",
        )));
    } else if !installed.is_empty() {
        page.append(&widgets::heading(&tr("Dynamic wallpapers")));
        page.append(&dynamic_gallery);
        page.append(&widgets::group(
            None,
            &[widgets::row_with_subtitle(
                &tr("Appearance"),
                Some(&tr(
                    "Automatic uses the light or dark picture that matches Appearance.",
                )),
                &variant,
            )],
        ));
    }
    if !landscapes.is_empty() {
        page.append(&widgets::heading(&tr("Landscape")));
        page.append(&landscape_gallery);
        page.append(&widgets::caption(&tr(
            "A landscape picture stays as it is. It does not switch with Appearance.",
        )));
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

fn file_name(path: &str) -> Option<String> {
    if path.is_empty() || !Path::new(path).is_file() {
        return None;
    }
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_picture_is_cropped_on_the_sides() {
        let (x, y, w, h) = cover(200.0, 100.0, 128.0, 72.0);
        assert!((y - 0.0).abs() < 0.05);
        assert!((h - 72.0).abs() < 0.05);
        assert!(w > 128.0);
        assert!(x < 0.0);
    }

    #[test]
    fn tall_picture_fills_the_width() {
        let (x, _y, w, h) = cover(100.0, 200.0, 128.0, 72.0);
        assert!((x - 0.0).abs() < 0.05);
        assert!((w - 128.0).abs() < 0.05);
        assert!(h > 72.0);
    }
}
