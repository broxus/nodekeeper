use std::collections::HashMap;
use std::net::SocketAddrV4;

use broxus_util::serde_base64_array;
use everscale_crypto::ed25519;
use serde::{Deserialize, Deserializer};

/// Validator node config
#[derive(Clone, Deserialize)]
pub struct NodeConfig {
    pub adnl_node: NodeConfigAdnl,
}

#[derive(Clone, Deserialize)]
pub struct NodeConfigAdnl {
    pub ip_address: SocketAddrV4,
    #[serde(deserialize_with = "serde_adnl_keys")]
    pub keys: Keys,
}

pub type Keys = HashMap<usize, ed25519::SecretKey>;

fn serde_adnl_keys<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Keys, D::Error> {
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

    Ok(Vec::<NodeConfigAdnlKeyInner>::deserialize(deserializer)?
        .into_iter()
        .map(|item| (item.tag, ed25519::SecretKey::from_bytes(item.data.pvt_key)))
        .collect())
}
