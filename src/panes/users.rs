//! Users: the accounts on this computer, their names, pictures, passwords,
//! and whether they can administer it.

use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::accounts::{self, User};
use crate::i18n::tr;
use crate::widgets;

struct Pane {
    conn: gio::DBusConnection,
    list: gtk::ListBox,
    add: gtk::Button,
    status: gtk::Label,
}

fn avatar(user: &User) -> gtk::Widget {
    crate::avatar::view(&user.initial(), &user.icon, crate::avatar::LIST)
}

impl Pane {
    fn refresh(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            match accounts::list(&this.conn).await {
                Some(users) => this.render(&users),
                None => this.render_fallback(),
            }
        });
    }

    /// AccountsService is missing: show who is logged in, and say why the
    /// rest is unavailable.
    fn render_fallback(&self) {
        widgets::clear(&self.list);
        let me = User {
            path: String::new(),
            uid: u64::from(unsafe { libc::getuid() }),
            name: glib::user_name().to_string_lossy().into_owned(),
            real_name: glib::real_name().to_string_lossy().into_owned(),
            admin: false,
            icon: String::new(),
        };
        let content = gtk::Box::builder().spacing(12).build();
        content.append(&avatar(&me));
        content.append(
            &gtk::Label::builder()
                .label(me.display_name())
                .xalign(0.0)
                .build(),
        );
        self.list.append(&widgets::plain_row(&content));
        self.add.set_sensitive(false);
        widgets::set_status(
            &self.status,
            &tr("Accounts can't be changed here because AccountsService is not running."),
        );
    }

    fn render(self: &Rc<Self>, users: &[User]) {
        widgets::clear(&self.list);
        self.add.set_sensitive(true);
        widgets::set_status(&self.status, "");
        for user in users {
            let content = gtk::Box::builder().spacing(12).build();
            content.append(&avatar(user));
            let labels = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .valign(gtk::Align::Center)
                .hexpand(true)
                .build();
            labels.append(
                &gtk::Label::builder()
                    .label(user.display_name())
                    .xalign(0.0)
                    .build(),
            );
            let mut kind = if user.admin {
                tr("Administrator")
            } else {
                tr("Standard")
            };
            if user.is_current() {
                kind = format!("{kind} · {}", tr("You"));
            }
            labels.append(&widgets::caption(&format!("{} · {kind}", user.name)));
            content.append(&labels);
            content.append(&self.menu(user));
            self.list.append(&widgets::plain_row(&content));
        }
    }

    fn menu(self: &Rc<Self>, user: &User) -> gtk::MenuButton {
        let actions = gio::SimpleActionGroup::new();
        let menu = gio::Menu::new();
        menu.append(Some(&tr("Change Name…")), Some("user.name"));
        menu.append(Some(&tr("Change Password…")), Some("user.password"));
        menu.append(Some(&tr("Change Picture…")), Some("user.picture"));
        if !user.is_current() {
            let admin = if user.admin {
                tr("Make Standard User")
            } else {
                tr("Make Administrator")
            };
            let section = gio::Menu::new();
            section.append(Some(&admin), Some("user.admin"));
            section.append(Some(&tr("Delete User…")), Some("user.delete"));
            menu.append_section(None, &section);
        }
        let button = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text(tr("Options"))
            .build();

        let add = |name: &str, f: Rc<dyn Fn(Rc<Pane>, User, gtk::Widget)>| {
            let action = gio::SimpleAction::new(name, None);
            let this = self.clone();
            let user = user.clone();
            let widget: gtk::Widget = button.clone().upcast();
            action.connect_activate(move |_, _| f(this.clone(), user.clone(), widget.clone()));
            actions.add_action(&action);
        };
        add(
            "name",
            Rc::new(|this, user, w| {
                glib::spawn_future_local(async move {
                    let Some(v) = widgets::ask(
                        &w,
                        &tr("Change Name"),
                        "",
                        &[(&tr("Full name"), false)],
                        &tr("Change"),
                    )
                    .await
                    else {
                        return;
                    };
                    let name = v[0].trim().to_owned();
                    if name.is_empty() {
                        return;
                    }
                    this.report(
                        &w,
                        accounts::set_real_name(&this.conn, &user.path, &name).await,
                    );
                });
            }),
        );
        add(
            "password",
            Rc::new(|this, user, w| {
                glib::spawn_future_local(async move {
                    let Some(v) = widgets::ask(
                        &w,
                        &tr("Change Password"),
                        &format!("{} {}.", tr("A new password for"), user.display_name()),
                        &[
                            (&tr("New password"), true),
                            (&tr("Repeat the new password"), true),
                        ],
                        &tr("Change"),
                    )
                    .await
                    else {
                        return;
                    };
                    if v[0] != v[1] {
                        widgets::show_error(&w, &tr("The passwords do not match."), "");
                        return;
                    }
                    if v[0].is_empty() {
                        widgets::show_error(&w, &tr("The password cannot be empty."), "");
                        return;
                    }
                    this.report(
                        &w,
                        accounts::set_password(&this.conn, &user.path, &v[0]).await,
                    );
                });
            }),
        );
        add(
            "picture",
            Rc::new(|this, user, w| {
                glib::spawn_future_local(async move {
                    let filter = gtk::FileFilter::new();
                    filter.set_name(Some(&tr("Images")));
                    filter.add_pixbuf_formats();
                    let filters = gio::ListStore::new::<gtk::FileFilter>();
                    filters.append(&filter);
                    let dialog = gtk::FileDialog::builder()
                        .title(tr("Choose a Picture"))
                        .filters(&filters)
                        .modal(true)
                        .build();
                    let parent = widgets::parent_window(&w);
                    let Ok(file) = dialog.open_future(parent.as_ref()).await else {
                        return;
                    };
                    let Some(path) = file.path() else {
                        return;
                    };
                    this.report(
                        &w,
                        accounts::set_icon(&this.conn, &user.path, &path.to_string_lossy()).await,
                    );
                });
            }),
        );
        add(
            "admin",
            Rc::new(|this, user, w| {
                glib::spawn_future_local(async move {
                    this.report(
                        &w,
                        accounts::set_admin(&this.conn, &user.path, !user.admin).await,
                    );
                });
            }),
        );
        add(
            "delete",
            Rc::new(|this, user, w| {
                glib::spawn_future_local(async move {
                    let (cancel, keep, delete) =
                        (tr("Cancel"), tr("Keep Files"), tr("Delete Files"));
                    let answer = widgets::choose(
                        &w,
                        &format!("{} {}?", tr("Delete"), user.display_name()),
                        &tr("Their files can be kept in /home, or deleted with the account."),
                        &[&cancel, &keep, &delete],
                        0,
                        0,
                    )
                    .await;
                    let remove_files = match answer {
                        1 => false,
                        2 => true,
                        _ => return,
                    };
                    this.report(
                        &w,
                        accounts::delete(&this.conn, user.uid, remove_files).await,
                    );
                });
            }),
        );
        button.insert_action_group("user", Some(&actions));
        button
    }

    fn report(self: &Rc<Self>, w: &gtk::Widget, result: Result<(), glib::Error>) {
        if let Err(e) = result {
            widgets::show_error(
                w,
                &tr("The account could not be changed."),
                &widgets::error_text(&e),
            );
        }
        self.refresh();
    }

    fn add_user(self: &Rc<Self>) {
        let this = self.clone();
        let w: gtk::Widget = self.add.clone().upcast();
        glib::spawn_future_local(async move {
            let Some(v) = widgets::ask(
                &w,
                &tr("Add a User"),
                &tr("The account name is used to log in and for the home folder. It cannot be changed later."),
                &[
                    (&tr("Full name"), false),
                    (&tr("Account name"), false),
                    (&tr("Password"), true),
                    (&tr("Repeat the password"), true),
                ],
                &tr("Add User"),
            )
            .await
            else {
                return;
            };
            let full = v[0].trim().to_owned();
            let mut name = v[1].trim().to_owned();
            if name.is_empty() {
                name = accounts::suggest_username(&full);
            }
            if !accounts::valid_username(&name) {
                widgets::show_error(
                    &w,
                    &tr("That account name can't be used."),
                    &tr("Use lowercase letters, digits, - and _, starting with a letter."),
                );
                return;
            }
            if v[2] != v[3] {
                widgets::show_error(&w, &tr("The passwords do not match."), "");
                return;
            }
            let result = accounts::create(&this.conn, &name, &full, false, &v[2]).await;
            this.report(&w, result);
        });
    }
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let Some(conn) = accounts::bus() else {
        return widgets::placeholder(
            "system-users-symbolic",
            &tr("Users"),
            &tr("The system bus is not available."),
        );
    };
    let (frame, list) = widgets::list();
    page.append(&widgets::heading(&tr("Users")));
    page.append(&frame);
    let add = gtk::Button::builder()
        .label(tr("Add User…"))
        .halign(gtk::Align::End)
        .build();
    page.append(&add);
    let status = widgets::status_label();
    page.append(&status);
    page.append(&widgets::caption(&tr(
        "Adding, deleting, and changing another person's account asks for an administrator's password.",
    )));

    let pane = Rc::new(Pane {
        conn: conn.clone(),
        list,
        add,
        status,
    });
    let p = pane.clone();
    pane.add.connect_clicked(move |_| p.add_user());
    let weak = Rc::downgrade(&pane);
    let sub = crate::backend::dbus::on_signal(
        &conn,
        "org.freedesktop.Accounts",
        None,
        None,
        move |_, name, _| {
            if matches!(name, "UserAdded" | "UserDeleted" | "Changed") {
                if let Some(p) = weak.upgrade() {
                    p.refresh();
                }
            }
        },
    );
    pane.refresh();
    widgets::keep_alive(&page, (pane, sub));
    page.upcast()
}
