//! `makakoo daemon install` — registers the daemon as an auto-start
//! service through the `PlatformAdapter` trait.

use anyhow::Result;

use makakoo_platform::{CurrentPlatform, PlatformAdapter};

pub async fn run() -> Result<()> {
    let platform = CurrentPlatform::default();
    let path = platform.daemon_install()?;
    println!(
        "installed makakoo daemon service descriptor at {}",
        path.display()
    );
    println!("(best-effort lifecycle: {} will start the service)", platform.name());
    Ok(())
}
