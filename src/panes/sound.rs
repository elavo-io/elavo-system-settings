//! Sound: output and input devices, their volume, the input level, and
//! the volume of each app that is playing.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::pulse::{Pulse, Snapshot, MAX_VOLUME};
use crate::i18n::tr;
use crate::widgets;

struct Side {
    devices: gtk::DropDown,
    names: RefCell<Vec<String>>,
    volume: gtk::Scale,
    mute: gtk::Switch,
}

struct Pane {
    pulse: Rc<Pulse>,
    root: gtk::Box,
    output: Side,
    input: Side,
    level: gtk::LevelBar,
    apps: gtk::ListBox,
    apps_box: gtk::Box,
    status: gtk::Label,
    syncing: Cell<bool>,
    last: RefCell<Snapshot>,
    meter: RefCell<Option<glib::SourceId>>,
    peak_source: RefCell<String>,
}

fn side() -> Side {
    let volume = widgets::slider(0.0, MAX_VOLUME, 0.01);
    Side {
        devices: gtk::DropDown::from_strings(&[]),
        names: RefCell::default(),
        volume,
        mute: gtk::Switch::new(),
    }
}

fn set_if_far(scale: &gtk::Scale, value: f64) {
    if (scale.value() - value).abs() > 0.005 {
        scale.set_value(value);
    }
}

impl Pane {
    fn render(self: &Rc<Self>, s: &Snapshot) {
        if s.failed && !s.ready {
            self.root.set_sensitive(false);
            widgets::set_status(&self.status, &tr("The sound server is not running."));
            return;
        }
        self.root.set_sensitive(s.ready);
        widgets::set_status(&self.status, "");
        let previous = self.last.replace(s.clone());
        self.syncing.set(true);

        for (side, list, default) in [
            (&self.output, &s.sinks, &s.default_sink),
            (&self.input, &s.sources, &s.default_source),
        ] {
            let names: Vec<String> = list.iter().map(|d| d.name.clone()).collect();
            if *side.names.borrow() != names {
                let labels: Vec<&str> = list.iter().map(|d| d.description.as_str()).collect();
                side.devices.set_model(Some(&gtk::StringList::new(&labels)));
                side.names.replace(names.clone());
            }
            side.devices.set_sensitive(names.len() > 1);
            if let Some(i) = names.iter().position(|n| n == default) {
                side.devices.set_selected(i as u32);
                let dev = &list[i];
                set_if_far(&side.volume, dev.volume.min(MAX_VOLUME));
                side.mute.set_active(dev.muted);
                side.volume.set_sensitive(!dev.muted);
            }
            let has = !list.is_empty();
            side.volume.set_sensitive(has && !side.mute.is_active());
            side.mute.set_sensitive(has);
        }

        if s.default_source != *self.peak_source.borrow() && self.root.is_mapped() {
            self.start_meter();
        }

        if previous
            .apps
            .iter()
            .map(|a| a.index)
            .ne(s.apps.iter().map(|a| a.index))
        {
            self.rebuild_apps(s);
        } else {
            self.update_apps(s);
        }
        self.syncing.set(false);
    }

    fn rebuild_apps(self: &Rc<Self>, s: &Snapshot) {
        widgets::clear(&self.apps);
        for app in &s.apps {
            let scale = widgets::slider(0.0, MAX_VOLUME, 0.01);
            scale.set_value(app.volume.min(MAX_VOLUME));
            scale.set_sensitive(!app.muted);
            let mute = gtk::ToggleButton::builder()
                .icon_name(if app.muted {
                    "audio-volume-muted-symbolic"
                } else {
                    "audio-volume-high-symbolic"
                })
                .active(app.muted)
                .tooltip_text(tr("Mute"))
                .css_classes(["flat"])
                .build();
            let this = self.clone();
            let index = app.index;
            scale.connect_value_changed(move |sc| {
                if this.syncing.get() {
                    return;
                }
                if let Some(a) = this.last.borrow().apps.iter().find(|a| a.index == index) {
                    this.pulse.set_app_volume(a, sc.value());
                }
            });
            let this = self.clone();
            mute.connect_toggled(move |b| {
                if !this.syncing.get() {
                    this.pulse.set_app_mute(index, b.is_active());
                }
            });
            let content = gtk::Box::builder().spacing(10).build();
            let icon = if app.icon.is_empty() {
                "application-x-executable-symbolic"
            } else {
                app.icon.as_str()
            };
            content.append(&gtk::Image::from_icon_name(icon));
            content.append(
                &gtk::Label::builder()
                    .label(&app.name)
                    .xalign(0.0)
                    .width_chars(14)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .build(),
            );
            content.append(&scale);
            content.append(&mute);
            let row = widgets::plain_row(&content);
            row.set_widget_name(&app.index.to_string());
            self.apps.append(&row);
        }
        self.apps_box.set_visible(!s.apps.is_empty());
    }

    fn update_apps(&self, s: &Snapshot) {
        let mut child = self.apps.first_child();
        let mut i = 0;
        while let Some(row) = child {
            if let (Some(app), Some(content)) = (
                s.apps.get(i),
                row.downcast_ref::<gtk::ListBoxRow>()
                    .and_then(|r| r.child()),
            ) {
                let mut w = content.first_child();
                while let Some(widget) = w {
                    if let Some(scale) = widget.downcast_ref::<gtk::Scale>() {
                        set_if_far(scale, app.volume.min(MAX_VOLUME));
                        scale.set_sensitive(!app.muted);
                    } else if let Some(b) = widget.downcast_ref::<gtk::ToggleButton>() {
                        b.set_active(app.muted);
                        b.set_icon_name(if app.muted {
                            "audio-volume-muted-symbolic"
                        } else {
                            "audio-volume-high-symbolic"
                        });
                    }
                    w = widget.next_sibling();
                }
            }
            i += 1;
            child = row.next_sibling();
        }
    }

    fn start_meter(self: &Rc<Self>) {
        let source = self.last.borrow().default_source.clone();
        self.peak_source.replace(source.clone());
        if source.is_empty() {
            self.pulse.stop_peak();
            return;
        }
        self.pulse.start_peak(&source);
        if self.meter.borrow().is_none() {
            let weak = Rc::downgrade(self);
            let id = glib::timeout_add_local(std::time::Duration::from_millis(60), move || {
                let Some(p) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let peak = if p.input.mute.is_active() {
                    0.0
                } else {
                    p.pulse.peak()
                };
                // Fall slowly so speech reads as a level, not a flicker.
                let shown = (p.level.value() * 0.7).max(f64::from(peak));
                p.level.set_value(shown);
                glib::ControlFlow::Continue
            });
            self.meter.replace(Some(id));
        }
    }

    fn stop_meter(&self) {
        if let Some(id) = self.meter.take() {
            id.remove();
        }
        self.pulse.stop_peak();
        self.peak_source.replace(String::new());
        self.level.set_value(0.0);
    }
}

fn connect_side(pane: &Rc<Pane>, output: bool) {
    let side = if output { &pane.output } else { &pane.input };
    let p = pane.clone();
    side.devices.connect_selected_notify(move |dd| {
        if p.syncing.get() {
            return;
        }
        let side = if output { &p.output } else { &p.input };
        let name = side.names.borrow().get(dd.selected() as usize).cloned();
        if let Some(name) = name {
            if output {
                p.pulse.set_default_sink(&name);
            } else {
                p.pulse.set_default_source(&name);
            }
        }
    });
    let p = pane.clone();
    side.volume.connect_value_changed(move |sc| {
        if p.syncing.get() {
            return;
        }
        let s = p.last.borrow();
        if output {
            if let Some(d) = s.sinks.iter().find(|d| d.name == s.default_sink) {
                p.pulse.set_sink_volume(d, sc.value());
            }
        } else if let Some(d) = s.sources.iter().find(|d| d.name == s.default_source) {
            p.pulse.set_source_volume(d, sc.value());
        }
    });
    let p = pane.clone();
    side.mute.connect_state_set(move |_, on| {
        if !p.syncing.get() {
            let s = p.last.borrow();
            if output {
                p.pulse.set_sink_mute(&s.default_sink, on);
            } else {
                p.pulse.set_source_mute(&s.default_source, on);
            }
        }
        glib::Propagation::Proceed
    });
}

pub fn build() -> gtk::Widget {
    let Some(pulse) = Pulse::get() else {
        return widgets::placeholder(
            "audio-volume-muted-symbolic",
            &tr("Sound"),
            &tr("Could not connect to the sound server."),
        );
    };
    let root = widgets::page();
    let output = side();
    let input = side();
    let level = gtk::LevelBar::builder()
        .min_value(0.0)
        .max_value(1.0)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .css_classes(["level-bar-input"])
        .build();
    level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_LOW));
    level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_HIGH));
    level.remove_offset_value(Some(gtk::LEVEL_BAR_OFFSET_FULL));

    root.append(&widgets::group(
        Some(&tr("Output")),
        &[
            widgets::row(&tr("Play sound through"), &output.devices),
            widgets::slider_row(&tr("Output volume"), &output.volume, "", ""),
            widgets::row(&tr("Mute"), &output.mute),
        ],
    ));
    root.append(&widgets::group(
        Some(&tr("Input")),
        &[
            widgets::row(&tr("Use for input"), &input.devices),
            widgets::slider_row(&tr("Input volume"), &input.volume, "", ""),
            widgets::row(&tr("Input level"), &level),
            widgets::row(&tr("Mute"), &input.mute),
        ],
    ));
    let apps_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .visible(false)
        .build();
    apps_box.append(&widgets::heading(&tr("Applications")));
    let (frame, apps) = widgets::list();
    apps_box.append(&frame);
    root.append(&apps_box);
    let status = widgets::status_label();
    root.append(&status);

    let pane = Rc::new(Pane {
        pulse: pulse.clone(),
        root: root.clone(),
        output,
        input,
        level,
        apps,
        apps_box,
        status,
        syncing: Cell::new(false),
        last: RefCell::default(),
        meter: RefCell::default(),
        peak_source: RefCell::default(),
    });
    connect_side(&pane, true);
    connect_side(&pane, false);

    let weak = Rc::downgrade(&pane);
    pulse.listen(move |s| match weak.upgrade() {
        Some(p) => {
            p.render(s);
            true
        }
        None => false,
    });
    let weak = Rc::downgrade(&pane);
    root.connect_map(move |_| {
        if let Some(p) = weak.upgrade() {
            p.start_meter();
        }
    });
    let weak = Rc::downgrade(&pane);
    root.connect_unmap(move |_| {
        if let Some(p) = weak.upgrade() {
            p.stop_meter();
        }
    });
    widgets::keep_alive(&root, pane);
    root.upcast()
}
