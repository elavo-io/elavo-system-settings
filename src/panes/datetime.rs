//! Date & Time: network time, the time zone, the clock format, and the
//! date and time when network time is off.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::timedate::{self, Status};
use crate::i18n::tr;
use crate::widgets;

struct Pane {
    conn: gio::DBusConnection,
    now: gtk::Label,
    ntp: gtk::Switch,
    manual_row: gtk::ListBoxRow,
    manual: gtk::Entry,
    zone: gtk::Label,
    syncing: Cell<bool>,
    status: RefCell<Status>,
}

const MANUAL_FORMAT: &str = "%Y-%m-%d %H:%M";

impl Pane {
    fn tick(&self) {
        let format = if timedate::uses_24h() {
            "%A, %B %-e, %Y  %H:%M:%S"
        } else {
            "%A, %B %-e, %Y  %-l:%M:%S %p"
        };
        if let Ok(text) = glib::DateTime::now_local().and_then(|t| t.format(format)) {
            self.now.set_label(&text);
        }
    }

    fn refresh(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let s = timedate::status(&this.conn).await;
            this.syncing.set(true);
            this.ntp.set_active(s.ntp);
            this.ntp.set_sensitive(s.available && s.can_ntp);
            this.manual_row.set_visible(s.available && !s.ntp);
            this.zone.set_label(&if s.timezone.is_empty() {
                tr("Unknown")
            } else {
                timedate::zone_label(&s.timezone)
            });
            if let Ok(text) = glib::DateTime::now_local().and_then(|t| t.format(MANUAL_FORMAT)) {
                if !this.manual.has_focus() {
                    this.manual.set_text(&text);
                }
            }
            this.syncing.set(false);
            this.status.replace(s);
        });
    }

    fn pick_zone(self: &Rc<Self>, button: &gtk::Button) {
        let this = self.clone();
        let button = button.clone();
        glib::spawn_future_local(async move {
            let zones = timedate::timezones(&this.conn).await;
            if zones.is_empty() {
                widgets::show_error(&button, &tr("The list of time zones is not available."), "");
                return;
            }
            let window = widgets::popup_window(&button, &tr("Time Zone"));
            window.set_default_size(380, 480);
            let search = gtk::SearchEntry::builder()
                .placeholder_text(tr("Search for a city"))
                .margin_top(12)
                .margin_start(12)
                .margin_end(12)
                .build();
            let list = gtk::ListBox::builder()
                .selection_mode(gtk::SelectionMode::Single)
                .css_classes(["navigation-sidebar"])
                .build();
            let labels: Vec<String> = zones.iter().map(|z| timedate::zone_label(z)).collect();
            for (zone, label) in zones.iter().zip(&labels) {
                let row = gtk::ListBoxRow::builder()
                    .child(&gtk::Label::builder().label(label).xalign(0.0).build())
                    .name(zone)
                    .build();
                list.append(&row);
            }
            let keys: Vec<String> = zones
                .iter()
                .zip(&labels)
                .map(|(z, l)| format!("{z} {l}").to_lowercase())
                .collect();
            let query = Rc::new(RefCell::new(String::new()));
            let q = query.clone();
            list.set_filter_func(move |row| {
                let q = q.borrow();
                q.is_empty()
                    || keys
                        .get(row.index() as usize)
                        .is_some_and(|k| k.contains(q.as_str()))
            });
            let l = list.clone();
            search.connect_search_changed(move |e| {
                query.replace(e.text().trim().to_lowercase());
                l.invalidate_filter();
            });
            let current = this.status.borrow().timezone.clone();
            if let Some(i) = zones.iter().position(|z| *z == current) {
                if let Some(row) = list.row_at_index(i as i32) {
                    list.select_row(Some(&row));
                }
            }
            let root = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(8)
                .build();
            root.append(&search);
            root.append(
                &gtk::ScrolledWindow::builder()
                    .vexpand(true)
                    .hscrollbar_policy(gtk::PolicyType::Never)
                    .child(&list)
                    .build(),
            );
            window.set_child(Some(&root));
            let p = this.clone();
            let w = window.downgrade();
            list.connect_row_activated(move |_, row| {
                let zone = row.widget_name().to_string();
                let p = p.clone();
                let w = w.clone();
                glib::spawn_future_local(async move {
                    let result = timedate::set_timezone(&p.conn, &zone).await;
                    if let Some(w) = w.upgrade() {
                        if let Err(e) = result {
                            widgets::show_error(
                                &w,
                                &tr("Could not change the time zone."),
                                &widgets::error_text(&e),
                            );
                        } else {
                            w.close();
                        }
                    }
                    p.refresh();
                });
            });
            window.present();
            search.grab_focus();
        });
    }
}

pub fn build() -> gtk::Widget {
    let page = widgets::page();
    let Some(conn) = timedate::bus() else {
        return widgets::placeholder(
            "preferences-system-time-symbolic",
            &tr("Date & Time"),
            &tr("The system bus is not available."),
        );
    };
    let now = gtk::Label::builder()
        .css_classes(["title-3"])
        .xalign(0.0)
        .selectable(true)
        .build();
    page.append(&now);

    let ntp = gtk::Switch::new();
    let manual = gtk::Entry::builder()
        .placeholder_text("2026-10-15 21:00")
        .width_chars(18)
        .build();
    let set = gtk::Button::with_label(&tr("Set"));
    let manual_box = gtk::Box::builder().spacing(6).build();
    manual_box.append(&manual);
    manual_box.append(&set);
    let manual_row = widgets::row(&tr("Date and time"), &manual_box);
    let zone = gtk::Label::builder().css_classes(["dim-label"]).build();
    let change = gtk::Button::with_label(&tr("Change…"));
    let zone_box = gtk::Box::builder().spacing(10).build();
    zone_box.append(&zone);
    zone_box.append(&change);
    let (h24_row, h24) = widgets::switch_row(
        &tr("24-hour time"),
        Some(&tr("The menu bar clock follows this.")),
    );
    h24.set_active(timedate::uses_24h());

    page.append(&widgets::group(
        None,
        &[
            widgets::row_with_subtitle(
                &tr("Set time and date automatically"),
                Some(&tr("Uses network time.")),
                &ntp,
            ),
            manual_row.clone(),
        ],
    ));
    page.append(&widgets::group(
        None,
        &[widgets::row(&tr("Time zone"), &zone_box), h24_row],
    ));

    let pane = Rc::new(Pane {
        conn: conn.clone(),
        now,
        ntp,
        manual_row,
        manual: manual.clone(),
        zone,
        syncing: Cell::new(false),
        status: RefCell::default(),
    });

    let p = pane.clone();
    pane.ntp.connect_state_set(move |sw, on| {
        if p.syncing.get() {
            return glib::Propagation::Proceed;
        }
        let p = p.clone();
        let sw = sw.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = timedate::set_ntp(&p.conn, on).await {
                widgets::show_error(
                    &sw,
                    &tr("Could not change network time."),
                    &widgets::error_text(&e),
                );
            }
            p.refresh();
        });
        glib::Propagation::Proceed
    });
    let p = pane.clone();
    let apply_manual = Rc::new(move |w: gtk::Widget| {
        let text = p.manual.text().to_string();
        let parsed = text.trim().split_once(' ').and_then(|(d, t)| {
            let mut d = d.split('-').filter_map(|x| x.parse::<i32>().ok());
            let mut t = t.split(':').filter_map(|x| x.parse::<i32>().ok());
            let (y, mo, da) = (d.next()?, d.next()?, d.next()?);
            let (h, mi) = (t.next()?, t.next()?);
            glib::DateTime::from_local(y, mo, da, h, mi, 0.0).ok()
        });
        let Some(when) = parsed else {
            widgets::show_error(&w, &tr("Enter the date and time as YYYY-MM-DD HH:MM."), "");
            return;
        };
        let p = p.clone();
        glib::spawn_future_local(async move {
            let usec = when.to_unix() * 1_000_000;
            if let Err(e) = timedate::set_time(&p.conn, usec).await {
                widgets::show_error(&w, &tr("Could not set the time."), &widgets::error_text(&e));
            }
            p.refresh();
        });
    });
    let a = apply_manual.clone();
    set.connect_clicked(move |b| a(b.clone().upcast()));
    let a = apply_manual.clone();
    manual.connect_activate(move |e| a(e.clone().upcast()));
    let p = pane.clone();
    change.connect_clicked(move |b| p.pick_zone(b));
    let p = pane.clone();
    h24.connect_state_set(move |_, on| {
        timedate::set_24h(on);
        p.tick();
        glib::Propagation::Proceed
    });

    let weak = Rc::downgrade(&pane);
    let sub = timedate::watch(&conn, move || {
        if let Some(p) = weak.upgrade() {
            p.refresh();
        }
    });
    let weak = Rc::downgrade(&pane);
    glib::timeout_add_seconds_local(1, move || match weak.upgrade() {
        Some(p) => {
            p.tick();
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });
    pane.tick();
    pane.refresh();
    widgets::keep_alive(&page, (pane, sub));
    page.upcast()
}
