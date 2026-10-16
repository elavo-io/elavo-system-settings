//! Displays: arrange displays by dragging them, and set resolution, refresh
//! rate, scale, and rotation. A change is tested, applied, and kept only
//! when the person confirms within 15 seconds; kept layouts are saved as a
//! kanshi profile so they come back at login and on reconnect.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use wayland_client::protocol::wl_output::Transform;

use crate::backend::kanshi::{self, ProfileOutput};
use crate::backend::outputs::{self, Controller, Event, HeadConfig, Outcome, Output};
use crate::i18n::tr;
use crate::widgets;

const SCALES: [f64; 8] = [1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 3.0];
const TRANSFORMS: [Transform; 4] = [
    Transform::Normal,
    Transform::_90,
    Transform::_180,
    Transform::_270,
];
const CANVAS_HEIGHT: i32 = 220;
const KEEP_SECONDS: u32 = 15;

#[derive(Clone, Debug, PartialEq)]
struct Edit {
    enabled: bool,
    mode: Option<usize>,
    x: i32,
    y: i32,
    transform: Transform,
    scale: f64,
}

impl Edit {
    fn of(o: &Output) -> Self {
        Self {
            enabled: o.enabled,
            mode: o
                .current_mode
                .or_else(|| o.modes.iter().position(|m| m.preferred)),
            x: o.x,
            y: o.y,
            transform: o.transform,
            scale: o.scale,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Rect {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl Rect {
    fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// Move `moving` to the closest spot that touches one of `others` and
/// overlaps none of them.
fn snap(moving: Rect, others: &[Rect]) -> (i32, i32) {
    if others.is_empty() {
        return (0, 0);
    }
    let align = |v: i32, target: i32| if (v - target).abs() < 48 { target } else { v };
    let mut candidates = Vec::new();
    for o in others {
        let y = align(moving.y, o.y).clamp(o.y - moving.h + 1, o.y + o.h - 1);
        let x = align(moving.x, o.x).clamp(o.x - moving.w + 1, o.x + o.w - 1);
        candidates.push((o.x + o.w, y));
        candidates.push((o.x - moving.w, y));
        candidates.push((x, o.y - moving.h));
        candidates.push((x, o.y + o.h));
    }
    let free = |&(x, y): &(i32, i32)| {
        let r = Rect { x, y, ..moving };
        !others.iter().any(|o| r.overlaps(o))
    };
    let dist = |&(x, y): &(i32, i32)| {
        let (dx, dy) = (i64::from(x - moving.x), i64::from(y - moving.y));
        dx * dx + dy * dy
    };
    candidates
        .iter()
        .copied()
        .filter(free)
        .min_by_key(dist)
        .unwrap_or((others[0].x + others[0].w, others[0].y))
}

type OnOutcome = Box<dyn FnOnce(Outcome)>;

struct Pane {
    controller: Controller,
    root: gtk::Box,
    outputs: RefCell<Vec<Output>>,
    edits: RefCell<Vec<Edit>>,
    serial: Cell<u32>,
    selected: Cell<usize>,
    callbacks: RefCell<HashMap<u32, OnOutcome>>,
    busy: Cell<bool>,
    syncing: Cell<bool>,

    area: gtk::DrawingArea,
    fixed: gtk::Fixed,
    tiles: RefCell<Vec<gtk::Button>>,
    drag_start: Cell<(i32, i32)>,
    layout: Cell<(f64, f64, f64, i32, i32)>,

    display_row: gtk::ListBoxRow,
    display_pick: gtk::DropDown,
    enabled_row: gtk::ListBoxRow,
    enabled: gtk::Switch,
    resolution: gtk::DropDown,
    resolutions: RefCell<Vec<(i32, i32)>>,
    refresh: gtk::DropDown,
    refreshes: RefCell<Vec<usize>>,
    scale: gtk::DropDown,
    rotation: gtk::DropDown,
    apply: gtk::Button,
    revert: gtk::Button,
    status: gtk::Label,
}

impl Pane {
    fn rect(&self, i: usize) -> Option<Rect> {
        let outputs = self.outputs.borrow();
        let edits = self.edits.borrow();
        let (o, e) = (outputs.get(i)?, edits.get(i)?);
        if !e.enabled {
            return None;
        }
        let (w, h) = o.logical_size(e.mode, e.transform, e.scale);
        Some(Rect {
            x: e.x,
            y: e.y,
            w,
            h,
        })
    }

    fn others(&self, i: usize) -> Vec<Rect> {
        (0..self.edits.borrow().len())
            .filter(|j| *j != i)
            .filter_map(|j| self.rect(j))
            .collect()
    }

    /// Shift everything so the top-left display starts at 0,0.
    fn normalize(&self) {
        let rects: Vec<Rect> = (0..self.edits.borrow().len())
            .filter_map(|i| self.rect(i))
            .collect();
        let (Some(minx), Some(miny)) = (
            rects.iter().map(|r| r.x).min(),
            rects.iter().map(|r| r.y).min(),
        ) else {
            return;
        };
        for e in self.edits.borrow_mut().iter_mut().filter(|e| e.enabled) {
            e.x -= minx;
            e.y -= miny;
        }
    }

    /// After a size change, move display `i` if it now overlaps another.
    fn resolve(&self, i: usize) {
        if let Some(r) = self.rect(i) {
            let others = self.others(i);
            if others.iter().any(|o| r.overlaps(o)) {
                let (x, y) = snap(r, &others);
                if let Some(e) = self.edits.borrow_mut().get_mut(i) {
                    e.x = x;
                    e.y = y;
                }
            }
        }
        self.normalize();
    }

    fn dirty(&self) -> bool {
        let outputs = self.outputs.borrow();
        let edits = self.edits.borrow();
        outputs
            .iter()
            .zip(edits.iter())
            .any(|(o, e)| Edit::of(o) != *e)
    }

    fn on_snapshot(self: &Rc<Self>, outputs: Vec<Output>, serial: u32) {
        self.serial.set(serial);
        let same_heads = {
            let old = self.outputs.borrow();
            old.len() == outputs.len() && old.iter().zip(&outputs).all(|(a, b)| a.key == b.key)
        };
        let keep_edits = same_heads && self.dirty() && !self.busy.get();
        if !keep_edits {
            self.edits.replace(outputs.iter().map(Edit::of).collect());
        }
        self.outputs.replace(outputs);
        if self.selected.get() >= self.outputs.borrow().len() {
            self.selected.set(0);
        }
        self.rebuild_tiles();
        self.sync_controls();
    }

    fn rebuild_tiles(self: &Rc<Self>) {
        for t in self.tiles.borrow_mut().drain(..) {
            self.fixed.remove(&t);
        }
        let outputs = self.outputs.borrow().clone();
        for (i, o) in outputs.iter().enumerate() {
            let label = gtk::Label::builder()
                .label(if o.model.is_empty() {
                    o.name.clone()
                } else {
                    o.model.clone()
                })
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .build();
            let tile = gtk::Button::builder()
                .child(&label)
                .tooltip_text(if o.description.is_empty() {
                    o.label()
                } else {
                    o.description.clone()
                })
                .build();
            let p = Rc::downgrade(self);
            tile.connect_clicked(move |_| {
                if let Some(p) = p.upgrade() {
                    p.select(i);
                }
            });
            let drag = gtk::GestureDrag::new();
            let p = Rc::downgrade(self);
            drag.connect_drag_begin(move |_, _, _| {
                if let Some(p) = p.upgrade() {
                    p.select(i);
                    let e = &p.edits.borrow()[i];
                    p.drag_start.set((e.x, e.y));
                }
            });
            let p = Rc::downgrade(self);
            drag.connect_drag_update(move |_, dx, dy| {
                if let Some(p) = p.upgrade() {
                    let (k, ox, oy, minx, miny) = p.layout.get();
                    let (sx, sy) = p.drag_start.get();
                    let x = ox + f64::from(sx - minx) * k + dx;
                    let y = oy + f64::from(sy - miny) * k + dy;
                    if let Some(t) = p.tiles.borrow().get(i) {
                        p.fixed.move_(t, x, y);
                    }
                }
            });
            let p = Rc::downgrade(self);
            drag.connect_drag_end(move |_, dx, dy| {
                let Some(p) = p.upgrade() else { return };
                let (k, ..) = p.layout.get();
                if k <= 0.0 || (dx.abs() < 3.0 && dy.abs() < 3.0) {
                    p.place_tiles();
                    return;
                }
                let (sx, sy) = p.drag_start.get();
                let moved = (sx + (dx / k).round() as i32, sy + (dy / k).round() as i32);
                if let Some(r) = p.rect(i) {
                    let r = Rect {
                        x: moved.0,
                        y: moved.1,
                        ..r
                    };
                    let (x, y) = snap(r, &p.others(i));
                    if let Some(e) = p.edits.borrow_mut().get_mut(i) {
                        e.x = x;
                        e.y = y;
                    }
                    p.normalize();
                }
                p.place_tiles();
                p.sync_buttons();
            });
            tile.add_controller(drag);
            self.fixed.put(&tile, 0.0, 0.0);
            self.tiles.borrow_mut().push(tile);
        }
        self.place_tiles();
    }

    /// Fit every enabled display into the canvas, centered.
    fn place_tiles(&self) {
        let width = f64::from(self.area.width().max(200));
        let height = f64::from(CANVAS_HEIGHT);
        let n = self.edits.borrow().len();
        let rects: Vec<Option<Rect>> = (0..n).map(|i| self.rect(i)).collect();
        let present: Vec<Rect> = rects.iter().flatten().copied().collect();
        let tiles = self.tiles.borrow();
        if present.is_empty() {
            for t in tiles.iter() {
                t.set_visible(false);
            }
            return;
        }
        let minx = present.iter().map(|r| r.x).min().unwrap_or(0);
        let miny = present.iter().map(|r| r.y).min().unwrap_or(0);
        let maxx = present.iter().map(|r| r.x + r.w).max().unwrap_or(1);
        let maxy = present.iter().map(|r| r.y + r.h).max().unwrap_or(1);
        let bw = f64::from((maxx - minx).max(1));
        let bh = f64::from((maxy - miny).max(1));
        let k = ((width - 48.0) / bw).min((height - 48.0) / bh);
        let ox = (width - bw * k) / 2.0;
        let oy = (height - bh * k) / 2.0;
        self.layout.set((k, ox, oy, minx, miny));
        for (i, t) in tiles.iter().enumerate() {
            match rects.get(i).copied().flatten() {
                Some(r) => {
                    t.set_visible(true);
                    t.set_size_request(
                        ((f64::from(r.w) * k) as i32 - 4).max(24),
                        ((f64::from(r.h) * k) as i32 - 4).max(18),
                    );
                    self.fixed.move_(
                        t,
                        ox + f64::from(r.x - minx) * k + 2.0,
                        oy + f64::from(r.y - miny) * k + 2.0,
                    );
                    if i == self.selected.get() {
                        t.add_css_class("suggested-action");
                    } else {
                        t.remove_css_class("suggested-action");
                    }
                }
                None => t.set_visible(false),
            }
        }
    }

    fn select(self: &Rc<Self>, i: usize) {
        self.selected.set(i);
        self.place_tiles();
        self.sync_controls();
    }

    fn sync_buttons(&self) {
        let dirty = self.dirty();
        self.apply.set_sensitive(dirty && !self.busy.get());
        self.revert.set_sensitive(dirty && !self.busy.get());
    }

    fn sync_controls(&self) {
        self.syncing.set(true);
        let outputs = self.outputs.borrow();
        let edits = self.edits.borrow();
        let i = self.selected.get();
        let many = outputs.len() > 1;
        self.display_row.set_visible(many);
        self.enabled_row.set_visible(many);
        let labels: Vec<String> = outputs.iter().map(Output::label).collect();
        let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
        self.display_pick
            .set_model(Some(&gtk::StringList::new(&refs)));
        self.display_pick.set_selected(i as u32);

        if let (Some(o), Some(e)) = (outputs.get(i), edits.get(i)) {
            self.enabled.set_active(e.enabled);
            let mut sizes: Vec<(i32, i32)> = Vec::new();
            for m in &o.modes {
                if !sizes.contains(&(m.width, m.height)) {
                    sizes.push((m.width, m.height));
                }
            }
            sizes.sort_by_key(|(w, h)| std::cmp::Reverse(i64::from(*w) * i64::from(*h)));
            let preferred = o
                .modes
                .iter()
                .find(|m| m.preferred)
                .map(|m| (m.width, m.height));
            let labels: Vec<String> = sizes
                .iter()
                .map(|(w, h)| {
                    if Some((*w, *h)) == preferred {
                        format!("{w} × {h} ({})", tr("recommended"))
                    } else {
                        format!("{w} × {h}")
                    }
                })
                .collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            self.resolution
                .set_model(Some(&gtk::StringList::new(&refs)));
            let current = e
                .mode
                .and_then(|m| o.modes.get(m))
                .map(|m| (m.width, m.height));
            if let Some(pos) = current.and_then(|c| sizes.iter().position(|s| *s == c)) {
                self.resolution.set_selected(pos as u32);
            }
            self.resolutions.replace(sizes);

            let mut rates: Vec<usize> = o
                .modes
                .iter()
                .enumerate()
                .filter(|(_, m)| Some((m.width, m.height)) == current)
                .map(|(idx, _)| idx)
                .collect();
            rates.sort_by_key(|idx| std::cmp::Reverse(o.modes[*idx].refresh));
            let labels: Vec<String> = rates
                .iter()
                .map(|idx| format!("{:.2} Hz", f64::from(o.modes[*idx].refresh) / 1000.0))
                .collect();
            let refs: Vec<&str> = labels.iter().map(String::as_str).collect();
            self.refresh.set_model(Some(&gtk::StringList::new(&refs)));
            if let Some(pos) = e.mode.and_then(|m| rates.iter().position(|r| *r == m)) {
                self.refresh.set_selected(pos as u32);
            }
            self.refresh.set_sensitive(e.enabled && rates.len() > 1);
            self.refreshes.replace(rates);

            let scale_pos = SCALES
                .iter()
                .enumerate()
                .min_by(|a, b| (a.1 - e.scale).abs().total_cmp(&(b.1 - e.scale).abs()))
                .map(|(i, _)| i)
                .unwrap_or(0);
            self.scale.set_selected(scale_pos as u32);
            let rot = TRANSFORMS
                .iter()
                .position(|t| *t == e.transform)
                .unwrap_or(0);
            self.rotation.set_selected(rot as u32);
            for w in [&self.resolution, &self.scale, &self.rotation] {
                w.set_sensitive(e.enabled);
            }
        }
        drop(outputs);
        drop(edits);
        self.syncing.set(false);
        self.sync_buttons();
    }

    fn edit(self: &Rc<Self>, f: impl FnOnce(&Output, &mut Edit)) {
        if self.syncing.get() {
            return;
        }
        let i = self.selected.get();
        {
            let outputs = self.outputs.borrow();
            let mut edits = self.edits.borrow_mut();
            if let (Some(o), Some(e)) = (outputs.get(i), edits.get_mut(i)) {
                f(o, e);
            }
        }
        self.resolve(i);
        self.place_tiles();
        self.sync_controls();
    }

    fn configs(&self, edits: &[Edit]) -> Vec<HeadConfig> {
        self.outputs
            .borrow()
            .iter()
            .zip(edits)
            .map(|(o, e)| HeadConfig {
                head: o.head.clone(),
                enabled: e.enabled,
                mode: e.mode.and_then(|m| o.modes.get(m)).map(|m| m.proxy.clone()),
                x: e.x,
                y: e.y,
                transform: e.transform,
                scale: e.scale,
            })
            .collect()
    }

    fn send(
        self: &Rc<Self>,
        edits: &[Edit],
        test_only: bool,
        done: impl FnOnce(Outcome) + 'static,
    ) {
        let id = self
            .controller
            .send(self.serial.get(), &self.configs(edits), test_only);
        self.callbacks.borrow_mut().insert(id, Box::new(done));
    }

    fn apply_changes(self: &Rc<Self>) {
        if self.edits.borrow().iter().all(|e| !e.enabled) {
            widgets::set_status(&self.status, &tr("At least one display has to stay on."));
            return;
        }
        let before: Vec<Edit> = self.outputs.borrow().iter().map(Edit::of).collect();
        let after = self.edits.borrow().clone();
        self.busy.set(true);
        self.sync_buttons();
        widgets::set_status(&self.status, "");
        let this = self.clone();
        self.send(&after.clone(), true, move |outcome| {
            if outcome != Outcome::Succeeded {
                this.busy.set(false);
                this.sync_buttons();
                widgets::set_status(
                    &this.status,
                    &tr("These settings are not supported by the display."),
                );
                return;
            }
            let this2 = this.clone();
            this.send(&after.clone(), false, move |outcome| {
                if outcome != Outcome::Succeeded {
                    this2.busy.set(false);
                    this2.sync_buttons();
                    widgets::set_status(&this2.status, &tr("The displays could not be changed."));
                    return;
                }
                this2.confirm_keep(before, after);
            });
        });
    }

    fn confirm_keep(self: &Rc<Self>, before: Vec<Edit>, after: Vec<Edit>) {
        let window = gtk::Window::builder()
            .title(tr("Keep these display settings?"))
            .modal(true)
            .resizable(false)
            .default_width(360)
            .build();
        if let Some(parent) = widgets::parent_window(&self.root) {
            window.set_transient_for(Some(&parent));
        }
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        body.append(
            &gtk::Label::builder()
                .label(tr("Keep these display settings?"))
                .xalign(0.0)
                .css_classes(["heading"])
                .build(),
        );
        let countdown = widgets::caption("");
        body.append(&countdown);
        let buttons = gtk::Box::builder()
            .spacing(8)
            .halign(gtk::Align::End)
            .build();
        let revert = gtk::Button::with_label(&tr("Revert"));
        let keep = gtk::Button::builder()
            .label(tr("Keep"))
            .css_classes(["suggested-action"])
            .build();
        buttons.append(&revert);
        buttons.append(&keep);
        body.append(&buttons);
        window.set_child(Some(&body));
        window.set_default_widget(Some(&keep));

        let decided = Rc::new(Cell::new(false));
        let remaining = Rc::new(Cell::new(KEEP_SECONDS));
        let text = move |n: u32| {
            format!(
                "{} {n} {}",
                tr("The previous settings come back in"),
                tr("seconds.")
            )
        };
        countdown.set_label(&text(KEEP_SECONDS));

        let finish: Rc<dyn Fn(bool)> = {
            let this = self.clone();
            let window = window.downgrade();
            let decided = decided.clone();
            Rc::new(move |keep: bool| {
                if decided.replace(true) {
                    return;
                }
                if let Some(w) = window.upgrade() {
                    w.close();
                }
                if keep {
                    this.busy.set(false);
                    this.save_profile(&after);
                    this.sync_buttons();
                } else {
                    let this2 = this.clone();
                    let before = before.clone();
                    this.send(&before, false, move |_| {
                        this2.busy.set(false);
                        this2.sync_buttons();
                    });
                }
            })
        };
        let f = finish.clone();
        keep.connect_clicked(move |_| f(true));
        let f = finish.clone();
        revert.connect_clicked(move |_| f(false));
        let f = finish.clone();
        window.connect_close_request(move |_| {
            f(false);
            glib::Propagation::Proceed
        });
        let f = finish.clone();
        let label = countdown.downgrade();
        glib::timeout_add_seconds_local(1, move || {
            if decided.get() {
                return glib::ControlFlow::Break;
            }
            let n = remaining.get().saturating_sub(1);
            remaining.set(n);
            if n == 0 {
                f(false);
                return glib::ControlFlow::Break;
            }
            if let Some(l) = label.upgrade() {
                l.set_label(&text(n));
            }
            glib::ControlFlow::Continue
        });
        window.present();
    }

    fn save_profile(&self, edits: &[Edit]) {
        let outputs = self.outputs.borrow();
        let profile: Vec<ProfileOutput> = outputs
            .iter()
            .zip(edits)
            .map(|(o, e)| {
                let mode = e.mode.and_then(|m| o.modes.get(m));
                ProfileOutput {
                    criteria: kanshi::criteria(&o.name, &o.make, &o.model, &o.serial),
                    enabled: e.enabled,
                    width: mode.map(|m| m.width).unwrap_or(0),
                    height: mode.map(|m| m.height).unwrap_or(0),
                    refresh: mode.map(|m| m.refresh).unwrap_or(0),
                    x: e.x,
                    y: e.y,
                    scale: e.scale,
                    transform: outputs::transform_name(e.transform),
                }
            })
            .collect();
        if let Err(e) = kanshi::save(&profile) {
            widgets::set_status(
                &self.status,
                &format!(
                    "{} {e}",
                    tr("The layout is applied but could not be saved:")
                ),
            );
        }
    }
}

pub fn build() -> gtk::Widget {
    let (controller, rx) = match outputs::start() {
        Ok(started) => started,
        Err(_) => {
            return widgets::placeholder(
                "video-display-symbolic",
                &tr("Displays"),
                &tr("This compositor does not let apps change display settings."),
            )
        }
    };
    let root = widgets::page();

    let area = gtk::DrawingArea::builder()
        .content_height(CANVAS_HEIGHT)
        .hexpand(true)
        .build();
    let fixed = gtk::Fixed::new();
    let overlay = gtk::Overlay::builder()
        .child(&area)
        .css_classes(["display-canvas"])
        .build();
    overlay.add_overlay(&fixed);
    root.append(&overlay);
    root.append(&widgets::caption(&tr(
        "Drag displays to arrange them. Click one to change its settings.",
    )));

    let display_pick = gtk::DropDown::from_strings(&[]);
    let enabled = gtk::Switch::new();
    let resolution = gtk::DropDown::from_strings(&[]);
    let refresh = gtk::DropDown::from_strings(&[]);
    let scale_labels: Vec<String> = SCALES
        .iter()
        .map(|s| format!("{}%", (s * 100.0).round()))
        .collect();
    let scale_refs: Vec<&str> = scale_labels.iter().map(String::as_str).collect();
    let scale = gtk::DropDown::from_strings(&scale_refs);
    let rot_labels = [tr("Standard"), tr("90°"), tr("180°"), tr("270°")];
    let rot_refs: Vec<&str> = rot_labels.iter().map(String::as_str).collect();
    let rotation = gtk::DropDown::from_strings(&rot_refs);
    let display_row = widgets::row(&tr("Display"), &display_pick);
    let enabled_row = widgets::row(&tr("Use this display"), &enabled);
    root.append(&widgets::group(
        None,
        &[
            display_row.clone(),
            enabled_row.clone(),
            widgets::row(&tr("Resolution"), &resolution),
            widgets::row(&tr("Refresh rate"), &refresh),
            widgets::row_with_subtitle(
                &tr("Scale"),
                Some(&tr(
                    "Larger makes text and controls bigger on high-resolution displays.",
                )),
                &scale,
            ),
            widgets::row(&tr("Rotation"), &rotation),
        ],
    ));

    let apply = gtk::Button::builder()
        .label(tr("Apply"))
        .css_classes(["suggested-action"])
        .sensitive(false)
        .build();
    let revert = gtk::Button::builder()
        .label(tr("Revert"))
        .sensitive(false)
        .build();
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    buttons.append(&revert);
    buttons.append(&apply);
    root.append(&buttons);
    let status = widgets::status_label();
    root.append(&status);

    let pane = Rc::new(Pane {
        controller,
        root: root.clone(),
        outputs: RefCell::default(),
        edits: RefCell::default(),
        serial: Cell::new(0),
        selected: Cell::new(0),
        callbacks: RefCell::default(),
        busy: Cell::new(false),
        syncing: Cell::new(false),
        area: area.clone(),
        fixed,
        tiles: RefCell::default(),
        drag_start: Cell::new((0, 0)),
        layout: Cell::new((1.0, 0.0, 0.0, 0, 0)),
        display_row,
        display_pick,
        enabled_row,
        enabled,
        resolution,
        resolutions: RefCell::default(),
        refresh,
        refreshes: RefCell::default(),
        scale,
        rotation,
        apply,
        revert,
        status,
    });

    let p = Rc::downgrade(&pane);
    area.connect_resize(move |_, _, _| {
        if let Some(p) = p.upgrade() {
            p.place_tiles();
        }
    });
    let p = pane.clone();
    pane.display_pick.connect_selected_notify(move |dd| {
        if !p.syncing.get() {
            p.select(dd.selected() as usize);
        }
    });
    let p = pane.clone();
    pane.enabled.connect_state_set(move |_, on| {
        p.edit(|o, e| {
            e.enabled = on;
            if on && e.mode.is_none() {
                e.mode = o.modes.iter().position(|m| m.preferred).or(Some(0));
            }
        });
        glib::Propagation::Proceed
    });
    let p = pane.clone();
    pane.resolution.connect_selected_notify(move |dd| {
        let size = p.resolutions.borrow().get(dd.selected() as usize).copied();
        if let Some((w, h)) = size {
            p.edit(|o, e| {
                // The fastest refresh rate at that size, as displays expect.
                e.mode = o
                    .modes
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| m.width == w && m.height == h)
                    .max_by_key(|(_, m)| (m.preferred, m.refresh))
                    .map(|(i, _)| i);
            });
        }
    });
    let p = pane.clone();
    pane.refresh.connect_selected_notify(move |dd| {
        let mode = p.refreshes.borrow().get(dd.selected() as usize).copied();
        if let Some(mode) = mode {
            p.edit(|_, e| e.mode = Some(mode));
        }
    });
    let p = pane.clone();
    pane.scale.connect_selected_notify(move |dd| {
        if let Some(s) = SCALES.get(dd.selected() as usize) {
            p.edit(|_, e| e.scale = *s);
        }
    });
    let p = pane.clone();
    pane.rotation.connect_selected_notify(move |dd| {
        if let Some(t) = TRANSFORMS.get(dd.selected() as usize) {
            p.edit(|_, e| e.transform = *t);
        }
    });
    let p = pane.clone();
    pane.apply.connect_clicked(move |_| p.apply_changes());
    let p = pane.clone();
    pane.revert.connect_clicked(move |_| {
        let fresh: Vec<Edit> = p.outputs.borrow().iter().map(Edit::of).collect();
        p.edits.replace(fresh);
        p.place_tiles();
        p.sync_controls();
    });

    let weak = Rc::downgrade(&pane);
    glib::spawn_future_local(async move {
        while let Ok(event) = rx.recv().await {
            let Some(p) = weak.upgrade() else { break };
            match event {
                Event::Snapshot { outputs, serial } => p.on_snapshot(outputs, serial),
                Event::Result { id, outcome } => {
                    let cb = p.callbacks.borrow_mut().remove(&id);
                    if let Some(cb) = cb {
                        cb(outcome);
                    }
                }
            }
        }
    });

    widgets::keep_alive(&root, pane);
    root.upcast()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    #[test]
    fn snaps_to_the_right_edge() {
        let laptop = r(0, 0, 1504, 1003);
        let dropped = r(1600, 40, 1920, 1080);
        assert_eq!(snap(dropped, &[laptop]), (1504, 0));
    }

    #[test]
    fn snaps_above() {
        let laptop = r(0, 0, 1504, 1003);
        let dropped = r(100, -1200, 1920, 1080);
        assert_eq!(snap(dropped, &[laptop]), (100, -1080));
    }

    #[test]
    fn never_overlaps() {
        let a = r(0, 0, 1000, 1000);
        let b = r(1000, 0, 1000, 1000);
        let dropped = r(900, 100, 800, 600);
        let (x, y) = snap(dropped, &[a, b]);
        let placed = r(x, y, 800, 600);
        assert!(!placed.overlaps(&a) && !placed.overlaps(&b));
    }
}
