use std::io::Write;
use std::net::SocketAddr;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use pomfrit::formatter::DisplayPrometheusExt;

use crate::node_rpc::{NodeRpc, NodeStats, ValidatorSetEntry};

pub struct Exporter {
    node_rpc: NodeRpc,
    interval: Duration,
    targets: Vec<Box<dyn ExporterTarget>>,
}

impl Exporter {
    pub fn new(
        node_rpc: NodeRpc,
        interval: Duration,
        targets: Vec<Box<dyn ExporterTarget>>,
    ) -> Self {
        Self {
            node_rpc,
            interval,
            targets,
        }
    }

    pub async fn serve(self) {
        if self.targets.is_empty() {
            return;
        }

        let mut interval = tokio::time::interval(self.interval);
        loop {
            interval.tick().await;

            let stats = match self.node_rpc.get_stats().await {
                Ok(stats) => stats,
                Err(e) => {
                    tracing::warn!("failed to collect stats: {e:?}");
                    continue;
                }
            };
            let collected_at = broxus_util::now();

            tracing::debug!("collected node stats");

            let metrics = Metrics {
                collected_at,
                stats: &stats,
            };

            for target in &self.targets {
                if let Err(e) = target.write(metrics) {
                    tracing::warn!(
                        "failed to write metrics to the {}: {e:?}",
                        target.target_name()
                    );
                }
            }
        }
    }
}

pub trait ExporterTarget {
    fn target_name(&self) -> &'static str;

    fn write(&self, metrics: Metrics<'_>) -> Result<()>;
}

#[derive(Copy, Clone)]
pub struct Metrics<'a> {
    pub collected_at: u32,
    pub stats: &'a NodeStats,
}

impl std::fmt::Display for Metrics<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const NODE_READY: &str = "node_ready";

        f.begin_metric("collected_at").value(self.collected_at)?;

        let stats = match self.stats {
            NodeStats::NotReady => return f.begin_metric(NODE_READY).value(0),
            NodeStats::Running(stats) => {
                f.begin_metric(NODE_READY).value(1)?;
                stats
            }
        };

        f.begin_metric("node_version_major")
            .value(stats.node_version.major)?;
        f.begin_metric("node_version_minor")
            .value(stats.node_version.minor)?;
        f.begin_metric("node_version_patch")
            .value(stats.node_version.patch)?;

        f.begin_metric("mc_seqno")
            .value(stats.last_mc_block.seq_no)?;

        f.begin_metric("mc_time").value(stats.mc_time)?;
        f.begin_metric("mc_time_diff").value(stats.mc_time_diff)?;
        f.begin_metric("sc_time_diff").value(stats.sc_time_diff)?;

        const ADNL_LABEL: &str = "adnl";

        const IN_CURRENT_VSET: &str = "in_current_vset";
        match &stats.in_current_vset {
            ValidatorSetEntry::None => {
                f.begin_metric(IN_CURRENT_VSET).value(0)?;
            }
            ValidatorSetEntry::Validator(adnl) => f
                .begin_metric(IN_CURRENT_VSET)
                .label(ADNL_LABEL, hex::encode(adnl))
                .value(1)?,
        };

        const IN_NEXT_VSET: &str = "in_next_vset";
        match &stats.in_next_vset {
            ValidatorSetEntry::None => {
                f.begin_metric(IN_NEXT_VSET).value(0)?;
            }
            ValidatorSetEntry::Validator(adnl) => f
                .begin_metric(IN_NEXT_VSET)
                .label(ADNL_LABEL, hex::encode(adnl))
                .value(1)?,
        };

        Ok(())
    }
}

pub struct HttpExporterTarget {
    state: Arc<parking_lot::RwLock<Option<(u32, NodeStats)>>>,
    _exporter: Arc<pomfrit::MetricsExporter>,
}

impl HttpExporterTarget {
    pub async fn new(addr: SocketAddr) -> Result<Self> {
        let (exporter, writer) = pomfrit::create_exporter(Some(pomfrit::Config {
            collection_interval_sec: 1,
            listen_address: addr,
            metrics_path: None,
        }))
        .await?;

        let state = Arc::new(parking_lot::RwLock::default());
        writer.spawn({
            let state = state.clone();
            move |writer| {
                let metrics = state.read();
                if let Some((collected_at, stats)) = &*metrics {
                    writer.write(Metrics {
                        collected_at: *collected_at,
                        stats,
                    });
                }
            }
        });

        Ok(Self {
            state,
            _exporter: exporter,
        })
    }
}

impl ExporterTarget for HttpExporterTarget {
    fn target_name(&self) -> &'static str {
        "http_exporter"
    }

    fn write(&self, metrics: Metrics<'_>) -> Result<()> {
        let mut state = self.state.write();
        *state = Some((metrics.collected_at, metrics.stats.clone()));
        Ok(())
    }
}

pub struct FileExporterTarget {
    file_path: PathBuf,
    temp_file_path: PathBuf,
}

impl FileExporterTarget {
    pub fn new(path: PathBuf) -> Self {
        let mut temp_extension = path.extension().unwrap_or_default().to_os_string();
        temp_extension.push(std::ffi::OsString::from("temp"));

        let mut temp_file_path = path.clone();
        temp_file_path.set_extension(temp_extension);

        Self {
            file_path: path,
            temp_file_path,
        }
    }
}

impl ExporterTarget for FileExporterTarget {
    fn target_name(&self) -> &'static str {
        "file_exporter"
    }

    fn write(&self, metrics: Metrics<'_>) -> Result<()> {
        let mut temp_file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .mode(0o644)
            .open(&self.temp_file_path)?;

        write!(temp_file, "{metrics}")?;
        drop(temp_file);

        std::fs::rename(&self.temp_file_path, &self.file_path)?;
        Ok(())
    }
}
