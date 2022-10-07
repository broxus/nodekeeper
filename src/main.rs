use anyhow::Result;

mod cli;
mod config;
mod exporter;
mod node_rpc;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    argh::from_env::<cli::App>().run().await
}
