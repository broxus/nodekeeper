use std::net::SocketAddr;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use argh::FromArgs;

use super::CliContext;
use crate::exporter::{
    Exporter, ExporterTarget, FileExporterTarget, HttpExporterTarget, StdoutExporterTarget,
};

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
    pub async fn run(self, ctx: CliContext) -> Result<()> {
        let mut targets = Vec::<Box<dyn ExporterTarget>>::new();

        // Add file exporter if path specified
        if let Some(file) = self.file {
            targets.push(Box::new(FileExporterTarget::new(file)));
        }

        // Add network exporter
        if let Some(addr) = self.addr {
            if self.once {
                return Err(ExporterError::OnceNotSupported.into());
            }
            targets.push(Box::new(HttpExporterTarget::new(addr).await?));
        }

        // Fallback to stdout exporter
        if targets.is_empty() {
            targets.push(Box::new(StdoutExporterTarget));
        }

        let exporter = Exporter::new(ctx.dirs, targets);
        if self.once {
            exporter.once().await
        } else {
            let interval = Duration::from_secs(self.interval as u64);
            exporter.serve(interval).await;
            Ok(())
        }
    }
}

#[derive(thiserror::Error, Debug)]
enum ExporterError {
    #[error("once flag is not supported by http exporter")]
    OnceNotSupported,
}
