use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use broxus_util::now;
use futures_util::FutureExt;
use rand::Rng;
use tokio::sync::Mutex;

use crate::config::*;
use crate::contracts::*;
use crate::dirs::ProjectDirs;
use crate::network::{ConfigWithId, NodeStats, NodeTcpRpc, NodeUdpRpc, Subscription};
use crate::util::Ever;

pub struct ValidationManager {
    dirs: ProjectDirs,
    params: ValidationParams,
    last_params: parking_lot::Mutex<Option<AppConfigValidator>>,
    guard: Arc<Mutex<()>>,
}

impl ValidationManager {
    pub fn new(dirs: ProjectDirs, params: ValidationParams) -> Self {
        Self {
            dirs,
            params,
            last_params: Default::default(),
            guard: Default::default(),
        }
    }

    pub fn guard(&self) -> &Arc<Mutex<()>> {
        &self.guard
    }

    pub async fn try_validate(&mut self) -> Result<()> {
        const SYNC_CHECK_INTERVAL: u32 = 10;

        tracing::info!("started validation loop");

        let mut random_shift = None;

        let mut interval = 0u32;
        loop {
            // Sleep with the requested interval
            if interval > 0 {
                interval = std::cmp::max(interval, 10);
                tokio::time::sleep(Duration::from_secs(interval as u64)).await;
            }

            // Read config
            let mut config = AppConfig::load(&self.dirs.app_config)?;
            let validator = match config.validator.take() {
                Some(validator) => validator,
                None => {
                    interval = SYNC_CHECK_INTERVAL;
                    continue;
                }
            };

            // Create tcp rpc and wait until node is synced
            let node_tcp_rpc = NodeTcpRpc::new(config.control()?).await?;
            if !self.is_synced(&node_tcp_rpc, validator.is_single()).await? {
                interval = SYNC_CHECK_INTERVAL;
                continue;
            }
            let node_udp_rpc = NodeUdpRpc::new(config.adnl()?).await?;

            // Create subscription
            let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);
            subscription.ensure_ready().await?;

            // Get current network config params
            let ConfigWithId {
                block_id: target_block,
                config: blockchain_config,
            } = subscription.tcp_rpc().get_config_all().await?;

            if !self.params.ignore_deploy && self.ensure_deployed(&validator, &subscription).await?
            {
                // Proceed to the next iteration after contracts deployment
                continue;
            }

            let elector_address = blockchain_config
                .elector_address()
                .context("invalid elector address")?;
            let timings = blockchain_config
                .elector_params()
                .context("invalid elector params")?;
            let current_vset = blockchain_config
                .validator_set()
                .context("invalid validator set")?;

            // Get block with the config
            tracing::info!("target block id: {target_block}");
            let target_block = subscription.udp_rpc().get_block(&target_block).await?;
            let target_block_info = target_block
                .read_brief_info()
                .context("invalid target block")?;

            // Compute where are we on the validation timeline
            let timeline = Timeline::compute(&timings, &current_vset, target_block_info.gen_utime);
            tracing::info!("timeline: {timeline}");

            let elections_end = match timeline {
                // If elections were not started yet, wait for the start (with an additional offset)
                Timeline::BeforeElections {
                    until_elections_start,
                } => {
                    random_shift = None; // reset random shift before each elections
                    tracing::info!("waiting for the elections to start");
                    interval = until_elections_start + self.params.elections_start_offset;
                    continue;
                }
                // If elections started
                Timeline::Elections {
                    since_elections_start,
                    until_elections_end,
                    elections_end,
                } => {
                    let random_shift = match random_shift {
                        Some(shift) => shift,
                        None if self.params.disable_random_shift => *random_shift.insert(0),
                        None => {
                            // Compute the random offset in the first 1/4 of elections
                            let range = (since_elections_start + until_elections_end)
                                .saturating_sub(self.params.elections_end_offset)
                                .saturating_sub(self.params.elections_start_offset)
                                / 4;
                            *random_shift.insert(rand::thread_rng().gen_range(0..range))
                        }
                    };

                    let start_offset = self.params.elections_start_offset + random_shift;

                    if let Some(offset) = start_offset.checked_sub(since_elections_start) {
                        if offset > 0 {
                            // Wait a bit after elections start
                            interval = offset;
                            continue;
                        }
                    } else if let Some(offset) = self
                        .params
                        .elections_end_offset
                        .checked_sub(until_elections_end)
                    {
                        // Elections will end soon, attempts are doomed
                        interval = offset;
                        continue;
                    }

                    // We can participate, remember elections end timestamp
                    elections_end
                }
                // Elections were already finished, wait for the new round
                Timeline::AfterElections { until_round_end } => {
                    tracing::info!("waiting for the new round to start");
                    interval = until_round_end;
                    continue;
                }
            };

            // Participate in elections
            let elector = Elector::new(elector_address, subscription.clone());
            let elector_data = elector
                .get_data()
                .await
                .context("failed to get elector data")?;

            // Get current election id
            let Some(election_id) = elector_data.election_id() else {
                tracing::info!("no current elections in the elector state");
                interval = 1; // retry nearly immediate
                continue;
            };

            // Wait until stakes are unfrozen
            if let Some(mut unfreeze_at) = elector_data.nearest_unfreeze_at(election_id) {
                unfreeze_at += self.params.stake_unfreeze_offset;
                if unfreeze_at > elections_end.saturating_sub(self.params.elections_end_offset) {
                    tracing::warn!(
                        unfreeze_at,
                        elections_end,
                        "stakes will unfreeze after the end of the elections"
                    );
                } else if let Some(until_unfreeze) = unfreeze_at.checked_sub(now()) {
                    if until_unfreeze > 0 {
                        tracing::info!(until_unfreeze, "waiting for stakes to unfreeze");
                        tokio::time::sleep(Duration::from_secs(until_unfreeze as u64)).await;
                    }
                }
            }

            // Prepare context
            let keypair = self.dirs.load_validator_keys()?;
            let ctx = ElectionsContext {
                subscription,
                elector,
                elector_data,
                election_id,
                timings,
                guard: &self.guard,
            };

            // Prepare election future
            let validation = match validator {
                AppConfigValidator::Single(validation) => validation.elect(keypair, ctx).boxed(),
                AppConfigValidator::DePool(validation) => validation.elect(keypair, ctx).boxed(),
            };

            // Try elect
            let deadline = Duration::from_secs(
                elections_end
                    .saturating_sub(self.params.elections_end_offset)
                    .saturating_sub(now()) as u64,
            );
            match tokio::time::timeout(deadline, validation).await {
                Ok(Ok(())) => tracing::info!("elections successful"),
                Ok(Err(e)) => return Err(e),
                Err(_) => tracing::warn!("elections deadline reached"),
            }

            interval = elections_end.saturating_sub(now());
        }
    }

    async fn ensure_deployed(
        &self,
        validator: &AppConfigValidator,
        subscription: &Arc<Subscription>,
    ) -> Result<bool> {
        if matches!(&*self.last_params.lock(), Some(last_params) if last_params == validator) {
            return Ok(false);
        }

        let ctx = DeploymentContext {
            subscription,
            dirs: &self.dirs,
            guard: &self.guard,
        };

        match validator {
            AppConfigValidator::Single(validator) => validator.deploy(ctx).await?,
            AppConfigValidator::DePool(validator) => validator.deploy(ctx).await?,
        }
        *self.last_params.lock() = Some(validator.clone());
        Ok(true)
    }

    async fn is_synced(&self, node_rpc: &NodeTcpRpc, only_mc: bool) -> Result<bool> {
        let interval = Duration::from_secs(10);
        let mut attempts = 6;
        loop {
            match node_rpc.get_stats().await? {
                NodeStats::Running(stats) => {
                    if stats.mc_time_diff < self.params.max_time_diff
                        && (only_mc || stats.sc_time_diff < self.params.max_time_diff)
                    {
                        break Ok(true);
                    }
                }
                NodeStats::NotReady => {
                    tracing::trace!("node not synced");
                }
            }

            attempts -= 1;
            if attempts > 0 {
                tokio::time::sleep(interval).await;
            } else {
                break Ok(false);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ValidationParams {
    pub max_time_diff: i32,
    pub stake_unfreeze_offset: u32,
    pub elections_start_offset: u32,
    pub elections_end_offset: u32,
    pub disable_random_shift: bool,
    pub ignore_deploy: bool,
}

#[derive(Clone, Copy)]
struct DeploymentContext<'a> {
    subscription: &'a Arc<Subscription>,
    dirs: &'a ProjectDirs,
    guard: &'a Mutex<()>,
}

struct ElectionsContext<'a> {
    subscription: Arc<Subscription>,
    elector: Elector,
    elector_data: elector::ElectorData,
    election_id: u32,
    timings: ton_block::ConfigParam15,
    guard: &'a Mutex<()>,
}

impl AppConfigValidatorSingle {
    async fn deploy(&self, _: DeploymentContext<'_>) -> Result<()> {
        // TODO: deploy validator wallet if it differs from ever wallet
        Ok(())
    }

    async fn elect(self, keypair: ed25519_dalek::Keypair, ctx: ElectionsContext<'_>) -> Result<()> {
        tracing::info!(
            election_id = ctx.election_id,
            address = %self.address,
            stake = %Ever(self.stake_per_round),
            stake_factor = ?self.stake_factor,
            "election as single"
        );

        let wallet = Wallet::new(-1, keypair, ctx.subscription.clone());
        anyhow::ensure!(
            wallet.address() == &self.address,
            "validator wallet address mismatch"
        );

        if let Some(stake) = ctx.elector_data.has_unfrozen_stake(wallet.address()) {
            wallet.wait_for_balance(2 * ONE_EVER).await?;

            // Prevent shutdown during stake recovery
            let _guard = ctx.guard.lock().await;

            // Send recover stake message
            tracing::info!(stake = %Ever(stake.0), "recovering stake");
            wallet
                .call(ctx.elector.recover_stake()?)
                .await
                .context("failed to recover stake")?;
        }

        if ctx.elector_data.elected(wallet.address()) {
            // Do nothing if elected
            tracing::info!("validator already elected");
            return Ok(());
        }

        // Wait until validator wallet balance is enough
        let target_balance = self.stake_per_round as u128 + 2 * ONE_EVER;
        wallet.wait_for_balance(target_balance).await?;

        let signature_id = ctx.subscription.get_signature_id().await?;

        // Prevent shutdown while electing
        let _guard = ctx.guard.lock().await;

        // Prepare node for elections
        let payload = ctx
            .elector
            .participate_in_elections(
                ctx.election_id,
                wallet.address(),
                self.stake_factor.unwrap_or(DEFAULT_STAKE_FACTOR),
                &ctx.timings,
                signature_id,
            )
            .await
            .context("failed to prepare new validator key")?;
        tracing::info!("generated election payload");

        // Send election message
        wallet
            .call(InternalMessage {
                dst: ctx.elector.address().clone(),
                amount: self.stake_per_round as u128 + ONE_EVER,
                payload,
            })
            .await
            .context("failed to participate in elections")?;

        // Done
        tracing::info!("sent validator stake");
        Ok(())
    }
}

impl AppConfigValidatorDePool {
    async fn deploy(&self, ctx: DeploymentContext<'_>) -> Result<()> {
        struct LazyWallet<'a> {
            state: Option<Wallet>,
            target: &'a ton_block::MsgAddressInt,
            ctx: DeploymentContext<'a>,
        }

        impl LazyWallet<'_> {
            fn get_or_init(&'_ mut self) -> Result<&'_ Wallet> {
                match &mut self.state {
                    Some(wallet) => Ok(wallet),
                    state => {
                        let keypair = self.ctx.dirs.load_validator_keys()?;
                        let res = Wallet::new(0, keypair, self.ctx.subscription.clone());
                        anyhow::ensure!(
                            res.address() == self.target,
                            "validator wallet address mismatch"
                        );
                        Ok(state.get_or_insert(res))
                    }
                }
            }
        }

        let mut wallet = LazyWallet {
            state: None,
            target: &self.owner,
            ctx,
        };

        let mut depool = DePool::new(
            self.depool_type,
            self.depool.clone(),
            ctx.subscription.clone(),
        );

        // Ensure that depool is deployed
        if depool
            .is_deployed()
            .await
            .context("failed to check DePool")?
        {
            tracing::info!("DePool was already deployed");
        } else {
            tracing::info!("DePool was not deployed yet");

            // Load deployment params
            let deploy = self
                .deploy
                .as_ref()
                .context("deployment params not found")?;

            // Load and check depool keypair
            depool.set_keypair(ctx.dirs.load_depool_keys()?)?;

            // Prepare wallet
            let wallet = wallet.get_or_init()?;

            // Get current depool balance
            let depool_balance = depool
                .get_balance()
                .await
                .context("failed to get DePool balance")?
                .unwrap_or_default();

            // Compute remaining depool balance
            let depool_initial_balance = DePool::INITIAL_BALANCE
                .checked_sub(depool_balance)
                .and_then(|diff| (diff > 0).then_some(std::cmp::max(diff, ONE_EVER)));

            // Wait until there are enough funds on the validator wallet
            let target =
                Wallet::INITIAL_BALANCE + depool_initial_balance.unwrap_or_default() + ONE_EVER;
            wallet.wait_for_balance(target).await?;

            // Transfer initial funds to the depool (if its balance is not enough)
            if let Some(balance) = depool_initial_balance {
                // Prevent shutdown during the operation
                let _guard = ctx.guard.lock();

                tracing::info!("transferring initial funds to the DePool");
                wallet
                    .call(InternalMessage::empty(depool.address().clone(), balance))
                    .await
                    .context("failed to transfer funds to the DePool contract")?;
            }

            // Prevent shutdown during the operation
            let _guard = ctx.guard.lock();

            // Call depool constructor
            tracing::info!("deploying DePool contract");
            depool
                .deploy(depool::DePoolInitParams {
                    min_stake: deploy.min_stake,
                    validator_assurance: deploy.validator_assurance,
                    owner: wallet.address().clone(),
                    participant_reward_fraction: deploy.participant_reward_fraction,
                })
                .await
                .context("failed to deploy DePool")?;
            tracing::info!("successfully deployed DePool");
        }

        // Handle stEVER depool case
        if self.depool_type.is_stever() {
            let depool_state = depool.get_state().await?;

            // Get allowed participants
            let allowed_participants = depool
                .get_allowed_participants(&depool_state)
                .context("failed to get allowed participant")?;

            // Strategy was not configured yet
            if allowed_participants.len() < 2 {
                let wallet = wallet.get_or_init()?;

                let subscription = ctx.subscription.clone();
                let strategy = if let Some(address) = self.strategy.clone() {
                    let strategy = Strategy::new(address, subscription);
                    let details = strategy
                        .get_details()
                        .await
                        .context("failed to get stEVER strategy details")?;

                    anyhow::ensure!(
                        &details.depool == depool.address(),
                        "strategy was deployed for the different DePool"
                    );

                    Some(strategy.address)
                } else if let Some(address) = self.strategy_factory.clone() {
                    let factory = StrategyFactory::new(address, subscription);
                    factory
                        .get_details()
                        .await
                        .context("failed to get stEVER strategy factory details")?;

                    // Prepare deployment message
                    let deployment_message = factory.deploy_strategy(depool.address())?;

                    // Wait until there are enough funds on the validator wallet
                    wallet
                        .wait_for_balance(deployment_message.amount + ONE_EVER)
                        .await?;

                    // Prevent shutdown during the operation
                    let _guard = ctx.guard.lock();

                    // Send an internal message to the factory
                    tracing::info!("deploying stEVER strategy");
                    let strategy = wallet
                        .call(deployment_message)
                        .await
                        .and_then(StrategyFactory::extract_strategy_address)
                        .context("failed to deploy stEVER DePool strategy")?;
                    tracing::info!(%strategy, "successfully deployed stEVER strategy");

                    Some(strategy)
                } else {
                    tracing::warn!(
                        "neither a strategy factory nor an explicit strategy was specified"
                    );
                    None
                };

                if let Some(strategy) = strategy {
                    // Prevent shutdown during the operation
                    let _guard = ctx.guard.lock();

                    // Set strategy as an allowed participant
                    tracing::info!(%strategy, "setting DePool strategy");
                    wallet
                        .call(depool.set_allowed_participant(&strategy)?)
                        .await
                        .context("failed to set update DePool strategy")?;
                    tracing::info!(%strategy, "DePool strategy successfully updated");
                }
            }
        }

        // Done
        Ok(())
    }

    async fn elect(self, keypair: ed25519_dalek::Keypair, ctx: ElectionsContext<'_>) -> Result<()> {
        tracing::info!(
            election_id = ctx.election_id,
            depool = %self.depool,
            depool_type = ?self.depool_type,
            owner = %self.owner,
            stake_factor = ?self.stake_factor,
            "election as DePool"
        );

        let wallet = Wallet::new(0, keypair, ctx.subscription.clone());
        anyhow::ensure!(
            wallet.address() == &self.owner,
            "validator wallet address mismatch"
        );

        let depool = DePool::new(
            self.depool_type,
            self.depool.clone(),
            ctx.subscription.clone(),
        );
        let depool_state = depool
            .get_state()
            .await
            .context("failed to get DePool state")?;

        let depool_info = depool
            .get_info(&depool_state)
            .context("failed to get DePool info")?;
        anyhow::ensure!(
            wallet.address() == &depool_info.validator_wallet,
            "DePool owner mismatch"
        );
        anyhow::ensure!(depool_info.proxies.len() == 2, "invalid DePool proxies");

        // Ensure that depool and proxy balances are enough
        self.maintain_balances(&wallet, &depool, &ctx)
            .await
            .context("failed to maintain balances")?;

        // Update depool
        let (round_id, step) = match self
            .update_depool(&wallet, &depool, &depool_info, depool_state, &ctx)
            .await
            .context("failed to update depool")?
        {
            Some(round) => round,
            None => {
                tracing::info!("skipping round");
                return Ok(());
            }
        };

        if step != depool::RoundStep::WaitingValidatorRequest {
            tracing::info!("depool is not waiting for the validator request");
            return Ok(());
        }

        let proxy = &depool_info.proxies[round_id as usize % 2];
        if ctx.elector_data.elected(proxy) {
            tracing::info!(%proxy, "proxy already elected");
            return Ok(());
        }

        // Wait until validator wallet balance is enough
        wallet.wait_for_balance(2 * ONE_EVER).await?;

        let signature_id = ctx.subscription.get_signature_id().await?;

        // Prevent shutdown while electing
        let _guard = ctx.guard.lock().await;

        // Prepare node for elections
        let payload = ctx
            .elector
            .participate_in_elections(
                ctx.election_id,
                proxy,
                self.stake_factor.unwrap_or(DEFAULT_STAKE_FACTOR),
                &ctx.timings,
                signature_id,
            )
            .await
            .context("failed to prepare new validator key")?;
        tracing::info!("generated election payload");

        // Send election message
        wallet
            .call(InternalMessage {
                dst: depool.address().clone(),
                amount: ONE_EVER,
                payload,
            })
            .await
            .context("failed to participate in elections")?;

        // Done
        tracing::info!("sent validator stake");
        Ok(())
    }

    async fn maintain_balances(
        &self,
        wallet: &Wallet,
        depool: &DePool,
        ctx: &ElectionsContext<'_>,
    ) -> Result<()> {
        // Check and refill depool and proxy balances
        let refill_messages = depool.maintain_balances().await?;
        for message in refill_messages {
            tracing::info!(
                target = %message.dst,
                amount = %message.amount,
                "replenishing depool contracts"
            );

            wallet.wait_for_balance(message.amount + ONE_EVER).await?;

            // Prevent shutdown during operation
            let _guard = ctx.guard.lock().await;

            // Send some funds to depool contracts
            wallet
                .call(message)
                .await
                .context("failed to replenish depool contracts")?;
        }

        Ok(())
    }

    async fn update_depool(
        &self,
        wallet: &Wallet,
        depool: &DePool,
        depool_info: &depool::DePoolInfo,
        mut depool_state: ton_block::AccountStuff,
        ctx: &ElectionsContext<'_>,
    ) -> Result<Option<(u64, depool::RoundStep)>> {
        const TICKTOCK_INTERVAL: Duration = Duration::from_secs(60);

        let mut attempts = 4;
        let mut sent_ticktock = false;
        loop {
            // Get validator stakes info
            let participant_info = depool
                .get_participant_info(&depool_state, wallet.address())
                .context("failed to get participant info")?;

            // Get all depool rounds
            let rounds = depool
                .get_rounds(&depool_state)
                .context("failed to get depool rounds")?
                .into_values()
                .collect::<Vec<_>>();
            anyhow::ensure!(rounds.len() == 4, "DePool rounds number mismatch");

            let prev_round = &rounds[0];
            let target_round = &rounds[1];
            let pooling_round = &rounds[2];

            let pooling_round_stake = match participant_info {
                Some(participant) => {
                    participant.compute_total_stake(pooling_round.id)
                        + participant.compute_total_stake(prev_round.id)
                }
                None => 0,
            };

            tracing::debug!(
                target_round_stake = %Ever(target_round.validator_stake),
                target_round_step = ?target_round.step,
                pooling_round_stake = %Ever(pooling_round_stake),
            );

            // Add ordinary stake to the pooling round if needed
            if let Some(mut remaining_stake) = depool_info
                .validator_assurance
                .checked_sub(pooling_round_stake)
            {
                if remaining_stake > 0 {
                    remaining_stake = std::cmp::max(remaining_stake, depool_info.min_stake);
                    wallet
                        .wait_for_balance(remaining_stake as u128 + ONE_EVER)
                        .await?;

                    // Prevent shutdown during sending stake
                    let _guard = ctx.guard.lock().await;

                    // Send recover stake message
                    tracing::info!(stake = %Ever(remaining_stake), "adding ordinary stake");
                    wallet
                        .call(depool.add_ordinary_stake(remaining_stake)?)
                        .await
                        .context("failed to add ordinary stake")?;
                }
            }

            if target_round.supposed_elected_at == ctx.election_id {
                // Return target round if it is configured
                break Ok(Some((target_round.id, target_round.step)));
            } else if sent_ticktock
                && target_round.completion_reason == depool::CompletionReason::FakeRound
            {
                // Skip initial fake round
                break Ok(None);
            } else {
                // Reduce attempts otherwise
                attempts -= 1;
                anyhow::ensure!(attempts > 0, "failed to update rounds");
            }

            // Update rounds
            wallet.wait_for_balance(2 * ONE_EVER).await?;

            tracing::info!("sending ticktock");
            wallet
                .call(depool.ticktock()?)
                .await
                .context("failed to send ticktock")?;
            sent_ticktock = true;
            tokio::time::sleep(TICKTOCK_INTERVAL).await;

            // Update depool state
            depool_state = depool
                .get_state()
                .await
                .context("failed to get DePool state")?;
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
        elections_end: u32,
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
                ..
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
                elections_end,
            };
        }

        Self::AfterElections {
            until_round_end: round_end.saturating_sub(now),
        }
    }
}

impl Wallet {
    async fn wait_for_balance(&self, target: u128) -> Result<u128> {
        let interval = Duration::from_secs(1);
        let mut last_balance = None;
        loop {
            match self.get_balance().await?.unwrap_or_default() {
                balance if balance >= target => {
                    if last_balance.is_some() {
                        tracing::info!(balance = %Ever(balance), "fetched wallet balance");
                    }
                    break Ok(balance);
                }
                balance => {
                    if !matches!(last_balance, Some(last_balance) if last_balance == balance) {
                        tracing::info!(
                            address = %self.address(),
                            current_balance = %Ever(balance),
                            target_balance = %Ever(target),
                            "waiting until validator wallet balance is enough",
                        );
                    }
                    last_balance = Some(balance);
                }
            }
            tokio::time::sleep(interval).await;
        }
    }
}

impl ProjectDirs {
    fn load_validator_keys(&self) -> Result<ed25519_dalek::Keypair> {
        let keys = StoredKeys::load(&self.validator_keys)
            .context("failed to load validator wallet keys")?;
        Ok(keys.as_keypair())
    }

    fn load_depool_keys(&self) -> Result<ed25519_dalek::Keypair> {
        let keys = StoredKeys::load(&self.depool_keys).context("failed to load DePool keys")?;
        Ok(keys.as_keypair())
    }
}

const DEFAULT_STAKE_FACTOR: u32 = 196608;
