//! The Drivers wizard opened from About. One window, hidden between uses.
//! GTK 4.22.5 crashes if a window with an icon is destroyed.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{glib, pango};

use crate::backend::{dbus, drivers, helper, power};
use crate::i18n::tr;
use crate::widgets;

struct Wizard {
    window: gtk::Window,
    content: gtk::Box,
}

thread_local! {
    static WIZARD: RefCell<Option<Rc<Wizard>>> = const { RefCell::new(None) };
}

pub fn button() -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Drivers…"));
    button.connect_clicked(present);
    button
}

fn present(anchor: &impl IsA<gtk::Widget>) {
    let wizard = WIZARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Some(existing) = slot.as_ref() {
            return existing.clone();
        }
        let window = widgets::popup_window(anchor, &tr("Drivers"));
        window.set_default_width(440);
        window.set_resizable(false);
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(18)
            .margin_bottom(18)
            .margin_start(18)
            .margin_end(18)
            .build();
        window.set_child(Some(&content));
        let wizard = Rc::new(Wizard { window, content });
        slot.replace(wizard.clone());
        wizard
    });
    show_list(&wizard);
    wizard.window.present();
}

fn clear(content: &gtk::Box) {
    while let Some(child) = content.first_child() {
        content.remove(&child);
    }
}

fn words(template: &str, name: &str) -> String {
    template.replacen("{}", name, 1)
}

fn text(content: &gtk::Box, label: &str) {
    content.append(
        &gtk::Label::builder()
            .label(label)
            .wrap(true)
            .wrap_mode(pango::WrapMode::Word)
            .xalign(0.0)
            .build(),
    );
}

fn buttons(row: &[gtk::Button]) -> gtk::Box {
    let box_ = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(6)
        .build();
    for button in row {
        box_.append(button);
    }
    box_
}

fn back_button(wizard: &Rc<Wizard>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Back"));
    let wizard = wizard.clone();
    button.connect_clicked(move |_| show_list(&wizard));
    button
}

fn close_button(wizard: &Rc<Wizard>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Close"));
    let window = wizard.window.clone();
    button.connect_clicked(move |_| window.set_visible(false));
    button
}

fn restart_button(wizard: &Rc<Wizard>) -> gtk::Button {
    let button = gtk::Button::with_label(&tr("Restart…"));
    let wizard = wizard.clone();
    button.connect_clicked(move |_| {
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

fn show_list(wizard: &Rc<Wizard>) {
    clear(&wizard.content);
    let found = drivers::scan();
    if found.is_empty() {
        text(
            &wizard.content,
            &tr("The devices that needed a driver are ready."),
        );
        wizard.content.append(&buttons(&[close_button(wizard)]));
        return;
    }
    text(
        &wizard.content,
        &tr("Choose a device to see what it needs."),
    );
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["rich-list"])
        .build();
    for gap in &found {
        list.append(&gap_row(gap));
    }
    let open = wizard.clone();
    list.connect_row_activated(move |_, row| {
        let index = row.index();
        if index < 0 {
            return;
        }
        let Some(gap) = found.get(index as usize) else {
            return;
        };
        open_gap(&open, gap);
    });
    wizard.content.append(&list);
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

fn open_gap(wizard: &Rc<Wizard>, gap: &drivers::Gap) {
    match gap {
        drivers::Gap::Wifi(chip) => show_wifi(wizard, chip.clone()),
        drivers::Gap::Nvidia => show_nvidia(wizard),
        drivers::Gap::T2 => show_note(
            wizard,
            &tr("Keyboard, trackpad, and speakers"),
            &tr("This Mac has an Apple T2 chip. The keyboard, trackpad, and speakers often stay off until that chip's bridge has a driver. Wi-Fi does not fix them. Elavo does not include that driver."),
        ),
        drivers::Gap::Facetime => show_note(
            wizard,
            &tr("Camera"),
            &tr("This FaceTime camera needs firmware taken from macOS. That firmware is not included."),
        ),
    }
}

fn show_note(wizard: &Rc<Wizard>, title: &str, body: &str) {
    clear(&wizard.content);
    wizard.content.append(&widgets::heading(title));
    text(&wizard.content, body);
    wizard.content.append(&buttons(&[back_button(wizard)]));
}

fn show_wifi(wizard: &Rc<Wizard>, chip: drivers::WifiChip) {
    clear(&wizard.content);
    wizard.content.append(&widgets::heading(&tr("Wi-Fi")));
    match chip.kind {
        drivers::WifiKind::Firmware => {
            text(
                &wizard.content,
                &words(
                    &tr("This Mac's Wi-Fi chip is a Broadcom {}. Its firmware belongs to Apple and is not included, so Wi-Fi cannot be turned on here."),
                    chip.name,
                ),
            );
            wizard.content.append(&buttons(&[back_button(wizard)]));
        }
        drivers::WifiKind::Proprietary | drivers::WifiKind::BuiltIn => {
            text(
                &wizard.content,
                &words(
                    &tr("Wi-Fi is off. This Broadcom {} chip has no working driver."),
                    chip.name,
                ),
            );
            text(
                &wizard.content,
                &tr("Bluetooth is a separate device and is left as it is."),
            );
            let next = wizard.clone();
            let continue_button = gtk::Button::with_label(&tr("Continue"));
            continue_button.connect_clicked(move |_| show_warning(&next, chip.clone()));
            wizard
                .content
                .append(&buttons(&[back_button(wizard), continue_button]));
        }
    }
}

fn show_warning(wizard: &Rc<Wizard>, chip: drivers::WifiChip) {
    clear(&wizard.content);
    wizard
        .content
        .append(&widgets::heading(&tr("What will change")));
    match chip.kind {
        drivers::WifiKind::Proprietary => {
            text(
                &wizard.content,
                &tr("Broadcom's closed-source driver will run in the kernel, and it will load on every later boot."),
            );
            text(
                &wizard.content,
                &tr("It takes the card from the built-in driver. Sleep is less reliable with this driver, and 5 GHz can be limited."),
            );
            text(
                &wizard.content,
                &tr("Wi-Fi often appears only after a restart, because the built-in driver is already holding the card."),
            );
            text(
                &wizard.content,
                &tr("Broadcom's license allows this driver to be included in unmodified form, and the copyright notice stays with it. It is provided as is, without warranty."),
            );
        }
        drivers::WifiKind::BuiltIn => {
            text(
                &wizard.content,
                &tr("The built-in driver will be turned on. This does not install Broadcom's proprietary driver."),
            );
            if chip.needs_option() {
                text(
                    &wizard.content,
                    &tr("This chip needs a module option. A restart may be needed before Wi-Fi appears."),
                );
            } else {
                text(
                    &wizard.content,
                    &tr("A restart may be needed before Wi-Fi appears."),
                );
            }
        }
        drivers::WifiKind::Firmware => return,
    }
    let activate = gtk::Button::with_label(&tr("Activate"));
    let next = wizard.clone();
    activate.connect_clicked(move |_| turn_on(&next, chip.clone()));
    wizard
        .content
        .append(&buttons(&[back_button(wizard), activate]));
}

fn turn_on(wizard: &Rc<Wizard>, chip: drivers::WifiChip) {
    clear(&wizard.content);
    text(&wizard.content, &tr("Turning on the Wi-Fi driver…"));
    let wizard = wizard.clone();
    let command = match chip.kind {
        drivers::WifiKind::Proprietary => "broadcom",
        drivers::WifiKind::BuiltIn => "brcmfmac",
        drivers::WifiKind::Firmware => return,
    };
    glib::spawn_future_local(async move {
        let status = Rc::new(RefCell::new(String::new()));
        let watch = status.clone();
        let result = helper::run(&["wifi-driver", command], move |line| {
            if matches!(line, "loaded" | "reboot" | "missing" | "refused") {
                watch.replace(line.to_owned());
            }
        })
        .await;
        let status = status.borrow().clone();
        match status.as_str() {
            "missing" => show_result(
                &wizard,
                &tr("This image does not include the Wi-Fi driver, so nothing was changed."),
                false,
            ),
            "refused" => show_result(
                &wizard,
                &tr("This computer refused the driver. It will not load while kernel lockdown is on."),
                false,
            ),
            "reboot" => show_result(
                &wizard,
                &tr("Restart to finish. Wi-Fi will be available after the restart."),
                true,
            ),
            "loaded" if drivers::chip_is_wireless(&chip.id) => show_result(
                &wizard,
                &tr("Wi-Fi is on. You can close this and join a network."),
                false,
            ),
            "loaded" => show_result(
                &wizard,
                &tr("Restart to finish. Wi-Fi will be available after the restart."),
                true,
            ),
            _ => {
                let detail = match result {
                    Ok(()) => tr("The driver was not turned on."),
                    Err(error) => error,
                };
                show_result(&wizard, &detail, false);
            }
        }
    });
}

fn show_result(wizard: &Rc<Wizard>, message: &str, restart: bool) {
    clear(&wizard.content);
    text(&wizard.content, message);
    let row = if restart {
        buttons(&[back_button(wizard), restart_button(wizard)])
    } else {
        buttons(&[back_button(wizard), close_button(wizard)])
    };
    wizard.content.append(&row);
}

fn show_nvidia(wizard: &Rc<Wizard>) {
    clear(&wizard.content);
    wizard.content.append(&widgets::heading(&tr("Graphics")));
    text(
        &wizard.content,
        &tr("This image has no NVIDIA driver. Graphics stay on the open driver, which is limited."),
    );
    text(
        &wizard.content,
        &tr("NVIDIA computers use a separate Elavo image, elavo-nvidia, on the same update channel this computer already uses."),
    );
    text(
        &wizard.content,
        &tr("Switching downloads that image and applies it at the next restart. Accounts and files stay. Software Update can go back to this image."),
    );
    text(
        &wizard.content,
        &tr("The next boot can ask to enroll a machine-owner key, because those modules are not signed by Fedora. That question is on the boot screen. This window cannot answer it."),
    );
    if drivers::nvidia_image_published() {
        let switch = gtk::Button::with_label(&tr("Switch to the NVIDIA image…"));
        let next = wizard.clone();
        switch.connect_clicked(move |_| switch_nvidia(&next));
        wizard
            .content
            .append(&buttons(&[back_button(wizard), switch]));
    } else {
        text(
            &wizard.content,
            &tr("The NVIDIA image is not published yet, so this cannot switch to it."),
        );
        wizard.content.append(&buttons(&[back_button(wizard)]));
    }
}

fn switch_nvidia(wizard: &Rc<Wizard>) {
    clear(&wizard.content);
    text(&wizard.content, &tr("Downloading the NVIDIA image…"));
    let wizard = wizard.clone();
    glib::spawn_future_local(async move {
        match helper::run(&["switch-nvidia"], |_| {}).await {
            Ok(()) => show_result(
                &wizard,
                &tr("The NVIDIA image is downloaded. Restart to use it. If the boot screen asks to enroll a key, that enrolls the driver."),
                true,
            ),
            Err(error) => show_result(&wizard, &error, false),
        }
    });
}
