use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use argh::FromArgs;
use nekoton_abi::{KnownParamType, KnownParamTypePlain, MaybeRef, UnpackAbi, UnpackAbiPlain};

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

            let until_elections_end = match timeline {
                Timeline::BeforeElections {
                    until_elections_start,
                } => {
                    tracing::info!("waiting for the elections to start");
                    interval = until_elections_start + self.elections_start_offset;
                    continue;
                }
                Timeline::Elections {
                    since_elections_start,
                    until_elections_end,
                } => {
                    if let Some(offset) = self
                        .elections_start_offset
                        .checked_sub(since_elections_start)
                    {
                        tracing::info!("too early to participate in elections");
                        interval = offset;
                        continue;
                    } else if let Some(offset) =
                        self.elections_end_offset.checked_sub(until_elections_end)
                    {
                        tracing::info!("too late to participate in elections");
                        interval = offset;
                        continue;
                    } else {
                        until_elections_end
                    }
                }
                Timeline::AfterElections { until_round_end } => {
                    tracing::info!("waiting for the new round to start");
                    interval = until_round_end;
                    continue;
                }
            };

            // PARTICIATE
            let elector_data = subscription.get_elector_data(elector_address).await?;
            let election_id = match elector_data.current_election.0 {
                Some(current_election) => current_election.elect_at,
                None => {
                    tracing::info!("no current elections in the elector state");
                    continue;
                }
            };
            tracing::info!("election id: {election_id}");

            // TODO: elect

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

impl Subscription {
    async fn get_elector_data(
        &self,
        elector_address: ton_types::UInt256,
    ) -> Result<PartialElectorData> {
        let elector = ton_block::MsgAddressInt::AddrStd(ton_block::MsgAddrStd {
            workchain_id: -1,
            address: elector_address.into(),
            ..Default::default()
        });

        let elector_state = self
            .get_account_state(&elector)
            .await
            .context("failed to get elector state")?
            .context("elector not found")?;

        let ton_block::AccountState::AccountActive { state_init } = elector_state.storage.state else {
            anyhow::bail!("elector account is not active");
        };
        let data = state_init.data.context("elector data is empty")?;
        let state: PartialElectorData = ton_abi::TokenValue::decode_params(
            elector_data_params(),
            data.into(),
            &ton_abi::contract::ABI_VERSION_2_1,
            true,
        )
        .context("failed to parse elector data")?
        .unpack()?;

        Ok(state)
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

fn elector_data_params() -> &'static [ton_abi::Param] {
    once!(Vec<ton_abi::Param>, || PartialElectorData::param_type())
}

#[derive(Debug, UnpackAbiPlain, KnownParamTypePlain)]
struct PartialElectorData {
    #[abi]
    current_election: MaybeRef<CurrentElectionData>,
    #[abi]
    credits: BTreeMap<ton_types::UInt256, ton_block::Grams>,
}

#[derive(Debug, UnpackAbi, KnownParamType)]
struct CurrentElectionData {
    #[abi(uint32)]
    elect_at: u32,
    #[abi(uint32)]
    elect_close: u32,
    #[abi(gram)]
    min_stake: u128,
    #[abi(gram)]
    total_stake: u128,
    #[abi]
    members: BTreeMap<ton_types::UInt256, ElectionMember>,
    #[abi(bool)]
    failed: bool,
    #[abi(bool)]
    finished: bool,
}

#[derive(Debug, UnpackAbi, KnownParamType)]
struct ElectionMember {
    #[abi(gram)]
    msg_value: u64,
    #[abi(uint32)]
    created_at: u32,
    #[abi(uint32)]
    max_factor: u32,
    #[abi(uint256)]
    src_addr: ton_types::UInt256,
    #[abi(uint256)]
    adnl_addr: ton_types::UInt256,
}
