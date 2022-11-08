use std::convert::{TryFrom, TryInto};
use std::path::Path;

use anyhow::{anyhow, Context, Result};
use broxus_util::serde_base64_array;
use everscale_network::proto;
use serde::{Deserialize, Deserializer};

#[derive(Clone)]
pub struct GlobalConfig {
    pub dht_nodes: Vec<proto::dht::NodeOwned>,
    pub zero_state: ton_block::BlockIdExt,
}

impl GlobalConfig {
    pub const MAINNET: &str = include_str!("mainnet.json");
    pub const TESTNET: &str = include_str!("testnet.json");

    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = std::fs::File::open(path).context("failed to open global config")?;
        let config = serde_json::from_reader(std::io::BufReader::new(file))
            .context("failed to deserialize global config")?;
        Ok(config)
    }
}

impl<'de> Deserialize<'de> for GlobalConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        GlobalConfigJson::deserialize(deserializer)?
            .try_into()
            .map_err(Error::custom)
    }
}

impl TryFrom<GlobalConfigJson> for GlobalConfig {
    type Error = anyhow::Error;

    fn try_from(value: GlobalConfigJson) -> Result<Self, Self::Error> {
        require_type(value.ty, "config.global")?;
        require_type(value.validator.ty, "validator.config.global")?;

        Ok(Self {
            dht_nodes: value.dht.try_into()?,
            zero_state: value.validator.zero_state.try_into()?,
        })
    }
}

impl TryFrom<DhtJson> for Vec<proto::dht::NodeOwned> {
    type Error = anyhow::Error;

    fn try_from(value: DhtJson) -> Result<Self, Self::Error> {
        require_type(value.ty, "dht.config.global")?;
        require_type(value.static_nodes.ty, "dht.nodes")?;
        value
            .static_nodes
            .nodes
            .into_iter()
            .map(TryFrom::try_from)
            .collect()
    }
}

impl TryFrom<DhtNodeJson> for proto::dht::NodeOwned {
    type Error = anyhow::Error;

    fn try_from(value: DhtNodeJson) -> Result<Self, Self::Error> {
        require_type(value.ty, "dht.node")?;
        require_type(value.id.ty, "pub.ed25519")?;

        Ok(Self {
            id: everscale_crypto::tl::PublicKeyOwned::Ed25519 { key: value.id.key },
            addr_list: value.addr_list.try_into()?,
            version: value.version as u32,
            signature: value.signature.to_vec().into(),
        })
    }
}

impl TryFrom<AddressListJson> for proto::adnl::AddressList {
    type Error = anyhow::Error;

    fn try_from(value: AddressListJson) -> Result<Self, Self::Error> {
        require_type(value.ty, "adnl.addressList")?;

        Ok(Self {
            address: value
                .addrs
                .into_iter()
                .next()
                .map(TryFrom::try_from)
                .transpose()?,
            version: value.version as u32,
            reinit_date: value.reinit_date as u32,
            expire_at: value.expire_at as u32,
        })
    }
}

impl TryFrom<AddressJson> for proto::adnl::Address {
    type Error = anyhow::Error;

    fn try_from(value: AddressJson) -> Result<Self, Self::Error> {
        require_type(value.ty, "adnl.address.udp")?;

        Ok(proto::adnl::Address {
            ip: value.ip as u32,
            port: value.port as u32,
        })
    }
}

impl TryFrom<BlockIdJson> for ton_block::BlockIdExt {
    type Error = anyhow::Error;

    fn try_from(value: BlockIdJson) -> Result<Self, Self::Error> {
        Ok(ton_block::BlockIdExt {
            shard_id: ton_block::ShardIdent::with_tagged_prefix(
                value.workchain,
                value.shard as u64,
            )?,
            seq_no: value.seqno as u32,
            root_hash: value.root_hash.into(),
            file_hash: value.file_hash.into(),
        })
    }
}

fn require_type(ty: String, required: &'static str) -> Result<()> {
    if ty == required {
        Ok(())
    } else {
        Err(anyhow!("Invalid type {ty}, expected {required}"))
    }
}

#[derive(Deserialize)]
struct GlobalConfigJson {
    #[serde(rename = "@type")]
    ty: String,
    dht: DhtJson,
    validator: ValidatorJson,
}

#[derive(Deserialize)]
struct DhtJson {
    #[serde(rename = "@type")]
    ty: String,
    static_nodes: StaticNodesJson,
}

#[derive(Deserialize)]
struct StaticNodesJson {
    #[serde(rename = "@type")]
    ty: String,
    nodes: Vec<DhtNodeJson>,
}

#[derive(Deserialize)]
struct DhtNodeJson {
    #[serde(rename = "@type")]
    ty: String,
    id: IdJson,
    addr_list: AddressListJson,
    version: i32,
    #[serde(with = "serde_base64_array")]
    signature: [u8; 64],
}

#[derive(Deserialize)]
struct IdJson {
    #[serde(rename = "@type")]
    ty: String,
    #[serde(with = "serde_base64_array")]
    key: [u8; 32],
}

#[derive(Deserialize)]
struct AddressListJson {
    #[serde(rename = "@type")]
    ty: String,
    addrs: Vec<AddressJson>,
    version: i32,
    reinit_date: i32,
    expire_at: i32,
}

#[derive(Deserialize)]
struct AddressJson {
    #[serde(rename = "@type")]
    ty: String,
    ip: i32,
    port: i32,
}

#[derive(Deserialize)]
struct ValidatorJson {
    #[serde(rename = "@type")]
    ty: String,
    zero_state: BlockIdJson,
}

#[derive(Deserialize)]
struct BlockIdJson {
    workchain: i32,
    shard: i64,
    seqno: i32,
    #[serde(with = "serde_base64_array")]
    root_hash: [u8; 32],
    #[serde(with = "serde_base64_array")]
    file_hash: [u8; 32],
}
