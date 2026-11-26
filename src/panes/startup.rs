//! Startup Disk: choose the default rEFInd boot option and whether the
//! boot menu shows at power-on or stays hidden unless a key is held.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;

use crate::backend::drivers;
use crate::backend::helper;
use crate::backend::power;
use crate::backend::startup::{
    self, Availability, Config, DefaultTarget, MenuVisibility, RefindState, StartupOption,
};
use crate::backend::dbus;
use crate::i18n::tr;
use crate::widgets;

struct State {
    options: Vec<StartupOption>,
    tiles: Vec<gtk::ToggleButton>,
    flow: gtk::FlowBox,
    footer: gtk::Label,
    spinner: gtk::Spinner,
    restart: gtk::Button,
    menu_dropdown: gtk::DropDown,
    saved_default: RefCell<DefaultTarget>,
    saved_menu: RefCell<MenuVisibility>,
    has_other_oses: bool,
    syncing: Cell<bool>,
    busy: Cell<bool>,
}

impl State {
    fn selected_option(&self) -> &StartupOption {
        let target = self.saved_default.borrow();
        self.options
            .iter()
            .find(|o| o.target == *target)
            .unwrap_or(&self.options[0])
    }

    fn footer_text(opt: &StartupOption) -> String {
        format!(
            "{} {} {} \"{}\".",
            tr("You have selected"),
            opt.summary_name,
            tr("on the disk"),
            opt.disk_name,
        )
    }

    fn sync_ui(&self) {
        self.syncing.set(true);
        let active_idx = {
            let target = self.saved_default.borrow();
            self.options
                .iter()
                .position(|o| o.target == *target)
                .unwrap_or(0)
        };
        if let Some(btn) = self.tiles.get(active_idx) {
            btn.set_active(true);
        }
        self.footer
            .set_label(&Self::footer_text(&self.options[active_idx]));
        let always = self
            .saved_menu
            .borrow()
            .is_always_shown(self.has_other_oses);
        self.menu_dropdown.set_selected(u32::from(always));
        self.syncing.set(false);
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        self.spinner.set_visible(busy);
        self.spinner.set_spinning(busy);
        self.flow.set_sensitive(!busy);
        self.menu_dropdown.set_sensitive(!busy);
        self.restart.set_sensitive(!busy);
    }

    fn apply(self: &Rc<Self>, next_default: DefaultTarget, next_menu: MenuVisibility) {
        if self.busy.get() {
            return;
        }
        self.set_busy(true);
        let this = Rc::clone(self);
        glib::spawn_future_local(async move {
            let def_arg = next_default.as_arg();
            let result = helper::run(&["startup-disk", &def_arg, next_menu.as_arg()], |_| {}).await;
            this.set_busy(false);
            match result {
                Ok(()) => {
                    *this.saved_default.borrow_mut() = next_default;
                    *this.saved_menu.borrow_mut() = next_menu;
                    this.sync_ui();
                }
                Err(e) => {
                    this.sync_ui();
                    if !e.contains(&tr("The administrator password was not accepted.")) {
                        widgets::show_error(
                            &this.flow,
                            &tr("Could not change startup disk settings."),
                            &e,
                        );
                    }
                }
            }
        });
    }
}

fn tile_widget(opt: &StartupOption) -> gtk::ToggleButton {
    let inner = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    inner.append(
        &gtk::Image::builder()
            .icon_name("elavo-startup-disk-symbolic")
            .pixel_size(48)
            .halign(gtk::Align::Center)
            .build(),
    );
    inner.append(
        &gtk::Label::builder()
            .label(&opt.name)
            .halign(gtk::Align::Center)
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build(),
    );
    if !opt.detail.is_empty() {
        inner.append(
            &gtk::Label::builder()
                .label(&opt.detail)
                .css_classes(["caption"])
                .halign(gtk::Align::Center)
                .wrap(true)
                .justify(gtk::Justification::Center)
                .build(),
        );
    }
    let btn = gtk::ToggleButton::builder()
        .css_classes(["startup-tile"])
        .child(&inner)
        .build();
    btn.update_property(&[gtk::accessible::Property::Label(&opt.summary_name)]);
    btn
}

fn unavailable(avail: Availability) -> gtk::Widget {
    let (title, body) = match avail {
        Availability::NotUefi => (
            tr("Startup Disk requires UEFI"),
            tr("This computer started in legacy BIOS mode, so the rEFInd boot menu is not used."),
        ),
        Availability::SecureBootEnabled => (
            tr("Startup Disk is unavailable while Secure Boot is on"),
            tr("rEFInd is not signed for Secure Boot, so this computer uses GRUB instead. Turn off Secure Boot in firmware settings to manage startup options here."),
        ),
        Availability::RefindNotInstalled | Availability::Available => (
            tr("Startup Disk is unavailable"),
            tr("The rEFInd boot manager is not installed on this computer."),
        ),
    };
    widgets::placeholder("elavo-startup-disk-symbolic", &title, &body)
}

pub fn build() -> gtk::Widget {
    let refind_state = RefindState::read();
    let avail = startup::detect_availability(
        drivers::secure_boot_enabled(),
        refind_state.as_ref(),
    );
    if avail != Availability::Available {
        return unavailable(avail);
    }

    let bls = startup::read_bls_entries();
    let labels = startup::read_disk_labels(refind_state.as_ref());
    let prev_suffix = tr("Previous");
    let rec_label = tr("Recovery (GRUB)");
    let rec_detail = tr("GRUB fallback");
    let other_detail = tr("EFI Boot");
    let options = startup::build_options(
        &bls,
        refind_state.as_ref(),
        &labels,
        &prev_suffix,
        &rec_label,
        &rec_detail,
        &other_detail,
    );

    let has_other_oses = refind_state
        .as_ref()
        .is_some_and(|s| !s.others.is_empty());
    let cfg = Config::read();
    let initial_default = if options.iter().any(|o| o.target == cfg.default) {
        cfg.default.clone()
    } else {
        DefaultTarget::Current
    };

    let flow = gtk::FlowBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .homogeneous(true)
        .min_children_per_line(1)
        .max_children_per_line(4)
        .row_spacing(10)
        .column_spacing(10)
        .margin_top(6)
        .margin_bottom(6)
        .build();

    let mut tiles = Vec::with_capacity(options.len());
    let mut first: Option<gtk::ToggleButton> = None;
    for opt in &options {
        let btn = tile_widget(opt);
        if let Some(first) = &first {
            btn.set_group(Some(first));
        } else {
            first = Some(btn.clone());
        }
        flow.insert(&btn, -1);
        tiles.push(btn);
    }

    let footer = gtk::Label::builder()
        .xalign(0.0)
        .hexpand(true)
        .wrap(true)
        .valign(gtk::Align::Center)
        .build();
    let spinner = gtk::Spinner::builder()
        .visible(false)
        .valign(gtk::Align::Center)
        .build();
    let restart = gtk::Button::builder()
        .label(tr("Restart…"))
        .valign(gtk::Align::Center)
        .build();
    let footer_box = gtk::Box::builder()
        .spacing(12)
        .margin_top(2)
        .margin_bottom(2)
        .build();
    footer_box.append(&footer);
    footer_box.append(&spinner);
    footer_box.append(&restart);

    let skip_label = tr("Skip unless a key is held");
    let always_label = tr("Always show");
    let menu_dropdown = gtk::DropDown::from_strings(&[&skip_label, &always_label]);

    let state = Rc::new(State {
        options,
        tiles,
        flow: flow.clone(),
        footer,
        spinner,
        restart: restart.clone(),
        menu_dropdown: menu_dropdown.clone(),
        saved_default: RefCell::new(initial_default),
        saved_menu: RefCell::new(cfg.menu),
        has_other_oses,
        syncing: Cell::new(false),
        busy: Cell::new(false),
    });
    state.sync_ui();

    for (idx, btn) in state.tiles.iter().enumerate() {
        let st = Rc::clone(&state);
        btn.connect_toggled(move |b| {
            if !b.is_active() || st.syncing.get() || st.busy.get() {
                return;
            }
            let target = st.options[idx].target.clone();
            if target == *st.saved_default.borrow() {
                return;
            }
            let menu = *st.saved_menu.borrow();
            st.apply(target, menu);
        });
    }

    {
        let st = Rc::clone(&state);
        menu_dropdown.connect_selected_notify(move |dd| {
            if st.syncing.get() || st.busy.get() {
                return;
            }
            let next_menu = if dd.selected() == 1 {
                MenuVisibility::Always
            } else {
                MenuVisibility::Hidden
            };
            if next_menu.is_always_shown(st.has_other_oses)
                == st.saved_menu.borrow().is_always_shown(st.has_other_oses)
                && *st.saved_menu.borrow() == next_menu
            {
                return;
            }
            let def = st.saved_default.borrow().clone();
            st.apply(def, next_menu);
        });
    }

    {
        let st = Rc::clone(&state);
        restart.connect_clicked(move |btn| {
            let st = Rc::clone(&st);
            let btn = btn.clone();
            glib::spawn_future_local(async move {
                let opt = st.selected_option().clone();
                let detail = format!(
                    "{} {} {} \"{}\".",
                    tr("Your computer will start up using"),
                    opt.summary_name,
                    tr("on the disk"),
                    opt.disk_name,
                );
                if widgets::confirm(
                    &btn,
                    &tr("Are you sure you want to restart your computer?"),
                    &detail,
                    &tr("Restart"),
                )
                .await
                {
                    let Some(conn) = dbus::system() else { return };
                    if let Err(e) = power::reboot(&conn).await {
                        widgets::show_error(
                            &btn,
                            &tr("Could not restart."),
                            &widgets::error_text(&e),
                        );
                    }
                }
            });
        });
    }

    let page = widgets::page();
    page.append(&widgets::group(
        Some(&tr(
            "Select the system you want to use to start up your computer",
        )),
        &[widgets::plain_row(&flow), widgets::plain_row(&footer_box)],
    ));
    page.append(&widgets::group(
        Some(&tr("Boot Menu")),
        &[widgets::row_with_subtitle(
            &tr("Startup boot menu"),
            Some(&tr(
                "Always show the rEFInd menu, or skip it with a black screen unless a key is held.",
            )),
            &menu_dropdown,
        )],
    ));
    page.append(&widgets::caption(&tr(
        "Hold any key while turning on your computer to show the boot menu and start from another version or Recovery (GRUB).",
    )));
    page.upcast()
}
