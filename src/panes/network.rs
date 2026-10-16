//! Wi-Fi and Network. Wi-Fi has the switch, the current network, nearby
//! networks, and the ones this computer remembers. Network lists every
//! connection and its state, with Wi-Fi opening its own pane.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::dbus::Debounce;
use crate::backend::network::{self, AccessPoint, Overview, Security};
use crate::i18n::tr;
use crate::widgets;

struct Pane {
    conn: gio::DBusConnection,
    root: gtk::Box,
    wifi_mode: bool,
    wifi_box: gtk::Box,
    wifi: gtk::Switch,
    current: gtk::ListBox,
    nearby: gtk::ListBox,
    nearby_box: gtk::Box,
    wired: gtk::ListBox,
    wired_box: gtk::Box,
    saved: gtk::ListBox,
    saved_box: gtk::Box,
    status: gtk::Label,
    syncing: Cell<bool>,
    overview: RefCell<Overview>,
    debounce: Rc<Debounce>,
    _subs: RefCell<Vec<gio::SignalSubscription>>,
}

fn signal_icon(strength: u8) -> &'static str {
    match strength {
        80..=100 => "network-wireless-signal-excellent-symbolic",
        55..=79 => "network-wireless-signal-good-symbolic",
        30..=54 => "network-wireless-signal-ok-symbolic",
        5..=29 => "network-wireless-signal-weak-symbolic",
        _ => "network-wireless-signal-none-symbolic",
    }
}

fn network_row(ap: &AccessPoint, trailing: &impl IsA<gtk::Widget>) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(10).build();
    content.append(
        &gtk::Image::builder()
            .icon_name(signal_icon(ap.strength))
            .css_classes(["signal-strength"])
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label(&ap.ssid)
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    if ap.security != Security::Open {
        content.append(
            &gtk::Image::builder()
                .icon_name("system-lock-screen-symbolic")
                .tooltip_text(tr("Needs a password"))
                .css_classes(["dim-label"])
                .build(),
        );
    }
    trailing.set_valign(gtk::Align::Center);
    content.append(trailing);
    widgets::plain_row(&content)
}

impl Pane {
    fn refresh(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let overview = network::overview(&this.conn).await;
            this.render(&overview);
            this.overview.replace(overview);
        });
    }

    fn wifi_device(&self) -> Option<network::Device> {
        self.overview
            .borrow()
            .devices
            .iter()
            .find(|d| d.is_wifi())
            .cloned()
    }

    fn render(self: &Rc<Self>, o: &Overview) {
        if !o.running {
            widgets::set_status(&self.status, &tr("NetworkManager is not running."));
            self.root.set_sensitive(false);
            return;
        }
        self.root.set_sensitive(true);
        widgets::set_status(&self.status, "");
        self.syncing.set(true);
        self.wifi.set_active(o.wifi_enabled);
        self.syncing.set(false);

        let wifi = o.devices.iter().find(|d| d.is_wifi());
        self.wifi.set_sensitive(wifi.is_some());
        widgets::clear(&self.current);
        widgets::clear(&self.nearby);
        let active_ssid = wifi
            .filter(|d| d.connected())
            .and_then(|d| o.networks.iter().find(|ap| ap.path == d.active_ap))
            .map(|ap| ap.ssid.clone());

        match (wifi, o.wifi_enabled) {
            (None, _) => {
                let (row, _) = widgets::value_row(&tr("Wi-Fi"), &tr("No Wi-Fi hardware"));
                self.current.append(&row);
            }
            (Some(_), false) => {
                let (row, _) = widgets::value_row(&tr("Status"), &tr("Off"));
                self.current.append(&row);
            }
            (Some(device), true) => {
                let active = o
                    .networks
                    .iter()
                    .find(|ap| device.connected() && ap.path == device.active_ap);
                if let Some(ap) = active {
                    let disconnect = gtk::Button::with_label(&tr("Disconnect"));
                    let this = self.clone();
                    let path = device.path.clone();
                    disconnect.connect_clicked(move |b| {
                        let this = this.clone();
                        let path = path.clone();
                        let b = b.clone();
                        glib::spawn_future_local(async move {
                            if let Err(e) = network::disconnect(&this.conn, &path).await {
                                widgets::show_error(
                                    &b,
                                    &tr("Could not disconnect."),
                                    &widgets::error_text(&e),
                                );
                            }
                        });
                    });
                    let row = network_row(ap, &disconnect);
                    self.current.append(&row);
                    let (detail, _) = widgets::value_row(
                        &tr("IP address"),
                        if device.address.is_empty() {
                            "—"
                        } else {
                            &device.address
                        },
                    );
                    self.current.append(&detail);
                    if !device.gateway.is_empty() {
                        let (gw, _) = widgets::value_row(&tr("Router"), &device.gateway);
                        self.current.append(&gw);
                    }
                } else {
                    let (row, _) = widgets::value_row(&tr("Status"), &tr("Not connected"));
                    self.current.append(&row);
                }
                for ap in o
                    .networks
                    .iter()
                    .filter(|ap| Some(&ap.ssid) != active_ssid.as_ref())
                {
                    let join = gtk::Button::with_label(&tr("Connect"));
                    let this = self.clone();
                    let ap_c = ap.clone();
                    join.connect_clicked(move |b| this.join(b, &ap_c));
                    self.nearby.append(&network_row(ap, &join));
                }
            }
        }
        self.nearby_box
            .set_visible(self.wifi_mode && o.wifi_enabled && self.nearby.first_child().is_some());
        self.wifi_box.set_visible(self.wifi_mode);

        widgets::clear(&self.wired);
        if !self.wifi_mode {
            let state = match (wifi, o.wifi_enabled, &active_ssid) {
                (None, _, _) => tr("Not available"),
                (Some(_), false, _) => tr("Off"),
                (Some(_), true, Some(_)) => tr("Connected"),
                (Some(_), true, None) => tr("Not connected"),
            };
            self.wired.append(&widgets::nav_row(
                "network-wireless-symbolic",
                "blue",
                &tr("Wi-Fi"),
                Some(&state),
                Some("wifi"),
            ));
        }
        for d in o.devices.iter().filter(|d| d.is_ethernet()) {
            let state = if d.connected() {
                if d.address.is_empty() {
                    tr("Connected")
                } else {
                    format!("{} · {}", tr("Connected"), d.address)
                }
            } else if d.available() {
                tr("Not connected")
            } else {
                tr("Cable unplugged")
            };
            let title = format!("{} ({})", tr("Ethernet"), d.interface);
            self.wired.append(&widgets::nav_row(
                "network-wired-symbolic",
                "blue",
                &title,
                Some(&state),
                None,
            ));
        }
        self.wired_box
            .set_visible(!self.wifi_mode && self.wired.first_child().is_some());

        widgets::clear(&self.saved);
        for s in &o.saved {
            let auto = gtk::Switch::builder()
                .active(s.autoconnect)
                .tooltip_text(tr("Join automatically"))
                .valign(gtk::Align::Center)
                .build();
            let this = self.clone();
            let path = s.path.clone();
            auto.connect_state_set(move |sw, on| {
                if this.syncing.get() {
                    return glib::Propagation::Proceed;
                }
                let this = this.clone();
                let path = path.clone();
                let sw = sw.clone();
                glib::spawn_future_local(async move {
                    if let Err(e) = network::set_autoconnect(&this.conn, &path, on).await {
                        widgets::show_error(
                            &sw,
                            &tr("Could not change this network."),
                            &widgets::error_text(&e),
                        );
                        this.refresh();
                    }
                });
                glib::Propagation::Proceed
            });
            let forget = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Forget this network"))
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .build();
            let this = self.clone();
            let path = s.path.clone();
            let name = s.id.clone();
            forget.connect_clicked(move |b| {
                let this = this.clone();
                let path = path.clone();
                let name = name.clone();
                let b = b.clone();
                glib::spawn_future_local(async move {
                    let ok = widgets::confirm(
                        &b,
                        &format!("{} “{name}”?", tr("Forget")),
                        &tr("This computer will stop joining it automatically, and you will need the password to join again."),
                        &tr("Forget"),
                    )
                    .await;
                    if ok {
                        if let Err(e) = network::forget(&this.conn, &path).await {
                            widgets::show_error(&b, &tr("Could not forget this network."), &widgets::error_text(&e));
                        }
                    }
                });
            });
            let controls = gtk::Box::builder().spacing(8).build();
            controls.append(&widgets::caption(&tr("Auto-join")));
            controls.append(&auto);
            controls.append(&forget);
            self.saved.append(&widgets::row(&s.id, &controls));
        }
        self.saved_box
            .set_visible(self.wifi_mode && self.saved.first_child().is_some());
    }

    fn join(self: &Rc<Self>, button: &gtk::Button, ap: &AccessPoint) {
        let Some(device) = self.wifi_device() else {
            return;
        };
        let saved = self
            .overview
            .borrow()
            .saved
            .iter()
            .find(|s| s.ssid.as_deref() == Some(ap.ssid.as_str()))
            .cloned();
        let this = self.clone();
        let ap = ap.clone();
        let button = button.clone();
        glib::spawn_future_local(async move {
            if ap.security == Security::Enterprise && saved.is_none() {
                widgets::show_error(
                    &button,
                    &tr("This network needs a user name and certificate."),
                    &tr("Enterprise Wi-Fi is not supported here yet. Use nmcli or nm-connection-editor to set it up."),
                );
                return;
            }
            button.set_sensitive(false);
            let mut result = Err(None);
            if saved.is_some() || ap.security == Security::Open {
                result = network::connect(&this.conn, &device.path, &ap, saved.as_ref(), None)
                    .await
                    .map_err(Some);
            }
            if result.is_err() && ap.security != Security::Open {
                let values = widgets::ask(
                    &button,
                    &format!("{} “{}”", tr("Join"), ap.ssid),
                    &tr("Enter the network's password."),
                    &[(&tr("Password"), true)],
                    &tr("Join"),
                )
                .await;
                if let Some(password) = values.and_then(|v| v.into_iter().next()) {
                    result = network::connect(
                        &this.conn,
                        &device.path,
                        &ap,
                        saved.as_ref(),
                        Some(&password),
                    )
                    .await
                    .map_err(Some);
                } else {
                    result = Err(None);
                }
            }
            button.set_sensitive(true);
            if let Err(Some(e)) = result {
                widgets::show_error(
                    &button,
                    &tr("Could not join the network."),
                    &widgets::error_text(&e),
                );
            }
        });
    }
}

fn section(title: &str) -> (gtk::Box, gtk::ListBox) {
    let outer = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    outer.append(&widgets::heading(title));
    let (frame, list) = widgets::list();
    outer.append(&frame);
    (outer, list)
}

pub fn build_wifi() -> gtk::Widget {
    build(true)
}

pub fn build_network() -> gtk::Widget {
    build(false)
}

fn build(wifi_mode: bool) -> gtk::Widget {
    let root = widgets::page();
    let Some(conn) = network::bus() else {
        return widgets::placeholder(
            "network-wireless-offline-symbolic",
            &if wifi_mode {
                tr("Wi-Fi")
            } else {
                tr("Network")
            },
            &tr("The system bus is not available."),
        );
    };

    let wifi = gtk::Switch::new();
    let wifi_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    let (wifi_frame, wifi_list) = widgets::list();
    wifi_box.append(&wifi_frame);
    wifi_list.append(&widgets::row(&tr("Wi-Fi"), &wifi));
    let (current_frame, current) = widgets::list();
    wifi_box.append(&current_frame);

    let scan = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text(tr("Look for networks"))
        .css_classes(["flat"])
        .build();
    let nearby_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    let nearby_header = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    let nearby_title = widgets::heading(&tr("Other networks"));
    nearby_title.set_hexpand(true);
    nearby_header.append(&nearby_title);
    nearby_header.append(&scan);
    nearby_box.append(&nearby_header);
    let (nearby_frame, nearby) = widgets::list();
    nearby_box.append(&nearby_frame);

    let wired_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    let (wired_frame, wired) = widgets::list();
    wired_box.append(&wired_frame);
    let (saved_box, saved) = section(&tr("Known networks"));
    let status = widgets::status_label();

    root.append(&wifi_box);
    root.append(&nearby_box);
    root.append(&wired_box);
    root.append(&saved_box);
    root.append(&status);

    let pane = Rc::new(Pane {
        conn: conn.clone(),
        root: root.clone(),
        wifi_mode,
        wifi_box: wifi_box.clone(),
        wifi,
        current,
        nearby,
        nearby_box,
        wired,
        wired_box,
        saved,
        saved_box,
        status,
        syncing: Cell::new(false),
        overview: RefCell::default(),
        debounce: Debounce::new(300),
        _subs: RefCell::default(),
    });

    let p = pane.clone();
    pane.wifi.connect_state_set(move |sw, on| {
        if p.syncing.get() {
            return glib::Propagation::Proceed;
        }
        let p = p.clone();
        let sw = sw.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = network::set_wifi_enabled(&p.conn, on).await {
                widgets::show_error(
                    &sw,
                    &tr("Could not change Wi-Fi."),
                    &widgets::error_text(&e),
                );
                p.refresh();
            }
        });
        glib::Propagation::Proceed
    });
    let p = pane.clone();
    scan.connect_clicked(move |_| {
        if let Some(d) = p.wifi_device() {
            let p = p.clone();
            glib::spawn_future_local(async move { network::scan(&p.conn, &d.path).await });
        }
    });

    let weak = Rc::downgrade(&pane);
    let subs = network::watch(&conn, move || {
        if let Some(p) = weak.upgrade() {
            let p2 = p.clone();
            p.debounce.run(move || p2.refresh());
        }
    });
    pane._subs.replace(subs);
    pane.refresh();
    // Scan each time the pane is shown; NetworkManager rate-limits scans.
    let weak = Rc::downgrade(&pane);
    root.connect_map(move |_| {
        if !wifi_mode {
            return;
        }
        if let Some(d) = weak.upgrade().and_then(|p| p.wifi_device().map(|d| (p, d))) {
            let (p, d) = d;
            glib::spawn_future_local(async move { network::scan(&p.conn, &d.path).await });
        }
    });

    widgets::keep_alive(&root, pane);
    root.upcast()
}
