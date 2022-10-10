use std::net::{SocketAddr, SocketAddrV4};
use std::time::Duration;

use broxus_util::{const_duration_ms, serde_base64_array, serde_duration_ms};
use everscale_crypto::ed25519;
use serde::{Deserialize, Deserializer};

use crate::util::{serde_public_key, serde_secret_key};

/// Tool config
#[derive(Clone, Deserialize)]
pub struct Config {
    /// Control server socket address
    pub server_address: SocketAddr,

    /// Control server pubkey
    #[serde(with = "serde_public_key")]
    pub server_pubkey: ed25519::PublicKey,

    /// Control client pubkey
    #[serde(with = "serde_secret_key")]
    pub client_secret: ed25519::SecretKey,

    /// Control server connection timeout
    #[serde(with = "serde_duration_ms", default = "const_duration_ms::<2000>")]
    pub connection_timeout: Duration,

    /// Control server query timeout
    #[serde(with = "serde_duration_ms", default = "const_duration_ms::<10000>")]
    pub query_timeout: Duration,
}

/// Validator node config
#[derive(Clone, Deserialize)]
pub struct NodeConfig {
    pub adnl_node: NodeConfigAdnl,
}

#[derive(Clone, Deserialize)]
pub struct NodeConfigAdnl {
    pub ip_address: SocketAddrV4,
    pub keys: Vec<NodeConfigAdnlKey>,
}

#[derive(Copy, Clone)]
pub struct NodeConfigAdnlKey {
    pub tag: usize,
    pub key: [u8; 32],
}

impl<'de> Deserialize<'de> for NodeConfigAdnlKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct NodeConfigAdnlKeyInner {
            tag: usize,
            data: NodeConfigAdnlKeyData,
        }

        #[derive(Deserialize)]
        struct NodeConfigAdnlKeyData {
            #[serde(with = "serde_base64_array")]
            pvt_key: [u8; 32],
        }

        let inner = NodeConfigAdnlKeyInner::deserialize(deserializer)?;
        Ok(Self {
            tag: inner.tag,
            key: inner.data.pvt_key,
        })
    }
}
