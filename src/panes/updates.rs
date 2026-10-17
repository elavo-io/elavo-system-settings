//! Software Update: what is installed, checking and downloading updates,
//! the update channel, and going back to the previous version. Every
//! change goes through the helper, so each asks for a password.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::updates::{self, Deployment, Status};
use crate::backend::{dbus, helper, power};
use crate::i18n::tr;
use crate::widgets;

const CHANNELS: [&str; 2] = ["stable", "latest"];

struct Pane {
    summary: gtk::Label,
    detail: gtk::Label,
    check: gtk::Button,
    install: gtk::Button,
    restart: gtk::Button,
    channel: gtk::DropDown,
    rollback_row: gtk::ListBoxRow,
    rollback: gtk::Button,
    rollback_detail: gtk::Label,
    log: gtk::Label,
    spinner: gtk::Spinner,
    progress: gtk::ProgressBar,
    notes: gtk::Label,
    channel_note: gtk::Label,
    download_started: RefCell<Option<std::time::Instant>>,
    status: RefCell<Status>,
    busy: std::cell::Cell<bool>,
    syncing: std::cell::Cell<bool>,
}

fn describe(d: &Deployment) -> String {
    let mut parts = Vec::new();
    if !d.version.is_empty() {
        parts.push(d.version.clone());
    }
    if d.timestamp > 0 {
        if let Ok(t) =
            glib::DateTime::from_unix_local(d.timestamp).and_then(|t| t.format("%B %-e, %Y"))
        {
            parts.push(t.to_string());
        }
    }
    let digest = d.short_digest();
    if !digest.is_empty() {
        parts.push(digest);
    }
    parts.join(" · ")
}

fn channel_label(tag: &str) -> String {
    match tag {
        "stable" => tr("Stable"),
        "latest" => tr("Latest (preview)"),
        other => other.to_owned(),
    }
}

impl Pane {
    fn refresh(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            match updates::status().await {
                Ok(s) => this.render(s),
                Err(e) => {
                    this.summary.set_label(&tr("Updates are not available"));
                    this.detail.set_label(&e);
                    this.check.set_sensitive(false);
                    this.channel.set_sensitive(false);
                    this.rollback_row.set_visible(false);
                }
            }
        });
    }

    fn render(self: &Rc<Self>, s: Status) {
        let booted = s.booted.clone().unwrap_or_default();
        self.summary.set_label(&match booted.channel() {
            Some(c) => format!("Elavo · {}", channel_label(c)),
            None => "Elavo".to_owned(),
        });
        let mut detail = describe(&booted);
        if s.staged.is_some() {
            detail = format!(
                "{detail}\n{}",
                tr("An update is ready. Restart to finish installing it.")
            );
        }
        self.detail.set_label(&detail);
        self.restart.set_visible(s.staged.is_some());
        self.install.set_visible(false);
        self.check.set_sensitive(!self.busy.get());
        self.syncing.set(true);
        match booted
            .channel()
            .and_then(|c| CHANNELS.iter().position(|x| *x == c))
        {
            Some(i) => {
                self.channel.set_selected(i as u32);
                self.channel.set_sensitive(!self.busy.get());
            }
            None => self.channel.set_sensitive(false),
        }
        self.syncing.set(false);
        match &s.rollback {
            Some(r) => {
                self.rollback_detail.set_label(&describe(r));
                self.rollback_row.set_visible(true);
            }
            None => self.rollback_row.set_visible(false),
        }
        if let Some(staged) = s.staged.clone() {
            self.show_notes(updates::at_digest(&staged.image, &staged.digest));
        }
        self.status.replace(s);
        self.note_stable();
    }

    fn show_notes(self: &Rc<Self>, image: String) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let Some(text) = updates::changelog(&image).await else {
                return;
            };
            this.notes
                .set_label(&format!("{}\n{text}", tr("What's new")));
            this.notes.set_visible(true);
        });
    }

    /// Latest stays what this computer follows. The note only appears once
    /// a stable release is posted, so the choice is visible.
    fn note_stable(self: &Rc<Self>) {
        let Some(image) = self
            .status
            .borrow()
            .booted
            .as_ref()
            .filter(|b| b.channel() == Some("latest"))
            .and_then(|b| updates::with_tag(&b.image, "stable"))
        else {
            self.channel_note.set_visible(false);
            return;
        };
        let this = self.clone();
        glib::spawn_future_local(async move {
            if updates::tag_exists(&image).await {
                this.channel_note.set_label(&tr(
                    "A stable release is posted. This computer follows Latest until you switch to Stable.",
                ));
                this.channel_note.set_visible(true);
            }
        });
    }

    fn show_download(&self, version: &str, got: updates::Download) {
        self.spinner.set_visible(false);
        self.progress.set_visible(true);
        self.detail.set_label(&format!(
            "{version}\n{} {}",
            updates::format_bytes(got.bytes_total),
            tr("to download")
        ));
        self.progress
            .set_fraction(got.bytes as f64 / got.bytes_total as f64);
        let mut started = self.download_started.borrow_mut();
        if started.is_none() && got.bytes > 0 {
            *started = Some(std::time::Instant::now());
        }
        let remaining = started.and_then(|t| {
            updates::seconds_left(t.elapsed().as_secs_f64(), got.bytes, got.bytes_total)
        });
        let mut text = format!(
            "{} {} {}",
            updates::format_bytes(got.bytes),
            tr("of"),
            updates::format_bytes(got.bytes_total)
        );
        if let Some(seconds) = remaining {
            text = format!("{text} · {}", updates::format_remaining(seconds));
        }
        self.progress.set_text(Some(&text));
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        self.spinner.set_spinning(busy);
        self.spinner.set_visible(busy);
        for b in [&self.check, &self.install, &self.rollback, &self.restart] {
            b.set_sensitive(!busy);
        }
        self.channel.set_sensitive(!busy);
    }

    /// Run a helper command, showing its output. Returns its output.
    async fn run(self: &Rc<Self>, args: &[&str]) -> Result<String, String> {
        self.set_busy(true);
        self.log.set_label("");
        self.progress.set_visible(false);
        self.progress.set_fraction(0.0);
        *self.download_started.borrow_mut() = None;
        let version = self.detail.text().lines().next().unwrap_or("").to_owned();
        let output = Rc::new(RefCell::new(String::new()));
        let out = output.clone();
        let log = self.log.clone();
        let this = self.clone();
        let result = helper::run(args, move |line| {
            if let Some(got) = updates::download_progress(line) {
                this.show_download(&version, got);
                return;
            }
            let mut o = out.borrow_mut();
            o.push_str(line);
            o.push('\n');
            log.set_label(o.trim_end());
        })
        .await;
        self.progress.set_visible(false);
        self.set_busy(false);
        let text = output.borrow().clone();
        result.map(|()| text)
    }

    fn check(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            this.detail.set_label(&tr("Checking for updates…"));
            match this.run(&["check"]).await {
                Ok(out) => {
                    let none = out
                        .lines()
                        .any(|l| l.contains("No changes") || l.contains("No update available"));
                    if let Ok(s) = updates::status().await {
                        this.render(s);
                    }
                    let note = if none {
                        tr("This computer is up to date.")
                    } else {
                        tr("An update is available.")
                    };
                    this.detail
                        .set_label(&format!("{}\n{note}", this.detail.text()));
                    this.install.set_visible(!none);
                    if !none {
                        if let Some(image) = this
                            .status
                            .borrow()
                            .booted
                            .as_ref()
                            .map(|b| b.image.clone())
                        {
                            this.show_notes(image);
                        }
                    } else if this.status.borrow().staged.is_none() {
                        this.notes.set_visible(false);
                    }
                }
                Err(e) => {
                    this.refresh();
                    widgets::show_error(&this.check, &tr("Could not check for updates."), &e);
                }
            }
        });
    }

    fn install(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let version = this.detail.text().lines().next().unwrap_or("").to_owned();
            this.detail
                .set_label(&format!("{version}\n{}", tr("Downloading the update…")));
            let result = this.run(&["upgrade"]).await;
            this.refresh();
            if let Err(e) = result {
                widgets::show_error(
                    &this.install,
                    &tr("The update could not be downloaded."),
                    &e,
                );
            }
        });
    }

    fn switch(self: &Rc<Self>, index: u32) {
        let Some(tag) = CHANNELS.get(index as usize).copied() else {
            return;
        };
        let current = self
            .status
            .borrow()
            .booted
            .as_ref()
            .and_then(|b| b.channel().map(str::to_owned));
        if current.as_deref() == Some(tag) {
            return;
        }
        let this = self.clone();
        glib::spawn_future_local(async move {
            let detail = if tag == "latest" {
                tr("Latest gets changes first and is less tested. The new version installs now and starts after a restart.")
            } else {
                tr("The new version installs now and starts after a restart.")
            };
            let ok = widgets::confirm(
                &this.channel,
                &format!("{} {}?", tr("Switch to"), channel_label(tag)),
                &detail,
                &tr("Switch"),
            )
            .await;
            if ok {
                let version = this.detail.text().lines().next().unwrap_or("").to_owned();
                this.detail
                    .set_label(&format!("{version}\n{}", tr("Switching channel…")));
                if let Err(e) = this.run(&["switch", tag]).await {
                    widgets::show_error(
                        &this.channel,
                        &tr("Could not change the update channel."),
                        &e,
                    );
                }
            }
            this.refresh();
        });
    }

    fn rollback(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let ok = widgets::confirm(
                &this.rollback,
                &tr("Go back to the previous version?"),
                &tr("The previous version starts after a restart. Your files and settings are kept."),
                &tr("Go Back"),
            )
            .await;
            if !ok {
                return;
            }
            let result = this.run(&["rollback"]).await;
            this.refresh();
            match result {
                Ok(_) => this.offer_restart().await,
                Err(e) => widgets::show_error(&this.rollback, &tr("Could not go back."), &e),
            }
        });
    }

    async fn offer_restart(self: &Rc<Self>) {
        if widgets::confirm(
            &self.restart,
            &tr("Restart now?"),
            &tr("The change takes effect after a restart."),
            &tr("Restart"),
        )
        .await
        {
            self.reboot().await;
        }
    }

    async fn reboot(&self) {
        let Some(conn) = dbus::system() else { return };
        if let Err(e) = power::reboot(&conn).await {
            widgets::show_error(
                &self.restart,
                &tr("Could not restart."),
                &widgets::error_text(&e),
            );
        }
    }
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let icon = gtk::Image::builder()
        .icon_name("system-software-update-symbolic")
        .pixel_size(48)
        .build();
    let summary = gtk::Label::builder()
        .css_classes(["title-3"])
        .xalign(0.0)
        .label(tr("Elavo"))
        .build();
    let detail = gtk::Label::builder()
        .css_classes(["dim-label"])
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .build();
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    let progress = gtk::ProgressBar::builder()
        .hexpand(true)
        .show_text(true)
        .visible(false)
        .build();
    let notes = gtk::Label::builder()
        .css_classes(["dim-label"])
        .xalign(0.0)
        .wrap(true)
        .selectable(true)
        .visible(false)
        .build();
    text.append(&summary);
    text.append(&detail);
    text.append(&progress);
    text.append(&notes);
    let spinner = gtk::Spinner::builder().visible(false).build();
    let check = gtk::Button::with_label(&tr("Check for Updates"));
    let install = gtk::Button::builder()
        .label(tr("Update Now"))
        .css_classes(["suggested-action"])
        .visible(false)
        .build();
    let restart = gtk::Button::builder()
        .label(tr("Restart Now"))
        .css_classes(["suggested-action"])
        .visible(false)
        .build();
    let buttons = gtk::Box::builder()
        .spacing(8)
        .valign(gtk::Align::Center)
        .build();
    buttons.append(&spinner);
    buttons.append(&check);
    buttons.append(&install);
    buttons.append(&restart);
    let head = gtk::Box::builder().spacing(14).build();
    head.append(&icon);
    head.append(&text);
    head.append(&buttons);
    page.append(&widgets::group(None, &[widgets::plain_row(&head)]));

    let labels: Vec<String> = CHANNELS.iter().map(|c| channel_label(c)).collect();
    let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let channel = gtk::DropDown::from_strings(&refs);
    let rollback = gtk::Button::with_label(&tr("Go Back…"));
    let rollback_detail = gtk::Label::builder()
        .css_classes(["dim-label", "caption"])
        .xalign(0.0)
        .build();
    let rb = gtk::Box::builder().spacing(10).build();
    rb.append(&rollback_detail);
    rb.append(&rollback);
    let rollback_row = widgets::row(&tr("Previous version"), &rb);
    page.append(&widgets::group(
        None,
        &[
            widgets::row_with_subtitle(
                &tr("Update channel"),
                Some(&tr("Stable is recommended. Latest gets new changes first.")),
                &channel,
            ),
            rollback_row.clone(),
        ],
    ));

    let log = gtk::Label::builder()
        .css_classes(["monospace-log"])
        .xalign(0.0)
        .yalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .build();
    let expander = gtk::Expander::builder()
        .label(tr("Details"))
        .child(&log)
        .build();
    page.append(&expander);
    let channel_note = widgets::caption("");
    channel_note.set_visible(false);
    page.append(&channel_note);
    page.append(&widgets::caption(&tr(
        "Elavo updates as a whole image. The current version stays available, so you can go back if something is wrong.",
    )));

    let pane = Rc::new(Pane {
        summary,
        detail,
        check,
        install,
        restart,
        channel,
        rollback_row,
        rollback,
        rollback_detail,
        log,
        spinner,
        progress,
        notes,
        channel_note,
        download_started: RefCell::default(),
        status: RefCell::default(),
        busy: Default::default(),
        syncing: Default::default(),
    });
    let p = pane.clone();
    pane.check.connect_clicked(move |_| p.check());
    let p = pane.clone();
    pane.install.connect_clicked(move |_| p.install());
    let p = pane.clone();
    pane.rollback.connect_clicked(move |_| p.rollback());
    let p = pane.clone();
    pane.restart.connect_clicked(move |_| {
        let p = p.clone();
        glib::spawn_future_local(async move { p.reboot().await });
    });
    let p = pane.clone();
    pane.channel.connect_selected_notify(move |dd| {
        if !p.syncing.get() {
            p.switch(dd.selected());
        }
    });
    pane.refresh();
    widgets::keep_alive(&page, pane);
    page.upcast()
}
