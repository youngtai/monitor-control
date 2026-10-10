# Monitor Control

A native Rust/GTK 4 app for controlling external-monitor brightness through DDC/CI.
Built for Ubuntu 26.04 and GNOME, without a custom GNOME Shell extension.

- Per-monitor sliders and grouped brightness controls.
- Configurable global keyboard shortcuts and brightness steps.
- Login autostart and a simple Open/Quit tray menu.
- Event-driven monitor discovery, with bounded retries and no periodic scanning.

## Build and install

Requires Rust 1.92 or newer and these development packages:

```sh
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev libddcutil-dev
./scripts/build-deb.sh
./install.sh
```

The installer requests administrator authentication. The `.deb` installs runtime
libraries, hardware-access rules, and a login startup entry. Neither Rust nor the
`ddcutil` command is required to run an installed package; the app uses
**libddcutil** directly. Other distributions are not yet tested.

## Use

Open **Monitor Control** from Applications. Adjust individual monitors or select
which monitors keyboard/group controls affect. Settings contains shortcut setup,
step size, and login startup options. Closing the window keeps the app running;
choose **Quit** to stop it.

Enable **DDC/CI** in your monitor's menu. Some cables and docks do not pass DDC
commands. The tray uses the desktop's existing AppIndicator support; Ubuntu users
can enable it from the app if necessary.

GNOME reserves plain brightness media keys. To use those keys here, choose
**Release GNOME brightness keys for Monitor Control**, then configure the keys.
Original bindings are saved. Choose **Restore GNOME brightness keys** before
uninstalling; disabling the app's shortcuts also restores them.

## Development

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo run -- --probe
```

The GTK app owns a single background controller. DDC requests run serially in a
separate worker process, with coalesced writes and a timeout. Display and resume
events trigger discovery; hardware calls never block the UI.

Optional desktop smoke tests need `xvfb`, `dbus-daemon`, and `python3-gi`:

```sh
cargo build --locked
GDK_BACKEND=x11 GSK_RENDERER=cairo GTK_A11Y=none \
  dbus-run-session -- xvfb-run -a /usr/bin/python3 tests/session-smoke.py target/debug/monitor-control
GDK_BACKEND=x11 GSK_RENDERER=cairo GTK_A11Y=none \
  dbus-run-session -- xvfb-run -a /usr/bin/python3 tests/tray-smoke.py target/debug/monitor-control
```

Tests use isolated app preferences. The smoke tests read connected monitors but
do not change brightness. The ignored GTK key-handoff test requires Xvfb,
`GSETTINGS_BACKEND=memory`, and a temporary `XDG_CONFIG_HOME` under `/tmp`.

Preferences live in `~/.config/monitor-control/settings.json` (or
`$XDG_CONFIG_HOME/monitor-control/settings.json`). Use `monitor-control --help`
for command-line controls and diagnostics.
