use std::collections::HashMap;
use std::hash::BuildHasherDefault;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use broxus_util::serde_hex_array;
use dashmap::DashMap;
use rustc_hash::FxHashMap;
use serde::Deserialize;
use ton_block::Deserializable;

pub type FxDashMap<K, V> = DashMap<K, V, BuildHasherDefault<rustc_hash::FxHasher>>;

pub fn print_output<T: std::fmt::Display>(arg: T) {
    if atty::is(atty::Stream::Stdout) {
        writeln!(std::io::stdout(), "{arg:#}")
    } else {
        write!(std::io::stdout(), "{arg}")
    }
    .unwrap()
}

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

pub fn parse_optional_state_init(data: Option<String>) -> Result<Option<ton_block::StateInit>> {
    data.as_deref().map(parse_state_init).transpose()
}

pub fn parse_state_init(data: &str) -> Result<ton_block::StateInit> {
    ton_block::StateInit::construct_from_base64(data)
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

pub fn parse_keys(keys: Option<PathBuf>) -> Result<Option<ed25519_dalek::Keypair>> {
    #[derive(Deserialize)]
    struct StoredKeyPair {
        #[serde(with = "serde_hex_array")]
        secret: [u8; 32],
    }

    let path = match keys {
        Some(path) => path,
        None => return Ok(None),
    };

    let keypair = std::fs::read_to_string(path).context("failed to read keys")?;
    let keypair =
        serde_json::from_str::<StoredKeyPair>(&keypair).context("failed to parse keys")?;

    let secret =
        ed25519_dalek::SecretKey::from_bytes(&keypair.secret).context("invalid secret key")?;
    let public = ed25519_dalek::PublicKey::from(&secret);

    Ok(Some(ed25519_dalek::Keypair { secret, public }))
}

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

pub mod serde_optional_public_key {
    use everscale_crypto::ed25519;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::serde_public_key;

    pub fn serialize<S: Serializer>(
        public: &Option<ed25519::PublicKey>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Helper<'a>(#[serde(with = "serde_public_key")] &'a ed25519::PublicKey);

        public.as_ref().map(Helper).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<ed25519::PublicKey>, D::Error> {
        #[derive(Deserialize)]
        struct Helper(#[serde(with = "serde_public_key")] ed25519::PublicKey);

        match Option::<Helper>::deserialize(deserializer)? {
            Some(Helper(public)) => Ok(Some(public)),
            None => Ok(None),
        }
    }
}

pub mod serde_public_key {
    use everscale_crypto::ed25519;
    use serde::de::{Deserialize, Deserializer, Error};
    use serde::ser::Serializer;

    pub fn serialize<S: Serializer>(
        public: &ed25519::PublicKey,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(public.as_bytes()))
    }

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
    use serde::ser::Serializer;

    pub fn serialize<S: Serializer>(
        secret: &ed25519::SecretKey,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(secret.as_bytes()))
    }

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

#[derive(Clone)]
pub struct TransactionWithHash {
    pub hash: ton_types::UInt256,
    pub data: ton_block::Transaction,
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

    pub fn read_brief_info(&self) -> Result<BriefBlockInfo> {
        let info = self.block.read_info()?;

        let (prev1, prev2) = match info.read_prev_ref()? {
            ton_block::BlkPrevInfo::Block { prev } => {
                let shard_id = if info.after_split() {
                    info.shard().merge()?
                } else {
                    *info.shard()
                };

                let id = ton_block::BlockIdExt {
                    shard_id,
                    seq_no: prev.seq_no,
                    root_hash: prev.root_hash,
                    file_hash: prev.file_hash,
                };

                (id, None)
            }
            ton_block::BlkPrevInfo::Blocks { prev1, prev2 } => {
                let prev1 = prev1.read_struct()?;
                let prev2 = prev2.read_struct()?;
                let (shard1, shard2) = info.shard().split()?;

                let id1 = ton_block::BlockIdExt {
                    shard_id: shard1,
                    seq_no: prev1.seq_no,
                    root_hash: prev1.root_hash,
                    file_hash: prev1.file_hash,
                };

                let id2 = ton_block::BlockIdExt {
                    shard_id: shard2,
                    seq_no: prev2.seq_no,
                    root_hash: prev2.root_hash,
                    file_hash: prev2.file_hash,
                };

                (id1, Some(id2))
            }
        };

        Ok(BriefBlockInfo {
            gen_utime: info.gen_utime().0,
            prev1,
            prev2,
        })
    }

    pub fn shard_blocks(&self) -> Result<FxHashMap<ton_block::ShardIdent, ton_block::BlockIdExt>> {
        let mut shards = FxHashMap::default();
        self.block()
            .read_extra()?
            .read_custom()?
            .context("given block is not a masterchain block")?
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

    pub fn shard_blocks_seq_no(&self) -> Result<FxHashMap<ton_block::ShardIdent, u32>> {
        let mut shards = FxHashMap::default();
        self.block()
            .read_extra()?
            .read_custom()?
            .context("given block is not a masterchain block")?
            .hashes()
            .iterate_shards(|ident, descr| {
                shards.insert(ident, descr.seq_no);
                Ok(true)
            })?;

        Ok(shards)
    }
}

#[derive(Clone)]
pub struct BriefBlockInfo {
    pub gen_utime: u32,
    pub prev1: ton_block::BlockIdExt,
    pub prev2: Option<ton_block::BlockIdExt>,
}
