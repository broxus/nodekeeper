use std::net::{SocketAddr, SocketAddrV4};
use std::time::Duration;

use broxus_util::{const_duration_ms, serde_duration_ms};
use everscale_crypto::ed25519;
use serde::{Deserialize, Serialize};

use crate::util::*;

/// Tool config
#[derive(Clone, Serialize, Deserialize)]
pub struct AppConfig {
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

    /// Server ADNL address
    pub server_adnl_address: Option<SocketAddrV4>,

    /// Server ADNL overlay pubkey
    #[serde(with = "serde_optional_public_key")]
    pub server_adnl_pubkey: Option<ed25519::PublicKey>,

    /// Zerostate file hash
    pub zerostate_file_hash: Option<[u8; 32]>,
}
