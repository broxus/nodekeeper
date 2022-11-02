use anyhow::Result;

#[macro_export]
macro_rules! once {
    ($ty:path, || $expr:expr) => {{
        static ONCE: once_cell::race::OnceBox<$ty> = once_cell::race::OnceBox::new();
        ONCE.get_or_init(|| Box::new($expr))
    }};
}

mod cli;
mod config;
mod contracts;
mod crypto;
mod exporter;
mod node_tcp_rpc;
mod node_udp_rpc;
mod subscription;
mod util;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    argh::from_env::<cli::App>().run().await
}
