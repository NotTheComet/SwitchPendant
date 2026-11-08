//! Persistent settings (~/.config/teleop/settings.json) and the GNOME/XDG autostart entry.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::transport::Config;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub cfg: Config,
    /// "The user wants the link up": reconnect automatically at launch and after failures.
    pub auto_connect: bool,
}

fn config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

fn settings_path() -> Option<PathBuf> {
    config_home().map(|d| d.join("teleop").join("settings.json"))
}

pub fn load() -> Settings {
    let Some(path) = settings_path() else { return Settings::default() };
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).unwrap_or_else(|e| {
            eprintln!("teleop: ignoring bad settings file {}: {e}", path.display());
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

pub fn save(settings: &Settings) -> Result<()> {
    let path = settings_path().context("no config directory (HOME unset)")?;
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(&path, serde_json::to_string_pretty(settings)?)
        .with_context(|| format!("writing {}", path.display()))
}

// ---- autostart -------------------------------------------------------------

fn autostart_path() -> Option<PathBuf> {
    config_home().map(|d| d.join("autostart").join("teleop-controller.desktop"))
}

pub fn autostart_enabled() -> bool {
    autostart_path().is_some_and(|p| p.exists())
}

pub fn set_autostart(enabled: bool) -> Result<()> {
    let path = autostart_path().context("no config directory (HOME unset)")?;
    if !enabled {
        if path.exists() {
            fs::remove_file(&path)?;
        }
        return Ok(());
    }
    let exe = std::env::current_exe().context("locating this executable")?;
    // Desktop Entry quoting: wrap in double quotes, escape  " \ ` $
    let exe = exe.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"").replace('`', "\\`").replace('$', "\\$");
    fs::create_dir_all(path.parent().unwrap())?;
    fs::write(
        &path,
        format!(
            "[Desktop Entry]\nType=Application\nName=Teleop Controller\nComment=Gamepad teleoperation (tray)\n\
             Exec=\"{exe}\" --background\nIcon=input-gaming\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
        ),
    )
    .with_context(|| format!("writing {}", path.display()))
}
