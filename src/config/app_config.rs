use std::net::SocketAddrV4;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use broxus_util::{
    const_duration_ms, serde_duration_ms, serde_hex_array, serde_optional_string, serde_string,
    serde_string_or_number,
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
    pub validator: Option<AppConfigValidator>,
}

impl AppConfig {
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(path).context("failed to read app config")?;
        toml::from_str(&content).context("failed to deserialize app config")
    }

    pub fn store<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let data = toml::to_string_pretty(self).context("failed to serialize config")?;
        std::fs::write(path, data).context("failed to save config")
    }

    pub fn control(&self) -> Result<&AppConfigControl> {
        self.control.as_ref().context("control config is empty")
    }

    pub fn adnl(&self) -> Result<&AppConfigAdnl> {
        self.adnl.as_ref().context("adnl config is empty")
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
    /// Local ADNL port
    pub client_port: u16,

    /// Server ADNL address
    pub server_address: SocketAddrV4,

    /// Server overlay pubkey
    #[serde(with = "serde_public_key")]
    pub server_pubkey: ed25519::PublicKey,

    /// Zerostate file hash from the global config
    #[serde(with = "serde_hex_array")]
    pub zerostate_file_hash: [u8; 32],
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase", tag = "type")]
pub enum AppConfigValidator {
    Single(AppConfigValidatorSingle),
    DePool(Box<AppConfigValidatorDePool>),
}

impl AppConfigValidator {
    pub fn is_single(&self) -> bool {
        matches!(self, Self::Single(_))
    }
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfigValidatorSingle {
    #[serde(with = "serde_mc_address")]
    pub address: ton_block::MsgAddressInt,
    #[serde(with = "serde_string_or_number")]
    pub stake_per_round: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stake_factor: Option<u32>,
    #[serde(
        default,
        skip_serializing_if = "AppConfigValidatorSigner::is_empty_simple"
    )]
    pub signer: AppConfigValidatorSigner,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppConfigValidatorDePool {
    #[serde(with = "serde_string")]
    pub owner: ton_block::MsgAddressInt,
    #[serde(with = "serde_string")]
    pub depool: ton_block::MsgAddressInt,
    pub depool_type: DePoolType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stake_factor: Option<u32>,
    #[serde(
        default,
        with = "serde_optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub strategy_factory: Option<ton_block::MsgAddressInt>,
    #[serde(
        default,
        with = "serde_optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    pub strategy: Option<ton_block::MsgAddressInt>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deploy: Option<AppConfigDePoolDeploymentParams>,
    #[serde(
        default,
        skip_serializing_if = "AppConfigValidatorSigner::is_empty_simple"
    )]
    pub signer: AppConfigValidatorSigner,
}

#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
pub struct AppConfigDePoolDeploymentParams {
    #[serde(with = "serde_string_or_number")]
    pub min_stake: u64,
    #[serde(with = "serde_string_or_number")]
    pub validator_assurance: u64,
    pub participant_reward_fraction: u8,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum DePoolType {
    #[serde(rename = "default_v3")]
    DefaultV3,
    #[serde(rename = "stever_v1")]
    StEverV1,
    #[serde(rename = "stever_v2")]
    StEverV2,
}

impl DePoolType {
    pub const LATEST_STEVER: Self = Self::StEverV2;

    pub fn is_default(&self) -> bool {
        matches!(self, Self::DefaultV3)
    }

    pub fn is_stever(&self) -> bool {
        !self.is_default()
    }
}

#[derive(Debug, Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum AppConfigValidatorSigner {
    Simple {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key: Option<PathBuf>,
    },
    Multisig {
        wallet_type: MultisigType,
        custodians: Vec<PathBuf>,
    },
}

impl Default for AppConfigValidatorSigner {
    fn default() -> Self {
        Self::Simple { key: None }
    }
}

impl AppConfigValidatorSigner {
    fn is_empty_simple(&self) -> bool {
        matches!(self, Self::Simple { key: None })
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Serialize, Deserialize)]
pub enum MultisigType {
    #[serde(rename = "old_multisig")]
    OldMultisig,
}
