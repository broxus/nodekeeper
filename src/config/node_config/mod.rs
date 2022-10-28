use std::collections::HashMap;
use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use broxus_util::serde_base64_array;
use everscale_crypto::ed25519;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Validator node config
#[derive(Clone, Serialize, Deserialize)]
pub struct NodeConfig(serde_json::Value);

impl NodeConfig {
    const IP_ADDRESS: &str = "ip_address";
    const CONTROL_SERVER: &str = "control_server";
    const CONTROL_SERVER_PORT: &str = "control_server_port";
    const ADNL_NODE: &str = "adnl_node";
    const GLOBAL_CONFIG_PATH: &str = "ton_global_config_name";
    const INTERNAL_DB_PATH: &str = "internal_db_path";

    const TEMPLATE: &str = include_str!("default_config.json");

    pub fn generate() -> Result<Self> {
        serde_json::from_str(Self::TEMPLATE).context("failed to generate node config")
    }

    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = std::fs::File::open(path).context("failed to open node config")?;
        let config = serde_json::from_reader(std::io::BufReader::new(file))
            .context("failed to deserialize node config")?;
        Ok(config)
    }

    pub fn store<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let data = serde_json::to_string_pretty(self).context("failed to serialize node config")?;
        std::fs::write(path, data).context("failed to write node config")
    }

    pub fn set_global_config_path<P: AsRef<Path>>(&mut self, path: P) -> Result<()> {
        self.set_field(Self::GLOBAL_CONFIG_PATH, path.as_ref())
    }

    pub fn get_internal_db_path(&self) -> Result<Option<PathBuf>> {
        self.get_field(Self::INTERNAL_DB_PATH)
    }

    pub fn set_internal_db_path(&mut self, path: &str) -> Result<()> {
        self.set_field(Self::INTERNAL_DB_PATH, path)
    }

    pub fn get_suggested_adnl_port(&self) -> Option<u16> {
        self.get_field::<SocketAddrV4>(Self::IP_ADDRESS)
            .ok()
            .flatten()
            .map(|addr| addr.port())
    }

    pub fn get_suggested_control_port(&self) -> Option<u16> {
        self.get_field(Self::CONTROL_SERVER_PORT).ok().flatten()
    }

    pub fn get_adnl_node(&self) -> Result<Option<NodeConfigAdnl>> {
        self.get_field(Self::ADNL_NODE)
    }

    pub fn set_adnl_node(&mut self, node: &NodeConfigAdnl) -> Result<()> {
        self.set_field(Self::ADNL_NODE, node)
    }

    pub fn get_control_server(&self) -> Result<Option<NodeConfigControlServer>> {
        self.get_field(Self::CONTROL_SERVER)
    }

    pub fn set_control_server(&mut self, node: &NodeConfigControlServer) -> Result<()> {
        self.set_field(Self::CONTROL_SERVER, node)
    }

    fn get_field<D>(&self, field: &str) -> Result<Option<D>>
    where
        for<'de> D: Deserialize<'de>,
    {
        match self.0.get(field).cloned() {
            Some(value) => Ok(serde_json::from_value(value)?),
            None => Ok(None),
        }
    }

    fn set_field<S>(&mut self, field: &str, value: &S) -> Result<()>
    where
        S: Serialize + ?Sized,
    {
        let value = serde_json::to_value(value)?;
        let config = self
            .0
            .as_object_mut()
            .ok_or(NodeConfigError::InvalidConfig)?;
        config.insert(field.to_owned(), value);
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
pub struct NodeConfigControlServer {
    pub address: SocketAddrV4,
    #[serde(with = "serde_control_clients")]
    pub clients: Clients,
    #[serde(with = "serde_node_secret_key")]
    pub server_key: ed25519::SecretKey,
    pub timeouts: Option<NodeConfigControlServerTimeouts>,
}

impl NodeConfigControlServer {
    pub fn from_addr_and_keys(
        addr: SocketAddrV4,
        server_key: ed25519::SecretKey,
        client_key: ed25519::PublicKey,
    ) -> Self {
        Self {
            address: addr,
            clients: Some(vec![client_key]),
            server_key,
            timeouts: None,
        }
    }
}

pub type Clients = Option<Vec<ed25519::PublicKey>>;

#[derive(Serialize, Deserialize)]
pub struct NodeConfigControlServerTimeouts {
    pub read: Duration,
    pub write: Duration,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct NodeConfigAdnl {
    pub ip_address: SocketAddrV4,
    #[serde(with = "serde_adnl_keys")]
    pub keys: Keys,
    #[serde(default)]
    pub recv_pipeline_pool: Option<u8>,
    #[serde(default)]
    pub recv_priority_pool: Option<u8>,
    #[serde(default)]
    pub throughput: Option<u32>,
}

impl NodeConfigAdnl {
    pub const DHT_TAG: usize = 1;
    pub const OVERLAY_TAG: usize = 2;

    pub fn from_addr_and_keys(addr: SocketAddrV4, keys: Keys) -> Self {
        Self {
            ip_address: addr,
            keys,
            recv_pipeline_pool: None,
            recv_priority_pool: None,
            throughput: None,
        }
    }

    pub fn generate_keys() -> Keys {
        let rng = &mut rand::thread_rng();
        HashMap::from([
            (Self::DHT_TAG, ed25519::SecretKey::generate(rng)),
            (Self::OVERLAY_TAG, ed25519::SecretKey::generate(rng)),
        ])
    }

    pub fn overlay_pubkey(&self) -> Result<ed25519::PublicKey> {
        self.keys
            .get(&Self::OVERLAY_TAG)
            .map(ed25519::PublicKey::from)
            .context("overlay key not found")
            .context("invalid ADNL node config")
    }
}

pub type Keys = HashMap<usize, ed25519::SecretKey>;

mod serde_control_clients {
    use super::*;

    pub fn serialize<S>(value: &Clients, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Item<'a>(#[serde(with = "serde_node_public_key")] &'a ed25519::PublicKey);

        struct List<'a>(&'a [ed25519::PublicKey]);

        impl Serialize for List<'_> {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                use serde::ser::SerializeSeq;

                let mut seq = serializer.serialize_seq(Some(self.0.len()))?;
                for pubkey in self.0 {
                    seq.serialize_element(&Item(pubkey))?;
                }
                seq.end()
            }
        }

        const NAME: &str = "NodeConfigControlClients";
        match value {
            None => serializer.serialize_unit_variant(NAME, 0, "any"),
            Some(clients) => serializer.serialize_newtype_variant(NAME, 1, "list", &List(clients)),
        }
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Clients, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[repr(transparent)]
        struct Item(#[serde(with = "serde_node_public_key")] ed25519::PublicKey);

        #[derive(Deserialize)]
        #[serde(rename_all = "lowercase")]
        enum NodeConfigControlClients {
            Any,
            List(Vec<Item>),
        }

        match NodeConfigControlClients::deserialize(deserializer)? {
            NodeConfigControlClients::Any => Ok(None),
            NodeConfigControlClients::List(clients) => Ok(Some(
                clients.into_iter().map(|Item(pubkey)| pubkey).collect(),
            )),
        }
    }
}

mod serde_node_public_key {
    use super::*;

    pub fn serialize<S>(value: &ed25519::PublicKey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Data<'a> {
            type_id: u32,
            #[serde(with = "serde_base64_array")]
            pub_key: &'a [u8; 32],
            pvt_key: (),
        }

        Data {
            type_id: everscale_crypto::tl::PublicKey::TL_ID_ED25519,
            pub_key: value.as_bytes(),
            pvt_key: (),
        }
        .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<ed25519::PublicKey, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::Error;

        #[derive(Deserialize)]
        struct Data {
            #[serde(with = "serde_base64_array")]
            pub_key: [u8; 32],
        }

        ed25519::PublicKey::from_bytes(Data::deserialize(deserializer)?.pub_key)
            .ok_or_else(|| Error::custom("invalid pubkey"))
    }
}

mod serde_adnl_keys {
    use super::*;

    pub fn serialize<S>(value: &Keys, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;

        #[derive(Serialize)]
        struct NodeConfigAdnlKeyInner<'a> {
            tag: usize,
            #[serde(with = "serde_node_secret_key")]
            data: &'a ed25519::SecretKey,
        }

        let mut seq = serializer.serialize_seq(Some(value.len()))?;
        for (tag, secret) in value {
            seq.serialize_element(&NodeConfigAdnlKeyInner {
                tag: *tag,
                data: secret,
            })?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Keys, D::Error> {
        #[derive(Deserialize)]
        struct NodeConfigAdnlKeyInner {
            tag: usize,
            #[serde(with = "serde_node_secret_key")]
            data: ed25519::SecretKey,
        }

        Ok(Vec::<NodeConfigAdnlKeyInner>::deserialize(deserializer)?
            .into_iter()
            .map(|item| (item.tag, item.data))
            .collect())
    }
}

mod serde_node_secret_key {
    use super::*;

    pub fn serialize<S>(value: &ed25519::SecretKey, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        #[derive(Serialize)]
        struct Data<'a> {
            type_id: u32,
            pub_key: (),
            #[serde(with = "serde_base64_array")]
            pvt_key: &'a [u8; 32],
        }

        Data {
            type_id: everscale_crypto::tl::PublicKey::TL_ID_ED25519,
            pub_key: (),
            pvt_key: value.as_bytes(),
        }
        .serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<ed25519::SecretKey, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Data {
            #[serde(with = "serde_base64_array")]
            pvt_key: [u8; 32],
        }

        Ok(ed25519::SecretKey::from_bytes(
            Data::deserialize(deserializer)?.pvt_key,
        ))
    }
}

#[derive(thiserror::Error, Debug)]
enum NodeConfigError {
    #[error("invalid node config")]
    InvalidConfig,
}
