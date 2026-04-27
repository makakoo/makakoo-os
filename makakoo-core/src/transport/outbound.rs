//! `ChannelOutboundAdapter` analog — send a reply through this
//! transport.  The umbrella `Transport::send` wraps this so the
//! router does not need to know about adapter sub-traits in v1.

use async_trait::async_trait;

use crate::transport::frame::MakakooOutboundFrame;
use crate::Result;

#[async_trait]
pub trait Outbound: Send + Sync {
    async fn send_payload(&self, frame: &MakakooOutboundFrame) -> Result<()>;
}
