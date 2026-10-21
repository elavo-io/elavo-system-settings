//! A person's picture as a fixed-size circle.
//!
//! `GtkPicture` reports the file's pixel size as its natural size, so a
//! large photo grows the row it sits in. This widget reports `size` by
//! `size` and draws the photo scaled to cover that square, clipped to a
//! circle.

use std::cell::{Cell, RefCell};

use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// The users list. Matches `.user-avatar` in the stylesheet.
pub const LIST: i32 = 48;
/// The sidebar card. Matches `.user-avatar.small`.
pub const SIDEBAR: i32 = 34;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct Avatar {
        pub size: Cell<i32>,
        pub texture: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Avatar {
        const NAME: &'static str = "ElavoSettingsAvatar";
        type Type = super::Avatar;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for Avatar {}

    impl WidgetImpl for Avatar {
        fn measure(&self, _orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let size = self.size.get().max(1);
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
            snapshot.push_rounded_clip(&gsk::RoundedRect::from_rect(bounds, w.min(h) / 2.0));
            let (x, y, cw, ch) = cover(texture.width() as f32, texture.height() as f32, w, h);
            snapshot.append_texture(&texture, &graphene::Rect::new(x, y, cw, ch));
            snapshot.pop();
        }
    }
}

glib::wrapper! {
    pub struct Avatar(ObjectSubclass<imp::Avatar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Avatar {
    fn new(size: i32, texture: gdk::Texture) -> Self {
        let avatar: Self = glib::Object::builder()
            .property("accessible-role", gtk::AccessibleRole::Img)
            .build();
        avatar.imp().size.set(size);
        avatar.imp().texture.replace(Some(texture));
        avatar.set_size_request(size, size);
        avatar.set_halign(gtk::Align::Center);
        avatar.set_valign(gtk::Align::Center);
        avatar.set_hexpand(false);
        avatar.set_vexpand(false);
        avatar.set_overflow(gtk::Overflow::Hidden);
        avatar
    }
}

/// The circle for `icon`, or the letter when there is no picture.
pub fn view(letter: &str, icon: &str, size: i32) -> gtk::Widget {
    let widget: gtk::Widget = match load(icon) {
        Some(texture) => Avatar::new(size, texture).upcast(),
        None => monogram(letter, size).upcast(),
    };
    widget.add_css_class("user-avatar");
    if size == SIDEBAR {
        widget.add_css_class("small");
    }
    widget
}

fn load(path: &str) -> Option<gdk::Texture> {
    if path.is_empty() {
        return None;
    }
    gdk::Texture::from_filename(path).ok()
}

fn monogram(letter: &str, size: i32) -> gtk::Label {
    gtk::Label::builder()
        .label(letter)
        .width_request(size)
        .height_request(size)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build()
}

/// Where `texture` sits so it covers `width` by `height`, centered.
fn cover(texture_w: f32, texture_h: f32, width: f32, height: f32) -> (f32, f32, f32, f32) {
    let scale = (width / texture_w).max(height / texture_h);
    let (cw, ch) = (texture_w * scale, texture_h * scale);
    ((width - cw) / 2.0, (height - ch) / 2.0, cw, ch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wide_picture_is_cropped_on_the_sides() {
        let (x, y, w, h) = cover(200.0, 100.0, 48.0, 48.0);
        assert!((y - 0.0).abs() < 0.05);
        assert!((h - 48.0).abs() < 0.05);
        assert!(w > 48.0);
        assert!(x < 0.0);
        assert!((x + w / 2.0 - 24.0).abs() < 0.05);
    }

    #[test]
    fn tall_picture_is_cropped_above_and_below() {
        let (x, y, w, h) = cover(100.0, 200.0, 48.0, 48.0);
        assert!((x - 0.0).abs() < 0.05);
        assert!((w - 48.0).abs() < 0.05);
        assert!(h > 48.0);
        assert!(y < 0.0);
        assert!((y + h / 2.0 - 24.0).abs() < 0.05);
    }

    #[test]
    fn square_picture_fills_the_circle() {
        let (x, y, w, h) = cover(100.0, 100.0, 48.0, 48.0);
        assert!((x - 0.0).abs() < 0.05);
        assert!((y - 0.0).abs() < 0.05);
        assert!((w - 48.0).abs() < 0.05);
        assert!((h - 48.0).abs() < 0.05);
    }
}
