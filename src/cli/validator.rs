use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use argh::FromArgs;
use ton_executor::blockchain_config;

use crate::node_tcp_rpc::{ConfigWithId, NodeStats, NodeTcpRpc, RunningStats};
use crate::node_udp_rpc::NodeUdpRpc;
use crate::subscription::Subscription;

use super::CliContext;

#[derive(FromArgs)]
/// Validation manager service
#[argh(subcommand, name = "validator")]
pub struct Cmd {
    /// max timediff (in seconds). 120 seconds default
    #[argh(option, default = "120")]
    max_time_diff: u16,

    /// elections offset (in seconds). 600 seconds default
    #[argh(option, default = "600")]
    elections_offset: u64,

    /// min retry interval (in seconds). 10 seconds default
    #[argh(option, default = "10")]
    min_retry_interval: u64,

    /// max retry interval (in seconds). 300 seconds default
    #[argh(option, default = "300")]
    max_retry_interval: u64,

    /// interval increase factor. 2.0 times default
    #[argh(option, default = "2.0")]
    retry_interval_multiplier: f64,
}

impl Cmd {
    pub async fn run(mut self, ctx: CliContext) -> Result<()> {
        let mut manager = ValidationManager {
            ctx,
            max_time_diff: std::cmp::max(self.max_time_diff as i32, 5),
            elections_offset: Duration::from_secs(self.elections_offset),
        };

        self.min_retry_interval = std::cmp::max(self.min_retry_interval, 1);
        self.max_retry_interval = std::cmp::max(self.max_retry_interval, self.min_retry_interval);
        self.retry_interval_multiplier = num::Float::max(self.retry_interval_multiplier, 1.0);

        let mut interval = self.min_retry_interval;
        loop {
            if let Err(e) = manager.try_validate().await {
                tracing::error!("error occured: {e:?}");
            }

            tracing::info!("retrying in {interval} seconds");
            tokio::time::sleep(Duration::from_secs(interval)).await;

            interval = std::cmp::min(
                self.max_retry_interval,
                (interval as f64 * self.retry_interval_multiplier) as u64,
            );
        }
    }
}

struct ValidationManager {
    ctx: CliContext,
    max_time_diff: i32,
    elections_offset: Duration,
}

impl ValidationManager {
    async fn try_validate(&mut self) -> Result<()> {
        tracing::info!("started validation loop");
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;

            // Read config
            let config = self.ctx.load_config()?;
            let validation = config.validation()?;

            // Prepare subscription
            let node_tcp_rpc = NodeTcpRpc::new(config.control()?).await?;
            self.wait_until_synced(&node_tcp_rpc, validation.is_single())
                .await?;

            let ConfigWithId {
                block_id: target_key_block,
                config: blockchain_config,
            } = node_tcp_rpc.get_config_all().await?;

            let timings = blockchain_config
                .elector_params()
                .context("invalid elector params")?;

            let node_udp_rpc = NodeUdpRpc::new(config.adnl()?).await?;
            let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);
            subscription.ensure_ready().await?;

            tracing::info!("target key block id: {target_key_block}");
            let target_block = subscription.udp_rpc().get_block(&target_key_block).await?;

            tracing::info!("node is synced");
        }
    }

    async fn wait_until_synced(
        &self,
        node_rpc: &NodeTcpRpc,
        only_mc: bool,
    ) -> Result<RunningStats> {
        let interval = Duration::from_secs(10);
        loop {
            match node_rpc.get_stats().await? {
                NodeStats::Running(stats) => {
                    if stats.mc_time_diff < self.max_time_diff
                        && (only_mc || stats.sc_time_diff < self.max_time_diff)
                    {
                        break Ok(stats);
                    }
                }
                NodeStats::NotReady => {
                    tracing::trace!("node not synced");
                }
            }
            tokio::time::sleep(interval).await;
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Timeline {
    BeforeElections,
    Elections,
    AfterElections,
}
