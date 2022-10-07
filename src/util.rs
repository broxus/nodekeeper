use std::hash::BuildHasherDefault;
use std::io::Read;
use std::str::FromStr;

use anyhow::{Context, Result};
use dashmap::DashMap;

pub type FxDashMap<K, V> = DashMap<K, V, BuildHasherDefault<rustc_hash::FxHasher>>;

pub fn parse_address(address: &str) -> Result<ton_block::MsgAddressInt> {
    ton_block::MsgAddressInt::from_str(address).map_err(From::from)
}

pub fn parse_optional_input(data: Option<String>, raw: bool) -> Result<Vec<u8>> {
    match data {
        Some(data) if raw => Ok(data.into()),
        Some(data) => parse_hex_or_base64(&data),
        None => {
            let mut data = Vec::new();
            std::io::stdin()
                .read_to_end(&mut data)
                .context("failed to read from stdin")?;
            Ok(data)
        }
    }
}

pub fn parse_key_hash(hash: &str) -> Result<[u8; 32]> {
    let data = parse_hex_or_base64(hash).context("invalid key hash")?;
    data.try_into()
        .map_err(|_| anyhow::Error::msg("invalid key hash length"))
}

pub fn parse_hex_or_base64(data: &str) -> Result<Vec<u8>> {
    if let Some(hash) = data.strip_prefix("0x") {
        hex::decode(hash).map_err(From::from)
    } else {
        match hex::decode(data) {
            Ok(bytes) => Ok(bytes),
            Err(e) => match base64::decode(data) {
                Ok(bytes) => Ok(bytes),
                _ => Err(e.into()),
            },
        }
    }
}

pub mod serde_public_key {
    use everscale_crypto::ed25519;
    use serde::de::{Deserialize, Deserializer, Error};

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ed25519::PublicKey, D::Error> {
        let str = String::deserialize(deserializer)?;
        let bytes = match hex::decode(&str) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            _ => match base64::decode(&str) {
                Ok(bytes) => bytes,
                Err(_) => return Err(Error::custom("invalid pubkey string")),
            },
        };

        let bytes = bytes
            .try_into()
            .map_err(|_| Error::custom("invalid pubkey length"))?;

        ed25519::PublicKey::from_bytes(bytes).ok_or_else(|| Error::custom("invalid pubkey"))
    }
}

pub mod serde_secret_key {
    use everscale_crypto::ed25519;
    use serde::de::{Deserialize, Deserializer, Error};

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ed25519::SecretKey, D::Error> {
        let str = String::deserialize(deserializer)?;
        let bytes = match hex::decode(&str) {
            Ok(bytes) if bytes.len() == 32 => bytes,
            _ => match base64::decode(&str) {
                Ok(bytes) => bytes,
                Err(_) => return Err(Error::custom("invalid secret key string")),
            },
        };

        let bytes = bytes
            .try_into()
            .map_err(|_| Error::custom("invalid secret key length"))?;

        Ok(ed25519::SecretKey::from_bytes(bytes))
    }
}

pub mod serde_block_id {
    use serde::ser::Serializer;

    pub fn serialize<S: Serializer>(
        block_id: &ton_block::BlockIdExt,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&format!(
            "{}:{:016x}:{}:{:x}:{:x}",
            block_id.shard_id.workchain_id(),
            block_id.shard_id.shard_prefix_with_tag(),
            block_id.seq_no,
            block_id.root_hash,
            block_id.file_hash
        ))
    }
}
