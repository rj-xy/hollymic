//! User settings, saved as TOML: `~/.config/hollymic/settings.toml` on Linux,
//! `~/Library/Application Support/hollymic/settings.toml` on macOS.

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Settings {
    /// Start HollyMic when you log in.
    pub start_on_login: bool,
    /// Start as a tray icon only, without opening the window.
    pub start_minimized: bool,
    /// Closing the window keeps HollyMic running in the tray (otherwise it quits).
    pub close_to_tray: bool,
    pub theme: Theme,
    /// Desktop notification when a mic's battery drops to `low_battery_percent`.
    pub low_battery_alert: bool,
    pub low_battery_percent: u8,
    /// Desktop notification when the receiver is plugged in or out.
    pub connection_alerts: bool,
    /// Ask Hollyland's server once a day whether newer firmware exists.
    pub check_firmware: bool,
    /// Show the input level meter in the window (Linux: reads the mic through PipeWire).
    pub level_meter: bool,
    /// The tray light of a slot with no mic flashes blue, like the receiver's LED.
    pub flash_empty_slots: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            start_on_login: false,
            start_minimized: true,
            close_to_tray: true,
            theme: Theme::System,
            low_battery_alert: true,
            low_battery_percent: 15,
            connection_alerts: false,
            check_firmware: true,
            level_meter: true,
            flash_empty_slots: true,
        }
    }
}

pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("HOLLYMIC_CONFIG_DIR") {
        return PathBuf::from(d);
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("hollymic")
}

pub fn path() -> PathBuf {
    config_dir().join("settings.toml")
}

impl Settings {
    /// Load the settings; missing file or keys fall back to the defaults.
    pub fn load() -> Settings {
        let p = path();
        match std::fs::read_to_string(&p) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                log::warn!("{}: {e}; using defaults", p.display());
                Settings::default()
            }),
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let p = path();
        std::fs::create_dir_all(p.parent().unwrap())
            .with_context(|| format!("creating {}", p.parent().unwrap().display()))?;
        let text = format!(
            "# HollyMic settings. Edited by the app; safe to edit by hand while it isn't running.\n{}",
            toml::to_string_pretty(self)?
        );
        let tmp = p.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &p)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_partial_files() {
        let s = Settings { start_on_login: true, theme: Theme::Dark, ..Default::default() };
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("start-on-login = true"));
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
        let partial: Settings = toml::from_str("theme = \"light\"\n").unwrap();
        assert_eq!(partial.theme, Theme::Light);
        assert!(partial.start_minimized, "missing keys keep their defaults");
    }
}
