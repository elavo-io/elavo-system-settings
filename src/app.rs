//! The application: one instance, a menubar exported on the session bus for
//! the panel's global menu (never drawn in the window), and `--pane` so the
//! dock, the panel, and the control center can open a pane directly.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};

use crate::i18n::tr;
use crate::window::Window;

const HELP_URL: &str = "https://github.com/elavo-io/elavo-system-settings";

thread_local! {
    static WINDOW: RefCell<Option<Rc<Window>>> = const { RefCell::new(None) };
}

fn window(app: &gtk::Application) -> Rc<Window> {
    if let Some(w) = WINDOW.with(|w| w.borrow().clone()) {
        return w;
    }
    let w = Window::new(app);
    w.window.connect_destroy(|_| {
        WINDOW.with(|w| w.replace(None));
    });
    WINDOW.with(|cell| cell.replace(Some(w.clone())));
    w
}

fn load_css() {
    let Some(display) = gdk::Display::default() else {
        return;
    };
    let css = gtk::CssProvider::new();
    css.load_from_resource("/io/elavo/SystemSettings/style.css");
    gtk::style_context_add_provider_for_display(
        &display,
        &css,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    crate::panes::appearance::install_live_accent(&display);
}

fn menubar(panes: &[(&'static str, String)]) -> gio::Menu {
    let menubar = gio::Menu::new();

    let app_menu = gio::Menu::new();
    app_menu.append(Some(&tr("About System Settings")), Some("app.about"));
    let quit = gio::Menu::new();
    quit.append(Some(&tr("Quit System Settings")), Some("app.quit"));
    app_menu.append_section(None, &quit);
    menubar.append_submenu(Some(&tr("System Settings")), &app_menu);

    let edit = gio::Menu::new();
    edit.append(Some(&tr("Find")), Some("app.find"));
    menubar.append_submenu(Some(&tr("Edit")), &edit);

    let view = gio::Menu::new();
    for (id, title) in panes {
        let item = gio::MenuItem::new(Some(title), None);
        item.set_action_and_target_value(Some("app.show-pane"), Some(&id.to_variant()));
        view.append_item(&item);
    }
    menubar.append_submenu(Some(&tr("View")), &view);

    let win = gio::Menu::new();
    win.append(Some(&tr("Close")), Some("app.close"));
    menubar.append_submenu(Some(&tr("Window")), &win);

    let help = gio::Menu::new();
    help.append(Some(&tr("System Settings Help")), Some("app.help"));
    menubar.append_submenu(Some(&tr("Help")), &help);
    menubar
}

fn add_actions(app: &gtk::Application) {
    let quit = gio::SimpleAction::new("quit", None);
    let a = app.downgrade();
    quit.connect_activate(move |_, _| {
        if let Some(app) = a.upgrade() {
            for w in app.windows() {
                w.close();
            }
            app.quit();
        }
    });
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Control>q"]);

    let close = gio::SimpleAction::new("close", None);
    let a = app.downgrade();
    close.connect_activate(move |_, _| {
        if let Some(w) = a.upgrade().and_then(|app| app.active_window()) {
            w.close();
        }
    });
    app.add_action(&close);
    app.set_accels_for_action("app.close", &["<Control>w"]);

    let find = gio::SimpleAction::new("find", None);
    let a = app.downgrade();
    find.connect_activate(move |_, _| {
        if let Some(app) = a.upgrade() {
            let w = window(&app);
            w.window.present();
            w.focus_search();
        }
    });
    app.add_action(&find);
    app.set_accels_for_action("app.find", &["<Control>f"]);

    let show = gio::SimpleAction::new("show-pane", Some(glib::VariantTy::STRING));
    let a = app.downgrade();
    show.connect_activate(move |_, param| {
        let (Some(app), Some(id)) = (a.upgrade(), param.and_then(|p| p.str())) else {
            return;
        };
        let w = window(&app);
        w.show(id);
        w.window.present();
    });
    app.add_action(&show);

    let about = gio::SimpleAction::new("about", None);
    let a = app.downgrade();
    about.connect_activate(move |_, _| {
        let Some(app) = a.upgrade() else {
            return;
        };
        let dialog = gtk::AboutDialog::builder()
            .program_name(tr("System Settings"))
            .logo_icon_name(crate::APP_ID)
            .version(env!("CARGO_PKG_VERSION"))
            .license_type(gtk::License::MitX11)
            .website(HELP_URL)
            .modal(true)
            .build();
        if let Some(parent) = app.active_window() {
            dialog.set_transient_for(Some(&parent));
        }
        dialog.present();
    });
    app.add_action(&about);

    let help = gio::SimpleAction::new("help", None);
    let a = app.downgrade();
    help.connect_activate(move |_, _| {
        let parent = a.upgrade().and_then(|app| app.active_window());
        gtk::UriLauncher::new(HELP_URL).launch(parent.as_ref(), gio::Cancellable::NONE, |_| {});
    });
    app.add_action(&help);
}

/// `--pane ID`, `--pane=ID`, or a bare ID.
pub fn requested_pane(args: &[String]) -> Option<String> {
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        if let Some(id) = arg.strip_prefix("--pane=") {
            return Some(id.to_owned());
        }
        if arg == "--pane" {
            return iter.next().cloned();
        }
        if !arg.starts_with('-') {
            return Some(arg.clone());
        }
    }
    None
}

const USAGE: &str = "Usage: elavo-system-settings [--pane ID] [--list-panes]";

pub fn run() -> glib::ExitCode {
    // Answered here, in the launching process, so the output reaches the
    // terminal that ran the command even when another instance is running.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return glib::ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "--list-panes") {
        for p in crate::panes::all() {
            println!("{}\t{}", p.id, (p.title)());
        }
        return glib::ExitCode::SUCCESS;
    }
    if let Some(id) = requested_pane(&args) {
        let ids: Vec<&str> = crate::panes::all().iter().map(|p| p.id).collect();
        if !ids.contains(&id.as_str()) {
            eprintln!("Unknown pane '{id}'. Panes: {}", ids.join(", "));
        }
    }

    let app = gtk::Application::builder()
        .application_id(crate::APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();

    app.connect_startup(|app| {
        gtk::Window::set_default_icon_name(crate::APP_ID);
        load_css();
        add_actions(app);
        let panes: Vec<(&'static str, String)> = crate::panes::all()
            .iter()
            .filter(|p| p.parent.is_none())
            .map(|p| (p.id, (p.title)()))
            .collect();
        app.set_menubar(Some(&menubar(&panes)));
    });

    app.connect_command_line(|app, cmd| {
        let args: Vec<String> = cmd
            .arguments()
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        let w = window(app);
        match requested_pane(&args) {
            Some(id) if w.has_pane(&id) => w.show(&id),
            _ => w.show_initial(),
        }
        w.window.present();
        glib::ExitCode::SUCCESS
    });

    app.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pane_arguments() {
        assert_eq!(requested_pane(&args(&["x"])), None);
        assert_eq!(
            requested_pane(&args(&["x", "--pane", "dock"])).as_deref(),
            Some("dock")
        );
        assert_eq!(
            requested_pane(&args(&["x", "--pane=sound"])).as_deref(),
            Some("sound")
        );
        assert_eq!(
            requested_pane(&args(&["x", "about"])).as_deref(),
            Some("about")
        );
        assert_eq!(requested_pane(&args(&["x", "--pane"])), None);
    }
}
