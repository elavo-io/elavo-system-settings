//! Sharing: how this computer receives from devices nearby, in the spirit
//! of macOS's AirDrop & Handoff page.
//!
//! LocalSend is Elavo's AirDrop: Elavo Files receives, and follows what
//! this page writes to `localsend.conf`. Receive From adds Favorites Only,
//! which LocalSend itself lacks: Files declines devices whose fingerprint
//! is not a favorite. A saved PIN is shown only after the person's own
//! password, and then stays shown until System Settings quits.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::sharing::{self, ReceiveFrom};
use crate::backend::{dbus, hostname};
use crate::i18n::tr;
use crate::widgets;

const HIDDEN: &str = "\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}";
const RECEIVE: [ReceiveFrom; 3] = [ReceiveFrom::Off, ReceiveFrom::Favorites, ReceiveFrom::Everyone];

struct Page {
    /// Set while the controls are filled from the files, so their handlers
    /// do not write back.
    filling: Cell<bool>,
    receive: gtk::DropDown,
    auto_accept: gtk::Switch,
    favorites_count: gtk::Label,
    require_pin: gtk::Switch,
    pin_value: gtk::Label,
    pin_buttons: gtk::Box,
    folder: gtk::Button,
    folder_label: gtk::Label,
    name: gtk::Label,
    ls_status: gtk::Label,
}

fn folder_name(path: &std::path::Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    gio::spawn_blocking(f)
        .await
        .unwrap_or_else(|_| Err("the keyring call stopped".to_owned()))
}

impl Page {
    fn fill(self: &Rc<Self>) {
        self.filling.set(true);
        let ls = sharing::localsend();
        let index = RECEIVE.iter().position(|r| *r == ls.receive_from).unwrap_or(2);
        self.receive.set_selected(index as u32);
        self.auto_accept.set_active(ls.auto_accept_favorites);
        self.auto_accept
            .set_sensitive(ls.receive_from != ReceiveFrom::Off);
        self.favorites_count.set_label(&match ls.favorites.len() {
            0 => tr("None"),
            1 => tr("1 device"),
            n => format!("{n} {}", tr("devices")),
        });
        self.require_pin.set_active(ls.require_pin);
        self.pin_buttons.set_sensitive(ls.require_pin);
        if !sharing::revealed() {
            self.pin_value.set_label(if ls.require_pin { HIDDEN } else { "" });
        }
        self.folder_label.set_label(&folder_name(&ls.save_folder));
        self.folder_label
            .set_tooltip_text(Some(&ls.save_folder.to_string_lossy()));

        self.filling.set(false);
        if sharing::revealed() {
            self.reveal_values();
        }
    }

    fn reveal_values(self: &Rc<Self>) {
        let me = self.clone();
        glib::spawn_future_local(async move {
            let pin = blocking(sharing::pin).await;
            me.pin_value.set_label(&match pin {
                Ok(Some(p)) => p,
                Ok(None) => tr("Not set"),
                Err(e) => e,
            });
        });
    }

    /// Ask for the person's password, then show the saved PIN.
    fn reveal(self: &Rc<Self>, status: gtk::Label) {
        let me = self.clone();
        glib::spawn_future_local(async move {
            match sharing::authorize_reveal().await {
                Ok(true) => {
                    widgets::set_status(&status, "");
                    me.reveal_values();
                }
                Ok(false) => {}
                Err(e) => widgets::set_status(
                    &status,
                    &format!("{} {e}", tr("Could not check your password:")),
                ),
            }
        });
    }
}

fn localsend_section(page: &gtk::Box, me: &Rc<Page>) {
    let receive_row = widgets::row_with_subtitle(
        &tr("Receive From"),
        Some(&tr("Who nearby can send files with LocalSend.")),
        &me.receive,
    );
    let auto_row = widgets::row_with_subtitle(
        &tr("Automatically Accept from Favorites"),
        Some(&tr("Files from favorite devices are saved without asking.")),
        &me.auto_accept,
    );

    let edit = gtk::Button::with_label(&tr("Edit\u{2026}"));
    let favorites_box = gtk::Box::builder().spacing(10).build();
    me.favorites_count.add_css_class("dim-label");
    favorites_box.append(&me.favorites_count);
    favorites_box.append(&edit);
    let favorites_row = widgets::row_with_subtitle(
        &tr("Favorites"),
        Some(&tr("Add devices from Files: right-click one under LocalSend.")),
        &favorites_box,
    );

    let pin_row = widgets::row_with_subtitle(
        &tr("Require PIN"),
        Some(&tr("Senders type this PIN before their files are offered.")),
        &me.require_pin,
    );
    let show_pin = gtk::Button::with_label(&tr("Show PIN"));
    let set_pin = gtk::Button::with_label(&tr("Set\u{2026}"));
    me.pin_value.add_css_class("dim-label");
    me.pin_value.set_selectable(true);
    me.pin_buttons.append(&me.pin_value);
    me.pin_buttons.append(&show_pin);
    me.pin_buttons.append(&set_pin);
    let pin_value_row = widgets::row(&tr("PIN"), &me.pin_buttons);

    me.folder.set_label(&tr("Change\u{2026}"));
    me.folder_label.add_css_class("dim-label");
    me.folder_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    me.folder_label.set_max_width_chars(28);
    let folder_box = gtk::Box::builder().spacing(10).build();
    folder_box.append(&me.folder_label);
    folder_box.append(&me.folder);
    let folder_row = widgets::row(&tr("Save To"), &folder_box);

    let about = gtk::Button::with_label(&tr("About\u{2026}"));
    about.set_action_name(Some("app.show-pane"));
    about.set_action_target_value(Some(&"about".to_variant()));
    let name_box = gtk::Box::builder().spacing(10).build();
    me.name.add_css_class("dim-label");
    name_box.append(&me.name);
    name_box.append(&about);
    let name_row = widgets::row_with_subtitle(
        &tr("Device Name"),
        Some(&tr("The computer\u{2019}s name, changed in About.")),
        &name_box,
    );

    page.append(&widgets::group(
        Some(&tr("LocalSend")),
        &[
            receive_row,
            auto_row,
            favorites_row,
            pin_row,
            pin_value_row,
            folder_row,
            name_row,
        ],
    ));
    page.append(&me.ls_status);
    page.append(&widgets::caption(&tr(
        "LocalSend works with the LocalSend app on phones, tablets, and other computers. Elavo Files receives and sends.",
    )));

    // Handlers
    let m = me.clone();
    me.receive.connect_selected_notify(move |dd| {
        if m.filling.get() {
            return;
        }
        let r = RECEIVE[(dd.selected() as usize).min(2)];
        if let Err(e) = sharing::set_receive_from(r) {
            widgets::set_status(&m.ls_status, &e);
        }
        m.auto_accept.set_sensitive(r != ReceiveFrom::Off);
    });
    let m = me.clone();
    me.auto_accept.connect_active_notify(move |s| {
        if m.filling.get() {
            return;
        }
        if let Err(e) = sharing::set_auto_accept(s.is_active()) {
            widgets::set_status(&m.ls_status, &e);
        }
    });
    let m = me.clone();
    edit.connect_clicked(move |b| favorites_window(b, &m));

    let m = me.clone();
    me.require_pin.connect_active_notify(move |s| {
        if m.filling.get() {
            return;
        }
        let on = s.is_active();
        let m = m.clone();
        let s = s.clone();
        glib::spawn_future_local(async move {
            // Turning it on with no PIN saved asks for one first.
            if on && !matches!(blocking(sharing::pin).await, Ok(Some(_))) && !ask_pin(&s, &m).await {
                m.filling.set(true);
                s.set_active(false);
                m.filling.set(false);
                return;
            }
            if let Err(e) = sharing::set_require_pin(on) {
                widgets::set_status(&m.ls_status, &e);
            }
            m.fill();
        });
    });
    let m = me.clone();
    set_pin.connect_clicked(move |b| {
        let m = m.clone();
        let b = b.clone();
        glib::spawn_future_local(async move {
            ask_pin(&b, &m).await;
            m.fill();
        });
    });
    let m = me.clone();
    show_pin.connect_clicked(move |_| m.reveal(m.ls_status.clone()));

    let m = me.clone();
    me.folder.connect_clicked(move |b| {
        let dialog = gtk::FileDialog::builder()
            .title(tr("Save Received Files To"))
            .initial_folder(&gio::File::for_path(sharing::localsend().save_folder))
            .modal(true)
            .build();
        let m = m.clone();
        dialog.select_folder(
            widgets::parent_window(b).as_ref(),
            gio::Cancellable::NONE,
            move |r| {
                if let Some(path) = r.ok().and_then(|f| f.path()) {
                    if let Err(e) = sharing::set_save_folder(&path) {
                        widgets::set_status(&m.ls_status, &e);
                    }
                    m.fill();
                }
            },
        );
    });
}

/// Ask for a new PIN twice and keep it. True when one was saved.
async fn ask_pin(widget: &impl IsA<gtk::Widget>, me: &Rc<Page>) -> bool {
    let Some(values) = widgets::ask(
        widget,
        &tr("Set LocalSend PIN"),
        &tr("Senders type this PIN in LocalSend. It is kept in your keyring."),
        &[(&tr("PIN"), true), (&tr("Confirm PIN"), true)],
        &tr("Set PIN"),
    )
    .await
    else {
        return false;
    };
    let (pin, again) = (values[0].trim().to_owned(), values[1].trim().to_owned());
    if let Some(problem) = sharing::pin_problem(&pin) {
        widgets::set_status(&me.ls_status, &tr(problem));
        return false;
    }
    if pin != again {
        widgets::set_status(&me.ls_status, &tr("The PINs don\u{2019}t match."));
        return false;
    }
    match blocking(move || sharing::set_pin(&pin)).await {
        Ok(()) => {
            widgets::set_status(&me.ls_status, "");
            true
        }
        Err(e) => {
            widgets::set_status(
                &me.ls_status,
                &format!("{} {e}", tr("Could not save the PIN:")),
            );
            false
        }
    }
}

/// The Favorites, each with Remove. Adding is in Files, which sees the
/// devices nearby.
fn favorites_window(from: &gtk::Button, me: &Rc<Page>) {
    let window = widgets::popup_window(from, &tr("LocalSend Favorites"));
    window.set_default_width(440);
    let root = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .margin_top(18)
        .margin_bottom(18)
        .margin_start(18)
        .margin_end(18)
        .build();
    let (frame, list) = widgets::list();
    let fill: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
    let f = {
        let list = list.clone();
        let fill = fill.clone();
        let me = me.clone();
        move || {
            widgets::clear(&list);
            let favorites = sharing::localsend().favorites;
            if favorites.is_empty() {
                list.append(&widgets::plain_row(&widgets::caption(&tr(
                    "No favorites yet. In Files, right-click a device under LocalSend and choose Add to LocalSend Favorites.",
                ))));
            }
            for fav in favorites {
                let remove = gtk::Button::with_label(&tr("Remove"));
                let short: String = fav.fingerprint.chars().take(16).collect();
                let title = if fav.alias.is_empty() {
                    short.clone()
                } else {
                    fav.alias.clone()
                };
                list.append(&widgets::row_with_subtitle(&title, Some(&short), &remove));
                let fill = fill.clone();
                let me = me.clone();
                remove.connect_clicked(move |_| {
                    if let Err(e) = sharing::remove_favorite(&fav.fingerprint) {
                        widgets::set_status(&me.ls_status, &e);
                    }
                    if let Some(f) = fill.borrow().as_ref() {
                        f();
                    }
                    me.fill();
                });
            }
        }
    };
    f();
    fill.replace(Some(Box::new(f)));
    root.append(&frame);
    let done = gtk::Button::builder()
        .label(tr("Done"))
        .halign(gtk::Align::End)
        .css_classes(["suggested-action"])
        .build();
    let w = window.downgrade();
    done.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    root.append(&done);
    window.set_child(Some(&root));
    widgets::keep_alive(&window, fill);
    window.present();
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let receive = gtk::DropDown::from_strings(&[
        &tr("Off"),
        &tr("Favorites Only"),
        &tr("Everyone"),
    ]);
    let me = Rc::new(Page {
        filling: Cell::new(false),
        receive,
        auto_accept: gtk::Switch::new(),
        favorites_count: gtk::Label::new(None),
        require_pin: gtk::Switch::new(),
        pin_value: gtk::Label::new(None),
        pin_buttons: gtk::Box::builder().spacing(10).build(),
        folder: gtk::Button::new(),
        folder_label: gtk::Label::new(None),
        name: gtk::Label::new(None),
        ls_status: widgets::status_label(),
    });
    localsend_section(&page, &me);
    me.fill();

    // The name is the computer's, from hostnamed; the kernel's until then.
    me.name.set_label(&glib::host_name());
    if let Some(conn) = dbus::system() {
        let name = me.name.clone();
        glib::spawn_future_local(async move {
            let info = hostname::info(&conn).await;
            if !info.pretty.is_empty() {
                name.set_label(&info.pretty);
            } else if !info.hostname.is_empty() {
                name.set_label(&info.hostname);
            }
        });
    }
    // Files can change favorites and the conf behind this page; read them
    // again whenever it is shown.
    let m = me.clone();
    page.connect_map(move |_| m.fill());
    widgets::keep_alive(&page, me);
    page.upcast()
}
