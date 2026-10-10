use crate::{
    config::{self, Settings},
    controller::{Controller, Snapshot},
    integrations::{self, ShortcutCommand},
    tray,
};
use adw::prelude::*;
use gtk::{gio, glib};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};
pub const APP_ID: &str = "io.github.monitorcontrol.MonitorControl";
#[derive(Clone)]
pub enum Action {
    Show,
    Settings,
    Quit,
    Refresh,
    Topology,
    Sleep(bool),
    Set(String, u8),
    Adjust(i32),
    Exclude(String, bool),
    Step(u8),
    Autostart(bool),
    Shortcuts,
    TakeMediaKeys,
    RestoreMediaKeys,
    ShortcutReceived(i32),
    OpenShortcutSettings,
    DisableShortcuts,
    ShortcutStatus(bool, String),
    TrayAvailable(bool),
    EnableTray,
    Notice(String),
}
struct Row {
    card: gtk::Box,
    scale: gtk::Scale,
    value: gtk::Label,
    detail: gtk::Label,
    group: gtk::CheckButton,
}
struct Ui {
    window: adw::ApplicationWindow,
    overlay: adw::ToastOverlay,
    stack: gtk::Stack,
    rows: gtk::Box,
    cards: RefCell<BTreeMap<String, Row>>,
    status: gtk::Label,
    tray_status: gtk::Label,
    enable_tray: gtk::Button,
    shortcut_status: gtk::Label,
    shortcut_button: gtk::Button,
    shortcut_received: gtk::Label,
    media_key_status: gtk::Label,
    spinner: gtk::Spinner,
    syncing: Rc<Cell<bool>>,
    tx: async_channel::Sender<Action>,
}
impl Ui {
    fn new(app: &adw::Application, settings: &Settings, tx: async_channel::Sender<Action>) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Monitor Control")
            .default_width(520)
            .default_height(580)
            .build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = adw::HeaderBar::new();
        let refresh = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh.set_tooltip_text(Some("Check connected monitors"));
        refresh.connect_clicked(send(tx.clone(), Action::Refresh));
        header.pack_start(&refresh);
        let spinner = gtk::Spinner::new();
        header.pack_start(&spinner);
        let settings_button = gtk::Button::from_icon_name("emblem-system-symbolic");
        settings_button.set_tooltip_text(Some("Settings"));
        settings_button.connect_clicked(send(tx.clone(), Action::Settings));
        header.pack_end(&settings_button);
        content.append(&header);
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        let monitors = padded_box();
        let title = gtk::Label::new(Some("Your displays"));
        title.add_css_class("title-1");
        title.set_xalign(0.0);
        monitors.append(&title);
        let intro =
            label("Adjust each display, or use keyboard shortcuts to change your selected group.");
        monitors.append(&intro);
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 14);
        monitors.append(&rows);
        let group = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        for (title, action) in [
            ("−  Group", Action::Adjust(-1)),
            ("+  Group", Action::Adjust(1)),
        ] {
            let button = gtk::Button::with_label(title);
            button.set_hexpand(true);
            button.connect_clicked(send(tx.clone(), action));
            group.append(&button);
        }
        monitors.append(&group);
        let status = label("Finding your monitors…");
        status.set_selectable(true);
        monitors.append(&status);
        let tray_status = label("");
        tray_status.add_css_class("dim-label");
        monitors.append(&tray_status);
        let enable_tray = gtk::Button::with_label("Enable Ubuntu’s top bar controls");
        enable_tray.set_visible(false);
        enable_tray.connect_clicked(send(tx.clone(), Action::EnableTray));
        monitors.append(&enable_tray);
        stack.add_named(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&monitors)
                .build(),
            Some("monitors"),
        );
        let preferences = padded_box();
        let back = gtk::Button::with_label("Back to displays");
        back.set_halign(gtk::Align::Start);
        back.connect_clicked(send(tx.clone(), Action::Show));
        preferences.append(&back);
        let behavior = adw::PreferencesGroup::builder()
            .title("Background operation")
            .build();
        let login = adw::SwitchRow::builder()
            .title("Start at login")
            .subtitle("Keep brightness controls ready after signing in")
            .active(config::autostart_enabled())
            .build();
        let t = tx.clone();
        login.connect_active_notify(move |row| {
            let _ = t.try_send(Action::Autostart(row.is_active()));
        });
        behavior.add(&login);
        let step = adw::SpinRow::with_range(1.0, 25.0, 1.0);
        step.set_title("Brightness step");
        step.set_subtitle("Percentage points per shortcut or tray action");
        step.set_value(settings.step as f64);
        let t = tx.clone();
        step.connect_value_notify(move |row| {
            let _ = t.try_send(Action::Step(row.value() as u8));
        });
        behavior.add(&step);
        preferences.append(&behavior);
        let shortcut_group = adw::PreferencesGroup::builder().title("Keyboard shortcuts").description("Choose brightness media keys, F1/F2, or your preferred combination in the desktop dialog.").build();
        let shortcut_button = gtk::Button::with_label("Set up keyboard shortcuts…");
        shortcut_button.connect_clicked(send(tx.clone(), Action::Shortcuts));
        shortcut_group.add(&shortcut_button);
        let shortcut_status = label("Not configured");
        shortcut_group.add(&shortcut_status);
        let shortcut_received =
            label("No shortcut received this session. Press a configured key to test.");
        shortcut_group.add(&shortcut_received);
        let media_key_status = label(if config::media_keys_conflict() {
            "GNOME already uses the plain brightness media keys. To use those keys here, transfer them below; otherwise choose F1/F2 or a modified shortcut."
        } else {
            ""
        });
        shortcut_group.add(&media_key_status);
        let take_keys =
            gtk::Button::with_label("Release GNOME brightness keys for Monitor Control");
        take_keys.connect_clicked(send(tx.clone(), Action::TakeMediaKeys));
        shortcut_group.add(&take_keys);
        let restore_keys = gtk::Button::with_label("Restore GNOME brightness keys");
        restore_keys.connect_clicked(send(tx.clone(), Action::RestoreMediaKeys));
        shortcut_group.add(&restore_keys);
        let disable = gtk::Button::with_label("Disable keyboard shortcuts");
        disable.connect_clicked(send(tx.clone(), Action::DisableShortcuts));
        shortcut_group.add(&disable);
        preferences.append(&shortcut_group);
        preferences.append(&label("Closing this window keeps Monitor Control running. Use Quit to stop it for this session."));
        let quit = gtk::Button::with_label("Quit Monitor Control");
        quit.connect_clicked(send(tx.clone(), Action::Quit));
        preferences.append(&quit);
        stack.add_named(
            &gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&preferences)
                .build(),
            Some("settings"),
        );
        content.append(&stack);
        let overlay = adw::ToastOverlay::new();
        overlay.set_child(Some(&content));
        window.set_content(Some(&overlay));
        window.connect_close_request(|window| {
            window.set_visible(false);
            glib::Propagation::Stop
        });
        Self {
            window,
            overlay,
            stack,
            rows,
            cards: RefCell::new(BTreeMap::new()),
            status,
            tray_status,
            enable_tray,
            shortcut_status,
            shortcut_button,
            shortcut_received,
            media_key_status,
            spinner,
            syncing: Rc::new(Cell::new(false)),
            tx,
        }
    }
    fn render(&self, snapshot: &Snapshot, settings: &Settings) {
        self.syncing.set(true);
        self.spinner.set_spinning(snapshot.busy);
        let mut cards = self.cards.borrow_mut();
        cards.retain(|id, row| {
            if snapshot.monitors.iter().any(|m| &m.id == id) {
                true
            } else {
                self.rows.remove(&row.card);
                false
            }
        });
        for m in &snapshot.monitors {
            let row = cards.entry(m.id.clone()).or_insert_with(|| {
                let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
                card.add_css_class("card");
                let inner = padded_box();
                card.append(&inner);
                let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let title = label(&m.name);
                title.add_css_class("title-3");
                title.set_hexpand(true);
                header.append(&title);
                let value = label("");
                value.add_css_class("title-3");
                header.append(&value);
                inner.append(&header);
                let detail = label("");
                detail.add_css_class("dim-label");
                inner.append(&detail);
                let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
                scale.set_hexpand(true);
                scale.update_property(&[gtk::accessible::Property::Label(&format!(
                    "{} brightness",
                    m.name
                ))]);
                let (t, id, syncing) = (self.tx.clone(), m.id.clone(), self.syncing.clone());
                scale.connect_value_changed(move |s| {
                    if !syncing.get() {
                        let _ = t.try_send(Action::Set(id.clone(), s.value().round() as u8));
                    }
                });
                inner.append(&scale);
                let group = gtk::CheckButton::with_label("Include in keyboard and group controls");
                let (t, id, syncing) = (self.tx.clone(), m.id.clone(), self.syncing.clone());
                group.connect_toggled(move |check| {
                    if !syncing.get() {
                        let _ = t.try_send(Action::Exclude(id.clone(), !check.is_active()));
                    }
                });
                inner.append(&group);
                self.rows.append(&card);
                Row {
                    card,
                    scale,
                    value,
                    detail,
                    group,
                }
            });
            row.scale
                .set_sensitive(m.maximum.is_some() && !snapshot.busy);
            row.group.set_active(!settings.excluded.contains(&m.id));
            if let Some(value) = snapshot.percent(m) {
                row.scale.set_value(value as f64);
                row.value.set_text(&format!("{value}%"));
            } else {
                row.value.set_text("Unavailable");
            }
            let detail = if let Some(e) = &m.error {
                format!("{} · {e}", m.connector)
            } else if snapshot.desired.contains_key(&m.id) {
                format!("{} · Applying…", m.connector)
            } else {
                format!("{} · {}", m.connector, m.serial)
            };
            row.detail.set_text(&detail);
        }
        self.status.set_text(if snapshot.busy { "Checking connected monitors…" }
            else if let Some(e) = &snapshot.error { e }
            else if snapshot.monitors.is_empty() { "No controllable displays found. Enable DDC/CI in your monitor’s menu and check the cable or dock. The app will retry when displays reconnect." }
            else { "Changes adjust the monitor’s hardware brightness." });
        self.syncing.set(false);
    }
}
fn label(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l
}
fn padded_box() -> gtk::Box {
    let b = gtk::Box::new(gtk::Orientation::Vertical, 14);
    for set in [
        gtk::prelude::WidgetExt::set_margin_top,
        gtk::prelude::WidgetExt::set_margin_bottom,
        gtk::prelude::WidgetExt::set_margin_start,
        gtk::prelude::WidgetExt::set_margin_end,
    ] {
        set(&b, 20);
    }
    b
}
fn send(tx: async_channel::Sender<Action>, action: Action) -> impl Fn(&gtk::Button) {
    move |_| {
        let _ = tx.try_send(action.clone());
    }
}

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::HANDLES_COMMAND_LINE)
        .build();
    let (tx, rx) = async_channel::unbounded::<Action>();
    let rx = Rc::new(RefCell::new(Some(rx)));
    let hold = Rc::new(RefCell::new(None));
    let controller_slot = Rc::new(RefCell::new(None::<Controller>));
    let controller_stop = controller_slot.clone();
    app.connect_shutdown(move |_| {
        if let Some(c) = controller_stop.borrow().as_ref() {
            c.stop();
        }
    });
    let t = tx.clone();
    app.connect_command_line(move |_, command| {
        let args: Vec<String> = command
            .arguments()
            .iter()
            .skip(1)
            .map(|s| s.to_string_lossy().into_owned())
            .collect();
        let action = match args.first().map(String::as_str) {
            None | Some("gui") => Some(Action::Show),
            Some("--background") => None,
            Some("--increase") => Some(Action::Adjust(1)),
            Some("--decrease") => Some(Action::Adjust(-1)),
            Some("--release-media-keys") => Some(Action::TakeMediaKeys),
            Some("--restore-media-keys") => Some(Action::RestoreMediaKeys),
            Some("--settings") => Some(Action::Settings),
            Some("--quit") => Some(Action::Quit),
            _ => {
                eprintln!("Unknown option; use --help\n");
                return glib::ExitCode::FAILURE;
            }
        };
        if let Some(action) = action {
            let _ = t.try_send(action);
        }
        glib::ExitCode::SUCCESS
    });
    app.connect_startup(move |app| {
        *hold.borrow_mut() = Some(app.hold());
        let loaded = Settings::load();
        let mut settings = loaded.as_ref().cloned().unwrap_or_default();
        let ui = Rc::new(Ui::new(app, &settings, tx.clone()));
        if let Err(e) = loaded { let _ = tx.try_send(Action::Notice(e));
 }
        let (snapshot_tx, snapshot_rx) = async_channel::unbounded();
        let controller = Controller::new(snapshot_tx);
 *controller_slot.borrow_mut() = Some(controller.clone());
        let shortcuts = integrations::start(tx.clone());
        if settings.shortcuts_enabled { let _ = shortcuts.try_send(ShortcutCommand::Connect);
 }
        let tray = tray::start(tx.clone());
        let shared_settings = Rc::new(RefCell::new(settings.clone()));
        let (u, prefs) = (ui.clone(), shared_settings.clone());
        glib::MainContext::default().spawn_local(async move {
            let _tray = tray;
            while let Ok(update) = snapshot_rx.recv().await {
                u.render(&update, &prefs.borrow());
            }
        });
        let actions = rx.borrow_mut().take().expect("primary application startup");
        let a = app.clone();
        let u = ui.clone();
        glib::MainContext::default().spawn_local(async move {
            while let Ok(action) = actions.recv().await {
                let mut save = false;
                match action {
                    Action::Show => { u.stack.set_visible_child_name("monitors");
 u.window.present();
 controller.refresh(false);
 }
                    Action::Settings => { u.stack.set_visible_child_name("settings");
 u.window.present();
 }
                    Action::Quit => { controller.stop();
 a.quit();
 break;
 }
                    Action::Refresh | Action::Topology => controller.refresh(true),
                    Action::Sleep(sleeping) => controller.sleep(sleeping),
                    Action::Set(id, value) => controller.set(&id, value),
                    Action::Adjust(direction) => controller.adjust(direction * i32::from(settings.step), &settings.excluded),
                    Action::Exclude(id, excluded) => { settings.excluded.retain(|x| x != &id);
 if excluded { settings.excluded.push(id);
 } save = true;
 }
                    Action::Step(step) => { settings.step = step.clamp(1, 25);
 save = true;
 }
                    Action::Autostart(enabled) => { if let Err(e) = config::set_autostart(enabled) { u.overlay.add_toast(adw::Toast::new(&e));
 } }
                    Action::ShortcutReceived(direction) => {
                        let time = glib::DateTime::now_local().and_then(|d| d.format("%H:%M:%S")).map(|s| s.to_string()).unwrap_or_default();
                        u.shortcut_received.set_text(&format!("Received {} at {time}", if direction > 0 { "increase" } else { "decrease" }));
                        controller.adjust(direction * i32::from(settings.step), &settings.excluded);
                    }
                    Action::TakeMediaKeys => {
                        match settings.take_media_keys() {
                            Ok(()) => {
                                u.media_key_status.set_text("GNOME’s plain brightness keys are released. Assign those keys in Configure keyboard shortcuts. Restore them here when you stop using Monitor Control.");
                                let _ = shortcuts.try_send(ShortcutCommand::Reconnect);
                            }
                            Err(e) => u.overlay.add_toast(adw::Toast::new(&e)),
                        }
                        save = true;
                    }
                    Action::RestoreMediaKeys => {
                        match settings.restore_media_keys() {
                            Ok(()) => u.media_key_status.set_text("GNOME brightness keys restored. Choose other keys for Monitor Control."),
                            Err(e) => u.overlay.add_toast(adw::Toast::new(&e)),
                        }
                        save = true;
                    }
                    Action::Shortcuts => { u.shortcut_status.set_text("Waiting for the desktop shortcut dialog…");
 let _ = shortcuts.try_send(ShortcutCommand::Configure);
 }
                    Action::OpenShortcutSettings => {
                        u.shortcut_status.set_text("Change bindings in Settings → Apps → Monitor Control → Shortcuts.");
                        let args = [std::ffi::OsStr::new("gnome-control-center"), std::ffi::OsStr::new("applications"), std::ffi::OsStr::new(APP_ID)];
                        if let Err(e) = gio::Subprocess::newv(&args, gio::SubprocessFlags::NONE) { u.overlay.add_toast(adw::Toast::new(&format!("Cannot open shortcut settings: {e}")));
 }
                    }
                    Action::DisableShortcuts => { if let Err(e) = settings.restore_media_keys() { u.overlay.add_toast(adw::Toast::new(&e));
 } u.media_key_status.set_text("Keyboard shortcuts disabled; any transferred GNOME keys have been restored.");
 settings.shortcuts_enabled = false;
 save = true;
 let _ = shortcuts.try_send(ShortcutCommand::Disable);
 }
                    Action::ShortcutStatus(enabled, description) => { u.shortcut_status.set_text(&description);
 u.shortcut_button.set_label(if enabled { "Configure keyboard shortcuts…" } else { "Set up keyboard shortcuts…" });
 if enabled { settings.shortcuts_enabled = true;
 save = true;
 } }
                    Action::TrayAvailable(available) => {
                        u.tray_status.set_text(if available { "" } else { "The desktop’s tray indicator support is disabled. The app and shortcuts still work; reopen from Applications." });
                        u.enable_tray.set_visible(!available && std::path::Path::new("/usr/share/gnome-shell/extensions/ubuntu-appindicators@ubuntu.com").exists());
                    }
                    Action::EnableTray => {
                        let tx = u.tx.clone();
                        std::thread::spawn(move || {
                            let result = std::process::Command::new("gnome-extensions").args(["enable", "ubuntu-appindicators@ubuntu.com"]).output();
                            match result {
                                Ok(output) if output.status.success() => (),
                                Ok(output) => { let _ = tx.try_send(Action::Notice(String::from_utf8_lossy(&output.stderr).into_owned()));
 }
                                Err(e) => { let _ = tx.try_send(Action::Notice(e.to_string()));
 }
                            }
                        });
                    }
                    Action::Notice(message) => { eprintln!("{message}");
 u.status.set_text(&message);
 u.overlay.add_toast(adw::Toast::new(&message));
 }
                }
                if save {
                    *shared_settings.borrow_mut() = settings.clone();
                    if let Err(e) = settings.save() { u.overlay.add_toast(adw::Toast::new(&format!("Could not save preferences: {e}")));
 }
                }
            }
        });
        if let Ok(ms) = std::env::var("MONITOR_CONTROL_SMOKE_MS").unwrap_or_default().parse::<u64>() {
            let app = app.clone();
 let ui = ui.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(ms), move || {
                if let Ok(path) = std::env::var("MONITOR_CONTROL_SCREENSHOT") { screenshot(&ui.window, &path);
 }
                app.quit();
            });
        }
    });
    app.run()
}
fn screenshot(window: &impl IsA<gtk::Window>, path: &str) {
    let window = window.as_ref();
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, window.width() as f64, window.height() as f64);
    if let (Some(node), Some(renderer)) = (snapshot.to_node(), window.renderer()) {
        let texture = renderer.render_texture(&node, None);
        if let Err(e) = texture.save_to_png(path) {
            eprintln!("Screenshot: {e}");
        }
    }
}
