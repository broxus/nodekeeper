pub mod depool;
pub mod wallet;

#[derive(Clone)]
pub struct InternalMessage {
    pub dst: ton_block::MsgAddressInt,
    pub amount: u64,
    pub payload: ton_types::Cell,
}

const ONE_EVER: u64 = 1_000_000_000;
