//! `makakoo daemon uninstall` — tears down the auto-start registration
//! through the `PlatformAdapter` trait. Idempotent.

use anyhow::Result;

use makakoo_platform::{CurrentPlatform, PlatformAdapter};

pub async fn run() -> Result<()> {
    let platform = CurrentPlatform::default();
    platform.daemon_uninstall()?;
    println!("makakoo daemon service descriptor removed (if it existed)");
    Ok(())
}
