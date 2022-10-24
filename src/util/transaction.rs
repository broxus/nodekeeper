use std::collections::HashMap;

use anyhow::Result;

pub fn split_address(address: &ton_block::MsgAddressInt) -> Result<(i32, ton_types::UInt256)> {
    match address {
        ton_block::MsgAddressInt::AddrStd(ton_block::MsgAddrStd {
            workchain_id,
            address,
            ..
        }) => Ok((
            *workchain_id as _,
            ton_types::UInt256::from_slice(&address.get_bytestring_on_stack(0)),
        )),
        ton_block::MsgAddressInt::AddrVar(_) => Err(anyhow::anyhow!("unsupported address")),
    }
}

pub fn make_default_headers(
    pubkey: Option<ed25519_dalek::PublicKey>,
    timeout: u32,
) -> (u32, HashMap<String, ton_abi::TokenValue>) {
    let time = broxus_util::now_ms_u64();
    let expire_at = (time / 1000) as u32 + timeout;

    let headers = HashMap::from([
        ("time".to_owned(), ton_abi::TokenValue::Time(time)),
        ("expire".to_owned(), ton_abi::TokenValue::Expire(expire_at)),
        ("pubkey".to_owned(), ton_abi::TokenValue::PublicKey(pubkey)),
    ]);

    (expire_at, headers)
}

#[derive(Clone)]
pub struct TransactionWithHash {
    pub hash: ton_types::UInt256,
    pub data: ton_block::Transaction,
}
