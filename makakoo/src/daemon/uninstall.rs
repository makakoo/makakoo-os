//! Platform-dispatching daemon uninstall.

use anyhow::Result;

#[cfg(target_os = "macos")]
pub async fn run() -> Result<()> {
    super::macos::uninstall()?;
    println!("removed launchd plist (if it existed)");
    Ok(())
}

#[cfg(target_os = "linux")]
pub async fn run() -> Result<()> {
    super::linux::uninstall()?;
    println!("removed systemd user unit (if it existed)");
    Ok(())
}

#[cfg(target_os = "windows")]
pub async fn run() -> Result<()> {
    super::windows::uninstall()?;
    println!("removed Windows auto-launch entry for Makakoo");
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
pub async fn run() -> Result<()> {
    Err(anyhow::anyhow!(
        "makakoo daemon uninstall is not implemented on this OS"
    ))
}
