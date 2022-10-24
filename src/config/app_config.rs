use std::net::SocketAddrV4;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use broxus_util::{const_duration_ms, serde_duration_ms};
use everscale_crypto::ed25519;
use serde::{Deserialize, Serialize};

use crate::util::{serde_public_key, serde_secret_key};

/// Tool config
#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Control config
    pub control: Option<AppConfigControl>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { control: None }
    }
}

impl AppConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        config::Config::builder()
            .add_source(config::File::from(path.as_ref()))
            .add_source(config::Environment::default())
            .build()
            .context("failed to build config")?
            .try_deserialize()
            .context("failed to parse config")
    }

    pub fn store<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let data = match path.as_ref().extension() {
            Some(ext) if ext == "json" => {
                serde_json::to_string_pretty(self).context("failed to serialize config")?
            }
            Some(ext) if ext == "yaml" => {
                serde_yaml::to_string(self).context("failed to serialize config")?
            }
            _ => anyhow::bail!("unknown config format"),
        };
        std::fs::write(path, data).context("failed to save config")
    }

    pub fn control(&self) -> Result<&AppConfigControl> {
        self.control.as_ref().context("control config is empty")
    }
}

#[derive(Clone, Serialize, Deserialize)]
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

/*
    /// Server ADNL address
    #[serde(default)]
    pub server_adnl_address: Option<SocketAddrV4>,

    /// Server ADNL overlay pubkey
    #[serde(default, with = "serde_optional_public_key")]
    pub server_adnl_pubkey: Option<ed25519::PublicKey>,

    /// Zerostate file hash
    #[serde(default)]
    pub zerostate_file_hash: Option<[u8; 32]>,
*/
