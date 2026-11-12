//! Start on login: an XDG autostart entry on Linux, a LaunchAgent on macOS.

use std::path::PathBuf;

use anyhow::{Context, Result};

#[cfg(target_os = "macos")]
pub const APP_ID: &str = "io.github.rj_xy.hollymic";

fn exe() -> Result<PathBuf> {
    std::env::current_exe().context("finding the hollymic binary")
}

#[cfg(not(target_os = "macos"))]
pub fn entry_path() -> PathBuf {
    if let Ok(d) = std::env::var("HOLLYMIC_AUTOSTART_DIR") {
        return PathBuf::from(d).join("hollymic.desktop");
    }
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("autostart").join("hollymic.desktop")
}

#[cfg(target_os = "macos")]
pub fn entry_path() -> PathBuf {
    if let Ok(d) = std::env::var("HOLLYMIC_AUTOSTART_DIR") {
        return PathBuf::from(d).join(format!("{APP_ID}.plist"));
    }
    dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join("Library/LaunchAgents").join(format!("{APP_ID}.plist"))
}

#[cfg(not(target_os = "macos"))]
fn entry_text(exe: &std::path::Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=HollyMic\nComment=Hollyland Lark M2 mic control in the tray\n\
         Exec=\"{}\" --autostart\nIcon=hollymic\nTerminal=false\nCategories=AudioVideo;Audio;\n\
         X-GNOME-Autostart-enabled=true\nX-KDE-autostart-phase=2\n",
        exe.display()
    )
}

#[cfg(target_os = "macos")]
fn entry_text(exe: &std::path::Path) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n  <key>Label</key><string>{APP_ID}</string>\n\
         \x20 <key>ProgramArguments</key><array><string>{}</string><string>--autostart</string></array>\n\
         \x20 <key>RunAtLoad</key><true/>\n  <key>ProcessType</key><string>Interactive</string>\n</dict>\n</plist>\n",
        exe.display()
    )
}

/// Write or remove the login entry so it matches the setting.
pub fn apply(enabled: bool) -> Result<()> {
    let p = entry_path();
    if enabled {
        let text = entry_text(&exe()?);
        if std::fs::read_to_string(&p).ok().as_deref() == Some(text.as_str()) {
            return Ok(());
        }
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, text).with_context(|| format!("writing {}", p.display()))?;
        log::info!("start on login: wrote {}", p.display());
    } else if p.exists() {
        std::fs::remove_file(&p).with_context(|| format!("removing {}", p.display()))?;
        log::info!("start on login: removed {}", p.display());
    }
    Ok(())
}
