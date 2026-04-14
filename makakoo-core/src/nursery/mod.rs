//! Nursery subsystem — named mascots, buddy tracker, and patrol runner.
//!
//! Rust port of `core/buddy/nursery.py`, `core/buddy/buddy.py`, and
//! `core/agent/mascot.py` (Olibia persona). Three layers:
//!
//! * [`registry`] — persistent [`MascotRegistry`]: canonical mascots
//!   (Olibia, Pixel, Cinder, Ziggy, Glimmer) seeded on first load,
//!   contributor additions registered at runtime, JSON-backed with
//!   file locking for cross-process safety.
//! * [`buddy`] — [`BuddyTracker`]: which mascot is currently "active",
//!   its mood and energy, a CLI [`BuddyTracker::display_frame`] that
//!   renders the species via the T7 gimmick compositor.
//! * [`patrol`] — [`PatrolRunner`] + [`PatrolJob`] trait and two
//!   concrete examples: [`patrol::OlibiaGuardianPatrol`] checks today's
//!   Brain journal format, [`patrol::PixelLogPatrol`] greps recent log
//!   files for 401/403/ERROR patterns.

pub mod buddy;
pub mod patrol;
pub mod registry;

pub use buddy::{BuddyState, BuddyTracker};
pub use patrol::{
    OlibiaGuardianPatrol, PatrolContext, PatrolJob, PatrolReport, PatrolRunner, PixelLogPatrol,
};
pub use registry::{Mascot, MascotRegistry, MascotStatus, MascotVoice};
