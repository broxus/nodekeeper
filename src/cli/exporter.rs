use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use argh::FromArgs;

use super::CliContext;
use crate::exporter::{Exporter, ExporterTarget, FileExporterTarget, HttpExporterTarget, Metrics};
use crate::node_tcp_rpc::NodeTcpRpc;

#[derive(FromArgs)]
/// Prometheus metrics exporter
#[argh(subcommand, name = "exporter")]
pub struct Cmd {
    /// socket addr to host the exporter
    #[argh(option, short = 'a')]
    addr: Option<SocketAddr>,

    /// path to the file where the metrics are written
    #[argh(option, short = 'f')]
    file: Option<PathBuf>,

    /// whether to run exporter once
    #[argh(switch)]
    once: bool,

    /// metrics collection interval (in seconds). 10 seconds default
    #[argh(option, short = 'i', default = "10")]
    interval: u32,
}

impl Cmd {
    pub async fn run(self, mut ctx: CliContext) -> Result<()> {
        let config = ctx.load_config()?;
        let node_rpc = NodeTcpRpc::new(&config).await?;

        match (self.once, self.file, self.addr) {
            (_, None, None) => Err(ExporterError::NoExporters.into()),
            (true, _, Some(_)) => Err(ExporterError::OnceNotSupported.into()),
            (true, Some(file), _) => {
                let exporter = FileExporterTarget::new(file);
                let stats = node_rpc.get_stats().await?;
                let metrics = Metrics {
                    collected_at: broxus_util::now(),
                    stats: &stats,
                };
                exporter.write(&metrics)
            }
            (false, file, addr) => {
                let mut targets = Vec::<Box<dyn ExporterTarget>>::new();
                if let Some(file) = file {
                    targets.push(Box::new(FileExporterTarget::new(file)));
                }
                if let Some(addr) = addr {
                    targets.push(Box::new(HttpExporterTarget::new(addr).await?));
                }

                let interval = Duration::from_secs(self.interval as u64);
                Exporter::new(node_rpc, interval, targets).serve().await;
                Ok(())
            }
        }
    }
}

#[derive(thiserror::Error, Debug)]
enum ExporterError {
    #[error("no exporters specified")]
    NoExporters,
    #[error("once flag is not supported for http exporter")]
    OnceNotSupported,
}
