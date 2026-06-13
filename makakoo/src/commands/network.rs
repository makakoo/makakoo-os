//! `makakoo network ...` — thin UX wrapper around the opt-in
//! `skill-brain-network` plugin.
//!
//! Brain federation is deliberately plugin-owned. The Rust CLI only
//! provides the ergonomic top-level verb and a clear install hint when
//! the plugin is absent.

use makakoo_core::plugin::PluginRegistry;

use crate::commands::skill;
use crate::context::CliContext;
use crate::output;

pub async fn run(args: Vec<String>, ctx: &CliContext) -> anyhow::Result<i32> {
    let registry = PluginRegistry::load_default(ctx.home()).unwrap_or_default();
    if registry.get("skill-brain-network").is_none() {
        output::print_error(
            concat!(
                "brain-network plugin not installed. Install it with: ",
                "makakoo distro install federation\n",
                "  or: makakoo plugin install --core skill-brain-network && ",
                "makakoo plugin install --core agent-octopus-peer"
            )
            .to_string(),
        );
        return Ok(1);
    }
    skill::run("brain-network", &args, ctx).await
}
