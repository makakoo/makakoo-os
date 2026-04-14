//! Windows auto-start writer via the `auto-launch` crate.
//!
//! The `auto-launch` crate writes a Registry entry under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` — the standard
//! per-user auto-launch hook.

use anyhow::{anyhow, Result};

pub const APP_NAME: &str = "Makakoo";

pub fn build() -> Result<auto_launch::AutoLaunch> {
    let exe = std::env::current_exe()?;
    let path = exe
        .to_str()
        .ok_or_else(|| anyhow!("exe path contains invalid UTF-8"))?;
    let args: Vec<&str> = vec!["daemon", "run"];
    let launch = auto_launch::AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(path)
        .set_args(&args)
        .build()
        .map_err(|e| anyhow!("auto-launch build: {e}"))?;
    Ok(launch)
}

pub fn install() -> Result<()> {
    let launch = build()?;
    launch.enable().map_err(|e| anyhow!("auto-launch enable: {e}"))?;
    Ok(())
}

pub fn uninstall() -> Result<()> {
    let launch = build()?;
    launch.disable().map_err(|e| anyhow!("auto-launch disable: {e}"))?;
    Ok(())
}

pub fn is_installed() -> bool {
    build()
        .and_then(|l| l.is_enabled().map_err(|e| anyhow!(e.to_string())))
        .unwrap_or(false)
}

pub fn is_running() -> bool {
    // Windows doesn't have a cheap "is process running by name" check from
    // stdlib — we conservatively report installed == running for now.
    is_installed()
}
