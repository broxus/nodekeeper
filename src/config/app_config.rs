use std::net::SocketAddrV4;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use broxus_util::{
    const_duration_ms, serde_duration_ms, serde_hex_array, serde_string, serde_string_or_number,
};
use everscale_crypto::ed25519;
use serde::{Deserialize, Serialize};

use crate::util::{serde_mc_address, serde_public_key, serde_secret_key};

/// Tool config
#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AppConfig {
    /// Control config
    pub control: Option<AppConfigControl>,
    /// ADNL config
    pub adnl: Option<AppConfigAdnl>,
    /// Validation config
    pub validation: Option<AppConfigValidation>,
}

impl AppConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let file = std::fs::File::open(path).context("failed to open app config")?;
        let d = &mut serde_json::Deserializer::from_reader(std::io::BufReader::new(file));
        serde_path_to_error::deserialize(d).context("failed to deserialize app config")
    }

    pub fn store<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let data = toml::to_string_pretty(self).context("failed to serialize config")?;
        std::fs::write(path, data).context("failed to save config")
    }

    pub fn control(&self) -> Result<&AppConfigControl> {
        self.control.as_ref().context("control config is empty")
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfigControl {
    /// Control server socket address
    pub server_address: SocketAddrV4,

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

impl AppConfigControl {
    pub fn from_addr_and_keys(
        addr: SocketAddrV4,
        server_key: ed25519::PublicKey,
        client_key: ed25519::SecretKey,
    ) -> Self {
        Self {
            server_address: addr,
            server_pubkey: server_key,
            client_secret: client_key,
            connection_timeout: Duration::from_millis(2000),
            query_timeout: Duration::from_millis(10000),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfigAdnl {
    /// Server ADNL address
    pub server_address: SocketAddrV4,

    /// Server overlay pubkey
    #[serde(with = "serde_public_key")]
    pub server_pubkey: ed25519::PublicKey,

    /// Zerostate file hash from the global config
    #[serde(with = "serde_hex_array")]
    pub zerostate_file_hash: [u8; 32],
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase", tag = "type")]
pub enum AppConfigValidation {
    Single {
        #[serde(with = "serde_mc_address")]
        address: ton_block::MsgAddressInt,
        #[serde(with = "serde_string_or_number")]
        stake: u64,
    },
    DePool {
        #[serde(with = "serde_string")]
        owner: ton_block::MsgAddressInt,
        #[serde(with = "serde_string")]
        depool: ton_block::MsgAddressInt,
        depool_type: DePoolType,
    },
}

#[derive(Copy, Clone, Serialize, Deserialize)]
pub enum DePoolType {
    #[serde(rename = "default_v3")]
    DefaultV3,
    #[serde(rename = "stever_v1")]
    StEver,
}
