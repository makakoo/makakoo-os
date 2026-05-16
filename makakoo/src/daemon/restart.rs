//! `makakoo daemon restart` — re-registers and starts the daemon service.
//!
//! Why re-register instead of only sending a restart signal? Homebrew and
//! curl-pipe upgrades can move the `makakoo` binary. A launchd/systemd
//! descriptor written by an older version may point at the old Cellar or
//! prefix path. Re-installing the service descriptor before starting it makes
//! the daemon pick up the current binary, not merely restart the stale one.

use anyhow::Result;

use makakoo_platform::{CurrentPlatform, PlatformAdapter};

use super::status;

pub async fn run() -> Result<()> {
    let platform = CurrentPlatform::default();
    let was_installed = platform.daemon_is_installed();
    let was_running = platform.daemon_is_running();

    if was_installed {
        platform.daemon_uninstall()?;
    }

    let path = platform.daemon_install()?;

    if was_installed {
        println!(
            "restarted makakoo daemon service descriptor at {}",
            path.display()
        );
    } else {
        println!(
            "makakoo daemon was not installed; installed service descriptor at {}",
            path.display()
        );
    }

    if was_running {
        println!(
            "previous daemon was running; restart requested on {}",
            platform.name()
        );
    } else if was_installed {
        println!(
            "previous daemon was installed but stopped; start requested on {}",
            platform.name()
        );
    } else {
        println!("daemon start requested on {}", platform.name());
    }

    println!("status: {}", status::current_state().as_str());
    Ok(())
}
