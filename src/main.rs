use anyhow::Result;

mod cli;
mod config;
mod exporter;
mod global_config;
mod node_tcp_rpc;
mod node_udp_rpc;
mod subscription;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    argh::from_env::<cli::App>().run().await
}
