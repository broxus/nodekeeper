pub mod depool;
pub mod wallet;

#[derive(Clone)]
pub struct InternalMessage {
    pub dst: ton_block::MsgAddressInt,
    pub amount: u128,
    pub payload: ton_types::Cell,
}

pub const ONE_EVER: u128 = 1_000_000_000;
