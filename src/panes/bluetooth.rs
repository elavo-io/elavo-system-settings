//! Bluetooth: the adapter switch, paired devices, and devices nearby while
//! the pane is open. Pairing codes are confirmed through the agent.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};

use crate::backend::bluetooth::{self, Answer, Device, Overview, Request};
use crate::backend::dbus::Debounce;
use crate::i18n::tr;
use crate::widgets;

struct Pane {
    conn: gio::DBusConnection,
    root: gtk::Box,
    power: gtk::Switch,
    name: gtk::Label,
    paired: gtk::ListBox,
    paired_box: gtk::Box,
    nearby: gtk::ListBox,
    nearby_box: gtk::Box,
    status: gtk::Label,
    syncing: Cell<bool>,
    overview: RefCell<Overview>,
    debounce: Rc<Debounce>,
    discovering: Cell<bool>,
    _subs: RefCell<Vec<gio::SignalSubscription>>,
    _agent: RefCell<Option<bluetooth::Agent>>,
}

fn device_icon(icon: &str) -> String {
    let known = [
        "audio-headphones",
        "audio-headset",
        "audio-speakers",
        "input-keyboard",
        "input-mouse",
        "input-gaming",
        "input-tablet",
        "phone",
        "computer",
        "camera-photo",
        "printer",
    ];
    let base = if icon.starts_with("audio-card") {
        "audio-speakers"
    } else if known.contains(&icon) {
        icon
    } else {
        "bluetooth-active"
    };
    format!("{base}-symbolic")
}

impl Pane {
    fn adapter(&self) -> Option<String> {
        self.overview
            .borrow()
            .adapter
            .as_ref()
            .map(|a| a.path.clone())
    }

    fn refresh(self: &Rc<Self>) {
        let this = self.clone();
        glib::spawn_future_local(async move {
            let overview = bluetooth::overview(&this.conn).await;
            this.overview.replace(overview.clone());
            this.render(&overview);
        });
    }

    fn act(self: &Rc<Self>, widget: &gtk::Widget, device: Device, what: &'static str) {
        let this = self.clone();
        let widget = widget.clone();
        glib::spawn_future_local(async move {
            widget.set_sensitive(false);
            let result = match what {
                "pair" => bluetooth::pair(&this.conn, &device.path).await,
                "connect" => bluetooth::connect(&this.conn, &device.path).await,
                "disconnect" => bluetooth::disconnect(&this.conn, &device.path).await,
                _ => {
                    let ok = widgets::confirm(
                        &widget,
                        &format!("{} “{}”?", tr("Forget"), device.name),
                        &tr("You will need to pair it again to use it with this computer."),
                        &tr("Forget"),
                    )
                    .await;
                    if !ok {
                        widget.set_sensitive(true);
                        return;
                    }
                    match this.adapter() {
                        Some(a) => bluetooth::remove(&this.conn, &a, &device.path).await,
                        None => Ok(()),
                    }
                }
            };
            widget.set_sensitive(true);
            if let Err(e) = result {
                widgets::show_error(
                    &widget,
                    &tr("Bluetooth could not do that."),
                    &widgets::error_text(&e),
                );
            }
        });
    }

    fn device_row(self: &Rc<Self>, d: &Device) -> gtk::ListBoxRow {
        let content = gtk::Box::builder().spacing(10).build();
        content.append(&gtk::Image::from_icon_name(&device_icon(&d.icon)));
        let labels = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .hexpand(true)
            .build();
        labels.append(&gtk::Label::builder().label(&d.name).xalign(0.0).build());
        if d.paired {
            labels.append(&widgets::caption(&if d.connected {
                tr("Connected")
            } else {
                tr("Not connected")
            }));
        }
        content.append(&labels);
        let buttons = gtk::Box::builder()
            .spacing(6)
            .valign(gtk::Align::Center)
            .build();
        if d.paired {
            let (label, what) = if d.connected {
                (tr("Disconnect"), "disconnect")
            } else {
                (tr("Connect"), "connect")
            };
            let b = gtk::Button::with_label(&label);
            let this = self.clone();
            let dev = d.clone();
            b.connect_clicked(move |b| this.act(b.upcast_ref(), dev.clone(), what));
            buttons.append(&b);
            let forget = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .tooltip_text(tr("Forget this device"))
                .css_classes(["flat"])
                .build();
            let this = self.clone();
            let dev = d.clone();
            forget.connect_clicked(move |b| this.act(b.upcast_ref(), dev.clone(), "remove"));
            buttons.append(&forget);
        } else {
            let b = gtk::Button::with_label(&tr("Pair"));
            let this = self.clone();
            let dev = d.clone();
            b.connect_clicked(move |b| this.act(b.upcast_ref(), dev.clone(), "pair"));
            buttons.append(&b);
        }
        content.append(&buttons);
        widgets::plain_row(&content)
    }

    fn render(self: &Rc<Self>, o: &Overview) {
        let Some(adapter) = &o.adapter else {
            self.root.set_sensitive(false);
            widgets::set_status(&self.status, &tr("No Bluetooth adapter was found."));
            self.paired_box.set_visible(false);
            self.nearby_box.set_visible(false);
            return;
        };
        self.root.set_sensitive(true);
        widgets::set_status(&self.status, "");
        self.syncing.set(true);
        self.power.set_active(adapter.powered);
        self.syncing.set(false);
        self.name.set_label(&if adapter.name.is_empty() {
            String::new()
        } else {
            format!(
                "{} “{}”.",
                tr("This computer appears to others as"),
                adapter.name
            )
        });

        if adapter.powered && !adapter.discovering && !self.discovering.get() {
            self.discovering.set(true);
            let conn = self.conn.clone();
            let path = adapter.path.clone();
            glib::spawn_future_local(async move {
                let _ = bluetooth::discover(&conn, &path, true).await;
            });
        }
        if !adapter.powered {
            self.discovering.set(false);
        }

        widgets::clear(&self.paired);
        widgets::clear(&self.nearby);
        for d in &o.devices {
            let row = self.device_row(d);
            if d.paired {
                self.paired.append(&row);
            } else {
                self.nearby.append(&row);
            }
        }
        self.paired_box
            .set_visible(adapter.powered && self.paired.first_child().is_some());
        self.nearby_box.set_visible(adapter.powered);
        if adapter.powered && self.nearby.first_child().is_none() {
            let (row, _) = widgets::value_row(&tr("Looking for devices…"), "");
            self.nearby.append(&row);
        }
    }
}

fn ask(
    root: glib::WeakRef<gtk::Box>,
    request: Request,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Answer>>> {
    Box::pin(async move {
        let Some(root) = root.upgrade() else {
            return Answer::Reject;
        };
        let path = match &request {
            Request::Confirm { device, .. }
            | Request::Pin { device }
            | Request::Passkey { device }
            | Request::Display { device, .. } => device.clone(),
        };
        let name = match bluetooth::bus() {
            Some(conn) => bluetooth::device_name(&conn, &path).await,
            None => String::new(),
        };
        let pair_title = if name.is_empty() {
            tr("Pair with this device?")
        } else {
            format!("{} {name}?", tr("Pair with"))
        };
        match request {
            Request::Confirm { passkey, .. } => {
                let ok = widgets::confirm(
                    &root,
                    &pair_title,
                    &format!(
                        "{}\n\n{:06}",
                        tr("Make sure the device shows this code:"),
                        passkey
                    ),
                    &tr("Pair"),
                )
                .await;
                if ok {
                    Answer::Accept
                } else {
                    Answer::Reject
                }
            }
            Request::Pin { .. } | Request::Passkey { .. } => {
                let values = widgets::ask(
                    &root,
                    &pair_title,
                    &tr("Enter the code shown on the device or in its instructions."),
                    &[(&tr("Code"), false)],
                    &tr("Pair"),
                )
                .await;
                match values.and_then(|v| v.into_iter().next()) {
                    Some(code) => Answer::Text(code),
                    None => Answer::Reject,
                }
            }
            Request::Display { code, .. } => {
                widgets::show_error(
                    &root,
                    &tr("Type this code on the device"),
                    &format!("{code}\n\n{}", tr("Then press Return on the device.")),
                );
                Answer::Accept
            }
        }
    })
}

pub fn build() -> gtk::Widget {
    let root = widgets::page();
    let Some(conn) = bluetooth::bus() else {
        return widgets::placeholder(
            "bluetooth-disabled-symbolic",
            &tr("Bluetooth"),
            &tr("The system bus is not available."),
        );
    };
    let power = gtk::Switch::new();
    let name = widgets::caption("");
    root.append(&widgets::group(
        None,
        &[widgets::row(&tr("Bluetooth"), &power)],
    ));
    root.append(&name);

    let paired_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    paired_box.append(&widgets::heading(&tr("My devices")));
    let (frame, paired) = widgets::list();
    paired_box.append(&frame);
    let nearby_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    nearby_box.append(&widgets::heading(&tr("Nearby devices")));
    let (frame, nearby) = widgets::list();
    nearby_box.append(&frame);
    let status = widgets::status_label();
    root.append(&paired_box);
    root.append(&nearby_box);
    root.append(&status);

    let pane = Rc::new(Pane {
        conn: conn.clone(),
        root: root.clone(),
        power,
        name,
        paired,
        paired_box,
        nearby,
        nearby_box,
        status,
        syncing: Cell::new(false),
        overview: RefCell::default(),
        debounce: Debounce::new(300),
        discovering: Cell::new(false),
        _subs: RefCell::default(),
        _agent: RefCell::default(),
    });

    let p = pane.clone();
    pane.power.connect_state_set(move |sw, on| {
        if p.syncing.get() {
            return glib::Propagation::Proceed;
        }
        let Some(adapter) = p.adapter() else {
            return glib::Propagation::Stop;
        };
        let p = p.clone();
        let sw = sw.clone();
        glib::spawn_future_local(async move {
            if let Err(e) = bluetooth::set_powered(&p.conn, &adapter, on).await {
                widgets::show_error(
                    &sw,
                    &tr("Could not change Bluetooth."),
                    &widgets::error_text(&e),
                );
                p.refresh();
            }
        });
        glib::Propagation::Proceed
    });

    let weak = Rc::downgrade(&pane);
    pane._subs.replace(bluetooth::watch(&conn, move || {
        if let Some(p) = weak.upgrade() {
            let p2 = p.clone();
            p.debounce.run(move || p2.refresh());
        }
    }));
    let weak_root = root.downgrade();
    pane._agent
        .replace(bluetooth::register_agent(&conn, move |r| {
            ask(weak_root.clone(), r)
        }));

    // Stop scanning when the pane is hidden, so the radio is not kept busy.
    let p = Rc::downgrade(&pane);
    root.connect_unmap(move |_| {
        if let Some(p) = p.upgrade() {
            if let Some(adapter) = p.adapter() {
                p.discovering.set(false);
                let conn = p.conn.clone();
                glib::spawn_future_local(async move {
                    let _ = bluetooth::discover(&conn, &adapter, false).await;
                });
            }
        }
    });
    let p = Rc::downgrade(&pane);
    root.connect_map(move |_| {
        if let Some(p) = p.upgrade() {
            p.refresh();
        }
    });

    pane.refresh();
    widgets::keep_alive(&root, pane);
    root.upcast()
}
