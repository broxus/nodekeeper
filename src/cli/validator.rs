use std::collections::BTreeMap;
use std::future::Future;
use std::time::Duration;

use anyhow::{Context, Result};
use argh::FromArgs;

use super::{CliContext, ProjectDirs};
use crate::config::{AppConfigValidation, StoredKeys};
use crate::contracts::{elector, wallet, InternalMessage, ONE_EVER};
use crate::node_tcp_rpc::{ConfigWithId, NodeStats, NodeTcpRpc, RunningStats};
use crate::node_udp_rpc::NodeUdpRpc;
use crate::subscription::Subscription;
use crate::util::Ever;

#[derive(FromArgs)]
/// Validation manager service
#[argh(subcommand, name = "validator")]
pub struct Cmd {
    /// max timediff (in seconds). 120 seconds default
    #[argh(option, default = "120")]
    max_time_diff: u16,

    /// elections start offset (in seconds). 600 seconds default
    #[argh(option, default = "600")]
    elections_start_offset: u32,

    /// elections end offset (in seconds). 120 seconds default
    #[argh(option, default = "120")]
    elections_end_offset: u32,

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
            elections_start_offset: self.elections_start_offset,
            elections_end_offset: self.elections_end_offset,
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
    elections_start_offset: u32,
    elections_end_offset: u32,
}

impl ValidationManager {
    async fn try_validate(&mut self) -> Result<()> {
        tracing::info!("started validation loop");

        let dirs = self.ctx.dirs();

        let mut interval = 0u32;
        loop {
            // Sleep with the requested interval
            if interval > 0 {
                interval = std::cmp::max(interval, 10);
                tokio::time::sleep(Duration::from_secs(interval as u64)).await;
            }

            // Read config
            let config = self.ctx.load_config()?;
            let validation = config.validation()?;

            // Create tcp rpc and wait until node is synced
            let node_tcp_rpc = NodeTcpRpc::new(config.control()?).await?;
            self.wait_until_synced(&node_tcp_rpc, validation.is_single())
                .await?;

            // Get current network config params
            let ConfigWithId {
                block_id: target_block,
                config: blockchain_config,
            } = node_tcp_rpc.get_config_all().await?;

            let elector_address = blockchain_config
                .elector_address()
                .context("invalid elector address")?;
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

            // Create subscription
            let node_udp_rpc = NodeUdpRpc::new(config.adnl()?).await?;
            let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);
            subscription.ensure_ready().await?;

            // Get block with the config
            tracing::info!("target block id: {target_block}");
            let target_block = subscription.udp_rpc().get_block(&target_block).await?;
            let target_block_info = target_block
                .read_brief_info()
                .context("invalid target block")?;

            // Compute where are we on the validation timeline
            let timeline = Timeline::compute(&timings, &current_vset, target_block_info.gen_utime);
            tracing::info!("timeline: {timeline}");

            let until_elections_end = match timeline {
                // If elections were not started yet, wait for the start (with an additonal offset)
                Timeline::BeforeElections {
                    until_elections_start,
                } => {
                    tracing::info!("waiting for the elections to start");
                    interval = until_elections_start + self.elections_start_offset;
                    continue;
                }
                // If elections started
                Timeline::Elections {
                    since_elections_start,
                    until_elections_end,
                } => {
                    if let Some(offset) = self
                        .elections_start_offset
                        .checked_sub(since_elections_start)
                    {
                        // Wait a bit after elections start
                        tracing::info!("too early to participate in elections");
                        interval = offset;
                        continue;
                    } else if let Some(offset) =
                        self.elections_end_offset.checked_sub(until_elections_end)
                    {
                        // Elections will end soon, attempts are doomed
                        tracing::info!("too late to participate in elections");
                        interval = offset;
                        continue;
                    } else {
                        // We can participate, remember elections end timestamp
                        until_elections_end
                    }
                }
                // Elections were already finished, wait for the new round
                Timeline::AfterElections { until_round_end } => {
                    tracing::info!("waiting for the new round to start");
                    interval = until_round_end;
                    continue;
                }
            };

            // Participate in elections
            let elector = elector::Elector::new(elector_address, subscription.clone());
            let elector_data = elector
                .get_data()
                .await
                .context("failed to get elector data")?;

            // Get current election id
            let Some(election_id) = elector_data.election_id() else {
                tracing::info!("no current elections in the elector state");
                continue;
            };
            tracing::info!("election id: {election_id}");

            // Prepare deadline future
            let deadline = tokio::time::sleep(Duration::from_secs(
                until_elections_end.saturating_sub(self.elections_end_offset) as u64,
            ));

            let keypair = dirs.load_validator_keys()?;
            match &validation {
                AppConfigValidation::Single(validation) => {
                    let wallet = wallet::Wallet::new(-1, keypair, subscription.clone())?;

                    if let Some(stake) = elector_data.has_unfrozen_stake(&validation.address) {
                        // Send recover stake message
                        tracing::info!(stake = stake.0, "recovering stake");
                        wallet
                            .call(elector.recover_stake()?)
                            .await
                            .context("failed to recover stake")?;
                    }

                    let elect = async {
                        let target_balance = validation.stake_per_round as u128 + 2 * ONE_EVER;

                        let balance = wait_for_balance(target_balance, || wallet.get_balance())
                            .await
                            .context("failed to fetch validator balance")?;
                        tracing::info!(balance = %Ever(balance), "fetched wallet balance");

                        let payload = elector
                            .participate_in_elections(
                                election_id,
                                &validation.address,
                                validation.stake_factor,
                                &timings,
                            )
                            .await
                            .context("failed to prepare new validator key")?;
                        tracing::info!("generated election payload");

                        wallet
                            .call(InternalMessage {
                                dst: elector.address().clone(),
                                amount: validation.stake_per_round as u128 + ONE_EVER,
                                payload,
                            })
                            .await
                            .context("failed to participate in elections")?;
                        tracing::info!("sent validator stake");

                        Ok::<_, anyhow::Error>(())
                    };

                    tokio::select! {
                        _ = deadline => {
                            tracing::info!("elections deadline reached");
                            continue
                        }
                        res = elect => res?,
                    };
                }
                AppConfigValidation::DePool(_validation) => {
                    tracing::info!("depool validator");
                    // wallet::Wallet::new(0, keypair, subscription.clone())?;
                }
            };

            interval = until_elections_end;
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

async fn wait_for_balance<F>(target: u128, mut f: impl FnMut() -> F) -> Result<u128>
where
    F: Future<Output = Result<Option<u128>>>,
{
    let interval = std::time::Duration::from_secs(1);
    loop {
        match f().await?.unwrap_or_default() {
            balance if balance >= target => break Ok(balance),
            balance => tracing::debug!(balance, target, "account balance not enough"),
        }
        tokio::time::sleep(interval).await;
    }
}

impl ProjectDirs {
    fn load_validator_keys(&self) -> Result<ed25519_dalek::Keypair> {
        let keys = StoredKeys::load(&self.validator_keys)
            .context("failed to load validator wallet keys")?;
        Ok(keys.as_keypair())
    }
}
