//! Render context shared by every flue_scaffold submodule.

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use makakoo_core::agents::llm_provider::DiscoveredProvider;
use makakoo_core::agents::spec::AgentSpec;

/// Bundles the spec with its output directory. Every renderer takes
/// `&RenderContext` so they don't have to thread the spec and the
/// out_dir through every call.
pub struct RenderContext<'a> {
    pub spec: &'a AgentSpec,
    pub out_dir: &'a Path,
    /// Phase 6: the LLM provider chosen at `makakoo agent create`
    /// time. When `Some`, the scaffolder emits `src/app.ts` with the
    /// right `registerProvider` call. When `None`, the scaffolder
    /// skips `app.ts` and the caller should surface a clear error.
    pub llm_provider: Option<DiscoveredProvider>,
}

impl<'a> RenderContext<'a> {
    pub fn new(spec: &'a AgentSpec, out_dir: &'a Path) -> Self {
        Self { spec, out_dir, llm_provider: None }
    }

    pub fn with_provider(mut self, provider: DiscoveredProvider) -> Self {
        self.llm_provider = Some(provider);
        self
    }

    /// `package.json` `name` field. `<slot>-flue-agent` matches the
    /// historical convention.
    pub fn project_name(&self) -> String {
        format!("{}-flue-agent", self.spec.name)
    }

    /// Write `content` to `<out_dir>/<rel>`, creating parent dirs.
    pub fn write(&self, rel: &str, content: &str) -> Result<()> {
        let path = self.out_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating dir {}", parent.display()))?;
        }
        std::fs::write(&path, content)
            .with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Returns a relative path useful in `ImportError` or `println!`
    /// messages.
    pub fn rel(&self, p: &Path) -> PathBuf {
        p.strip_prefix(self.out_dir).unwrap_or(p).to_path_buf()
    }
}
