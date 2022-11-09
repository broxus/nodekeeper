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
    elections_offset: u32,

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
            elections_offset: self.elections_offset,
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
    elections_offset: u32,
}

impl ValidationManager {
    async fn try_validate(&mut self) -> Result<()> {
        tracing::info!("started validation loop");

        let mut interval = 0u32;
        loop {
            if interval > 0 {
                interval = std::cmp::max(interval, 10);
                tokio::time::sleep(Duration::from_secs(interval as u64)).await;
            }

            // Read config
            let config = self.ctx.load_config()?;
            let validation = config.validation()?;

            // Prepare subscription
            let node_tcp_rpc = NodeTcpRpc::new(config.control()?).await?;
            self.wait_until_synced(&node_tcp_rpc, validation.is_single())
                .await?;

            let ConfigWithId {
                block_id: target_block,
                config: blockchain_config,
            } = node_tcp_rpc.get_config_all().await?;

            let timings = blockchain_config
                .elector_params()
                .context("invalid elector params")?;
            let current_vset = blockchain_config
                .validator_set()
                .context("invalid validator set")?;
            let next_vset = blockchain_config
                .next_validator_set_present()
                .and_then(|some| {
                    some.then(|| blockchain_config.next_validator_set())
                        .transpose()
                })
                .context("invalid validator set")?;

            let node_udp_rpc = NodeUdpRpc::new(config.adnl()?).await?;
            let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);
            subscription.ensure_ready().await?;

            tracing::info!("target block id: {target_block}");
            let target_block = subscription.udp_rpc().get_block(&target_block).await?;
            let target_block_info = target_block
                .read_brief_info()
                .context("invalid target block")?;

            let timeline = Timeline::compute(&timings, &current_vset, target_block_info.gen_utime);
            tracing::info!("timeline: {timeline}");

            match timeline {
                Timeline::BeforeElections {
                    until_elections_start,
                } => {
                    interval = until_elections_start + self.elections_offset;
                }
                Timeline::Elections {
                    since_elections_start,
                    until_elections_end,
                } => {
                    // TODO: elect

                    interval = until_elections_end;
                }
                Timeline::AfterElections { until_round_end } => {
                    interval = until_round_end;
                }
            }
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
    BeforeElections {
        until_elections_start: u32,
    },
    Elections {
        since_elections_start: u32,
        until_elections_end: u32,
    },
    AfterElections {
        until_round_end: u32,
    },
}

impl std::fmt::Display for Timeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BeforeElections {
                until_elections_start,
            } => f.write_fmt(format_args!(
                "before elections ({until_elections_start}s remaining)"
            )),
            Self::Elections {
                since_elections_start: since,
                until_elections_end: until,
            } => f.write_fmt(format_args!(
                "elections (started {since}s ago, {until}s remaining)"
            )),
            Self::AfterElections { until_round_end } => f.write_fmt(format_args!(
                "after elections ({until_round_end}s until new round)"
            )),
        }
    }
}

impl Timeline {
    fn compute(
        timings: &ton_block::ConfigParam15,
        current_vset: &ton_block::ValidatorSet,
        now: u32,
    ) -> Self {
        let round_end = current_vset.utime_until();
        let elections_start = round_end.saturating_sub(timings.elections_start_before);
        let elections_end = round_end.saturating_sub(timings.elections_end_before);

        if let Some(until_elections) = elections_start.checked_sub(now) {
            return Self::BeforeElections {
                until_elections_start: until_elections,
            };
        }

        if let Some(until_end) = elections_end.checked_sub(now) {
            return Self::Elections {
                since_elections_start: now.saturating_sub(elections_start),
                until_elections_end: until_end,
            };
        }

        Self::AfterElections {
            until_round_end: round_end.saturating_sub(now),
        }
    }
}
