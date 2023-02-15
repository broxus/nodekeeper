use std::time::Duration;

use anyhow::Result;
use pomfrit::formatter::DisplayPrometheusExt;

pub use self::file_target::FileExporterTarget;
pub use self::http_target::HttpExporterTarget;
pub use self::stdout_target::StdoutExporterTarget;
use crate::config::{AppConfig, AppConfigValidator, DePoolType};
use crate::dirs::ProjectDirs;
use crate::network::{NodeStats, NodeTcpRpc, ValidatorSetEntry};

mod file_target;
mod http_target;
mod stdout_target;

pub struct Exporter {
    dirs: ProjectDirs,
    targets: Vec<Box<dyn ExporterTarget>>,
}

impl Exporter {
    pub fn new(dirs: ProjectDirs, targets: Vec<Box<dyn ExporterTarget>>) -> Self {
        Self { dirs, targets }
    }

    pub async fn serve(self, interval: Duration) {
        if self.targets.is_empty() {
            return;
        }

        let mut interval = tokio::time::interval(interval);
        loop {
            interval.tick().await;

            let (config, node_rpc) = match self.init_node_rpc().await {
                Ok(value) => value,
                Err((e, fallback)) => {
                    tracing::error!("failed to prepare exporter: {e:?}");
                    self.export(&fallback);
                    continue;
                }
            };

            if let Err(e) = self.collect(&config, &node_rpc).await {
                tracing::error!("failed to collect metrics: {e:?}");
            }
        }
    }

    pub async fn once(self) -> Result<()> {
        match self.init_node_rpc().await {
            Ok((config, node_rpc)) => self.collect(&config, &node_rpc).await,
            Err((e, _)) => Err(e),
        }
    }

    async fn collect(&self, config: &AppConfig, node_rpc: &NodeTcpRpc) -> Result<()> {
        let stats = node_rpc.get_stats().await?;
        let collected_at = broxus_util::now();

        tracing::debug!("collected node stats");

        let metrics = Metrics {
            collected_at,
            config,
            stats: &stats,
        };
        self.export(&metrics);

        Ok(())
    }

    fn export(&self, metrics: &dyn std::fmt::Display) {
        for target in &self.targets {
            if let Err(e) = target.write(metrics) {
                tracing::warn!(
                    "failed to write metrics to the {}: {e:?}",
                    target.target_name()
                );
            }
        }
    }

    async fn init_node_rpc(
        &self,
    ) -> Result<(AppConfig, NodeTcpRpc), (anyhow::Error, MetricsFallback)> {
        fn fallback<const V: bool>(e: anyhow::Error) -> (anyhow::Error, MetricsFallback) {
            (e, MetricsFallback { config_is_valid: V })
        }

        let config = AppConfig::load(&self.dirs.app_config).map_err(fallback::<false>)?;
        let control = config.control().map_err(fallback::<true>)?;
        let node_rpc = NodeTcpRpc::new(control).await.map_err(fallback::<true>)?;
        Ok((config, node_rpc))
    }
}

pub trait ExporterTarget {
    fn target_name(&self) -> &'static str;

    fn write(&self, metrics: &dyn std::fmt::Display) -> Result<()>;
}

struct MetricsFallback {
    config_is_valid: bool,
}

impl std::fmt::Display for MetricsFallback {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.begin_metric(CONFIG_IS_VALID)
            .value(self.config_is_valid as u8)?;
        f.begin_metric(EXPORTER_READY).value(0)
    }
}

#[derive(Copy, Clone)]
struct Metrics<'a> {
    collected_at: u32,
    config: &'a AppConfig,
    stats: &'a NodeStats,
}

impl std::fmt::Display for Metrics<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const NODE_READY: &str = "node_ready";
        const SYNC_STATUS: &str = "sync_status";

        f.begin_metric(CONFIG_IS_VALID).value(1)?;
        f.begin_metric(EXPORTER_READY).value(1)?;

        f.begin_metric("collected_at").value(self.collected_at)?;

        let stats = match self.stats {
            NodeStats::NotReady(sync_status) => {
                return f
                    .begin_metric(NODE_READY)
                    .label(SYNC_STATUS, sync_status)
                    .value(0)
            }
            NodeStats::Running(stats) => {
                f.begin_metric(NODE_READY)
                    .label(SYNC_STATUS, stats.sync_status)
                    .value(1)?;
                stats
            }
        };

        let node_version = &stats.node_version;

        f.begin_metric("node_version")
            .label(
                "version",
                format!(
                    "{}.{}.{}",
                    node_version.major, node_version.minor, node_version.patch
                ),
            )
            .value(0)?;

        f.begin_metric("node_version_major")
            .value(node_version.major)?;
        f.begin_metric("node_version_minor")
            .value(node_version.minor)?;
        f.begin_metric("node_version_patch")
            .value(node_version.patch)?;

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

        const VALIDATION_ENABLED: &str = "validation_enabled";
        const VALIDATOR_TYPE: &str = "validator_type";

        if let Some(validator) = &self.config.validator {
            f.begin_metric(VALIDATION_ENABLED).value(1)?;
            match validator {
                AppConfigValidator::Single(single) => {
                    f.begin_metric(VALIDATOR_TYPE).value(0)?;
                    f.begin_metric("validator_single_stake_per_round")
                        .label("validator", &single.address)
                        .value(single.stake_per_round)?;
                }
                AppConfigValidator::DePool(depool) => {
                    f.begin_metric(VALIDATOR_TYPE).value(1)?;
                    f.begin_metric("validator_depool_type")
                        .label("validator", &depool.owner)
                        .label("depool", &depool.depool)
                        .value(depool.depool_type.into_u8())?;
                }
            }
        } else {
            f.begin_metric(VALIDATION_ENABLED).value(0)?;
        }

        Ok(())
    }
}

impl DePoolType {
    fn into_u8(self) -> u8 {
        match self {
            Self::DefaultV3 => 0,
            Self::StEverV1 => 1,
            Self::StEverV2 => 2,
        }
    }
}

const CONFIG_IS_VALID: &str = "config_is_valid";
const EXPORTER_READY: &str = "exporter_ready";
