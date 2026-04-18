//! Concrete `CapabilityHandler` implementations.
//!
//! Phase E/3 wires three services the socket layer can serve:
//!
//! - [`composite::CompositeHandler`] — method-prefix demuxer. Composes
//!   sub-handlers so a single server socket can expose state + secrets
//!   without the socket core knowing which verb goes where.
//! - [`state::StateHandler`] — read/write/list within the plugin's own
//!   state dir. The path is jailed to `$MAKAKOO_HOME/state/<plugin>/`
//!   so grant-matching on `state/plugin` plus the jail together
//!   prevent directory traversal.
//! - [`secrets::SecretHandler`] — keyring / env / in-memory lookup of
//!   secret values. The verb is `secrets/read` with the scope encoding
//!   which key the plugin may ask for.
//!
//! Brain + LLM handlers are in the next slice (E/3b) — they reuse
//! existing `makakoo-core` subsystems so they ship separately from
//! the primitives here.

pub mod brain;
pub mod composite;
pub mod llm;
pub mod secrets;
pub mod state;

pub use brain::{append_journal_line, BrainHandler};
pub use composite::CompositeHandler;
pub use llm::LlmHandler;
pub use secrets::{EnvSecretBackend, InMemorySecretBackend, SecretBackend, SecretError, SecretHandler};
pub use state::{StateError, StateHandler};
