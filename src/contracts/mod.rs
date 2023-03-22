pub use cluster::Cluster;
pub use depool::DePool;
pub use elector::Elector;
pub use strategy::Strategy;
pub use wallet::Wallet;

pub mod cluster;
pub mod depool;
pub mod elector;
pub mod strategy;
pub mod wallet;

#[derive(Clone)]
pub struct InternalMessage {
    pub dst: ton_block::MsgAddressInt,
    pub amount: u128,
    pub payload: ton_types::Cell,
}

impl InternalMessage {
    pub fn empty(dst: ton_block::MsgAddressInt, amount: u128) -> Self {
        Self {
            dst,
            amount,
            payload: Default::default(),
        }
    }
}

pub const ONE_EVER: u128 = 1_000_000_000;
