//! The driver setup wizard. One window, hidden between uses. GTK 4.22.5
//! crashes if a window with an icon is destroyed.
//!
//! Four steps: Introduction, Review, Set Up, Summary. A simulation
//! (`ELAVO_SETTINGS_SIMULATE_DRIVERS`) never asks pkexec, writes a file,
//! or restarts the computer.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{glib, pango};

use crate::backend::{dbus, drivers, helper, power};
use crate::i18n::tr;
use crate::widgets;

const INTRO: u8 = 0;
const REVIEW: u8 = 1;
const SETUP: u8 = 2;
const SUMMARY: u8 = 3;
const NONE: usize = usize::MAX;

struct Wizard {
    window: gtk::Window,
    steps: gtk::ListBox,
    body: gtk::Box,
    step: Cell<u8>,
    gaps: RefCell<Vec<drivers::Gap>>,
    selected: Cell<usize>,
    outcome: RefCell<Option<drivers::SetupOutcome>>,
    busy: Cell<bool>,
    showing_license: Cell<bool>,
    sim_note: RefCell<Option<String>>,
}

thread_local! {
    static WIZARD: RefCell<Option<Rc<Wizard>>> = const { RefCell::new(None) };
}

pub fn button() -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Set Up Drivers"));
    button.connect_clicked(|anchor| open(anchor, None));
    button
}

/// Open the wizard. `target` selects that gap when it is one of the gaps
/// found now, which is how the Wi-Fi notice starts on the right hardware.
pub fn open(anchor: &impl IsA<gtk::Widget>, target: Option<drivers::Gap>) {
    let wizard = WIZARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            return existing.clone();
        }
        let window = widgets::popup_window(anchor, &tr("Set Up Drivers"));
        window.set_default_width(720);
        window.set_resizable(true);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        let shell = gtk::Box::builder().spacing(18).build();
        let steps = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .width_request(176)
            .vexpand(true)
            .css_classes(["driver-steps", "rich-list"])
            .build();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .hexpand(true)
            .build();
        shell.append(&steps);
        shell.append(&body);
        content.append(&shell);
        window.set_child(Some(&content));
        let wizard = Rc::new(Wizard {
            window,
            steps,
            body,
            step: Cell::new(INTRO),
            gaps: RefCell::default(),
            selected: Cell::new(NONE),
            outcome: RefCell::default(),
            busy: Cell::new(false),
            showing_license: Cell::new(false),
            sim_note: RefCell::default(),
        });
        let guard = wizard.clone();
        wizard.window.connect_close_request(move |_| {
            if guard.busy.get() {
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        let jump = wizard.clone();
        wizard.steps.connect_row_activated(move |_, row| {
            if jump.busy.get() {
                return;
            }
            let index = row.index();
            if index < 0 || index as u8 > jump.step.get() {
                return;
            }
            jump.showing_license.set(false);
            jump.step.set(index as u8);
            show(&jump);
        });
        slot.replace(wizard.clone());
        wizard
    });
    if wizard.busy.get() {
        wizard.window.present();
        return;
    }
    let found = drivers::scan();
    let selected = target
        .as_ref()
        .and_then(|gap| found.iter().position(|found| found == gap))
        .or_else(|| (found.len() == 1).then_some(0))
        .unwrap_or(NONE);
    wizard.gaps.replace(found);
    wizard.selected.set(selected);
    wizard.outcome.replace(None);
    wizard.sim_note.replace(None);
    wizard.showing_license.set(false);
    wizard.step.set(INTRO);
    show(&wizard);
    wizard.window.present();
}

fn show(wizard: &Rc<Wizard>) {
    paint_steps(wizard);
    clear(&wizard.body);
    if drivers::mode() == drivers::Mode::Rejected {
        paragraph(
            &wizard.body,
            &tr("This simulation name isn't recognized, so nothing will change."),
        );
        wizard
            .body
            .append(&button_row(&[], &[close_button(wizard)]));
        return;
    }
    if wizard.showing_license.get() {
        show_license(wizard);
        return;
    }
    match wizard.step.get() {
        REVIEW => show_review(wizard),
        SETUP => show_setup(wizard),
        SUMMARY => show_summary(wizard),
        _ => show_intro(wizard),
    }
}

fn paint_steps(wizard: &Rc<Wizard>) {
    while let Some(child) = wizard.steps.first_child() {
        wizard.steps.remove(&child);
    }
    let current = wizard.step.get();
    for index in 0..4 {
        let labels = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        let name = gtk::Label::builder()
            .label(step_name(index))
            .xalign(0.0)
            .css_classes(if index == current {
                vec!["driver-step-current"]
            } else {
                vec![]
            })
            .build();
        labels.append(&name);
        if index < current {
            labels.append(
                &gtk::Label::builder()
                    .label(tr("Done"))
                    .xalign(0.0)
                    .css_classes(["dim-label", "caption"])
                    .build(),
            );
        }
        let row = gtk::ListBoxRow::builder()
            .activatable(!wizard.busy.get() && index <= current)
            .selectable(false)
            .child(&labels)
            .build();
        if index == current {
            row.add_css_class("current");
        }
        if index > current {
            row.add_css_class("dim-label");
        }
        wizard.steps.append(&row);
    }
}

fn step_name(step: u8) -> String {
    match step {
        REVIEW => tr("Review"),
        SETUP => tr("Set Up"),
        SUMMARY => tr("Summary"),
        _ => tr("Introduction"),
    }
}

fn show_intro(wizard: &Rc<Wizard>) {
    wizard.body.append(&widgets::heading(&tr("Introduction")));
    let found = wizard.gaps.borrow().clone();
    if found.is_empty() {
        paragraph(
            &wizard.body,
            &tr("Everything on this computer that needed a driver is set up."),
        );
        wizard
            .body
            .append(&button_row(&[], &[close_button(wizard)]));
        return;
    }
    if found.len() > 1 {
        paragraph(&wizard.body, &tr("Choose what you'd like to set up."));
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .css_classes(["rich-list"])
            .build();
        for (index, gap) in found.iter().enumerate() {
            let row = gap_row(gap);
            if index == wizard.selected.get() {
                row.add_css_class("current");
            }
            list.append(&row);
        }
        let wizard_for_row = wizard.clone();
        list.connect_row_activated(move |_, row| {
            let index = row.index();
            if index < 0 {
                return;
            }
            wizard_for_row.selected.set(index as usize);
            show(&wizard_for_row);
        });
        wizard.body.append(&list);
    }
    if let Some(gap) = selected(wizard) {
        for line in intro_lines(&gap) {
            paragraph(&wizard.body, &line);
        }
        details(&wizard.body, &gap);
    }
    let continue_button = continue_button(wizard, REVIEW);
    continue_button.set_sensitive(selected(wizard).is_some());
    wizard.body.append(&button_row(&[], &[continue_button]));
}

fn show_review(wizard: &Rc<Wizard>) {
    wizard.body.append(&widgets::heading(&tr("Review")));
    let Some(gap) = selected(wizard) else {
        wizard.step.set(INTRO);
        show(wizard);
        return;
    };
    for line in review_lines(&gap) {
        paragraph(&wizard.body, &line);
    }
    if gap.shows_as_is_disclosure() {
        paragraph(
            &wizard.body,
            &tr("Broadcom provides this driver as is, without warranty."),
        );
        let license = gtk::Button::with_label(&tr("View License"));
        let wizard_for_license = wizard.clone();
        license.connect_clicked(move |_| {
            wizard_for_license.showing_license.set(true);
            show(&wizard_for_license);
        });
        license.set_halign(gtk::Align::Start);
        wizard.body.append(&license);
    }
    details(&wizard.body, &gap);
    wizard.body.append(&button_row(
        &[back_button(wizard, INTRO)],
        &[continue_button(wizard, SETUP)],
    ));
}

fn show_license(wizard: &Rc<Wizard>) {
    wizard.body.append(&widgets::heading(&tr("View License")));
    match drivers::license_text() {
        Some(text) => {
            let label = gtk::Label::builder()
                .label(&text)
                .wrap(true)
                .wrap_mode(pango::WrapMode::WordChar)
                .xalign(0.0)
                .selectable(true)
                .build();
            let scrolled = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .min_content_height(220)
                .child(&label)
                .build();
            wizard.body.append(&scrolled);
        }
        None => paragraph(
            &wizard.body,
            &tr("The license file couldn't be found on this computer."),
        ),
    }
    let back = gtk::Button::with_label(&tr("Back"));
    let wizard_for_back = wizard.clone();
    back.connect_clicked(move |_| {
        wizard_for_back.showing_license.set(false);
        show(&wizard_for_back);
    });
    wizard.body.append(&button_row(&[back], &[]));
}

fn show_setup(wizard: &Rc<Wizard>) {
    wizard.body.append(&widgets::heading(&tr("Set Up")));
    if wizard.busy.get() {
        let spinner = gtk::Spinner::builder()
            .spinning(true)
            .halign(gtk::Align::Start)
            .build();
        wizard.body.append(&spinner);
        if let Some(gap) = selected(wizard) {
            paragraph(&wizard.body, &activity(&gap));
        }
        return;
    }
    if wizard.outcome.borrow().is_some() {
        paragraph(&wizard.body, &tr("Setup is complete."));
        wizard.body.append(&button_row(
            &[back_button(wizard, REVIEW)],
            &[continue_button(wizard, SUMMARY)],
        ));
        return;
    }
    let Some(gap) = selected(wizard) else {
        wizard.step.set(INTRO);
        show(wizard);
        return;
    };
    if !gap.is_actionable_wifi() && !matches!(gap, drivers::Gap::Nvidia) {
        paragraph(
            &wizard.body,
            &tr("There's nothing to set up for this hardware."),
        );
        let next = wizard.clone();
        let continue_button = gtk::Button::with_label(&tr("Continue"));
        continue_button.add_css_class("suggested-action");
        continue_button.connect_clicked(move |_| finish(&next, drivers::SetupOutcome::Explained));
        wizard.body.append(&button_row(
            &[back_button(wizard, REVIEW)],
            &[continue_button],
        ));
        return;
    }
    if matches!(gap, drivers::Gap::Nvidia) && !drivers::nvidia_image_published() {
        paragraph(
            &wizard.body,
            &tr("The NVIDIA edition of Elavo isn't available yet."),
        );
        let next = wizard.clone();
        let continue_button = gtk::Button::with_label(&tr("Continue"));
        continue_button.add_css_class("suggested-action");
        continue_button.connect_clicked(move |_| finish(&next, drivers::SetupOutcome::Explained));
        wizard.body.append(&button_row(
            &[back_button(wizard, REVIEW)],
            &[continue_button],
        ));
        return;
    }
    paragraph(
        &wizard.body,
        &tr("When you click Set Up, you'll be asked for an administrator password."),
    );
    let start = gtk::Button::with_label(&tr("Set Up"));
    start.add_css_class("suggested-action");
    let next = wizard.clone();
    start.connect_clicked(move |_| begin(&next));
    wizard
        .body
        .append(&button_row(&[back_button(wizard, REVIEW)], &[start]));
}

fn begin(wizard: &Rc<Wizard>) {
    if wizard.busy.get() {
        return;
    }
    let Some(gap) = selected(wizard) else {
        return;
    };
    if let Some(outcome) = drivers::simulated_setup(&gap) {
        wizard.busy.set(true);
        show(wizard);
        let wizard = wizard.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(700), move || {
            wizard.busy.set(false);
            finish(&wizard, outcome);
        });
        return;
    }
    match &gap {
        drivers::Gap::Wifi(chip) => begin_wifi(wizard, chip.clone()),
        drivers::Gap::Nvidia => begin_nvidia(wizard),
        _ => finish(wizard, drivers::SetupOutcome::Explained),
    }
}

fn begin_wifi(wizard: &Rc<Wizard>, chip: drivers::WifiChip) {
    let command = match chip.kind {
        drivers::WifiKind::Proprietary => "broadcom",
        drivers::WifiKind::BuiltIn => "brcmfmac",
        drivers::WifiKind::Firmware => {
            finish(wizard, drivers::SetupOutcome::Explained);
            return;
        }
    };
    wizard.busy.set(true);
    show(wizard);
    let wizard = wizard.clone();
    glib::spawn_future_local(async move {
        let status = Rc::new(RefCell::new(String::new()));
        let watch = status.clone();
        let result = helper::run(&["wifi-driver", command], move |line| {
            if matches!(line, "loaded" | "reboot" | "missing" | "refused") {
                watch.replace(line.to_owned());
            }
        })
        .await;
        wizard.busy.set(false);
        let status = status.borrow().clone();
        let error = result.err();
        let outcome = drivers::outcome_from_status(
            &status,
            drivers::chip_is_wireless(&chip.id),
            error.as_deref(),
        );
        finish(&wizard, outcome);
    });
}

fn begin_nvidia(wizard: &Rc<Wizard>) {
    wizard.busy.set(true);
    show(wizard);
    let wizard = wizard.clone();
    glib::spawn_future_local(async move {
        let outcome = match helper::run(&["switch-nvidia"], |_| {}).await {
            Ok(()) => drivers::SetupOutcome::Restart,
            Err(error) => drivers::SetupOutcome::Failed(error),
        };
        wizard.busy.set(false);
        finish(&wizard, outcome);
    });
}

fn finish(wizard: &Rc<Wizard>, outcome: drivers::SetupOutcome) {
    wizard.busy.set(false);
    if matches!(outcome, drivers::SetupOutcome::Ready) && !drivers::mode().allows_system_changes() {
        drivers::mark_simulated_wifi_ready();
    }
    wizard.outcome.replace(Some(outcome));
    wizard.step.set(SUMMARY);
    show(wizard);
    drivers::notify_driver_change();
}

fn show_summary(wizard: &Rc<Wizard>) {
    wizard.body.append(&widgets::heading(&tr("Summary")));
    let outcome = wizard.outcome.borrow().clone();
    let Some(outcome) = outcome else {
        wizard.step.set(SETUP);
        show(wizard);
        return;
    };
    match &outcome {
        drivers::SetupOutcome::Ready => paragraph(
            &wizard.body,
            &tr("Wi-Fi is ready. Choose a network to connect."),
        ),
        drivers::SetupOutcome::Restart => paragraph(
            &wizard.body,
            &tr("Restart your computer to finish setting up Wi-Fi."),
        ),
        drivers::SetupOutcome::Missing => paragraph(
            &wizard.body,
            &tr("This version of Elavo doesn't include the Wi-Fi driver. Nothing was changed."),
        ),
        drivers::SetupOutcome::Blocked => {
            paragraph(
                &wizard.body,
                &tr("This computer's security settings blocked the driver. Nothing was changed."),
            );
            paragraph(
                &wizard.body,
                &tr("This can happen when Secure Boot is turned on."),
            );
        }
        drivers::SetupOutcome::Failed(detail) => {
            paragraph(
                &wizard.body,
                &tr("Wi-Fi couldn't be set up. Some changes may already have been made, so try again before restarting."),
            );
            if !detail.is_empty() {
                text_details(&wizard.body, detail);
            }
        }
        drivers::SetupOutcome::Explained => {
            if let Some(gap) = selected(wizard) {
                for line in intro_lines(&gap) {
                    paragraph(&wizard.body, &line);
                }
            }
        }
    }
    if let Some(note) = wizard.sim_note.borrow().clone() {
        paragraph(&wizard.body, &note);
    }
    let mut end = Vec::new();
    match outcome {
        drivers::SetupOutcome::Ready => {
            end.push(choose_network(wizard));
            end.push(close_button(wizard));
        }
        drivers::SetupOutcome::Restart => {
            end.push(later_button(wizard));
            end.push(restart_button(wizard));
        }
        drivers::SetupOutcome::Failed(_) => {
            end.push(retry_button(wizard));
            end.push(close_button(wizard));
        }
        _ => end.push(close_button(wizard)),
    }
    wizard.body.append(&button_row(&[], &end));
}

fn intro_lines(gap: &drivers::Gap) -> Vec<String> {
    match gap {
        drivers::Gap::Wifi(chip) => match chip.kind {
            drivers::WifiKind::Proprietary => vec![
                tr("Wi-Fi isn't working because this computer's wireless card needs a driver."),
                tr("Elavo already includes the driver, so you don't need an internet connection to set it up."),
            ],
            drivers::WifiKind::BuiltIn => vec![
                tr("Wi-Fi isn't working because this computer's wireless card needs a driver."),
                tr("Elavo can turn on a driver that's already part of the system, so you don't need an internet connection."),
            ],
            drivers::WifiKind::Firmware => vec![firmware_explanation()],
        },
        drivers::Gap::Nvidia => vec![
            tr("This computer has NVIDIA graphics but is using a basic graphics driver, which can be slower and may not support every display."),
            tr("To use NVIDIA's driver, Elavo switches this computer to its NVIDIA edition. Your accounts, apps, and files stay as they are, and you can switch back in Software Update."),
            tr("After you restart, you may be asked to enroll a security key. Follow the steps on that screen to finish."),
        ],
        drivers::Gap::T2 => vec![tr(
            "This computer has an Apple T2 chip. Its keyboard, trackpad, and speakers may not work, because Elavo doesn't include a driver for them yet.",
        )],
        drivers::Gap::Facetime => vec![tr(
            "The built-in camera needs files from macOS that Elavo can't include, so it may not work.",
        )],
    }
}

/// Shared with the Wi-Fi notice. The BCM4364 only ships in Macs, so Apple
/// is named here.
pub fn firmware_explanation() -> String {
    tr("This computer's wireless card needs files from Apple that Elavo can't include, so Wi-Fi can't be set up here.")
}

fn review_lines(gap: &drivers::Gap) -> Vec<String> {
    match gap {
        drivers::Gap::Wifi(chip) => match chip.kind {
            drivers::WifiKind::Proprietary => vec![
                tr("This driver is made by Broadcom. Once it's on, it starts every time the computer starts."),
                tr("With this driver, sleep may be less reliable and some 5 GHz networks may not appear."),
                tr("You'll probably need to restart before Wi-Fi appears."),
            ],
            drivers::WifiKind::BuiltIn => vec![
                tr("This uses a Wi-Fi driver that's already part of Elavo."),
                tr("You may need to restart before Wi-Fi appears."),
            ],
            drivers::WifiKind::Firmware => vec![tr("Nothing will be changed on this computer.")],
        },
        drivers::Gap::Nvidia if !drivers::nvidia_image_published() => {
            vec![tr("The NVIDIA edition of Elavo isn't available yet.")]
        }
        drivers::Gap::Nvidia => vec![tr(
            "Elavo will download its NVIDIA edition and switch to it when you restart.",
        )],
        _ => vec![tr("Nothing will be changed on this computer.")],
    }
}

fn details(parent: &gtk::Box, gap: &drivers::Gap) {
    text_details(parent, &detail_text(gap));
}

fn text_details(parent: &gtk::Box, text: &str) {
    let label = gtk::Label::builder()
        .label(text)
        .wrap(true)
        .wrap_mode(pango::WrapMode::Word)
        .xalign(0.0)
        .selectable(true)
        .css_classes(["caption"])
        .build();
    let expander = gtk::Expander::builder()
        .label(tr("Details"))
        .child(&label)
        .build();
    parent.append(&expander);
}

fn detail_text(gap: &drivers::Gap) -> String {
    match gap {
        drivers::Gap::Wifi(chip) => match chip.kind {
            drivers::WifiKind::Proprietary => format!(
                "{}\n{}",
                words(&tr("Driver: wl. PCI id 14e4:{}."), &chip.id),
                tr("The module is not loaded until it is set up.")
            ),
            drivers::WifiKind::BuiltIn => {
                let mut text = words(&tr("Driver: brcmfmac. PCI id 14e4:{}."), &chip.id);
                if chip.needs_option() {
                    text.push('\n');
                    text.push_str(&tr("Module option: feature_disable=0x82000."));
                }
                text
            }
            drivers::WifiKind::Firmware => words(
                &tr("Driver: brcmfmac. PCI id 14e4:{}. Firmware is not included."),
                &chip.id,
            ),
        },
        drivers::Gap::Nvidia => {
            tr("PCI vendor 10de. The nvidia module is not loaded. Image name: elavo-nvidia.")
        }
        drivers::Gap::T2 => tr("Apple T2 bridge, PCI ids 106b:1801 and 106b:1802."),
        drivers::Gap::Facetime => tr("FaceTime HD camera, PCI id 14e4:1570."),
    }
}

fn activity(gap: &drivers::Gap) -> String {
    let live = drivers::mode().allows_system_changes();
    match gap {
        drivers::Gap::Wifi(_) if live => tr("Setting up Wi-Fi…"),
        drivers::Gap::Wifi(_) => tr("Simulation: setting up Wi-Fi…"),
        drivers::Gap::Nvidia if live => tr("Downloading the NVIDIA edition…"),
        drivers::Gap::Nvidia => tr("Simulation: nothing is being downloaded…"),
        _ => tr("Working…"),
    }
}

fn selected(wizard: &Wizard) -> Option<drivers::Gap> {
    wizard.gaps.borrow().get(wizard.selected.get()).cloned()
}

fn clear(content: &gtk::Box) {
    while let Some(child) = content.first_child() {
        content.remove(&child);
    }
}

fn words(template: &str, name: &str) -> String {
    template.replacen("{}", name, 1)
}

fn paragraph(content: &gtk::Box, label: &str) {
    content.append(
        &gtk::Label::builder()
            .label(label)
            .wrap(true)
            .wrap_mode(pango::WrapMode::Word)
            .xalign(0.0)
            .build(),
    );
}

fn button_row(start: &[gtk::Button], end: &[gtk::Button]) -> gtk::Box {
    let row = gtk::Box::builder().spacing(8).margin_top(6).build();
    let left = gtk::Box::builder()
        .spacing(8)
        .hexpand(true)
        .halign(gtk::Align::Start)
        .build();
    let right = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .build();
    for button in start {
        left.append(button);
    }
    for button in end {
        right.append(button);
    }
    row.append(&left);
    row.append(&right);
    row
}

fn back_button(wizard: &Rc<Wizard>, step: u8) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Back"));
    let wizard = wizard.clone();
    button.connect_clicked(move |_| {
        if wizard.busy.get() {
            return;
        }
        wizard.showing_license.set(false);
        wizard.step.set(step);
        show(&wizard);
    });
    button
}

fn continue_button(wizard: &Rc<Wizard>, step: u8) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Continue"));
    button.add_css_class("suggested-action");
    let wizard = wizard.clone();
    button.connect_clicked(move |_| {
        if wizard.busy.get() {
            return;
        }
        wizard.step.set(step);
        show(&wizard);
    });
    button
}

fn close_button(wizard: &Rc<Wizard>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Close"));
    let window = wizard.window.clone();
    button.connect_clicked(move |_| window.set_visible(false));
    button
}

fn later_button(wizard: &Rc<Wizard>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Later"));
    let window = wizard.window.clone();
    button.connect_clicked(move |_| window.set_visible(false));
    button
}

fn restart_button(wizard: &Rc<Wizard>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Restart…"));
    button.add_css_class("suggested-action");
    let wizard = wizard.clone();
    button.connect_clicked(move |_| {
        if !drivers::mode().allows_system_changes() {
            wizard
                .sim_note
                .replace(Some(tr("Simulation: your computer wasn't restarted.")));
            show(&wizard);
            return;
        }
        let wizard = wizard.clone();
        glib::spawn_future_local(async move {
            let Some(conn) = dbus::system() else {
                return;
            };
            if let Err(error) = power::reboot(&conn).await {
                widgets::show_error(
                    &wizard.window,
                    &tr("Could not restart."),
                    &widgets::error_text(&error),
                );
            }
        });
    });
    button
}

fn choose_network(wizard: &Rc<Wizard>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Choose a Network"));
    button.add_css_class("suggested-action");
    let wizard = wizard.clone();
    button.connect_clicked(move |_| {
        if !drivers::mode().allows_system_changes() {
            wizard
                .sim_note
                .replace(Some(tr("Simulation: no network settings were changed.")));
            show(&wizard);
            return;
        }
        if let Some(app) = wizard.window.application() {
            app.activate_action("show-pane", Some(&"wifi".to_variant()));
        }
        wizard.window.set_visible(false);
    });
    button
}

fn retry_button(wizard: &Rc<Wizard>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Try Again"));
    let wizard = wizard.clone();
    button.connect_clicked(move |_| {
        if wizard.busy.get() {
            return;
        }
        wizard.outcome.replace(None);
        wizard.sim_note.replace(None);
        wizard.step.set(SETUP);
        show(&wizard);
    });
    button
}

fn gap_row(gap: &drivers::Gap) -> gtk::ListBoxRow {
    let (title, subtitle) = match gap {
        drivers::Gap::Wifi(chip) => (tr("Wi-Fi"), format!("Broadcom {}", chip.name)),
        drivers::Gap::Nvidia => (tr("Graphics"), "NVIDIA".to_owned()),
        drivers::Gap::T2 => (tr("Keyboard, trackpad, and speakers"), tr("Apple T2")),
        drivers::Gap::Facetime => (tr("Camera"), tr("FaceTime HD")),
    };
    let labels = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    labels.append(&gtk::Label::builder().label(&title).xalign(0.0).build());
    labels.append(
        &gtk::Label::builder()
            .label(&subtitle)
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    gtk::ListBoxRow::builder()
        .activatable(true)
        .selectable(false)
        .child(&labels)
        .build()
}
