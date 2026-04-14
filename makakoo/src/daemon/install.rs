//! Platform-dispatching daemon install.

use anyhow::Result;

#[cfg(target_os = "macos")]
pub async fn run() -> Result<()> {
    let path = super::macos::install()?;
    println!("installed launchd plist at {}", path.display());
    println!("daemon loaded via launchctl (best-effort)");
    Ok(())
}

#[cfg(target_os = "linux")]
pub async fn run() -> Result<()> {
    let path = super::linux::install()?;
    println!("installed systemd user unit at {}", path.display());
    println!("systemctl --user daemon-reload / enable / start (best-effort)");
    Ok(())
}

#[cfg(target_os = "windows")]
pub async fn run() -> Result<()> {
    super::windows::install()?;
    println!("installed Windows auto-launch entry for Makakoo");
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub async fn run() -> Result<()> {
    Err(anyhow::anyhow!(
        "makakoo daemon install is not implemented on this OS"
    ))
}
