//! Pattern dispatch substrate — `makakoo run pattern=NAME`.
//!
//! SPRINT-PATTERN-SUBSTRATE-V1. The composer (`composer.rs`), strategy
//! loader (`strategies.rs`), mascot loader (`mascots.rs`), and variable
//! substitution (`variables.rs`) are decoupled so:
//!
//!   1. The CLI verb at `makakoo/src/commands/run.rs` calls into them.
//!   2. The MCP auto-expose layer at `makakoo-mcp/src/patterns.rs`
//!      (Phase 5) reuses the same compose() function — no copy-paste.
//!
//! Composition algebra:
//!
//! ```text
//! prompt = strategy ⊕ mascot ⊕ pattern   (variables interpolated)
//! route  = vendor : model
//! ```
//!
//! This module owns the composition. The route resolver lives in
//! `resolver.rs` (Phase 3).

pub mod composer;
pub mod mascots;
pub mod resolver;
pub mod strategies;
pub mod variables;

pub use composer::{compose, ComposeError, ComposeRequest, ComposedPrompt};
pub use mascots::{load_mascot, MascotLoadError};
pub use resolver::{resolve_route, ResolvedRoute};
pub use strategies::{load_strategy, StrategyLoadError, BUILTIN_STRATEGIES};
pub use variables::{substitute, SubstitutionError};
