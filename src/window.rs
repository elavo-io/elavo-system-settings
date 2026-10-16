//! The window: a sidebar with a search field and the person's card on the
//! left, the open pane on the right, and a header with Back, Forward, and
//! the pane's name.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, glib};

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
    let avatar = gtk::Label::builder()
        .label(initial)
        .css_classes(["user-avatar", "small"])
        .valign(gtk::Align::Center)
        .build();
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
    content.append(&avatar);
    content.append(&labels);
    let button = gtk::Button::builder()
        .child(&content)
        .css_classes(["flat", "user-card"])
        .build();
    button.set_action_name(Some("app.show-pane"));
    button.set_action_target_value(Some(&"users".to_variant()));
    button
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

        let weak = Rc::downgrade(self);
        self.window.connect_close_request(move |window| {
            if let Some(w) = weak.upgrade() {
                state::save(
                    *w.current.borrow(),
                    window.default_width(),
                    window.default_height(),
                );
            }
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
        if let Some(i) = self.sidebar_index(pane.id) {
            let row = &self.rows[i];
            if self.sidebar.selected_row().as_ref() != Some(row) {
                self.sidebar.select_row(Some(row));
            }
        }
        true
    }

    fn update_buttons(&self) {
        self.back.set_sensitive(!self.history.borrow().is_empty());
        self.forward.set_sensitive(!self.future.borrow().is_empty());
    }

    fn go_back(&self) {
        let Some(previous) = self.history.borrow_mut().pop() else {
            return;
        };
        let current = *self.current.borrow();
        if self.navigate(previous) && !current.is_empty() {
            self.future.borrow_mut().push(current);
        }
        self.update_buttons();
    }

    fn go_forward(&self) {
        let Some(next) = self.future.borrow_mut().pop() else {
            return;
        };
        let current = *self.current.borrow();
        if self.navigate(next) && !current.is_empty() {
            self.history.borrow_mut().push(current);
        }
        self.update_buttons();
    }

    pub fn show_initial(&self) {
        let id = state::last_pane()
            .filter(|id| self.has_pane(id))
            .unwrap_or_else(|| panes::DEFAULT.to_owned());
        self.show(&id);
    }

    pub fn focus_search(&self) {
        self.search.grab_focus();
    }
}
