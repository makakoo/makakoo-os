use std::path::Path;

use anyhow::Context as _;
use makakoo_core::agents::spec::AgentSpec;

pub struct RenderContext<'a> {
    pub spec: &'a AgentSpec,
    pub out_dir: &'a Path,
}

impl RenderContext<'_> {
    pub fn write(&self, relative: &str, body: &str) -> anyhow::Result<()> {
        let path = self.out_dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        std::fs::write(&path, body).with_context(|| format!("write {}", path.display()))
    }

    pub fn write_private(&self, relative: &str, body: &str) -> anyhow::Result<()> {
        let path = self.out_dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create {}", parent.display()))?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&path)
            .with_context(|| format!("open private {}", path.display()))?;
        use std::io::Write as _;
        file.write_all(body.as_bytes())
            .with_context(|| format!("write private {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("chmod 0600 {}", path.display()))?;
        }
        Ok(())
    }
}
