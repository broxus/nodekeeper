use tl_proto::{TlRead, TlWrite};

#[derive(TlWrite, TlRead)]
#[tl(boxed, id = "tonNode.getCapabilities", scheme = "proto.tl")]
pub struct GetCapabilities;

#[derive(Debug, Copy, Clone, Eq, PartialEq, TlWrite, TlRead)]
#[tl(
    boxed,
    id = "tonNode.capabilities",
    size_hint = 12,
    scheme = "proto.tl"
)]
pub struct Capabilities {
    pub version: u32,
    pub capabilities: u64,
}
