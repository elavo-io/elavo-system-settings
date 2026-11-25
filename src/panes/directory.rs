//! Domain: join this computer to an Active Directory domain or a FreeIPA
//! realm, choose who can sign in and who administers it, and leave again.
//! Opened from Users & Groups.
//!
//! realmd finds, joins, and leaves the directory and asks polkit for an
//! administrator itself. The image's `elavo-directory` then sets this
//! computer up (short names, homes made at first sign-in, offline sign-in)
//! and undoes it after leaving; this page waits for it. Active Directory
//! keeps who can sign in on the computer, through realmd; FreeIPA keeps it
//! on the server, in the HBAC rule `elavo-<hostname>`. Administrator groups
//! go through the helper into `/etc/elavo/directory.conf`.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::realm::{self, Kind, Policy, Realm};
use crate::backend::{directory, ipa};
use crate::i18n::tr;
use crate::widgets;

/// Pause after the last keystroke before asking realmd about the name.
const DISCOVER_DELAY_MS: u64 = 700;

struct Pane {
    conn: gio::DBusConnection,
    stack: gtk::Stack,
    // Joining
    domain: gtk::Entry,
    found_icon: gtk::Image,
    found_spinner: gtk::Spinner,
    found: gtk::Label,
    warning: gtk::Label,
    credentials: gtk::Box,
    admin: gtk::Entry,
    password: gtk::PasswordEntry,
    ou_row: gtk::ListBoxRow,
    ou: gtk::Entry,
    join: gtk::Button,
    stop: gtk::Button,
    join_status: gtk::Label,
    log: gtk::Label,
    log_expander: gtk::Expander,
    discovered: RefCell<Option<Realm>>,
    discover_cancel: RefCell<Option<gio::Cancellable>>,
    discover_seq: Cell<u32>,
    operation: RefCell<Option<String>>,
    busy: Cell<bool>,
    // Joined
    joined_box: gtk::Box,
    joined: RefCell<Option<Realm>>,
    ipa: RefCell<Option<Rc<ipa::Session>>>,
    ipa_rule: RefCell<Option<(ipa::Rule, bool)>>,
    // Groups set aside while everyone may sign in, put back on the way out.
    saved_groups: RefCell<Vec<String>>,
}

fn status_icon(name: &str) -> gtk::Image {
    gtk::Image::builder()
        .icon_name(name)
        .pixel_size(16)
        .valign(gtk::Align::Center)
        .build()
}

/// A row with a group's name and a button that removes it.
fn group_row(name: &str, on_remove: impl Fn() + 'static) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(10).build();
    content.append(&status_icon("system-users-symbolic"));
    content.append(
        &gtk::Label::builder()
            .label(name)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    let remove = gtk::Button::builder()
        .icon_name("list-remove-symbolic")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .tooltip_text(tr("Remove"))
        .build();
    remove.connect_clicked(move |_| on_remove());
    content.append(&remove);
    widgets::plain_row(&content)
}

/// A read-only value that stays on one line.
fn value(title: &str, text: &str) -> (gtk::ListBoxRow, gtk::Label) {
    let (row, label) = widgets::value_row(title, text);
    label.set_wrap(false);
    (row, label)
}

fn empty_row(text: &str) -> gtk::ListBoxRow {
    widgets::plain_row(
        &gtk::Label::builder()
            .label(text)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build(),
    )
}

/// This computer's name as FreeIPA enrolled it.
fn ipa_host() -> String {
    std::fs::read_to_string("/etc/ipa/default.conf")
        .ok()
        .and_then(|text| {
            text.lines().find_map(|line| {
                let (key, value) = line.split_once('=')?;
                (key.trim() == "host").then(|| value.trim().to_owned())
            })
        })
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| glib::host_name().to_string())
}

fn sign_in_example(realm: &Realm) -> String {
    if directory::short_names() {
        tr("Their short name, such as ada")
    } else {
        realm
            .login_formats
            .first()
            .map(|f| f.replace("%U", "ada"))
            .unwrap_or_else(|| format!("ada@{}", realm.name))
    }
}

impl Pane {
    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        self.domain.set_sensitive(!busy);
        self.admin.set_sensitive(!busy);
        self.password.set_sensitive(!busy);
        self.ou.set_sensitive(!busy);
        self.stop.set_visible(busy);
        self.update_join_button();
    }

    fn update_join_button(&self) {
        let ready = self
            .discovered
            .borrow()
            .as_ref()
            .is_some_and(|r| !r.configured)
            && !self.admin.text().trim().is_empty()
            && !self.password.text().is_empty();
        self.join.set_sensitive(ready && !self.busy.get());
    }

    /// Show what realmd knows: the join page or the joined page.
    fn refresh(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            match realm::joined(&this.conn).await {
                Ok(list) => match list.into_iter().next() {
                    Some(r) => this.render_joined(r).await,
                    None => {
                        this.joined.replace(None);
                        this.stack.set_visible_child_name("join");
                    }
                },
                Err(e) => {
                    let page = widgets::placeholder(
                        "network-workgroup-symbolic",
                        &tr("Domain"),
                        &format!(
                            "{}\n\n{}",
                            tr("realmd, which joins directories, is not available."),
                            widgets::error_text(&e)
                        ),
                    );
                    if let Some(old) = this.stack.child_by_name("error") {
                        this.stack.remove(&old);
                    }
                    this.stack.add_named(&page, Some("error"));
                    this.stack.set_visible_child_name("error");
                }
            }
        });
    }

    // ----- Joining -----

    fn domain_changed(self: &Rc<Self>) {
        if let Some(c) = self.discover_cancel.take() {
            c.cancel();
        }
        self.discovered.replace(None);
        self.credentials.set_visible(false);
        self.warning.set_visible(false);
        self.found_spinner.set_visible(false);
        self.found_spinner.set_spinning(false);
        self.update_join_button();
        let text = self
            .domain
            .text()
            .trim()
            .trim_end_matches('.')
            .to_lowercase();
        let seq = self.discover_seq.get().wrapping_add(1);
        self.discover_seq.set(seq);
        if text.is_empty() {
            self.found_icon
                .set_icon_name(Some("dialog-information-symbolic"));
            self.found.set_label(&tr(
                "Type the domain name, such as corp.example.com. Elavo finds the directory as you type.",
            ));
            return;
        }
        if !realm::plausible_domain(&text) {
            self.found_icon
                .set_icon_name(Some("dialog-information-symbolic"));
            self.found
                .set_label(&tr("A domain name has a dot, such as corp.example.com."));
            return;
        }
        let this = self.clone();
        glib::timeout_add_local_once(
            std::time::Duration::from_millis(DISCOVER_DELAY_MS),
            move || {
                if this.discover_seq.get() == seq {
                    this.discover(text, seq);
                }
            },
        );
    }

    fn discover(self: &Rc<Self>, domain: String, seq: u32) {
        let cancel = gio::Cancellable::new();
        self.discover_cancel.replace(Some(cancel.clone()));
        self.found_icon.set_visible(false);
        self.found_spinner.set_visible(true);
        self.found_spinner.set_spinning(true);
        self.found
            .set_label(&format!("{} {domain}…", tr("Looking for")));
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = realm::discover(&this.conn, &domain, &cancel).await;
            if this.discover_seq.get() != seq || cancel.is_cancelled() {
                return;
            }
            let servers = match &result {
                Ok(Some(r)) => realm::servers(&r.name, r.kind).await,
                _ => Vec::new(),
            };
            if this.discover_seq.get() != seq {
                return;
            }
            this.found_spinner.set_spinning(false);
            this.found_spinner.set_visible(false);
            this.found_icon.set_visible(true);
            match result {
                Ok(Some(r)) => this.show_found(r, &servers),
                Ok(None) => {
                    this.found_icon
                        .set_icon_name(Some("dialog-warning-symbolic"));
                    this.found.set_label(&format!(
                        "{} {domain}. {}",
                        tr("No Active Directory domain or FreeIPA realm answered at"),
                        tr("Check the name, and that this computer uses the directory's DNS servers.")
                    ));
                }
                Err(e) => {
                    this.found_icon
                        .set_icon_name(Some("dialog-warning-symbolic"));
                    this.found.set_label(&realm::error_text(&e));
                }
            }
        });
    }

    fn show_found(self: &Rc<Self>, r: Realm, servers: &[String]) {
        self.found_icon
            .set_icon_name(Some("object-select-symbolic"));
        let mut line = format!("{} {}", r.kind_name(), r.realm_name);
        if let Some(server) = servers.first() {
            line.push_str(&format!(" · {} {server}", tr("server")));
        }
        self.found.set_label(&line);
        let mut warnings = Vec::new();
        if realm::is_mdns_domain(&r.name) {
            warnings.push(tr(
                "Names ending in .local are also used by nearby devices, so this domain may not resolve reliably.",
            ));
        }
        if servers.is_empty() {
            warnings.push(tr(
                "DNS has no directory server records for this domain. Joining may fail.",
            ));
        }
        if r.kind == Kind::FreeIpa {
            if let Some(host) = realm::hostname_problem(&glib::host_name()) {
                warnings.push(format!(
                    "{} \"{host}\" {} {}.",
                    tr("FreeIPA enrolls this computer by its full name, and"),
                    tr("is not one. Set a name such as"),
                    format_args!("{host}.{}", r.name)
                ));
            }
        }
        if r.configured {
            warnings.push(tr("This computer is already joined to this domain."));
        }
        widgets::set_status(&self.warning, &warnings.join("\n"));
        if self.admin.text().is_empty()
            || self.admin.text() == "Administrator"
            || self.admin.text() == "admin"
        {
            let suggested = if r.suggested_admin.is_empty() {
                match r.kind {
                    Kind::FreeIpa => "admin".to_owned(),
                    _ => "Administrator".to_owned(),
                }
            } else {
                r.suggested_admin.clone()
            };
            self.admin.set_text(&suggested);
        }
        self.ou_row.set_visible(r.kind == Kind::ActiveDirectory);
        self.credentials.set_visible(true);
        self.discovered.replace(Some(r));
        self.update_join_button();
    }

    fn log_line(&self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        let mut text = self.log.text().to_string();
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(line);
        self.log.set_label(&text);
        self.log_expander.set_visible(true);
        let short = line.trim_start_matches(['*', ' ']);
        widgets::set_status(&self.join_status, short);
    }

    fn start_join(self: &Rc<Self>) {
        let Some(r) = self.discovered.borrow().clone() else {
            return;
        };
        let admin = self.admin.text().trim().to_owned();
        let password = self.password.text().to_string();
        let ou = self.ou.text().trim().to_owned();
        let operation = realm::operation_id();
        self.operation.replace(Some(operation.clone()));
        self.log.set_label("");
        self.log_expander.set_visible(false);
        self.set_busy(true);
        widgets::set_status(
            &self.join_status,
            &format!("{} {}…", tr("Joining"), r.realm_name),
        );
        let this = self.clone();
        let weak = Rc::downgrade(self);
        let sub = realm::on_diagnostics(&self.conn, &operation, move |line| {
            if let Some(p) = weak.upgrade() {
                p.log_line(line);
            }
        });
        glib::spawn_future_local(async move {
            // One prompt covers the join and the login-policy change
            // that follows on Active Directory (see JOIN_ACTION).
            match realm::authorize_join().await {
                Ok(true) => {}
                Ok(false) => {
                    this.operation.replace(None);
                    drop(sub);
                    this.set_busy(false);
                    widgets::set_status(&this.join_status, "");
                    return;
                }
                Err(e) => {
                    this.operation.replace(None);
                    drop(sub);
                    this.set_busy(false);
                    widgets::set_status(&this.join_status, "");
                    widgets::show_error(&this.join, &tr("Could not join the directory"), &e);
                    return;
                }
            }
            let result = realm::join(&this.conn, &r, &admin, &password, &ou, &operation).await;
            this.operation.replace(None);
            if let Err(e) = result {
                drop(sub);
                this.set_busy(false);
                widgets::set_status(&this.join_status, "");
                let text = realm::error_text(&e);
                let log = this.log.text().to_string();
                widgets::show_error_action(
                    &this.join,
                    &format!("{} {}", tr("Could not join"), r.realm_name),
                    &text,
                    &log,
                    "",
                )
                .await;
                return;
            }
            widgets::set_status(&this.join_status, &tr("Joined. Setting up this computer…"));
            // Who can sign in starts closed: Active Directory lets nobody in
            // until groups are chosen; FreeIPA gets this computer's rule.
            let mut follow_up: Option<String> = None;
            if let Some(joined) = realm::joined(&this.conn)
                .await
                .ok()
                .and_then(|l| l.into_iter().next())
            {
                match joined.kind {
                    Kind::ActiveDirectory => {
                        if let Err(e) =
                            realm::set_policy(&this.conn, &joined, Policy::Nobody, &[], &[]).await
                        {
                            follow_up = Some(format!(
                                "{} {}",
                                tr("Everyone in the directory can sign in until you choose groups."),
                                realm::error_text(&e)
                            ));
                        }
                    }
                    Kind::FreeIpa => {
                        match ipa::Session::open(&admin, &joined.realm_name, &password).await {
                            Ok(session) => {
                                let session = Rc::new(session);
                                let rule = ipa::rule_name(&ipa_host());
                                if let Err(e) =
                                    session.set_rule(&rule, &ipa_host(), false, &[]).await
                                {
                                    follow_up = Some(format!(
                                        "{} {e}",
                                        tr("The sign-in rule could not be made.")
                                    ));
                                }
                                this.ipa.replace(Some(session));
                            }
                            Err(e) => follow_up = Some(e),
                        }
                    }
                    Kind::Other => {}
                }
            }
            drop(password);
            directory::wait_applied(true, 40).await;
            drop(sub);
            this.password.set_text("");
            this.set_busy(false);
            widgets::set_status(&this.join_status, "");
            this.refresh();
            if let Some(text) = follow_up {
                widgets::show_error(&this.join, &tr("Joined, with a problem"), &text);
            }
            // The join left a ticket; show the new rule while it lasts.
            if this.ipa.borrow().is_some() {
                this.load_ipa_rule().await;
            }
        });
    }

    // ----- Joined -----

    async fn render_joined(self: &Rc<Self>, r: Realm) {
        let b = &self.joined_box;
        while let Some(child) = b.first_child() {
            b.remove(&child);
        }
        let servers = realm::servers(&r.name, r.kind).await;
        let server = servers.first().cloned();

        // Status card
        let (server_row, server_label) = value(
            &tr("Server"),
            &server.clone().unwrap_or_else(|| tr("None found in DNS")),
        );
        let setup = if directory::applied() {
            tr("Done")
        } else if directory::supported() {
            tr("Not finished")
        } else {
            tr("Not available in this system image")
        };
        b.append(&widgets::group(
            Some(&tr("Directory")),
            &[
                value(&tr("Domain"), &r.realm_name).0,
                value(&tr("Type"), &r.kind_name()).0,
                server_row,
                value(&tr("People sign in with"), &sign_in_example(&r)).0,
                value(&tr("Elavo setup"), &setup).0,
            ],
        ));
        if let Some(server) = server {
            let label = server_label.clone();
            glib::spawn_future_local(async move {
                let state = if realm::reachable(&server).await {
                    tr("reachable")
                } else {
                    tr("not reachable; saved sign-ins still work")
                };
                label.set_label(&format!("{server} · {state}"));
            });
        }
        b.append(&widgets::caption(&tr(
            "People with directory accounts get a home folder on this computer the first time they sign in, and can sign in again when the directory can't be reached.",
        )));

        self.joined.replace(Some(r.clone()));
        match r.kind {
            Kind::FreeIpa => self.render_ipa_sign_in(&r),
            _ => self.render_ad_sign_in(&r),
        }
        self.render_admins(&r);

        let leave = gtk::Button::builder()
            .label(tr("Leave Domain…"))
            .halign(gtk::Align::End)
            .css_classes(["destructive-action"])
            .margin_top(12)
            .build();
        let this = self.clone();
        leave.connect_clicked(move |button| this.leave(button));
        b.append(&leave);
        b.append(&widgets::caption(&tr(
            "Leaving keeps everyone's files. Joining, leaving, and these choices ask for an administrator's password.",
        )));
        self.stack.set_visible_child_name("joined");
    }

    fn sign_in_group(
        &self,
        rows: Vec<gtk::ListBoxRow>,
        everyone: bool,
        on_mode: impl Fn(bool) + 'static,
        on_add: impl Fn(gtk::Button) + 'static,
    ) {
        let only = gtk::CheckButton::with_label(&tr("Only these groups"));
        let all = gtk::CheckButton::with_label(&tr("Everyone in the directory"));
        all.set_group(Some(&only));
        only.set_active(!everyone);
        all.set_active(everyone);
        let on_mode = Rc::new(on_mode);
        let m = on_mode.clone();
        all.connect_toggled(move |b| {
            if b.is_active() {
                m(true);
            }
        });
        only.connect_toggled(move |b| {
            if b.is_active() {
                on_mode(false);
            }
        });
        let mut all_rows = vec![widgets::plain_row(&only)];
        if !everyone {
            all_rows.extend(rows);
        }
        all_rows.push(widgets::plain_row(&all));
        self.joined_box
            .append(&widgets::group(Some(&tr("Who Can Sign In")), &all_rows));
        let add = gtk::Button::builder()
            .label(tr("Add Group…"))
            .halign(gtk::Align::End)
            .sensitive(!everyone)
            .build();
        add.connect_clicked(move |b| on_add(b.clone()));
        self.joined_box.append(&add);
    }

    fn render_ad_sign_in(self: &Rc<Self>, r: &Realm) {
        let everyone = r.policy == Policy::Everyone;
        let mut rows: Vec<gtk::ListBoxRow> = Vec::new();
        for g in &r.permitted_groups {
            let this = self.clone();
            let realm = r.clone();
            let group = g.clone();
            rows.push(group_row(g, move || {
                let this = this.clone();
                let realm = realm.clone();
                let group = group.clone();
                glib::spawn_future_local(async move {
                    let left =
                        realm.permitted_groups.len() <= 1 && realm.permitted_logins.is_empty();
                    // Removing the last group would leave realmd letting
                    // everyone in; keep it closed instead.
                    let policy = if left {
                        Policy::Nobody
                    } else {
                        Policy::Permitted
                    };
                    let result = realm::set_policy(&this.conn, &realm, policy, &[], &[group]).await;
                    this.after_change(result.map_err(|e| realm::error_text(&e)));
                });
            }));
        }
        if rows.is_empty() {
            rows.push(empty_row(&tr(
                "No directory groups yet. Nobody from the directory can sign in.",
            )));
        }
        let this = self.clone();
        let realm_mode = r.clone();
        let realm_add = r.clone();
        let this_add = self.clone();
        self.sign_in_group(
            rows,
            everyone,
            move |everyone| {
                let this = this.clone();
                let realm = realm_mode.clone();
                if everyone == (realm.policy == Policy::Everyone) {
                    return;
                }
                glib::spawn_future_local(async move {
                    let mut add = Vec::new();
                    if everyone {
                        this.saved_groups.replace(realm.permitted_groups.clone());
                    } else if realm.permitted_groups.is_empty() {
                        add = this.saved_groups.take();
                    }
                    let policy = if everyone {
                        Policy::Everyone
                    } else if add.is_empty()
                        && realm.permitted_groups.is_empty()
                        && realm.permitted_logins.is_empty()
                    {
                        Policy::Nobody
                    } else {
                        Policy::Permitted
                    };
                    let result = realm::set_policy(&this.conn, &realm, policy, &add, &[]).await;
                    this.after_change(result.map_err(|e| realm::error_text(&e)));
                });
            },
            move |button| {
                let this = this_add.clone();
                let realm = realm_add.clone();
                glib::spawn_future_local(async move {
                    let Some(group) = this.ask_group(&button, &realm, false).await else {
                        return;
                    };
                    let result =
                        realm::set_policy(&this.conn, &realm, Policy::Permitted, &[group], &[])
                            .await;
                    this.after_change(result.map_err(|e| realm::error_text(&e)));
                });
            },
        );
        self.joined_box.append(&widgets::caption(&tr(
            "Members of these groups can sign in. Administrators need to be in one of them as well.",
        )));
    }

    fn render_ipa_sign_in(self: &Rc<Self>, r: &Realm) {
        let rule_name = ipa::rule_name(&ipa_host());
        let state = self.ipa_rule.borrow().clone();
        let Some((rule, allow_all)) = state else {
            // The rule lives on the server; reading it needs a ticket.
            let show = gtk::Button::builder()
                .label(tr("Show Groups…"))
                .valign(gtk::Align::Center)
                .build();
            let this = self.clone();
            let realm = r.clone();
            show.connect_clicked(move |b| {
                let this = this.clone();
                let realm = realm.clone();
                let b = b.clone();
                glib::spawn_future_local(async move {
                    if this.ipa_session(&b, &realm).await.is_some() {
                        this.load_ipa_rule().await;
                    }
                });
            });
            self.joined_box.append(&widgets::group(
                Some(&tr("Who Can Sign In")),
                &[widgets::row_with_subtitle(
                    &tr("Kept on the FreeIPA server"),
                    Some(&format!("{} {rule_name}", tr("HBAC rule"))),
                    &show,
                )],
            ));
            self.joined_box.append(&widgets::caption(&tr(
                "Seeing and changing the groups asks for a FreeIPA administrator's name and password.",
            )));
            return;
        };
        let mut rows: Vec<gtk::ListBoxRow> = Vec::new();
        for g in &rule.groups {
            let this = self.clone();
            let group = g.clone();
            let current = rule.clone();
            let name = rule_name.clone();
            rows.push(group_row(g, move || {
                let groups: Vec<String> = current
                    .groups
                    .iter()
                    .filter(|x| *x != &group)
                    .cloned()
                    .collect();
                this.set_ipa_rule(name.clone(), false, groups);
            }));
        }
        if rows.is_empty() {
            rows.push(empty_row(&tr("No groups yet.")));
        }
        let this = self.clone();
        let name = rule_name.clone();
        let current = rule.clone();
        let this_add = self.clone();
        let name_add = rule_name.clone();
        let current_add = rule.clone();
        let realm_add = r.clone();
        self.sign_in_group(
            rows,
            rule.everyone,
            move |everyone| {
                if everyone == current.everyone {
                    return;
                }
                // FreeIPA drops the groups when everyone may sign in, so
                // keep them here to put back.
                let groups = if everyone {
                    this.saved_groups.replace(current.groups.clone());
                    current.groups.clone()
                } else if current.groups.is_empty() {
                    this.saved_groups.take()
                } else {
                    current.groups.clone()
                };
                this.set_ipa_rule(name.clone(), everyone, groups);
            },
            move |button| {
                let this = this_add.clone();
                let name = name_add.clone();
                let current = current_add.clone();
                let realm = realm_add.clone();
                glib::spawn_future_local(async move {
                    let Some(group) = this.ask_group(&button, &realm, true).await else {
                        return;
                    };
                    let mut groups = current.groups.clone();
                    if !groups.iter().any(|g| g.eq_ignore_ascii_case(&group)) {
                        groups.push(group);
                    }
                    this.set_ipa_rule(name, false, groups);
                });
            },
        );
        let mut note = format!(
            "{} {rule_name} {}",
            tr("Kept on the FreeIPA server in HBAC rule"),
            tr("for this computer, for every service.")
        );
        if allow_all {
            note.push(' ');
            note.push_str(&tr(
                "FreeIPA's allow_all rule is still on, so everyone can sign in anyway. Turn it off on the server to make this list count.",
            ));
        }
        self.joined_box.append(&widgets::caption(&note));
    }

    async fn ipa_session(
        self: &Rc<Self>,
        widget: &gtk::Button,
        realm: &Realm,
    ) -> Option<Rc<ipa::Session>> {
        if let Some(s) = self.ipa.borrow().clone() {
            return Some(s);
        }
        let values = widgets::ask(
            widget,
            &tr("FreeIPA Administrator"),
            &format!(
                "{} {}.",
                tr("A FreeIPA administrator's name and password for"),
                realm.realm_name
            ),
            &[(&tr("Name"), false), (&tr("Password"), true)],
            &tr("Continue"),
        )
        .await?;
        let admin = if values[0].trim().is_empty() {
            "admin".to_owned()
        } else {
            values[0].trim().to_owned()
        };
        match ipa::Session::open(&admin, &realm.realm_name, &values[1]).await {
            Ok(s) => {
                let s = Rc::new(s);
                self.ipa.replace(Some(s.clone()));
                Some(s)
            }
            Err(e) => {
                widgets::show_error(widget, &tr("Could not sign in to FreeIPA"), &e);
                None
            }
        }
    }

    async fn load_ipa_rule(self: &Rc<Self>) {
        let Some(session) = self.ipa.borrow().clone() else {
            return;
        };
        match session.rule(&ipa::rule_name(&ipa_host())).await {
            Ok(state) => {
                self.ipa_rule.replace(Some(state));
            }
            Err(e) => {
                widgets::show_error(&self.joined_box, &tr("Could not read the sign-in rule"), &e)
            }
        }
        self.refresh();
    }

    fn set_ipa_rule(self: &Rc<Self>, name: String, everyone: bool, groups: Vec<String>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Some(session) = this.ipa.borrow().clone() else {
                return;
            };
            // A handful of round trips to the server; hold the page still.
            this.joined_box.set_sensitive(false);
            let result = session
                .set_rule(&name, &ipa_host(), everyone, &groups)
                .await;
            this.joined_box.set_sensitive(true);
            if let Err(e) = result {
                widgets::show_error(
                    &this.joined_box,
                    &tr("Could not change who can sign in"),
                    &e,
                );
            }
            this.load_ipa_rule().await;
        });
    }

    fn render_admins(self: &Rc<Self>, r: &Realm) {
        let groups = directory::admin_groups();
        let mut rows: Vec<gtk::ListBoxRow> = Vec::new();
        for g in &groups {
            let this = self.clone();
            let group = g.clone();
            let all = groups.clone();
            rows.push(group_row(g, move || {
                let rest: Vec<String> = all.iter().filter(|x| *x != &group).cloned().collect();
                this.set_admins(rest);
            }));
        }
        if rows.is_empty() {
            rows.push(empty_row(&format!(
                "{} {}",
                tr("None yet. Most directories use"),
                r.usual_admin_group()
            )));
        }
        self.joined_box
            .append(&widgets::group(Some(&tr("Administrators")), &rows));
        let add = gtk::Button::builder()
            .label(tr("Add Group…"))
            .halign(gtk::Align::End)
            .build();
        let this = self.clone();
        let realm = r.clone();
        add.connect_clicked(move |b| {
            let this = this.clone();
            let realm = realm.clone();
            let b = b.clone();
            glib::spawn_future_local(async move {
                let Some(group) = this.ask_group(&b, &realm, false).await else {
                    return;
                };
                let mut groups = directory::admin_groups();
                if !groups.iter().any(|g| g.eq_ignore_ascii_case(&group)) {
                    groups.push(group);
                }
                this.set_admins(groups);
            });
        });
        self.joined_box.append(&add);
        self.joined_box.append(&widgets::caption(&tr(
            "Members of these groups can install apps, use sudo, and answer administrator password prompts on this computer.",
        )));
    }

    fn set_admins(self: &Rc<Self>, groups: Vec<String>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = directory::set_admin_groups(&groups).await;
            this.after_change(result);
        });
    }

    /// Ask for a group name and check that the directory has it.
    async fn ask_group(
        &self,
        widget: &gtk::Button,
        realm: &Realm,
        ipa_names: bool,
    ) -> Option<String> {
        let values = widgets::ask(
            widget,
            &tr("Add Group"),
            &format!("{} {}.", tr("A group name in"), realm.realm_name),
            &[(realm.usual_admin_group(), false)],
            &tr("Add"),
        )
        .await?;
        let group = values[0].trim().to_owned();
        if group.is_empty() {
            return None;
        }
        if ipa_names && !ipa::valid_group(&group) {
            widgets::show_error(
                widget,
                &tr("That is not a FreeIPA group name."),
                &tr("FreeIPA group names use letters, digits, dots, dashes, and underscores."),
            );
            return None;
        }
        if !directory::group_exists(&group) {
            widgets::show_error(
                widget,
                &format!("{} \"{group}\"", tr("No group named")),
                &tr("The directory has no group by that name, or it can't be reached right now."),
            );
            return None;
        }
        Some(group)
    }

    fn after_change(self: &Rc<Self>, result: Result<(), String>) {
        if let Err(e) = result {
            widgets::show_error(&self.joined_box, &tr("The change was not made"), &e);
        }
        self.refresh();
    }

    fn leave(self: &Rc<Self>, button: &gtk::Button) {
        let Some(r) = self.joined.borrow().clone() else {
            return;
        };
        let this = self.clone();
        let button = button.clone();
        glib::spawn_future_local(async move {
            let cancel = tr("Cancel");
            let remove = tr("Remove Computer Account…");
            let leave = tr("Leave");
            let choice = widgets::choose_styled(
                &button,
                &format!("{} {}?", tr("Leave"), r.realm_name),
                &tr("People with directory accounts can no longer sign in here. Their files stay on this computer. You can also remove this computer's account from the directory, which needs a directory administrator."),
                &[&cancel, &remove, &leave],
                0,
                0,
                Some((2, "destructive-action")),
            )
            .await;
            let admin = match choice {
                1 => {
                    let Some(v) = widgets::ask(
                        &button,
                        &tr("Directory Administrator"),
                        &format!(
                            "{} {}.",
                            tr("A directory administrator's name and password for"),
                            r.realm_name
                        ),
                        &[(&tr("Name"), false), (&tr("Password"), true)],
                        &tr("Leave"),
                    )
                    .await
                    else {
                        return;
                    };
                    Some((v[0].trim().to_owned(), v[1].clone()))
                }
                2 => None,
                _ => return,
            };
            button.set_sensitive(false);
            if let (Kind::FreeIpa, Some((user, password))) = (r.kind, &admin) {
                // The computer's rule goes with its account.
                if let Ok(s) = ipa::Session::open(user, &r.realm_name, password).await {
                    let _ = s.delete_rule(&ipa::rule_name(&ipa_host())).await;
                }
            }
            let operation = realm::operation_id();
            let result = realm::leave(
                &this.conn,
                &r,
                admin.as_ref().map(|(u, p)| (u.as_str(), p.as_str())),
                &operation,
            )
            .await;
            match result {
                Ok(()) => {
                    if let Some(s) = this.ipa.take() {
                        s.close();
                    }
                    this.ipa_rule.replace(None);
                    this.saved_groups.take();
                    directory::wait_applied(false, 40).await;
                    this.domain.set_text("");
                }
                Err(e) => {
                    button.set_sensitive(true);
                    widgets::show_error(
                        &button,
                        &format!("{} {}", tr("Could not leave"), r.realm_name),
                        &realm::error_text(&e),
                    );
                }
            }
            this.refresh();
        });
    }
}

pub fn build() -> gtk::Widget {
    let Some(conn) = crate::backend::dbus::system() else {
        return widgets::placeholder(
            "network-workgroup-symbolic",
            &tr("Domain"),
            &tr("The system bus is not available."),
        );
    };
    let stack = gtk::Stack::builder().vhomogeneous(false).build();
    stack.add_named(
        &gtk::Spinner::builder()
            .spinning(true)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_top(48)
            .build(),
        Some("loading"),
    );

    // Join page
    let join_page = widgets::page();
    join_page.append(&widgets::hub_header(
        "network-workgroup-symbolic",
        "graphite",
        &tr("Domain"),
        &tr("Join an Active Directory domain or a FreeIPA realm so people can sign in with their work account. Their files stay on this computer, and they can sign in when the directory can't be reached."),
    ));
    let domain = gtk::Entry::builder()
        .placeholder_text("corp.example.com")
        .hexpand(true)
        .valign(gtk::Align::Center)
        .input_purpose(gtk::InputPurpose::Url)
        .build();
    let found_icon = status_icon("dialog-information-symbolic");
    let found_spinner = gtk::Spinner::builder().visible(false).build();
    let found = gtk::Label::builder()
        .label(tr("Type the domain name, such as corp.example.com. Elavo finds the directory as you type."))
        .xalign(0.0)
        .wrap(true)
        .hexpand(true)
        .build();
    let found_box = gtk::Box::builder().spacing(10).build();
    found_box.append(&found_icon);
    found_box.append(&found_spinner);
    found_box.append(&found);
    join_page.append(&widgets::group(
        None,
        &[
            widgets::row(&tr("Domain"), &domain),
            widgets::plain_row(&found_box),
        ],
    ));
    let warning = widgets::status_label();
    join_page.append(&warning);

    let admin = gtk::Entry::builder()
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let password = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .hexpand(true)
        .activates_default(true)
        .build();
    let ou = gtk::Entry::builder()
        .placeholder_text(tr("Optional, such as OU=Laptops,DC=corp,DC=example,DC=com"))
        .hexpand(true)
        .build();
    let ou_row = widgets::row(&tr("Organizational unit"), &ou);
    let credentials = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .visible(false)
        .build();
    credentials.append(&widgets::group(
        Some(&tr("Directory Administrator")),
        &[
            widgets::row(&tr("Name"), &admin),
            widgets::row(&tr("Password"), &password),
            ou_row.clone(),
        ],
    ));
    credentials.append(&widgets::caption(&tr(
        "Used once to add this computer to the directory, and not saved.",
    )));
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    let stop = gtk::Button::builder()
        .label(tr("Stop"))
        .visible(false)
        .build();
    let join = gtk::Button::builder()
        .label(tr("Join"))
        .sensitive(false)
        .css_classes(["suggested-action"])
        .build();
    buttons.append(&stop);
    buttons.append(&join);
    credentials.append(&buttons);
    let join_status = widgets::status_label();
    credentials.append(&join_status);
    let log = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .css_classes(["caption", "monospace-log"])
        .build();
    let log_expander = gtk::Expander::builder()
        .label(tr("Details"))
        .child(
            &gtk::ScrolledWindow::builder()
                .child(&log)
                .max_content_height(200)
                .propagate_natural_height(true)
                .build(),
        )
        .visible(false)
        .build();
    credentials.append(&log_expander);
    join_page.append(&credentials);
    stack.add_named(&join_page, Some("join"));

    // Joined page
    let joined_page = widgets::page();
    joined_page.append(&widgets::hub_header(
        "network-workgroup-symbolic",
        "graphite",
        &tr("Domain"),
        &tr("This computer is joined to a directory. People sign in with their directory account."),
    ));
    let joined_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    joined_page.append(&joined_box);
    stack.add_named(&joined_page, Some("joined"));
    stack.set_visible_child_name("loading");

    let pane = Rc::new(Pane {
        conn,
        stack: stack.clone(),
        domain,
        found_icon,
        found_spinner,
        found,
        warning,
        credentials,
        admin,
        password,
        ou_row,
        ou,
        join,
        stop,
        join_status,
        log,
        log_expander,
        discovered: RefCell::new(None),
        discover_cancel: RefCell::new(None),
        discover_seq: Cell::new(0),
        operation: RefCell::new(None),
        busy: Cell::new(false),
        joined_box,
        joined: RefCell::new(None),
        ipa: RefCell::new(None),
        ipa_rule: RefCell::new(None),
        saved_groups: RefCell::new(Vec::new()),
    });

    let p = pane.clone();
    pane.domain.connect_changed(move |_| p.domain_changed());
    let p = pane.clone();
    pane.admin.connect_changed(move |_| p.update_join_button());
    let p = pane.clone();
    pane.password
        .connect_changed(move |_| p.update_join_button());
    let p = pane.clone();
    pane.password.connect_activate(move |_| {
        if p.join.is_sensitive() {
            p.start_join();
        }
    });
    let p = pane.clone();
    pane.join.connect_clicked(move |_| p.start_join());
    let p = pane.clone();
    pane.stop.connect_clicked(move |_| {
        if let Some(op) = p.operation.borrow().as_ref() {
            realm::cancel(&p.conn, op);
        }
    });
    // realmd changes after a join or leave from a terminal too.
    let weak = Rc::downgrade(&pane);
    let debounce = crate::backend::dbus::Debounce::new(500);
    let sub = crate::backend::dbus::on_properties_changed(
        &pane.conn,
        "org.freedesktop.realmd",
        None,
        move |_, _, _| {
            if let Some(p) = weak.upgrade() {
                if !p.busy.get() {
                    debounce.run(move || p.refresh());
                }
            }
        },
    );
    let p = pane.clone();
    stack.connect_map(move |_| p.refresh());

    widgets::keep_alive(&stack, (pane, sub));
    stack.upcast()
}

/// "Not joined", or the joined domain, for the row on Users & Groups.
pub async fn summary(conn: &gio::DBusConnection) -> String {
    match realm::joined(conn).await {
        Ok(list) => list
            .first()
            .map(|r| r.realm_name.clone())
            .unwrap_or_else(|| tr("Not joined")),
        Err(_) => tr("Not available"),
    }
}
