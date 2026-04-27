//! `ChannelGatewayAdapter` analog — receive inbound messages from a
//! transport and emit `MakakooInboundFrame` events.
//!
//! Locked seam from SPRINT-MULTI-BOT-SUBAGENTS Q12. Each adapter
//! implements its own poller (Telegram `getUpdates`) or websocket
//! listener (Slack Socket Mode).

use async_trait::async_trait;
use tokio::sync::mpsc;

use crate::transport::frame::MakakooInboundFrame;
use crate::Result;

/// Sender side of an inbound-frame channel. The gateway adapter
/// pushes frames here; the IPC layer drains them and writes to the
/// per-slot Unix socket.
pub type InboundSink = mpsc::Sender<MakakooInboundFrame>;

#[async_trait]
pub trait Gateway: Send + Sync {
    /// Start the inbound listener. Returns when the listener exits
    /// (e.g. SIGTERM).  Implementations must not panic on transient
    /// errors — they should log and reconnect.
    async fn start(&self, sink: InboundSink) -> Result<()>;
}
