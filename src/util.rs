use std::hash::BuildHasherDefault;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use dashmap::DashMap;
use rustc_hash::FxHashMap;
use ton_block::Deserializable;

pub type FxDashMap<K, V> = DashMap<K, V, BuildHasherDefault<rustc_hash::FxHasher>>;

pub fn parse_contract_abi<P>(path: P) -> Result<ton_abi::Contract>
where
    P: AsRef<Path>,
{
    let data = std::fs::read(path.as_ref()).context("failed to read JSON ABI")?;
    let mut jd = serde_json::Deserializer::from_slice(&data);
    let contract: ton_abi::contract::SerdeContract =
        serde_path_to_error::deserialize(&mut jd).context("failed to parse JSON ABI")?;
    ton_abi::Contract::try_from(contract)
}

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

pub struct BlockStuff {
    id: ton_block::BlockIdExt,
    block: ton_block::Block,
}

impl BlockStuff {
    pub fn new(mut data: &[u8], id: ton_block::BlockIdExt) -> Result<Self> {
        let file_hash = ton_types::UInt256::calc_file_hash(data);
        anyhow::ensure!(id.file_hash() == file_hash, "wrong file_hash for {id}");

        let root = ton_types::deserialize_tree_of_cells(&mut data)?;
        anyhow::ensure!(
            id.root_hash() == root.repr_hash(),
            "wrong root hash for {id}"
        );

        let block = ton_block::Block::construct_from(&mut root.into())?;
        Ok(Self { id, block })
    }

    #[inline(always)]
    pub fn id(&self) -> &ton_block::BlockIdExt {
        &self.id
    }

    #[inline(always)]
    pub fn block(&self) -> &ton_block::Block {
        &self.block
    }

    pub fn shard_blocks(&self) -> Result<FxHashMap<ton_block::ShardIdent, ton_block::BlockIdExt>> {
        let mut shards = FxHashMap::default();
        self.block()
            .read_extra()?
            .read_custom()?
            .context("Given block is not a master block.")?
            .hashes()
            .iterate_shards(|ident, descr| {
                let last_shard_block = ton_block::BlockIdExt {
                    shard_id: ident,
                    seq_no: descr.seq_no,
                    root_hash: descr.root_hash,
                    file_hash: descr.file_hash,
                };
                shards.insert(ident, last_shard_block);
                Ok(true)
            })?;

        Ok(shards)
    }
}
