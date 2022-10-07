use anyhow::Result;

mod cli;
mod config;
mod node_rpc;
mod tcp_adnl;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    argh::from_env::<cli::App>().run().await
}
