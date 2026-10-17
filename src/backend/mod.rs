//! Everything that is not a widget: files, D-Bus services, PipeWire, and
//! the compositor. Panes call these and never talk to a service inline.

pub mod accent;
pub mod accounts;
pub mod bluetooth;
pub mod dbus;
pub mod gsettings;
pub mod helper;
pub mod hostname;
pub mod kanshi;
pub mod keyfile;
pub mod labwc;
pub mod network;
pub mod outputs;
pub mod power;
pub mod process;
pub mod pulse;
pub mod sysinfo;
pub mod timedate;
pub mod updates;
pub mod wallpaper;
