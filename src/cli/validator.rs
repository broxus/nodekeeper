use std::time::Duration;

use anyhow::{Context, Result};
use argh::FromArgs;
use dialoguer::console::style;
use tokio_util::sync::CancellationToken;

use super::CliContext;
use crate::config::{AppConfigValidator, StoredKeys};
use crate::contracts::{depool, wallet, InternalMessage, ONE_EVER};
use crate::network::{ConfigWithId, NodeTcpRpc, NodeUdpRpc, Subscription};
use crate::util::*;
use crate::validator::{ValidationManager, ValidationParams};

#[derive(FromArgs)]
/// Validator management stuff
#[argh(subcommand, name = "validator")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: SubCmd,
}

impl Cmd {
    pub async fn run(self, ctx: CliContext) -> Result<()> {
        match self.subcommand {
            SubCmd::Balance(cmd) => cmd.run(ctx).await,
            SubCmd::Tick(cmd) => invoke_as_cli(cmd.run(ctx)).await,
            SubCmd::Withdraw(cmd) => invoke_as_cli(cmd.run(ctx)).await,
            SubCmd::Unstake(cmd) => invoke_as_cli(cmd.run(ctx)).await,
            SubCmd::Run(cmd) => cmd.run(ctx).await,
        }
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    Balance(CmdBalance),
    Tick(CmdTick),
    Withdraw(CmdWithdraw),
    Unstake(CmdUnstake),
    Run(CmdRun),
}

#[derive(FromArgs)]
/// Fetches the validator wallet balance
#[argh(subcommand, name = "balance")]
struct CmdBalance {}

impl CmdBalance {
    async fn run(self, ctx: CliContext) -> Result<()> {
        // Load config
        let mut config = ctx.load_config()?;
        let validator = config
            .validator
            .take()
            .context("validator entry not found in the app config")?;

        // Prepare RPC client
        let node_rpc = NodeTcpRpc::new(config.control()?).await?;

        // Get current network config params
        let ConfigWithId {
            config: blockchain_config,
            ..
        } = node_rpc.get_config_all().await?;
        let storage_prices = &StoragePrices::new(&blockchain_config)?;

        // Prepare helpers
        let get_account_balance = |address: &ton_block::MsgAddressInt| {
            let node_rpc = &node_rpc;
            let address = address.clone();
            async move {
                let state = node_rpc.get_shard_account_state(&address).await?;
                Ok::<_, anyhow::Error>(match state.read_account()? {
                    ton_block::Account::Account(account) => {
                        let storage_fee = storage_prices.compute_fee(
                            &account.storage_stat,
                            address.is_masterchain(),
                            broxus_util::now(),
                        );

                        Some((account.storage.balance.grams, storage_fee))
                    }
                    ton_block::Account::AccountNone => None,
                })
            }
        };

        let make_balance_entry =
            |address: &ton_block::MsgAddressInt, balance: Option<(ton_block::Grams, u128)>| {
                serde_json::json!({
                    "address": address.to_string(),
                    "balance": balance.as_ref().map(|(b, _)| b.to_string()),
                    "storage_fee": balance.as_ref().map(|(_, f)| f.to_string()),
                })
            };

        // Fetch balance
        let output = match validator {
            // Just wallet balance for a single validator
            AppConfigValidator::Single(single) => {
                let wallet_balance = get_account_balance(&single.address).await?;

                serde_json::json!({
                    "wallet": make_balance_entry(&single.address, wallet_balance)
                })
            }
            // All balances for depool setup
            AppConfigValidator::DePool(config) => {
                let wallet_balance = get_account_balance(&config.owner).await?;

                let depool = node_rpc.get_shard_account_state(&config.depool).await?;

                let mut depool_balance = None;
                let mut proxies = None;
                if let ton_block::Account::Account(ref state) = depool.read_account()? {
                    depool_balance = {
                        let storage_fee = storage_prices.compute_fee(
                            &state.storage_stat,
                            config.depool.is_masterchain(),
                            broxus_util::now(),
                        );
                        Some((state.storage.balance.grams, storage_fee))
                    };

                    // Get balances of proxies if depool is deployed
                    if matches!(
                        &state.storage.state,
                        ton_block::AccountState::AccountActive { .. }
                    ) {
                        let state = depool::DePoolState {
                            state,
                            ty: config.depool_type,
                        };

                        let info = state.get_info()?;
                        let proxies = proxies.insert(Vec::with_capacity(info.proxies.len()));
                        for proxy in info.proxies {
                            let proxy_balance = get_account_balance(&proxy).await?;
                            proxies.push(make_balance_entry(&proxy, proxy_balance));
                        }
                    }
                }

                serde_json::json!({
                    "wallet": make_balance_entry(&config.owner, wallet_balance),
                    "depool": make_balance_entry(&config.depool, depool_balance),
                    "proxies": proxies,
                })
            }
        };

        print_output(output);
        Ok(())
    }
}

#[derive(FromArgs)]
/// Ticktock depool.
#[argh(subcommand, name = "tick")]
struct CmdTick {}

impl CmdTick {
    async fn run(self, ctx: CliContext) -> Result<()> {
        let DePoolCmdContext {
            currency,
            wallet,
            depool,
        } = DePoolCmdContext::new(&ctx).await?;

        // Check wallet balance
        let wallet_balance = wallet.get_balance().await?.unwrap_or_default();
        anyhow::ensure!(
            ONE_EVER * 2 < wallet_balance,
            "wallet balance is not enough ({} {currency})",
            Tokens(wallet_balance)
        );

        // Send external message and wait until it is delivered
        let TransactionWithHash {
            hash: tx_hash,
            data: tx,
        } = wallet.transfer(depool.ticktock()?).await?;

        // Parse transaction
        let msg_hash = tx
            .in_msg
            .context("external inbound message not found")?
            .hash();

        // Done
        print_output(serde_json::json!({
            "tx_hash": tx_hash.to_hex_string(),
            "msg_hash": msg_hash.to_hex_string(),
        }));
        Ok(())
    }
}

#[derive(FromArgs)]
/// Reduce the validator stake in the depool.
#[argh(subcommand, name = "unstake")]
struct CmdUnstake {
    /// amount to unstake in tokens
    #[argh(positional)]
    amount: u128,

    /// never prompt
    #[argh(switch, short = 'f')]
    force: bool,

    /// interpret amount as amount in nano tokens
    #[argh(switch)]
    nano: bool,

    /// unstake from a pooling round
    #[argh(switch)]
    from_pooling: bool,
}

impl CmdUnstake {
    async fn run(self, ctx: CliContext) -> Result<()> {
        let DePoolCmdContext {
            currency,
            wallet,
            depool,
        } = DePoolCmdContext::new(&ctx).await?;

        // Parse arguments
        let mut amount = self.amount;
        if !self.nano {
            amount = amount.saturating_mul(ONE_EVER);
        }

        // Get participant info
        let depool_state = depool.get_state().await?;
        let Some(participant_info) =
            depool.get_participant_info(&depool_state, wallet.address())?
        else {
            anyhow::bail!("validator wallet is not a participant of the depool");
        };

        // Check participant info
        anyhow::ensure!(
            amount <= participant_info.total as u128,
            "participant stake is not enough ({} {currency})",
            Tokens(participant_info.total),
        );

        // Check wallet balance
        let wallet_balance = wallet.get_balance().await?.unwrap_or_default();
        anyhow::ensure!(
            ONE_EVER * 2 < wallet_balance,
            "wallet balance is not enough ({} {currency})",
            Tokens(wallet_balance)
        );

        if is_terminal() {
            eprintln!(
                "{}\n{}\n{}\n{}\n\n{}\n{}\n{}\n{}\n",
                style("Wallet address:").green().bold(),
                style(wallet.address()).bold(),
                style("DePool address:").green().bold(),
                style(depool.address()).bold(),
                style("Total stake:").green().bold(),
                style(format!("{} {currency}", Tokens(participant_info.total))).bold(),
                style("Amount to unstake:").green().bold(),
                style(format!("{} {currency}", Tokens(amount))).bold()
            );

            if !self.force
                && !confirm(
                    &dialoguer::theme::ColorfulTheme::default(),
                    false,
                    "Do you really want to unstake tokens?",
                )?
            {
                return Ok(());
            }
        }

        // Send external message and wait until it is delivered
        let TransactionWithHash {
            hash: tx_hash,
            data: tx,
        } = wallet
            .transfer(depool.withdraw_part(amount as u64, self.from_pooling)?)
            .await?;

        // Parse transaction
        let msg_hash = tx
            .in_msg
            .context("external inbound message not found")?
            .hash();

        // Done
        print_output(serde_json::json!({
            "tx_hash": tx_hash.to_hex_string(),
            "msg_hash": msg_hash.to_hex_string(),
        }));
        Ok(())
    }
}

#[derive(FromArgs)]
/// Withdraws tokens from the validator wallet
#[argh(subcommand, name = "withdraw")]
struct CmdWithdraw {
    /// destination account address
    #[argh(positional)]
    dest: String,

    /// amount to withdraw in tokens
    #[argh(positional)]
    amount: u128,

    /// never prompt
    #[argh(switch, short = 'f')]
    force: bool,

    /// interpret amount as amount in nano tokens
    #[argh(switch)]
    nano: bool,
}

impl CmdWithdraw {
    async fn run(self, ctx: CliContext) -> Result<()> {
        // Load config
        let mut config = ctx.load_config()?;
        let validator = config
            .validator
            .take()
            .context("validator entry not found in the app config")?;
        let currency = config.currency();

        // Prepare RPC clients
        let node_tcp_rpc = NodeTcpRpc::new(config.control()?)
            .await
            .context("failed to build node TCP client")?;
        let node_udp_rpc = NodeUdpRpc::new(config.adnl()?)
            .await
            .context("failed to build node UDP client")?;

        let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);
        subscription.ensure_ready().await?;

        // Parse arguments
        let dest = parse_address(&self.dest)?;
        let mut amount = self.amount;
        if !self.nano {
            amount = amount.saturating_mul(ONE_EVER);
        }

        // Prepare wallet
        let keypair = StoredKeys::load(&ctx.dirs.validator_keys)
            .context("failed to load validator wallet keys")?
            .as_keypair();

        let wallet_address = match validator {
            AppConfigValidator::Single(single) => single.address,
            AppConfigValidator::DePool(depool) => depool.owner,
        };

        let wallet =
            wallet::Wallet::new(wallet_address.workchain_id() as i8, keypair, subscription);
        anyhow::ensure!(
            wallet.address() == &wallet_address,
            "validator wallet address mismatch"
        );

        // Check wallet balance
        let wallet_balance = wallet.get_balance().await?.unwrap_or_default();
        anyhow::ensure!(
            amount < wallet_balance,
            "wallet balance is not enough ({} {currency})",
            Tokens(wallet_balance)
        );

        if is_terminal() {
            eprintln!(
                "{}\n{}\n{}\n{}\n\n{}\n{}\n{}\n{}\n",
                style("Wallet address:").green().bold(),
                style(wallet.address()).bold(),
                style("Wallet balance:").green().bold(),
                style(format!("{} {currency}", Tokens(wallet_balance))).bold(),
                style("Target address:").green().bold(),
                style(&dest).bold(),
                style("Amount to send:").green().bold(),
                style(format!("{} {currency}", Tokens(amount))).bold()
            );

            if !self.force
                && !confirm(
                    &dialoguer::theme::ColorfulTheme::default(),
                    false,
                    "Do you really want to send tokens?",
                )?
            {
                return Ok(());
            }
        }

        // Send external message and wait until it is delivered
        let TransactionWithHash {
            hash: tx_hash,
            data: tx,
        } = wallet
            .transfer(InternalMessage {
                dst: dest,
                amount,
                payload: Default::default(),
                bounce: false,
            })
            .await?;

        // Parse transaction
        let msg_hash = tx
            .in_msg
            .context("external inbound message not found")?
            .hash();

        // Done
        print_output(serde_json::json!({
            "tx_hash": tx_hash.to_hex_string(),
            "msg_hash": msg_hash.to_hex_string(),
        }));
        Ok(())
    }
}

#[derive(FromArgs)]
/// Starts managing validation
#[argh(subcommand, name = "run")]
struct CmdRun {
    /// max timediff (in seconds). 120 seconds default
    #[argh(option, default = "120")]
    max_time_diff: u16,

    /// offset after stake unfreeze (in seconds). 600 seconds default
    #[argh(option, default = "600")]
    stake_unfreeze_offset: u32,

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

    /// forces stakes to be sent right after the start of each election
    #[argh(switch)]
    disable_random_shift: bool,

    /// ignore contracts deployment
    #[argh(switch)]
    ignore_deploy: bool,

    /// force elect
    #[argh(switch)]
    force: bool,
}

impl CmdRun {
    async fn run(mut self, ctx: CliContext) -> Result<()> {
        // Start listening termination signals
        let signal_rx = broxus_util::any_signal(broxus_util::TERMINATION_SIGNALS);

        // Create validation manager
        let mut manager = ValidationManager::new(
            ctx.dirs,
            ValidationParams {
                max_time_diff: std::cmp::max(self.max_time_diff as i32, 5),
                stake_unfreeze_offset: self.stake_unfreeze_offset,
                elections_start_offset: self.elections_start_offset,
                elections_end_offset: self.elections_end_offset,
                disable_random_shift: self.disable_random_shift,
                ignore_deploy: self.ignore_deploy,
            },
        );

        if self.force {
            return manager.force_elect().await;
        }

        // Spawn cancellation future
        let cancellation_token = CancellationToken::new();
        let cancelled = cancellation_token.cancelled();

        tokio::spawn({
            let guard = manager.guard().clone();
            let cancellation_token = cancellation_token.clone();

            async move {
                if let Ok(signal) = signal_rx.await {
                    tracing::warn!(?signal, "received termination signal");
                    let _guard = guard.lock().await;
                    cancellation_token.cancel();
                }
            }
        });

        // Prepare validation future
        let validation_fut = async {
            self.min_retry_interval = std::cmp::max(self.min_retry_interval, 1);
            self.max_retry_interval =
                std::cmp::max(self.max_retry_interval, self.min_retry_interval);
            self.retry_interval_multiplier = num::Float::max(self.retry_interval_multiplier, 1.0);

            let mut interval = self.min_retry_interval;
            loop {
                if let Err(e) = manager.try_validate().await {
                    tracing::error!("error occurred: {e:?}");
                }

                tracing::info!("retrying in {interval} seconds");
                tokio::time::sleep(Duration::from_secs(interval)).await;

                interval = std::cmp::min(
                    self.max_retry_interval,
                    (interval as f64 * self.retry_interval_multiplier) as u64,
                );
            }
        };

        // Cancellable main loop
        tokio::select! {
            _ = validation_fut => {},
            _ = cancelled => {},
        };

        Ok(())
    }
}

struct DePoolCmdContext {
    currency: &'static str,
    wallet: wallet::Wallet,
    depool: depool::DePool,
}

impl DePoolCmdContext {
    async fn new(ctx: &CliContext) -> Result<Self> {
        // Load config
        let mut config = ctx.load_config()?;
        let validator = match config.validator.take() {
            Some(AppConfigValidator::DePool(depool)) => depool,
            Some(AppConfigValidator::Single(_)) => {
                anyhow::bail!("validator is not configured as a depool");
            }
            None => {
                anyhow::bail!("validator entry not found in the app config");
            }
        };

        // Prepare RPC clients
        let node_tcp_rpc = NodeTcpRpc::new(config.control()?)
            .await
            .context("failed to build node TCP client")?;
        let node_udp_rpc = NodeUdpRpc::new(config.adnl()?)
            .await
            .context("failed to build node UDP client")?;

        let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);
        subscription.ensure_ready().await?;

        // Prepare wallet
        let wallet_keys = StoredKeys::load(&ctx.dirs.validator_keys)
            .context("failed to load validator wallet keys")?
            .as_keypair();

        let wallet = wallet::Wallet::new(
            validator.owner.workchain_id() as i8,
            wallet_keys,
            subscription.clone(),
        );
        anyhow::ensure!(
            wallet.address() == &validator.owner,
            "validator wallet address mismatch"
        );

        // Prepare depool
        let depool = depool::DePool::new(validator.depool_type, validator.depool, subscription);

        Ok(Self {
            currency: config.currency(),
            wallet,
            depool,
        })
    }
}
