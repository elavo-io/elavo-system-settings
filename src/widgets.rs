//! The pieces every pane is made of: a page, rounded groups of rows, and
//! rows with a title on the left and a control on the right. The look
//! matches `elavo_dock_settings::page()` so the embedded Dock page does not
//! stand out.

use std::any::Any;
use std::cell::RefCell;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::i18n::tr;

/// A vertical page with the standard padding.
pub fn page() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .css_classes(["pane"])
        .build()
}

pub fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["group-title"])
        .build()
}

pub fn caption(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .css_classes(["caption"])
        .build()
}

/// An empty rounded group. Rows are appended to the returned list.
pub fn list() -> (gtk::Frame, gtk::ListBox) {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["rich-list"])
        .build();
    let frame = gtk::Frame::builder().child(&list).build();
    (frame, list)
}

/// A rounded group of rows, with an optional title above it.
pub fn group(title: Option<&str>, rows: &[gtk::ListBoxRow]) -> gtk::Box {
    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    if let Some(title) = title {
        outer.append(&heading(title));
    }
    let (frame, list) = list();
    for row in rows {
        list.append(row);
    }
    outer.append(&frame);
    outer
}

fn title_box(title: &str, subtitle: Option<&str>) -> gtk::Box {
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    labels.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .wrap(true)
            .build(),
    );
    if let Some(subtitle) = subtitle {
        labels.append(&caption(subtitle));
    }
    labels
}

/// Title on the left, the control on the right.
pub fn row(title: &str, control: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    row_with_subtitle(title, None, control)
}

pub fn row_with_subtitle(
    title: &str,
    subtitle: Option<&str>,
    control: &impl IsA<gtk::Widget>,
) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(12).build();
    content.append(&title_box(title, subtitle));
    control.set_valign(gtk::Align::Center);
    content.append(control);
    plain_row(&content)
}

pub fn plain_row(child: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    gtk::ListBoxRow::builder()
        .activatable(false)
        .selectable(false)
        .child(child)
        .build()
}

/// A pane's icon on its colored rounded square. `size` is `small` for the
/// sidebar and rows, `large` for a hub page's header.
pub fn tile(icon: &str, color: &str, size: &str) -> gtk::Image {
    gtk::Image::builder()
        .icon_name(icon)
        .pixel_size(if size == "large" { 34 } else { 14 })
        .css_classes(["pane-tile", color, size])
        .valign(gtk::Align::Center)
        .build()
}

/// A row that opens another pane: its tile, its title, an optional status
/// on the right, and a chevron. Without a target it is a plain status row.
pub fn nav_row(
    icon: &str,
    color: &str,
    title: &str,
    status: Option<&str>,
    target: Option<&str>,
) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(10).build();
    content.append(&tile(icon, color, "small"));
    content.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    if let Some(status) = status {
        content.append(
            &gtk::Label::builder()
                .label(status)
                .css_classes(["dim-label"])
                .build(),
        );
    }
    let row = gtk::ListBoxRow::builder()
        .selectable(false)
        .activatable(target.is_some())
        .child(&content)
        .build();
    if let Some(target) = target {
        content.append(
            &gtk::Image::builder()
                .icon_name("go-next-symbolic")
                .css_classes(["dim-label", "nav-chevron"])
                .build(),
        );
        row.add_css_class("nav-row");
        row.set_action_name(Some("app.show-pane"));
        row.set_action_target_value(Some(&target.to_variant()));
    }
    row
}

/// The top of a hub page: a large tile, the title, and what it covers.
pub fn hub_header(icon: &str, color: &str, title: &str, text: &str) -> gtk::Box {
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .css_classes(["hub-header"])
        .build();
    let t = tile(icon, color, "large");
    t.set_halign(gtk::Align::Center);
    header.append(&t);
    header.append(
        &gtk::Label::builder()
            .label(title)
            .css_classes(["hub-title"])
            .build(),
    );
    header.append(
        &gtk::Label::builder()
            .label(text)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .max_width_chars(60)
            .css_classes(["caption"])
            .build(),
    );
    header
}

/// A read-only value. The label is returned so it can be updated.
pub fn value_row(title: &str, value: &str) -> (gtk::ListBoxRow, gtk::Label) {
    let label = gtk::Label::builder()
        .label(value)
        .xalign(1.0)
        .selectable(true)
        .wrap(true)
        .css_classes(["dim-label"])
        .build();
    (row(title, &label), label)
}

pub fn switch_row(title: &str, subtitle: Option<&str>) -> (gtk::ListBoxRow, gtk::Switch) {
    let switch = gtk::Switch::new();
    (row_with_subtitle(title, subtitle, &switch), switch)
}

pub fn slider(min: f64, max: f64, step: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_draw_value(false);
    scale.set_hexpand(true);
    scale.set_width_request(220);
    scale
}

/// A slider with captions at both ends, the way the Dock page draws them.
pub fn slider_row(title: &str, scale: &gtk::Scale, low: &str, high: &str) -> gtk::ListBoxRow {
    let slider = gtk::Box::builder().spacing(8).build();
    slider.append(&caption(low));
    slider.append(scale);
    slider.append(&caption(high));
    row(title, &slider)
}

/// A pane that is not built yet: an icon, a title, and a sentence.
pub fn placeholder(icon: &str, title: &str, text: &str) -> gtk::Widget {
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .vexpand(true)
        .css_classes(["placeholder-pane"])
        .build();
    root.append(&gtk::Image::builder().icon_name(icon).pixel_size(64).build());
    root.append(
        &gtk::Label::builder()
            .label(title)
            .css_classes(["title-2"])
            .build(),
    );
    root.append(
        &gtk::Label::builder()
            .label(text)
            .wrap(true)
            .max_width_chars(48)
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build(),
    );
    root.upcast()
}

/// Ties `value` to the widget's lifetime, so pane state and D-Bus
/// subscriptions live exactly as long as the pane.
pub fn keep_alive(widget: &impl IsA<gtk::Widget>, value: impl Any) {
    let slot = RefCell::new(Some(Box::new(value) as Box<dyn Any>));
    widget.connect_destroy(move |_| {
        slot.take();
    });
}

/// Remove every row of a list box.
pub fn clear(list: &gtk::ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

pub fn parent_window(widget: &impl IsA<gtk::Widget>) -> Option<gtk::Window> {
    widget.root().and_downcast::<gtk::Window>()
}

/// A yes/no question. Returns true when the confirm button was pressed.
pub async fn confirm(
    widget: &impl IsA<gtk::Widget>,
    message: &str,
    detail: &str,
    confirm_label: &str,
) -> bool {
    let cancel = tr("Cancel");
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .buttons([cancel.as_str(), confirm_label])
        .cancel_button(0)
        .default_button(1)
        .modal(true)
        .build();
    let parent = parent_window(widget);
    matches!(dialog.choose_future(parent.as_ref()).await, Ok(1))
}

pub fn show_error(widget: &impl IsA<gtk::Widget>, message: &str, detail: &str) {
    let dialog = gtk::AlertDialog::builder()
        .message(message)
        .detail(detail)
        .modal(true)
        .build();
    dialog.show(parent_window(widget).as_ref());
}

/// The text of a D-Bus or other GLib error without the remote error name.
pub fn error_text(error: &glib::Error) -> String {
    let message = error.message();
    if error.matches(gio::DBusError::AccessDenied) || message.contains("NotAuthorized") {
        return tr("You are not allowed to make this change.");
    }
    let stripped = gio::DBusError::remote_error(error).and_then(|name| {
        message
            .strip_prefix(&format!("GDBus.Error:{name}: "))
            .map(str::to_owned)
    });
    stripped.unwrap_or_else(|| message.to_string())
}

/// One or more text fields in a small modal window. Returns the values when
/// the user presses the confirm button, `None` on cancel.
pub async fn ask(
    widget: &impl IsA<gtk::Widget>,
    title: &str,
    detail: &str,
    fields: &[(&str, bool)],
    confirm_label: &str,
) -> Option<Vec<String>> {
    let (tx, rx) = async_channel::bounded::<Option<Vec<String>>>(1);
    let window = gtk::Window::builder()
        .title(title)
        .modal(true)
        .resizable(false)
        .default_width(380)
        .build();
    if let Some(parent) = parent_window(widget) {
        window.set_transient_for(Some(&parent));
    }
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    root.append(
        &gtk::Label::builder()
            .label(title)
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );
    if !detail.is_empty() {
        root.append(&caption(detail));
    }
    let entries: Vec<gtk::Widget> = fields
        .iter()
        .map(|(placeholder, secret)| {
            let entry: gtk::Widget = if *secret {
                gtk::PasswordEntry::builder()
                    .placeholder_text(*placeholder)
                    .show_peek_icon(true)
                    .activates_default(true)
                    .build()
                    .upcast()
            } else {
                gtk::Entry::builder()
                    .placeholder_text(*placeholder)
                    .activates_default(true)
                    .build()
                    .upcast()
            };
            root.append(&entry);
            entry
        })
        .collect();
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(6)
        .build();
    let cancel = gtk::Button::with_label(&tr("Cancel"));
    let ok = gtk::Button::builder()
        .label(confirm_label)
        .css_classes(["suggested-action"])
        .build();
    buttons.append(&cancel);
    buttons.append(&ok);
    root.append(&buttons);
    window.set_child(Some(&root));
    window.set_default_widget(Some(&ok));

    let tx_cancel = tx.clone();
    let w = window.downgrade();
    cancel.connect_clicked(move |_| {
        let _ = tx_cancel.try_send(None);
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let tx_ok = tx.clone();
    let w = window.downgrade();
    ok.connect_clicked(move |_| {
        let values = entries
            .iter()
            .map(|e| {
                e.downcast_ref::<gtk::Editable>()
                    .map(|e| e.text().to_string())
                    .unwrap_or_default()
            })
            .collect();
        let _ = tx_ok.try_send(Some(values));
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    window.connect_close_request(move |_| {
        let _ = tx.try_send(None);
        glib::Propagation::Proceed
    });
    window.present();
    rx.recv().await.ok().flatten()
}

/// A status line under a group, hidden while empty.
pub fn status_label() -> gtk::Label {
    let label = caption("");
    label.set_visible(false);
    label
}

pub fn set_status(label: &gtk::Label, text: &str) {
    label.set_label(text);
    label.set_visible(!text.is_empty());
}
