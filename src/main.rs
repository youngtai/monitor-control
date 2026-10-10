mod config;
mod controller;
mod gui;
mod hardware;
mod integrations;
mod model;
mod tray;
mod worker;

fn main() -> gtk::glib::ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--worker") => {
            if let Err(e) = hardware::run_worker() {
                eprintln!("{e}");
                return gtk::glib::ExitCode::FAILURE;
            }
            gtk::glib::ExitCode::SUCCESS
        }
        Some("list" | "--probe") => {
            let result =
                worker::Worker::spawn().and_then(|mut w| w.request(&model::Request::Discover));
            match result {
                Ok(response) => {
                    println!("{}", serde_json::to_string_pretty(&response).unwrap());
                    if response.error.is_some() {
                        gtk::glib::ExitCode::FAILURE
                    } else {
                        gtk::glib::ExitCode::SUCCESS
                    }
                }
                Err(e) => {
                    eprintln!("{e}");
                    gtk::glib::ExitCode::FAILURE
                }
            }
        }
        Some("--check-shortcuts") => match integrations::probe_shortcuts() {
            Ok(()) => gtk::glib::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("{e}");
                gtk::glib::ExitCode::FAILURE
            }
        },
        Some("--help" | "-h") => {
            println!(
                "\
Monitor Control

Usage: monitor-control [OPTION]

  (no option)           Open the app
  --background          Start quietly
  --increase            Increase brightness for the selected group
  --decrease            Decrease brightness for the selected group
  --settings            Open settings
  --release-media-keys  Save and release competing GNOME brightness bindings
  --restore-media-keys  Restore transferred GNOME brightness bindings
  --quit                Exit the running app
  --probe               Read-only hardware diagnostics (JSON)
  --check-shortcuts     Check the desktop shortcut portal
  --help                Show this help"
            );
            gtk::glib::ExitCode::SUCCESS
        }
        _ => gui::run(),
    }
}
