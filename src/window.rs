//! The window: a sidebar with a search field and the person's card on the
//! left, the open pane on the right, and a header with Back, Forward, and
//! the pane's name.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::avatar;
use crate::backend::accounts;
use crate::backend::dbus;
use crate::backend::keyfile::state;
use crate::i18n::tr;
use crate::panes::{self, Pane};
use crate::widgets;

pub struct Window {
    pub window: gtk::ApplicationWindow,
    panes: Vec<Pane>,
    stack: gtk::Stack,
    sidebar: gtk::ListBox,
    /// The sidebar's panes, in row order.
    sidebar_ids: Vec<&'static str>,
    rows: Vec<gtk::ListBoxRow>,
    title: gtk::Label,
    back: gtk::Button,
    forward: gtk::Button,
    search: gtk::SearchEntry,
    current: RefCell<&'static str>,
    history: RefCell<Vec<&'static str>>,
    future: RefCell<Vec<&'static str>>,
    query: RefCell<String>,
}

fn sidebar_row(pane: &Pane) -> gtk::ListBoxRow {
    let content = gtk::Box::builder().spacing(8).build();
    content.append(&widgets::tile(pane.icon, pane.color, "small"));
    content.append(
        &gtk::Label::builder()
            .label((pane.title)())
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build(),
    );
    gtk::ListBoxRow::builder()
        .child(&content)
        .name(pane.id)
        .build()
}

/// The person using the computer, at the top of the sidebar. It opens
/// Users & Groups.
fn user_card() -> gtk::Button {
    let real = glib::real_name().to_string_lossy().into_owned();
    let login = glib::user_name().to_string_lossy().into_owned();
    let name = if real.is_empty() || real == "Unknown" {
        login.clone()
    } else {
        real
    };
    let initial: String = name
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_default();
    let icon = accounts::local_icon();
    let slot = gtk::Box::builder().build();
    slot.append(&avatar::view(&initial, &icon, avatar::SIDEBAR));
    watch_sidebar_avatar(&slot, &initial, &icon);
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .build();
    labels.append(
        &gtk::Label::builder()
            .label(&name)
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["user-card-name"])
            .build(),
    );
    labels.append(
        &gtk::Label::builder()
            .label(tr("Users & Groups"))
            .xalign(0.0)
            .css_classes(["caption"])
            .build(),
    );
    let content = gtk::Box::builder().spacing(10).build();
    content.append(&slot);
    content.append(&labels);
    let button = gtk::Button::builder()
        .child(&content)
        .css_classes(["flat", "user-card"])
        .build();
    button.set_action_name(Some("app.show-pane"));
    button.set_action_target_value(Some(&"users".to_variant()));
    button
}

/// Replace the sidebar picture when AccountsService reports a new one.
/// The same path is kept when the file is overwritten, so a change reloads
/// even though the path did not move.
fn watch_sidebar_avatar(slot: &gtk::Box, letter: &str, icon: &str) {
    let Some(conn) = accounts::bus() else {
        return;
    };
    let slot = slot.clone();
    let letter = letter.to_owned();
    let shown = Rc::new(RefCell::new(icon.to_owned()));
    let generation = Rc::new(Cell::new(0u32));
    let debounce = dbus::Debounce::new(100);
    let paint: Rc<dyn Fn(bool)> = {
        let conn = conn.clone();
        let slot = slot.clone();
        Rc::new(move |force| {
            let ticket = generation.get().wrapping_add(1);
            generation.set(ticket);
            let generation = Rc::clone(&generation);
            let shown = Rc::clone(&shown);
            let slot = slot.clone();
            let letter = letter.clone();
            let conn = conn.clone();
            glib::spawn_future_local(async move {
                let Some(user) = accounts::current(&conn).await else {
                    return;
                };
                if generation.get() != ticket {
                    return;
                }
                if !force && *shown.borrow() == user.icon {
                    return;
                }
                shown.replace(user.icon.clone());
                while let Some(child) = slot.first_child() {
                    slot.remove(&child);
                }
                slot.append(&avatar::view(&letter, &user.icon, avatar::SIDEBAR));
            });
        })
    };
    paint(false);
    let sub = dbus::on_signal(
        &conn,
        "org.freedesktop.Accounts",
        None,
        None,
        move |_, name, _| {
            if name == "Changed" {
                let paint = Rc::clone(&paint);
                debounce.run(move || paint(true));
            }
        },
    );
    widgets::keep_alive(&slot, sub);
}

fn search_text(pane: &Pane) -> String {
    format!("{} {} {}", (pane.title)(), pane.id, pane.keywords.join(" ")).to_lowercase()
}

fn matches(text: &str, query: &str) -> bool {
    query.split_whitespace().all(|word| text.contains(word))
}

impl Window {
    pub fn new(app: &gtk::Application) -> Rc<Self> {
        let panes = panes::all();
        let (width, height) = state::size().unwrap_or((900, 640));
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(tr("System Settings"))
            .icon_name(crate::APP_ID)
            .default_width(width)
            .default_height(height)
            .show_menubar(false)
            .build();
        add_edit_actions(&window);

        let back = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text(tr("Back"))
            .sensitive(false)
            .build();
        let forward = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .tooltip_text(tr("Forward"))
            .sensitive(false)
            .build();
        let nav = gtk::Box::builder().css_classes(["linked"]).build();
        nav.append(&back);
        nav.append(&forward);
        let title = gtk::Label::builder().css_classes(["pane-title"]).build();
        let header = gtk::HeaderBar::builder()
            .title_widget(&gtk::Label::new(None))
            .build();
        let start = gtk::Box::builder()
            .spacing(10)
            .valign(gtk::Align::Center)
            .build();
        start.append(&nav);
        start.append(&title);
        header.pack_start(&start);
        window.set_titlebar(Some(&header));

        let search = gtk::SearchEntry::builder()
            .placeholder_text(tr("Search"))
            .build();
        let sidebar = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["navigation-sidebar"])
            .build();
        let top: Vec<&Pane> = panes.iter().filter(|p| p.parent.is_none()).collect();
        let sidebar_ids: Vec<&'static str> = top.iter().map(|p| p.id).collect();
        let rows: Vec<gtk::ListBoxRow> = top.iter().map(|p| sidebar_row(p)).collect();
        for row in &rows {
            sidebar.append(row);
        }
        let side_content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        side_content.append(&user_card());
        side_content.append(&sidebar);
        let side = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .width_request(220)
            .css_classes(["settings-sidebar"])
            .build();
        side.append(&search);
        side.append(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vexpand(true)
                .child(&side_content)
                .build(),
        );

        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .transition_duration(120)
            .hexpand(true)
            .vexpand(true)
            .build();
        let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        body.append(&side);
        body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
        body.append(&stack);
        window.set_child(Some(&body));

        let this = Rc::new(Self {
            window,
            panes,
            stack,
            sidebar,
            sidebar_ids,
            rows,
            title,
            back,
            forward,
            search,
            current: RefCell::new(""),
            history: RefCell::default(),
            future: RefCell::default(),
            query: RefCell::default(),
        });
        this.connect();
        this
    }

    fn pane(&self, id: &str) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    /// The sidebar entry that stands for `id`: itself, or its hub.
    fn sidebar_index(&self, id: &str) -> Option<usize> {
        let id = self.pane(id).and_then(|p| p.parent).unwrap_or(id);
        self.sidebar_ids.iter().position(|s| *s == id)
    }

    /// What a search matches for a sidebar entry: its own words and those
    /// of the pages it opens.
    fn sidebar_search_text(&self, id: &str) -> String {
        self.panes
            .iter()
            .filter(|p| p.id == id || p.parent == Some(id))
            .map(search_text)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The pane Return opens while searching: the first sidebar entry that
    /// matches, or the first of its pages that does.
    fn best_match(&self, query: &str) -> Option<&'static str> {
        for id in &self.sidebar_ids {
            let pane = self.pane(id)?;
            if matches(&search_text(pane), query) {
                return Some(pane.id);
            }
            if let Some(child) = self
                .panes
                .iter()
                .find(|p| p.parent == Some(id) && matches(&search_text(p), query))
            {
                return Some(child.id);
            }
        }
        None
    }

    fn connect(self: &Rc<Self>) {
        let groups: Vec<u8> = self
            .sidebar_ids
            .iter()
            .filter_map(|id| self.pane(id).map(|p| p.group))
            .collect();
        let query_holder = Rc::downgrade(self);
        self.sidebar.set_header_func(move |row, before| {
            let searching = query_holder
                .upgrade()
                .is_some_and(|w| !w.query.borrow().is_empty());
            let group = |r: &gtk::ListBoxRow| groups.get(r.index() as usize).copied();
            let gap = match before {
                Some(before) => !searching && group(row) != group(before),
                None => false,
            };
            if gap {
                row.set_header(Some(
                    &gtk::Box::builder().css_classes(["group-gap"]).build(),
                ));
            } else {
                row.set_header(None::<&gtk::Widget>);
            }
        });

        let texts: Vec<String> = self
            .sidebar_ids
            .iter()
            .map(|id| self.sidebar_search_text(id))
            .collect();
        let weak = Rc::downgrade(self);
        self.sidebar.set_filter_func(move |row| {
            let Some(w) = weak.upgrade() else {
                return true;
            };
            let query = w.query.borrow();
            query.is_empty()
                || texts
                    .get(row.index() as usize)
                    .is_some_and(|t| matches(t, &query))
        });

        let weak = Rc::downgrade(self);
        self.search.connect_search_changed(move |entry| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            w.query.replace(entry.text().trim().to_lowercase());
            w.sidebar.invalidate_filter();
            w.sidebar.invalidate_headers();
        });
        let weak = Rc::downgrade(self);
        self.search.connect_activate(move |_| {
            let Some(w) = weak.upgrade() else {
                return;
            };
            let query = w.query.borrow().clone();
            if let Some(id) = w.best_match(&query) {
                w.show(id);
            }
        });
        let weak = Rc::downgrade(self);
        self.search.connect_stop_search(move |entry| {
            entry.set_text("");
            if let Some(w) = weak.upgrade() {
                w.focus_current_row();
            }
        });

        let weak = Rc::downgrade(self);
        self.sidebar.connect_row_selected(move |_, row| {
            let (Some(w), Some(row)) = (weak.upgrade(), row) else {
                return;
            };
            let Some(id) = w.sidebar_ids.get(row.index() as usize).copied() else {
                return;
            };
            // Selecting the hub while one of its pages is open is the
            // highlight following along, not a request to leave the page.
            if w.sidebar_index(&w.current.borrow()) != Some(row.index() as usize) {
                w.show(id);
            }
        });

        // Clicking the highlighted hub while one of its pages is open goes
        // back up to the hub; selection does not change, so this is the
        // only signal for it.
        let weak = Rc::downgrade(self);
        self.sidebar.connect_row_activated(move |_, row| {
            if let Some(w) = weak.upgrade() {
                if let Some(id) = w.sidebar_ids.get(row.index() as usize).copied() {
                    w.show(id);
                }
            }
        });

        let weak = Rc::downgrade(self);
        self.back.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.go_back();
            }
        });
        let weak = Rc::downgrade(self);
        self.forward.connect_clicked(move |_| {
            if let Some(w) = weak.upgrade() {
                w.go_forward();
            }
        });

        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let Some(w) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            let ctrl = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            let alt = modifiers.contains(gdk::ModifierType::ALT_MASK);
            if (alt && key == gdk::Key::Left) || (ctrl && key == gdk::Key::bracketleft) {
                w.go_back();
                return glib::Propagation::Stop;
            }
            if (alt && key == gdk::Key::Right) || (ctrl && key == gdk::Key::bracketright) {
                w.go_forward();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        self.window.add_controller(keys);

        self.window.connect_close_request(move |window| {
            state::save(window.default_width(), window.default_height());
            glib::Propagation::Proceed
        });
    }

    fn focus_current_row(&self) {
        if let Some(i) = self.sidebar_index(&self.current.borrow()) {
            self.rows[i].grab_focus();
        }
    }

    pub fn has_pane(&self, id: &str) -> bool {
        self.pane(id).is_some()
    }

    /// Show a pane by id, building it the first time.
    pub fn show(&self, id: &str) {
        let previous = *self.current.borrow();
        if self.navigate(id) && !previous.is_empty() {
            let mut history = self.history.borrow_mut();
            history.push(previous);
            if history.len() > 50 {
                history.remove(0);
            }
            self.future.borrow_mut().clear();
        }
        self.update_buttons();
    }

    /// Switch the stack to `id` without touching the history. Returns
    /// whether the visible pane changed.
    fn navigate(&self, id: &str) -> bool {
        let Some(pane) = self.pane(id) else {
            return false;
        };
        if *self.current.borrow() == pane.id {
            return false;
        }
        if self.stack.child_by_name(pane.id).is_none() {
            let content = (pane.build)();
            let scrolled = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&content)
                .build();
            self.stack.add_named(&scrolled, Some(pane.id));
        }
        self.stack.set_visible_child_name(pane.id);
        self.current.replace(pane.id);
        self.title.set_label(&(pane.title)());
        if let Some(action) = self
            .window
            .application()
            .and_then(|app| app.lookup_action("show-pane"))
            .and_downcast::<gio::SimpleAction>()
        {
            let shown = pane.parent.unwrap_or(pane.id);
            action.set_state(&shown.to_variant());
        }
        if let Some(i) = self.sidebar_index(pane.id) {
            let row = &self.rows[i];
            if self.sidebar.selected_row().as_ref() != Some(row) {
                self.sidebar.select_row(Some(row));
            }
        }
        true
    }

    fn update_buttons(&self) {
        let back = !self.history.borrow().is_empty();
        let forward = !self.future.borrow().is_empty();
        self.back.set_sensitive(back);
        self.forward.set_sensitive(forward);
        let Some(app) = self.window.application() else {
            return;
        };
        set_enabled(&app, "back", back);
        set_enabled(&app, "forward", forward);
    }

    pub fn go_back(&self) {
        let Some(previous) = self.history.borrow_mut().pop() else {
            return;
        };
        let current = *self.current.borrow();
        if self.navigate(previous) && !current.is_empty() {
            self.future.borrow_mut().push(current);
        }
        self.update_buttons();
    }

    pub fn go_forward(&self) {
        let Some(next) = self.future.borrow_mut().pop() else {
            return;
        };
        let current = *self.current.borrow();
        if self.navigate(next) && !current.is_empty() {
            self.history.borrow_mut().push(current);
        }
        self.update_buttons();
    }

    pub fn focus_search(&self) {
        self.search.grab_focus();
    }
}

fn set_enabled(group: &impl IsA<gio::ActionMap>, name: &str, enabled: bool) {
    if let Some(action) = group
        .lookup_action(name)
        .and_downcast::<gio::SimpleAction>()
    {
        action.set_enabled(enabled);
    }
}

/// Cut, Copy, and the rest act on the focused text. Each one stays off
/// unless that field can actually do it: a selection for Cut, Copy, and
/// Delete, text on the clipboard for Paste, characters left to select for
/// Select All, and a real undo or redo step.
fn add_edit_actions(window: &gtk::ApplicationWindow) {
    const ACTIONS: &[(&str, &str)] = &[
        ("undo", "text.undo"),
        ("redo", "text.redo"),
        ("cut", "clipboard.cut"),
        ("copy", "clipboard.copy"),
        ("paste", "clipboard.paste"),
        ("delete", "selection.delete"),
        ("select-all", "selection.select-all"),
    ];
    for (name, widget_action) in ACTIONS {
        let action = gio::SimpleAction::new(name, None);
        action.set_enabled(false);
        let weak = window.downgrade();
        let widget_action = *widget_action;
        action.connect_activate(move |_, _| {
            let Some(window) = weak.upgrade() else {
                return;
            };
            if let Some(widget) = text_focus(&window) {
                let _ = widget.activate_action(widget_action, None);
            }
        });
        window.add_action(&action);
    }

    let weak = window.downgrade();
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(window) = weak.upgrade() {
            refresh_edit(&window);
        }
    });
    if let Some(display) = gdk::Display::default() {
        let clipboard = display.clipboard();
        note_clipboard(&clipboard, &refresh);
        let refresh = refresh.clone();
        clipboard.connect_changed(move |clipboard| note_clipboard(clipboard, &refresh));
    }
    refresh();
    let refresh = refresh.clone();
    window.connect_notify_local(Some("focus-widget"), move |_, _| refresh());
}

fn note_clipboard(clipboard: &gdk::Clipboard, refresh: &Rc<dyn Fn()>) {
    let offers_text = clipboard.formats().contains_type(glib::Type::STRING);
    CAN_PASTE.with(|flag| flag.set(offers_text));
    refresh();
    if !offers_text {
        return;
    }
    let refresh = refresh.clone();
    clipboard.read_text_async(gio::Cancellable::NONE, move |result| {
        let has = result.ok().flatten().is_some_and(|text| !text.is_empty());
        CAN_PASTE.with(|flag| flag.set(has));
        refresh();
    });
}

fn refresh_edit(window: &gtk::ApplicationWindow) {
    let Some(widget) = text_focus(window) else {
        for name in [
            "undo",
            "redo",
            "cut",
            "copy",
            "paste",
            "delete",
            "select-all",
        ] {
            set_enabled(window, name, false);
        }
        return;
    };
    if let Some(text) = widget.downcast_ref::<gtk::Text>() {
        watch_text(window, text);
        let (undo, redo) = undo_levels(text);
        let editable = text.is_editable();
        let visible = text.is_visible();
        let history = editable && visible && text.enables_undo();
        let selected = text.selection_bounds().is_some();
        let chars = text.text().chars().count();
        let all = text
            .selection_bounds()
            .is_some_and(|(start, end)| start.min(end) == 0 && start.max(end) as usize == chars);
        let paste = CAN_PASTE.with(|flag| flag.get());
        set_enabled(window, "undo", history && undo > 0);
        set_enabled(window, "redo", history && redo > 0);
        set_enabled(window, "cut", visible && editable && selected);
        set_enabled(window, "copy", visible && selected);
        set_enabled(window, "paste", editable && paste);
        set_enabled(window, "delete", editable && selected);
        set_enabled(window, "select-all", chars > 0 && !all);
        return;
    }
    if let Some(view) = widget.downcast_ref::<gtk::TextView>() {
        let buffer = view.buffer();
        let editable = view.is_editable();
        let selected = buffer.has_selection();
        let chars = buffer.char_count() as usize;
        let paste = CAN_PASTE.with(|flag| flag.get());
        set_enabled(window, "undo", editable && buffer.can_undo());
        set_enabled(window, "redo", editable && buffer.can_redo());
        set_enabled(window, "cut", editable && selected);
        set_enabled(window, "copy", selected);
        set_enabled(window, "paste", editable && paste);
        set_enabled(window, "delete", editable && selected);
        set_enabled(window, "select-all", chars > 0);
    }
}

fn watch_text(window: &gtk::ApplicationWindow, text: &gtk::Text) {
    let key = text.as_ptr() as usize;
    let fresh = UNDO.with(|map| {
        let mut map = map.borrow_mut();
        let std::collections::hash_map::Entry::Vacant(slot) = map.entry(key) else {
            return false;
        };
        slot.insert(FieldUndo {
            undo: Cell::new(0),
            redo: Cell::new(0),
            last: Cell::new(LastEdit::None),
            pending: Cell::new(0),
            applied: Cell::new(false),
            same_turn: Cell::new(false),
            prev: RefCell::new(text.text().to_string()),
        });
        true
    });
    if !fresh {
        return;
    }

    let weak_window = window.downgrade();
    let weak_text = text.downgrade();
    let refresh: Rc<dyn Fn()> = Rc::new(move || {
        if let Some(window) = weak_window.upgrade() {
            refresh_edit(&window);
        }
    });
    text.connect_notify_local(Some("text"), {
        let weak_text = weak_text.clone();
        let refresh = refresh.clone();
        move |_, _| {
            if let Some(text) = weak_text.upgrade() {
                note_text_edit(&text);
                refresh();
            }
        }
    });
    for property in [
        "cursor-position",
        "selection-bound",
        "editable",
        "visibility",
        "enable-undo",
    ] {
        let refresh = refresh.clone();
        text.connect_notify_local(Some(property), move |_, _| refresh());
    }

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak_text = text.downgrade();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        let kind = history_key(key, modifiers);
        if kind == 0 {
            return glib::Propagation::Proceed;
        }
        let Some(text) = weak_text.upgrade() else {
            return glib::Propagation::Proceed;
        };
        let key_id = text.as_ptr() as usize;
        UNDO.with(|map| {
            let map = map.borrow();
            let Some(state) = map.get(&key_id) else {
                return;
            };
            state.pending.set(kind);
            state.applied.set(false);
        });
        let weak_text = text.downgrade();
        glib::idle_add_local_once(move || {
            let Some(text) = weak_text.upgrade() else {
                return;
            };
            UNDO.with(|map| {
                let map = map.borrow();
                let Some(state) = map.get(&(text.as_ptr() as usize)) else {
                    return;
                };
                state.pending.set(0);
                state.applied.set(false);
                state.same_turn.set(false);
            });
        });
        glib::Propagation::Proceed
    });
    text.add_controller(keys);

    let key = text.as_ptr() as usize;
    text.connect_destroy(move |_| {
        UNDO.with(|map| {
            map.borrow_mut().remove(&key);
        });
    });
}

fn note_text_edit(text: &gtk::Text) {
    let key = text.as_ptr() as usize;
    let old = UNDO.with(|map| {
        map.borrow()
            .get(&key)
            .map(|state| state.prev.borrow().clone())
    });
    let Some(old) = old else {
        return;
    };
    let new = text.text().to_string();
    if old == new {
        return;
    }
    let cursor = text.position();
    UNDO.with(|map| {
        let map = map.borrow();
        let Some(state) = map.get(&key) else {
            return;
        };
        let mut step = UndoStep {
            undo: state.undo.get(),
            redo: state.redo.get(),
            last: state.last.get(),
        };
        let pending = state.pending.get();
        if pending != 0 {
            if !state.applied.get() {
                apply_history(&mut step, pending);
                state.applied.set(true);
            }
        } else {
            apply_user_change(&mut step, state.same_turn.get(), &old, &new, cursor);
            if !state.same_turn.get() {
                state.same_turn.set(true);
                let weak = text.downgrade();
                glib::idle_add_local_once(move || {
                    let Some(text) = weak.upgrade() else {
                        return;
                    };
                    UNDO.with(|map| {
                        if let Some(state) = map.borrow().get(&(text.as_ptr() as usize)) {
                            state.same_turn.set(false);
                        }
                    });
                });
            }
        }
        state.undo.set(step.undo);
        state.redo.set(step.redo);
        state.last.set(step.last);
        *state.prev.borrow_mut() = new;
    });
}

fn undo_levels(text: &gtk::Text) -> (u32, u32) {
    let key = text.as_ptr() as usize;
    UNDO.with(|map| {
        map.borrow()
            .get(&key)
            .map(|state| (state.undo.get(), state.redo.get()))
            .unwrap_or((0, 0))
    })
}

fn text_focus(window: &gtk::ApplicationWindow) -> Option<gtk::Widget> {
    let mut widget = gtk::prelude::GtkWindowExt::focus(window)?;
    loop {
        if widget.is::<gtk::Text>() || widget.is::<gtk::TextView>() || widget.is::<gtk::Editable>()
        {
            return Some(widget);
        }
        widget = widget.parent()?;
    }
}

thread_local! {
    static CAN_PASTE: Cell<bool> = const { Cell::new(false) };
    static UNDO: RefCell<HashMap<usize, FieldUndo>> = RefCell::new(HashMap::new());
}

struct FieldUndo {
    undo: Cell<u32>,
    redo: Cell<u32>,
    last: Cell<LastEdit>,
    /// 1 while Ctrl+Z is being handled, 2 for redo, otherwise 0.
    pending: Cell<u8>,
    applied: Cell<bool>,
    /// Delete and insert from one keypress are one undo step.
    same_turn: Cell<bool>,
    prev: RefCell<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LastEdit {
    None,
    Insert { end: i32 },
    Backspace { begin: i32 },
}

struct UndoStep {
    undo: u32,
    redo: u32,
    last: LastEdit,
}

/// Ctrl+Z is undo. Ctrl+Y and Ctrl+Shift+Z are redo.
fn history_key(key: gdk::Key, modifiers: gdk::ModifierType) -> u8 {
    if !modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
        return 0;
    }
    let shift = modifiers.contains(gdk::ModifierType::SHIFT_MASK);
    if key == gdk::Key::z && !shift {
        1
    } else if key == gdk::Key::y || (key == gdk::Key::z && shift) {
        2
    } else {
        0
    }
}

fn apply_history(step: &mut UndoStep, kind: u8) {
    step.last = LastEdit::None;
    match kind {
        1 if step.undo > 0 => {
            step.undo -= 1;
            step.redo += 1;
        }
        2 if step.redo > 0 => {
            step.redo -= 1;
            step.undo += 1;
        }
        _ => {}
    }
}

/// One typed word is one undo step, and so is one run of backspaces.
/// A space starts a new step. `cursor` is the caret after the change, in
/// characters, or the caret from before the change when GTK has not moved
/// it yet.
fn apply_user_change(step: &mut UndoStep, same_turn: bool, old: &str, new: &str, cursor: i32) {
    let old_len = old.chars().count() as i32;
    let new_len = new.chars().count() as i32;
    let delta = new_len - old_len;
    let begin = if delta > 0 && cursor + delta == new_len {
        cursor
    } else {
        cursor - delta
    };
    if begin < 0 {
        if !same_turn {
            step.undo = step.undo.saturating_add(1);
            step.redo = 0;
        }
        step.last = LastEdit::None;
        return;
    }
    let chains = match (step.last, delta) {
        (LastEdit::Insert { end }, d) if d > 0 && end == begin => {
            let inserted: String = new.chars().skip(begin as usize).take(d as usize).collect();
            chains_with_previous(&inserted)
        }
        (LastEdit::Backspace { begin: at }, d) if d == -1 && cursor + 1 == at => true,
        _ => false,
    };
    if !same_turn && !chains {
        step.undo = step.undo.saturating_add(1);
        step.redo = 0;
    }
    step.last = if delta > 0 {
        LastEdit::Insert { end: begin + delta }
    } else if delta == -1 {
        LastEdit::Backspace { begin: cursor }
    } else {
        LastEdit::None
    };
}

fn chains_with_previous(inserted: &str) -> bool {
    let mut chars = inserted.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first == '\n' || first.is_whitespace() {
        return false;
    }
    let rest_is_one_word =
        inserted.chars().count() == 1 || !inserted.chars().any(char::is_whitespace);
    rest_is_one_word
}

#[cfg(test)]
mod edit_tests {
    use super::*;

    fn step() -> UndoStep {
        UndoStep {
            undo: 0,
            redo: 0,
            last: LastEdit::None,
        }
    }

    #[test]
    fn typing_a_word_is_one_undo_and_a_space_starts_another() {
        let mut edit = step();
        apply_user_change(&mut edit, false, "", "h", 1);
        apply_user_change(&mut edit, false, "h", "he", 2);
        apply_user_change(&mut edit, false, "he", "hel", 3);
        assert_eq!(edit.undo, 1);
        apply_user_change(&mut edit, false, "hel", "hel ", 4);
        assert_eq!(edit.undo, 2);
        apply_user_change(&mut edit, false, "hel ", "hel w", 5);
        assert_eq!(edit.undo, 2);
    }

    #[test]
    fn undo_and_redo_move_one_step() {
        let mut edit = UndoStep {
            undo: 1,
            redo: 0,
            last: LastEdit::Insert { end: 1 },
        };
        apply_history(&mut edit, 1);
        assert_eq!((edit.undo, edit.redo), (0, 1));
        apply_history(&mut edit, 2);
        assert_eq!((edit.undo, edit.redo), (1, 0));
        apply_history(&mut edit, 1);
        apply_history(&mut edit, 1);
        assert_eq!((edit.undo, edit.redo), (0, 1));
    }

    #[test]
    fn one_key_that_replaces_a_selection_is_one_step() {
        let mut edit = step();
        apply_user_change(&mut edit, false, "hello", "heo", 2);
        apply_user_change(&mut edit, true, "heo", "hexo", 3);
        assert_eq!(edit.undo, 1);
    }

    #[test]
    fn a_paste_containing_spaces_does_not_join_the_word() {
        let mut edit = UndoStep {
            undo: 1,
            redo: 0,
            last: LastEdit::Insert { end: 5 },
        };
        apply_user_change(&mut edit, false, "hello", "hello wide paste", 16);
        assert_eq!(edit.undo, 2);
    }
}
