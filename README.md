# System Settings

Settings for the Elavo desktop. GTK 4 and Rust, no libadwaita, styled to
match the rest of [Folio](https://github.com/elavo-io/elavo-folio).

App id `io.elavo.SystemSettings`, binary `elavo-system-settings`.

## Layout

A sidebar of panes, grouped the way people look for them. Panes with a
chevron in their rows are hubs: General opens its own pages, and Back
returns to it.

| Group | Panes |
| --- | --- |
| Connections | Wi-Fi, Bluetooth, Network |
| | Sound |
| System | General, Appearance, Lens |
| Desktop | Desktop & Dock, Displays, Wallpaper, Screen Saver, Battery |
| People | Lock Screen, Users & Groups |
| Input | Keyboard, Mouse, Trackpad |

General opens About, Software Update, Storage, and Date & Time. The card at
the top of the sidebar is the person logged in and opens Users & Groups.
Battery is called Energy on a computer without one.

Wallpaper and Screen Saver are placeholders: both belong to the shell,
which will draw them, so their settings will arrive with it.

## Opening a pane

```sh
elavo-system-settings --pane dock
elavo-system-settings --list-panes
```

A running instance switches to the pane instead of opening a second
window. Other components open panes the same way: the dock's Dock
Settings… item uses `--pane dock`.

## Where settings go

| Setting | Stored in |
| --- | --- |
| Light or dark | gsettings `org.gnome.desktop.interface color-scheme` |
| Accent color | `~/.local/share/themes/Elavo-user` (imports Elavo) and the `Elavo-mirror` symlink, `~/.config/gtk-4.0/elavo-accent.css`, and gsettings `accent-color` and `gtk-theme`. The theme name reloads apps that load the theme. A high-contrast round trip reloads libadwaita apps such as Files. Blue stays on the Elavo theme. |
| 24-hour time | gsettings `org.gnome.desktop.interface clock-format` |
| Lock and display-off times | `~/.config/elavo/idle.conf`, read by `elavo-idle.service` |
| What Lens shows | `~/.config/elavo/lens.conf` |
| Key repeat, pointer, trackpad | `~/.config/labwc/rc.xml`, then labwc is reloaded |
| Keyboard layout | `~/.config/labwc/environment` |
| Display arrangement | applied live, saved as a kanshi profile in `~/.config/kanshi/config` |
| Dock | the dock's own settings, through `elavo-dock-settings` |

Only the user's labwc files are written; the image's files in
`/etc/xdg/labwc` stay as they are and labwc merges the two.

Wi-Fi, Bluetooth, users, date and time, the computer's name, and power
modes go through their system services over D-Bus (NetworkManager, BlueZ,
AccountsService, timedated, hostnamed, power-profiles-daemon, logind), and
polkit asks for a password where the service needs one.

### The helper

Software updates, the lid setting, and what the login window shows need
root. They go through `/usr/libexec/elavo-system-settings-helper`, run with
`pkexec`. It accepts only these commands and nothing else:

```text
check | upgrade | rollback | switch stable|latest | lid suspend|lock|ignore|poweroff | login-window list|name
```

`login-window` writes `/etc/elavo/greeter.conf`, which Folio's
`elavo-greeter` reads when it starts.

`io.elavo.SystemSettings.policy` has one polkit action per command.

## Building

Fedora packages: `gtk4-devel`, `pulseaudio-libs-devel`, `glib2-devel`.

```sh
cargo build --release
scripts/install            # DESTDIR and PREFIX are honored
```

`scripts/dev-run` builds in the `elavo-dev` toolbox and restarts the app.

Checks, also run in CI:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

## License

MIT. See [LICENSE.md](LICENSE.md).
