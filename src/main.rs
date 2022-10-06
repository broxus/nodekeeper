use std::time::Duration;

use anyhow::{Context, Result};
use everscale_crypto::ed25519;

use crate::config::Config;
use crate::node_rpc::NodeRpc;

mod config;
mod node_rpc;
mod tcp_adnl;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let node_rpc = NodeRpc::new(&Config {
        server_address: "127.0.0.1:5031".parse()?,
        server_pubkey: ed25519::PublicKey::from_bytes(
            base64::decode("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")?
                .as_slice()
                .try_into()?,
        )
        .context("Invalid pubkey")?,
        client_secret: ed25519::SecretKey::from_bytes(
            base64::decode("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")?
                .as_slice()
                .try_into()?,
        ),
        query_timeout: Duration::from_secs(1),
    })
    .await?;

    node_rpc.get_stats().await?;

    Ok(())
}
