//! IPC layer between the per-slot Rust transport runtime and the
//! per-slot Python chat gateway.
//!
//! Wire protocol (locked, SPRINT.md "## IPC envelope schema"):
//!   - Unix-domain socket at `~/MAKAKOO/run/agents/<slot_id>/ipc.sock`
//!     (parent dir mode 0700).
//!   - Newline-delimited JSON: one envelope object per `\n`-terminated
//!     line.
//!   - At-most-once delivery. No frame ack, no IPC-layer retry.
//!     Drops on gateway-unavailable are logged with a structured
//!     `event = "ipc.gateway_unavailable"` line.
//!   - tokio per-stream write mutex prevents two transport tasks
//!     interleaving partial writes (see `unix_socket::IpcWriter`).
//!
//! Reuse from `MakakooFrame::to_line` / `from_line`.

pub mod unix_socket;
