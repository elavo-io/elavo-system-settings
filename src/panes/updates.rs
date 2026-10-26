//! Software Update: what is installed, whether a newer build is published,
//! what changed, automatic updates, the update channel, and going back to
//! the previous version.
//!
//! Checking reads the registry and needs no password. It runs when the pane
//! opens, at most once every half hour, and again every half hour while the
//! pane stays open. The top card shows one state with one button.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gtk::glib;
use gtk::prelude::*;

use crate::backend::auto::Auto;
use crate::backend::updates::{self, Changes, Deployment, Phase, Remote, Status};
use crate::backend::{dbus, helper, power};
use crate::i18n::tr;
use crate::widgets;

const CHANNELS: [&str; 2] = ["stable", "latest"];
const CHECK_EVERY: Duration = Duration::from_secs(30 * 60);

struct Checked {
    at: Instant,
    image: String,
    remote: Remote,
}

thread_local! {
    /// The last check, kept while the app runs so reopening the pane does not
    /// check again.
    static LAST_CHECK: RefCell<Option<Checked>> = const { RefCell::new(None) };
    /// What changed up to a revision, by that revision.
    static CHANGES: RefCell<Option<(String, Changes)>> = const { RefCell::new(None) };
}

fn cached_remote(image: &str) -> Option<(Instant, Remote)> {
    LAST_CHECK.with_borrow(|c| {
        c.as_ref()
            .filter(|c| c.image == image)
            .map(|c| (c.at, c.remote.clone()))
    })
}

struct Pane {
    summary: gtk::Label,
    detail: gtk::Label,
    state: gtk::Label,
    progress: gtk::ProgressBar,
    spinner: gtk::Spinner,
    action: gtk::Button,
    check: gtk::Button,
    news: gtk::Box,
    news_subject: gtk::Label,
    news_meta: gtk::Label,
    see_all: gtk::Button,
    changes_window: RefCell<Option<(gtk::Window, gtk::ListBox, gtk::Label)>>,
    download: gtk::Switch,
    install: gtk::Switch,
    channel: gtk::DropDown,
    rollback_row: gtk::ListBoxRow,
    rollback: gtk::Button,
    rollback_detail: gtk::Label,
    log: gtk::Label,
    channel_note: gtk::Label,
    download_started: RefCell<Option<Instant>>,
    status: RefCell<Option<Status>>,
    remote: RefCell<Option<Remote>>,
    checked_at: Cell<Option<Instant>>,
    /// The last check, successful or not.
    attempted_at: Cell<Option<Instant>>,
    check_error: RefCell<Option<String>>,
    checking: Cell<bool>,
    /// The revision What's New shows changes up to.
    news_for: RefCell<Option<String>>,
    busy: Cell<bool>,
    syncing: Cell<bool>,
}

fn format_date(rfc3339: &str) -> Option<String> {
    let t = glib::DateTime::from_iso8601(rfc3339, None).ok()?;
    let t = t.to_local().ok()?;
    t.format("%B %-e, %Y").ok().map(|s| s.to_string())
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

fn commit_meta(commit: &updates::Commit) -> String {
    let mut parts = Vec::new();
    if let Some(date) = format_date(&commit.date) {
        parts.push(date);
    }
    if !commit.sha.is_empty() {
        parts.push(commit.sha.chars().take(7).collect());
    }
    parts.join(" · ")
}

impl Pane {
    fn booted(&self) -> Option<Deployment> {
        self.status.borrow().as_ref().and_then(|s| s.booted.clone())
    }

    fn phase(&self) -> Phase {
        match self.status.borrow().as_ref() {
            Some(s) => updates::phase(s, updates::staged_locked(), self.remote.borrow().as_ref()),
            None => Phase::Unknown,
        }
    }

    /// Reads the deployments, then checks the registry if the last check is
    /// older than half an hour.
    fn refresh(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            match updates::status().await {
                Ok(s) => {
                    this.status.replace(Some(s));
                    this.render();
                    this.check_if_due();
                }
                Err(e) => {
                    this.summary.set_label(&tr("Updates are not available"));
                    this.detail.set_label(&e);
                    this.state.set_visible(false);
                    this.action.set_visible(false);
                    this.check.set_visible(false);
                    this.channel.set_sensitive(false);
                    this.rollback_row.set_visible(false);
                }
            }
        });
    }

    fn check_if_due(self: &Rc<Self>) {
        let Some(booted) = self.booted() else { return };
        if let Some((at, remote)) = cached_remote(&booted.image) {
            self.remote.replace(Some(remote));
            self.checked_at.set(Some(at));
            self.render();
            if at.elapsed() < CHECK_EVERY {
                return;
            }
        }
        self.check();
    }

    fn check(self: &Rc<Self>) {
        let Some(booted) = self.booted() else { return };
        if self.checking.get() || self.busy.get() {
            return;
        }
        self.checking.set(true);
        self.attempted_at.set(Some(Instant::now()));
        self.render();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = updates::remote(&booted.image).await;
            this.checking.set(false);
            match result {
                Ok(remote) => {
                    let at = Instant::now();
                    LAST_CHECK.with_borrow_mut(|c| {
                        *c = Some(Checked {
                            at,
                            image: booted.image.clone(),
                            remote: remote.clone(),
                        })
                    });
                    this.remote.replace(Some(remote));
                    this.checked_at.set(Some(at));
                    this.check_error.replace(None);
                }
                Err(e) => {
                    this.check_error.replace(Some(e));
                }
            }
            this.render();
        });
    }

    fn render(self: &Rc<Self>) {
        let Some(status) = self.status.borrow().clone() else {
            return;
        };
        let booted = status.booted.clone().unwrap_or_default();
        let version = updates::os_version().unwrap_or_else(|| booted.version.clone());
        self.summary.set_label(&if version.is_empty() {
            "Elavo".to_owned()
        } else {
            format!("Elavo {version}")
        });
        let mut detail = Vec::new();
        if let Some(c) = booted.channel() {
            detail.push(channel_label(c));
        }
        let built = describe(&Deployment {
            version: String::new(),
            ..booted.clone()
        });
        if !built.is_empty() {
            detail.push(built);
        }
        self.detail.set_label(&detail.join(" · "));

        self.render_state();

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
        match &status.rollback {
            Some(r) => {
                self.rollback_detail.set_label(&describe(r));
                self.rollback_row.set_visible(true);
            }
            None => self.rollback_row.set_visible(false),
        }
        self.note_stable();
    }

    /// The one state line and its one button.
    fn render_state(self: &Rc<Self>) {
        if self.busy.get() {
            return;
        }
        let phase = self.phase();
        self.spinner.set_visible(self.checking.get());
        self.spinner.set_spinning(self.checking.get());
        let (text, action) = match phase {
            Phase::Ready => (
                tr("An update is installed. It starts after a restart."),
                Some(tr("Restart Now")),
            ),
            Phase::Downloaded => (
                tr("An update is downloaded. Install it to start using it after a restart."),
                Some(tr("Install Now")),
            ),
            Phase::Available => {
                let published = self
                    .remote
                    .borrow()
                    .as_ref()
                    .and_then(|r| format_date(&r.created));
                (
                    match published {
                        Some(date) => {
                            format!("{} {date}.", tr("A new version is available, published"))
                        }
                        None => tr("A new version is available."),
                    },
                    Some(tr("Update Now")),
                )
            }
            Phase::UpToDate => {
                let when = self
                    .checked_at
                    .get()
                    .and_then(|at| {
                        glib::DateTime::now_local()
                            .ok()?
                            .add_seconds(-at.elapsed().as_secs_f64())
                            .ok()
                    })
                    .and_then(|t| t.format("%H:%M").ok());
                (
                    match when {
                        Some(t) => {
                            format!("{} {} {t}.", tr("Elavo is up to date."), tr("Checked at"))
                        }
                        None => tr("Elavo is up to date."),
                    },
                    None,
                )
            }
            Phase::Unknown if self.checking.get() => (tr("Checking for updates…"), None),
            Phase::Unknown => match self.check_error.borrow().as_ref() {
                Some(e) => (format!("{} {e}", tr("Could not check for updates.")), None),
                None => (String::new(), None),
            },
        };
        self.state.set_label(&text);
        self.state.set_visible(!text.is_empty());
        match action {
            Some(label) => {
                self.action.set_label(&label);
                self.action.set_visible(true);
            }
            None => self.action.set_visible(false),
        }
        self.check
            .set_visible(!self.checking.get() && matches!(phase, Phase::UpToDate | Phase::Unknown));
        self.show_news(phase);
    }

    /// The newest change in the update this computer can install.
    fn show_news(self: &Rc<Self>, phase: Phase) {
        match phase {
            Phase::Ready | Phase::Downloaded => {
                let Some(staged) = self.status.borrow().as_ref().and_then(|s| s.staged.clone())
                else {
                    return self.set_news(None);
                };
                let this = self.clone();
                glib::spawn_future_local(async move {
                    match updates::revision_of(&staged).await {
                        Some(revision) => this.load_news(revision),
                        None => this.set_news(None),
                    }
                });
            }
            Phase::Available => {
                let revision = self
                    .remote
                    .borrow()
                    .as_ref()
                    .and_then(|r| r.revision.clone());
                match revision {
                    Some(revision) => self.load_news(revision),
                    None => self.set_news(None),
                }
            }
            Phase::UpToDate | Phase::Unknown => self.set_news(None),
        }
    }

    fn load_news(self: &Rc<Self>, revision: String) {
        if self.news_for.borrow().as_deref() == Some(revision.as_str()) {
            return;
        }
        self.news_for.replace(Some(revision.clone()));
        let cached = CHANGES.with_borrow(|c| {
            c.as_ref()
                .filter(|(r, _)| *r == revision)
                .map(|(_, changes)| changes.clone())
        });
        if let Some(changes) = cached {
            self.set_news(Some(changes));
            return;
        }
        let Some(booted) = self.booted() else { return };
        let this = self.clone();
        glib::spawn_future_local(async move {
            let changes = updates::changes(&booted, &revision).await;
            if this.news_for.borrow().as_deref() != Some(revision.as_str()) {
                return;
            }
            if let Some(changes) = &changes {
                CHANGES.with_borrow_mut(|c| *c = Some((revision.clone(), changes.clone())));
            }
            this.set_news(changes);
        });
    }

    fn set_news(&self, changes: Option<Changes>) {
        let Some(newest) = changes.as_ref().and_then(|c| c.commits.first()) else {
            self.news.set_visible(false);
            if changes.is_none() {
                self.news_for.replace(None);
            }
            return;
        };
        self.news_subject.set_label(&newest.subject);
        self.news_meta.set_label(&commit_meta(newest));
        let total = changes.as_ref().map(|c| c.total).unwrap_or(0);
        self.see_all
            .set_label(&format!("{} {total} {}", tr("See All"), tr("Changes…")));
        self.see_all.set_visible(total > 1);
        self.news.set_visible(true);
    }

    /// Every change in the update, newest first, in a window that is made
    /// once and reused.
    fn show_all_changes(self: &Rc<Self>) {
        let Some(revision) = self.news_for.borrow().clone() else {
            return;
        };
        let Some(changes) = CHANGES.with_borrow(|c| {
            c.as_ref()
                .filter(|(r, _)| *r == revision)
                .map(|(_, changes)| changes.clone())
        }) else {
            return;
        };
        let mut slot = self.changes_window.borrow_mut();
        let (window, list, footer) = slot.get_or_insert_with(|| {
            let window = widgets::popup_window(&self.see_all, &tr("What's New"));
            window.set_default_size(520, 560);
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::None)
                .css_classes(["rich-list"])
                .build();
            let frame = gtk::Frame::builder().child(&list).build();
            let footer = widgets::caption("");
            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(10)
                .margin_top(16)
                .margin_bottom(16)
                .margin_start(16)
                .margin_end(16)
                .build();
            content.append(&frame);
            content.append(&footer);
            let scroller = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&content)
                .build();
            window.set_child(Some(&scroller));
            (window, list, footer)
        });
        widgets::clear(list);
        for commit in &changes.commits {
            let subject = gtk::Label::builder()
                .label(&commit.subject)
                .xalign(0.0)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .selectable(true)
                .build();
            let meta = widgets::caption(&commit_meta(commit));
            meta.add_css_class("dim-label");
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(2)
                .margin_top(6)
                .margin_bottom(6)
                .build();
            text.append(&subject);
            text.append(&meta);
            list.append(&widgets::plain_row(&text));
        }
        let hidden = changes.total.saturating_sub(changes.commits.len());
        footer.set_label(&if hidden > 0 {
            format!("{hidden} {}", tr("earlier changes are not listed."))
        } else {
            String::new()
        });
        footer.set_visible(hidden > 0);
        window.present();
    }

    /// Latest stays what this computer follows. The note only appears once
    /// a stable release is posted, so the choice is visible.
    fn note_stable(self: &Rc<Self>) {
        let Some(image) = self
            .booted()
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

    fn show_download(&self, got: updates::Download) {
        self.spinner.set_visible(false);
        self.progress.set_visible(true);
        self.state.set_label(&format!(
            "{} {} {}",
            tr("Downloading"),
            updates::format_bytes(got.bytes_total),
            tr("of updates…")
        ));
        self.progress
            .set_fraction(got.bytes as f64 / got.bytes_total as f64);
        let mut started = self.download_started.borrow_mut();
        if started.is_none() && got.bytes > 0 {
            *started = Some(Instant::now());
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
        if busy {
            self.action.set_visible(false);
            self.check.set_visible(false);
        }
        self.rollback.set_sensitive(!busy);
        self.channel.set_sensitive(!busy);
    }

    /// Run a helper command, showing its output and download progress.
    async fn run(self: &Rc<Self>, args: &[&str], doing: &str) -> Result<(), String> {
        self.set_busy(true);
        self.state.set_label(doing);
        self.state.set_visible(true);
        self.log.set_label("");
        self.progress.set_visible(false);
        self.progress.set_fraction(0.0);
        *self.download_started.borrow_mut() = None;
        let output = Rc::new(RefCell::new(String::new()));
        let out = output.clone();
        let this = self.clone();
        let result = helper::run(args, move |line| {
            if let Some(got) = updates::download_progress(line) {
                this.show_download(got);
                return;
            }
            let mut o = out.borrow_mut();
            o.push_str(line);
            o.push('\n');
            this.log.set_label(o.trim_end());
        })
        .await;
        self.progress.set_visible(false);
        self.set_busy(false);
        result
    }

    fn act(self: &Rc<Self>) {
        let phase = self.phase();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let (args, doing, failed) = match phase {
                Phase::Ready => {
                    this.reboot().await;
                    return;
                }
                Phase::Downloaded => (
                    "install",
                    tr("Installing the update…"),
                    tr("The update could not be installed."),
                ),
                Phase::Available => (
                    "upgrade",
                    tr("Downloading the update…"),
                    tr("The update could not be downloaded."),
                ),
                Phase::UpToDate | Phase::Unknown => return,
            };
            let result = this.run(&[args], &doing).await;
            this.refresh();
            if let Err(e) = result {
                widgets::show_error(&this.action, &failed, &e);
            }
        });
    }

    fn sync_auto(&self, auto: Auto) {
        self.syncing.set(true);
        self.download.set_active(auto.downloads());
        self.install.set_active(auto.installs());
        self.install.set_sensitive(auto.downloads());
        self.syncing.set(false);
    }

    /// Saves both switches. The helper writes the setting for the system
    /// timer, so this asks for a password and the timer never does.
    fn set_auto(self: &Rc<Self>) {
        if !self.download.is_active() && self.install.is_active() {
            self.syncing.set(true);
            self.install.set_active(false);
            self.syncing.set(false);
        }
        self.install.set_sensitive(self.download.is_active());
        let auto = Auto::from_switches(self.download.is_active(), self.install.is_active());
        let this = self.clone();
        glib::spawn_future_local(async move {
            this.download.set_sensitive(false);
            this.install.set_sensitive(false);
            let result = helper::run(&["auto", auto.arg()], |_| {}).await;
            this.download.set_sensitive(true);
            this.sync_auto(Auto::read());
            if let Err(e) = result {
                if e != tr("The administrator password was not accepted.") {
                    widgets::show_error(
                        &this.download,
                        &tr("Could not change automatic updates."),
                        &e,
                    );
                }
            }
        });
    }

    fn switch(self: &Rc<Self>, index: u32) {
        let Some(tag) = CHANNELS.get(index as usize).copied() else {
            return;
        };
        let current = self.booted().and_then(|b| b.channel().map(str::to_owned));
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
                if let Err(e) = this.run(&["switch", tag], &tr("Switching channel…")).await {
                    widgets::show_error(
                        &this.channel,
                        &tr("Could not change the update channel."),
                        &e,
                    );
                }
                LAST_CHECK.with_borrow_mut(|c| *c = None);
                this.remote.replace(None);
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
            let result = this.run(&["rollback"], &tr("Going back…")).await;
            this.refresh();
            match result {
                Ok(()) => this.offer_restart().await,
                Err(e) => widgets::show_error(&this.rollback, &tr("Could not go back."), &e),
            }
        });
    }

    async fn offer_restart(self: &Rc<Self>) {
        if widgets::confirm(
            &self.rollback,
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
                &self.action,
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
        .valign(gtk::Align::Start)
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
    let state = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .visible(false)
        .margin_top(6)
        .build();
    let progress = gtk::ProgressBar::builder()
        .hexpand(true)
        .show_text(true)
        .visible(false)
        .build();
    let news_title = gtk::Label::builder()
        .label(tr("What's New"))
        .css_classes(["heading"])
        .xalign(0.0)
        .build();
    let news_subject = gtk::Label::builder()
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .selectable(true)
        .build();
    let news_meta = widgets::caption("");
    news_meta.add_css_class("dim-label");
    let see_all = gtk::Button::builder()
        .css_classes(["flat"])
        .halign(gtk::Align::Start)
        .visible(false)
        .build();
    let news = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .margin_top(10)
        .visible(false)
        .build();
    news.append(&news_title);
    news.append(&news_subject);
    news.append(&news_meta);
    news.append(&see_all);
    let text = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .hexpand(true)
        .valign(gtk::Align::Start)
        .build();
    text.append(&summary);
    text.append(&detail);
    text.append(&state);
    text.append(&progress);
    text.append(&news);
    let spinner = gtk::Spinner::builder().visible(false).build();
    let check = gtk::Button::builder()
        .label(tr("Check Now"))
        .css_classes(["flat"])
        .visible(false)
        .build();
    let action = gtk::Button::builder()
        .css_classes(["suggested-action"])
        .visible(false)
        .build();
    let buttons = gtk::Box::builder()
        .spacing(8)
        .valign(gtk::Align::Start)
        .build();
    buttons.append(&spinner);
    buttons.append(&check);
    buttons.append(&action);
    let head = gtk::Box::builder().spacing(14).build();
    head.append(&icon);
    head.append(&text);
    head.append(&buttons);
    page.append(&widgets::group(None, &[widgets::plain_row(&head)]));

    let (download_row, download) = widgets::switch_row(
        &tr("Download updates automatically"),
        Some(&tr(
            "Downloads new versions in the background, except on metered networks.",
        )),
    );
    let (install_row, install) = widgets::switch_row(
        &tr("Install updates automatically"),
        Some(&tr(
            "Installs downloaded versions. They start the next time you restart.",
        )),
    );
    page.append(&widgets::group(
        Some(&tr("Automatic Updates")),
        &[download_row, install_row],
    ));

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
        state,
        progress,
        spinner,
        action,
        check,
        news,
        news_subject,
        news_meta,
        see_all,
        changes_window: RefCell::default(),
        download,
        install,
        channel,
        rollback_row,
        rollback,
        rollback_detail,
        log,
        channel_note,
        download_started: RefCell::default(),
        status: RefCell::default(),
        remote: RefCell::default(),
        checked_at: Cell::default(),
        attempted_at: Cell::default(),
        check_error: RefCell::default(),
        checking: Cell::default(),
        news_for: RefCell::default(),
        busy: Cell::default(),
        syncing: Cell::default(),
    });
    pane.sync_auto(Auto::read());
    let p = pane.clone();
    pane.check.connect_clicked(move |_| p.check());
    let p = pane.clone();
    pane.action.connect_clicked(move |_| p.act());
    let p = pane.clone();
    pane.see_all.connect_clicked(move |_| p.show_all_changes());
    let p = pane.clone();
    pane.rollback.connect_clicked(move |_| p.rollback());
    for switch in [&pane.download, &pane.install] {
        let p = pane.clone();
        switch.connect_active_notify(move |_| {
            if !p.syncing.get() {
                p.set_auto();
            }
        });
    }
    let p = pane.clone();
    pane.channel.connect_selected_notify(move |dd| {
        if !p.syncing.get() {
            p.switch(dd.selected());
        }
    });

    // While the pane is open, re-read the deployments and check again once
    // the last check is half an hour old.
    let weak = Rc::downgrade(&pane);
    glib::timeout_add_seconds_local(60, move || {
        let Some(p) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        let due = p
            .checked_at
            .get()
            .max(p.attempted_at.get())
            .is_none_or(|at| at.elapsed() >= CHECK_EVERY);
        if due && !p.busy.get() && !p.checking.get() {
            p.refresh();
        }
        glib::ControlFlow::Continue
    });

    pane.refresh();
    widgets::keep_alive(&page, pane);
    page.upcast()
}
