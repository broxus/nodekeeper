use std::net::SocketAddr;
use std::time::Duration;

use broxus_util::{const_duration_ms, serde_duration_ms};
use everscale_crypto::ed25519;
use serde::Deserialize;

use crate::util::{serde_public_key, serde_secret_key};

#[derive(Clone, Deserialize)]
pub struct Config {
    pub server_address: SocketAddr,
    #[serde(with = "serde_public_key")]
    pub server_pubkey: ed25519::PublicKey,
    #[serde(with = "serde_secret_key")]
    pub client_secret: ed25519::SecretKey,
    #[serde(with = "serde_duration_ms", default = "const_duration_ms::<2000>")]
    pub connection_timeout: Duration,
    #[serde(with = "serde_duration_ms", default = "const_duration_ms::<10000>")]
    pub query_timeout: Duration,
}
