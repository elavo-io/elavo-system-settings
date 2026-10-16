mod app;
mod backend;
mod i18n;
mod panes;
mod widgets;
mod window;

use gtk::glib;

pub const APP_ID: &str = "io.elavo.SystemSettings";

fn main() -> glib::ExitCode {
    i18n::init();
    gtk::gio::resources_register_include!("elavo-system-settings.gresource")
        .expect("compiled resources");
    app::run()
}
