use serde::{Deserialize, Serialize};
use std::{io::Write, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub step: u8,
    pub excluded: Vec<String>,
    pub shortcuts_enabled: bool,
    pub media_key_backups: Vec<MediaKeyBackup>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            step: 5,
            excluded: Vec::new(),
            shortcuts_enabled: false,
            media_key_backups: Vec::new(),
        }
    }
}
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}
fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
}
fn path() -> PathBuf {
    config_home().join("monitor-control/settings.json")
}
impl Settings {
    pub fn load() -> Result<Self, String> {
        let bytes = match std::fs::read(path()) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(format!("Cannot read preferences: {e}")),
        };
        let mut settings: Self =
            serde_json::from_slice(&bytes).map_err(|e| format!("Cannot read preferences: {e}"))?;
        settings.step = settings.step.clamp(1, 25);
        Ok(settings)
    }
    pub fn save(&self) -> Result<(), String> {
        let path = path();
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let temp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
        file.write_all(&serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(temp, path).map_err(|e| e.to_string())
    }
}
pub fn autostart_enabled() -> bool {
    let user = config_home().join("autostart/io.github.monitorcontrol.MonitorControl.desktop");
    match std::fs::read_to_string(user) {
        Ok(text) => !text.lines().any(|line| line.trim() == "Hidden=true"),
        Err(_) => std::path::Path::new(
            "/etc/xdg/autostart/io.github.monitorcontrol.MonitorControl.desktop",
        )
        .exists(),
    }
}
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    let path = config_home().join("autostart/io.github.monitorcontrol.MonitorControl.desktop");
    if enabled {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        // Desktop Entry Exec quoting is different from shell quoting.
        let escaped = exe
            .to_string_lossy()
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('`', "\\`")
            .replace('$', "\\$")
            .replace('%', "%%");
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path, format!("[Desktop Entry]\nType=Application\nName=Monitor Control\nExec=\"{escaped}\" --background\nTerminal=false\nX-GNOME-Autostart-enabled=true\n")).map_err(|e| e.to_string())
    } else {
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::write(path, "[Desktop Entry]\nType=Application\nName=Monitor Control\nExec=monitor-control --background\nHidden=true\n").map_err(|e| e.to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MediaKeyBackup {
    schema: String,
    key: String,
    original: Vec<String>,
    replacement: Vec<String>,
}

fn desktop_key_settings() -> Vec<(gtk::gio::Settings, String)> {
    let Some(source) = gtk::gio::SettingsSchemaSource::default() else {
        return Vec::new();
    };
    let mut result = Vec::new();
    for name in [
        "org.gnome.shell.keybindings",
        "org.gnome.settings-daemon.plugins.media-keys",
    ] {
        if let Some(schema) = source.lookup(name, true) {
            for key in [
                "screen-brightness-up",
                "screen-brightness-down",
                "screen-brightness-up-static",
                "screen-brightness-down-static",
            ] {
                if schema.has_key(key) {
                    result.push((
                        gtk::gio::Settings::new_full(
                            &schema,
                            None::<&gtk::gio::SettingsBackend>,
                            None,
                        ),
                        key.to_owned(),
                    ));
                }
            }
        }
    }
    result
}
fn is_plain_brightness(accelerator: &str) -> bool {
    gtk::accelerator_parse(accelerator).is_some_and(|(key, modifiers)| {
        modifiers.is_empty()
            && [
                gtk::gdk::Key::MonBrightnessUp,
                gtk::gdk::Key::MonBrightnessDown,
            ]
            .contains(&key)
    })
}
pub fn media_keys_conflict() -> bool {
    use gtk::prelude::*;
    desktop_key_settings()
        .iter()
        .any(|(settings, key)| settings.strv(key).iter().any(|s| is_plain_brightness(s)))
}
impl Settings {
    pub fn take_media_keys(&mut self) -> Result<(), String> {
        use gtk::prelude::*;
        let targets = desktop_key_settings();
        for (settings, key) in &targets {
            let original: Vec<String> =
                settings.strv(key).iter().map(ToString::to_string).collect();
            let replacement: Vec<String> = original
                .iter()
                .filter(|s| !is_plain_brightness(s))
                .cloned()
                .collect();
            if original == replacement {
                continue;
            }
            let schema = settings
                .schema_id()
                .ok_or("Missing desktop settings schema")?
                .to_string();
            if !self
                .media_key_backups
                .iter()
                .any(|b| b.schema == schema && b.key == *key)
            {
                self.media_key_backups.push(MediaKeyBackup {
                    schema,
                    key: key.clone(),
                    original,
                    replacement,
                });
            }
        }
        // Persist recovery information before changing desktop bindings.
        self.save()?;
        for backup in &self.media_key_backups {
            let settings = gtk::gio::Settings::new(&backup.schema);
            let current: Vec<String> = settings
                .strv(&backup.key)
                .iter()
                .map(ToString::to_string)
                .collect();
            if current == backup.original {
                settings
                    .set_strv(&backup.key, backup.replacement.as_slice())
                    .map_err(|e| e.to_string())?;
            }
        }
        gtk::gio::Settings::sync();
        Ok(())
    }
    pub fn restore_media_keys(&mut self) -> Result<(), String> {
        use gtk::prelude::*;
        let Some(source) = gtk::gio::SettingsSchemaSource::default() else {
            return Err("Desktop settings unavailable".into());
        };
        for backup in &self.media_key_backups {
            let Some(schema) = source.lookup(&backup.schema, true) else {
                continue;
            };
            if !schema.has_key(&backup.key) {
                continue;
            }
            let settings =
                gtk::gio::Settings::new_full(&schema, None::<&gtk::gio::SettingsBackend>, None);
            let current: Vec<String> = settings
                .strv(&backup.key)
                .iter()
                .map(ToString::to_string)
                .collect();
            // Preserve changes the user made independently after the handoff.
            if current == backup.replacement {
                settings
                    .set_strv(&backup.key, backup.original.as_slice())
                    .map_err(|e| e.to_string())?;
            }
        }
        gtk::gio::Settings::sync();
        self.media_key_backups.clear();
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;

    #[test]
    #[ignore = "Requires Xvfb, GSETTINGS_BACKEND=memory and an isolated XDG_CONFIG_HOME"]
    fn media_key_handoff_restores_and_preserves_external_edits() {
        assert_eq!(std::env::var("GSETTINGS_BACKEND").unwrap(), "memory");
        assert!(std::env::var("XDG_CONFIG_HOME")
            .unwrap()
            .starts_with("/tmp/"));
        gtk::init().unwrap();
        let desktop = gtk::gio::Settings::new("org.gnome.shell.keybindings");
        let up = "screen-brightness-up";
        let down = "screen-brightness-down";
        desktop
            .set_strv(up, ["XF86MonBrightnessUp", "<Control>F2"])
            .unwrap();
        desktop.set_strv(down, ["XF86MonBrightnessDown"]).unwrap();
        let mut prefs = Settings::default();
        assert!(media_keys_conflict());
        prefs.take_media_keys().unwrap();
        assert!(!media_keys_conflict());
        assert_eq!(
            desktop
                .strv(up)
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            ["<Control>F2"]
        );
        assert!(!Settings::load().unwrap().media_key_backups.is_empty());
        // User chooses a new desktop shortcut after the handoff.
        desktop.set_strv(down, ["<Control>F1"]).unwrap();
        prefs.restore_media_keys().unwrap();
        assert_eq!(
            desktop
                .strv(up)
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            ["XF86MonBrightnessUp", "<Control>F2"]
        );
        assert_eq!(
            desktop
                .strv(down)
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            ["<Control>F1"]
        );
        assert!(Settings::load().unwrap().media_key_backups.is_empty());
    }
}
