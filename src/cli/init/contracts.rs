use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use argh::FromArgs;
use console::style;
use dialoguer::theme::Theme;
use dialoguer::{Input, Select};

use crate::cli::{CliContext, ProjectDirs};
use crate::config::*;
use crate::crypto;
use crate::node_tcp_rpc::NodeTcpRpc;
use crate::node_udp_rpc::NodeUdpRpc;
use crate::subscription::Subscription;
use crate::util::*;

const DEFAULT_MIN_STAKE: u64 = 10;
const DEFAULT_VALIDATOR_ASSURANCE: u64 = 10_000;
const DEFAULT_PARTICIPANT_REWARD_FRACTION: u8 = 95;

const DEFAULT_STRATEGIES_FACTORY: &str =
    "0:519a1205bd021e5e0aa4b64f5ab689bc383efb4f94f283eac78926da71cfe100";

#[derive(FromArgs)]
/// Deploys contracts required for validation
#[argh(subcommand, name = "contracts")]
pub struct Cmd {}

impl Cmd {
    pub async fn run(self, theme: &dyn Theme, ctx: CliContext) -> Result<()> {
        let mut config = ctx.load_config()?;
        let dirs = ctx.dirs();

        // Create keys directory if it doesn't exist
        if !dirs.keys_dir.exists() {
            std::fs::create_dir_all(&dirs.keys_dir).context("failed to create keys dir")?;
        }

        // Check whether validation was already configured
        if config.validation.is_some()
            && !confirm(
                theme,
                false,
                "Validator is already configured. Update config?",
            )?
        {
            return Ok(());
        }

        // Create RPC clients
        let node_tcp_rpc = NodeTcpRpc::new(config.control()?)
            .await
            .context("failed to create node TCP client")?;
        let node_udp_rpc = NodeUdpRpc::new(config.adnl()?)
            .await
            .context("failed to create node UDP client")?;

        // Create subscription
        let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);

        // Check node status
        subscription.ensure_ready().await?;

        // Select validator type
        match Select::with_theme(theme)
            .with_prompt("Select validator type")
            .item("Single")
            .item("DePool")
            .default(0)
            .interact()?
        {
            // Prepare validator as a single node
            0 => prepare_single_validator(theme, dirs, &mut config, subscription).await,
            // Prepare validator as a depool
            _ => prepare_depool_validator(theme, dirs, &mut config, subscription).await,
        }
    }
}

async fn prepare_single_validator(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    app_config: &mut AppConfig,
    subscrioption: Arc<Subscription>,
) -> Result<()> {
    use crate::contracts::*;

    let mut steps = Steps::new(3);

    // Fetch blockchain stakes config for input validation
    let stakes_config = subscrioption
        .get_blockchain_config()
        .await
        .context("failed to get blockchain config")?
        .stakes_config()
        .context("failed to get stakes config")?;
    let min_stake = num::integer::div_ceil(stakes_config.min_stake.0, ONE_EVER);
    let max_stake = num::integer::div_floor(stakes_config.max_stake.0, ONE_EVER);

    // Prepare validator wallet
    steps.next("Creating validator wallet");
    let keypair = prepare_keys(theme, "Validator wallet seed phrase", &dirs.validator_keys)?;

    let wallet = wallet::Wallet::new(-1, keypair, subscrioption)
        .context("failed to create validator wallet")?;

    print_important_value("Validator wallet address", wallet.address());

    // Configure stake params
    steps.next("Configuring the stake");

    // Configure stake per round
    let stake_per_round: u64 = Input::with_theme(theme)
        .with_prompt("Stake per round (EVER)")
        .validate_with(|stake: &u64| match *stake as u128 {
            x if x > max_stake => Err(format!("Too big stake (max stake is {max_stake} EVER)")),
            x if x < min_stake => Err(format!("Too small stake (min stake is {min_stake} EVER)")),
            _ => Ok(()),
        })
        .interact_text()?;
    let stake_per_round = stake_per_round.saturating_mul(ONE_EVER as u64);

    // Save config
    app_config.validation = Some(AppConfigValidation::Single(AppConfigValidationSingle {
        address: wallet.address().clone(),
        stake_per_round,
        stake_factor: None,
    }));
    dirs.store_app_config(app_config)?;

    // Wait until validator wallet will have enough funds for staking
    steps.next("Replenishing the balance");

    let balance = wait_for_balance(
        "Waiting for the initial balance",
        stake_per_round as u128 * 2 + 10 * ONE_EVER,
        || wallet.get_balance(),
    )
    .await?;
    println!("Validator wallet balance: {}", Ever(balance));

    // Done
    steps.next("Validator configured successfully. Great!");
    Ok(())
}

async fn prepare_depool_validator(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    app_config: &mut AppConfig,
    subscription: Arc<Subscription>,
) -> Result<()> {
    use crate::contracts::*;

    const VALIDATOR_MIN_BALANCE: u128 = 10 * ONE_EVER;
    const DEPOOL_MIN_BALANCE: u128 = 30 * ONE_EVER;

    let mut steps = Steps::new(4);
    let default_strategies_factory: ton_block::MsgAddressInt =
        DEFAULT_STRATEGIES_FACTORY.parse().unwrap();

    // Prepare validator wallet
    steps.next("Creating validator wallet");
    let wallet_keypair = prepare_keys(theme, "Validator wallet seed phrase", &dirs.validator_keys)?;

    let wallet = wallet::Wallet::new(0, wallet_keypair, subscription.clone())
        .context("failed to create validator wallet")?;

    print_important_value("Validator wallet address", wallet.address());

    // Create depool
    steps.next("Creating depool");

    // Generate depool keys
    let depool_keypair = prepare_keys(theme, "DePool seed phrase", &dirs.depool_keys)?;

    // Select depool type
    let depool_type = match Select::with_theme(theme)
        .with_prompt("Select DePool type")
        .item("stEVER")
        .item("DePoolV3")
        .default(0)
        .interact()?
    {
        0 => DePoolType::StEver,
        _ => DePoolType::DefaultV3,
    };

    // Create depool wrapper
    let depool = depool::DePool::from_keypair(depool_type, depool_keypair, subscription.clone())
        .context("failed to create DePool")?;

    print_important_value("DePool address", depool.address());

    // Save config
    app_config.validation = Some(AppConfigValidation::DePool(Box::new(
        AppConfigValidationDePool {
            owner: wallet.address().clone(),
            depool: depool.address().clone(),
            depool_type,
            strategy: None,
            stake_factor: None,
        },
    )));
    dirs.store_app_config(app_config)?;

    // Configure and deploy DePool
    steps.next("Configuring DePool");

    if depool
        .is_deployed()
        .await
        .context("failed to check DePool")?
    {
        println!("DePool was already deployed");
    } else {
        // Deploy and configure depool

        // Get current depool balance
        let depool_balance = depool
            .get_balance()
            .await
            .context("failed to get DePool balance")?
            .unwrap_or_default();

        // Configure min participants stake
        let min_stake: u64 = Input::with_theme(theme)
            .with_prompt("Minimal participant stake (EVER)")
            .default(DEFAULT_MIN_STAKE)
            .validate_with(|value: &u64| match *value {
                x if x < 10 => Err("Minimal stake is too small (< 10 EVER)"),
                _ => Ok(()),
            })
            .interact_text()?;
        let min_stake = min_stake.saturating_mul(ONE_EVER as u64);

        // Configure validator assurance
        let validator_assurance: u64 = Input::with_theme(theme)
            .with_prompt("Validator assurance (EVER)")
            .default(DEFAULT_VALIDATOR_ASSURANCE)
            .validate_with(|value: &u64| match *value {
                x if x < 10 => Err("Too small validator assurance (< 10 EVER)"),
                _ => Ok(()),
            })
            .interact_text()?;
        let validator_assurance = validator_assurance.saturating_mul(ONE_EVER as u64);

        // Configure participant reward fraction
        let participant_reward_fraction: u8 = Input::with_theme(theme)
            .with_prompt("Participant reward fraction (%, 1..99)")
            .default(DEFAULT_PARTICIPANT_REWARD_FRACTION)
            .validate_with(|value: &u8| match *value {
                x if x < 1 => Err("Too small fraction (< 1%)"),
                x if x > 99 => Err("Too big fraction (> 99%)"),
                _ => Ok(()),
            })
            .interact_text()?;

        // Compute remaining depool balance
        let depool_initial_balance = DEPOOL_MIN_BALANCE
            .checked_sub(depool_balance)
            .and_then(|diff| (diff > 0).then_some(std::cmp::max(diff, ONE_EVER)));

        // Wait until validator wallet will have enough funds
        let balance = wait_for_balance(
            "Waiting for the initial validator balance",
            VALIDATOR_MIN_BALANCE + depool_initial_balance.unwrap_or_default(),
            || wallet.get_balance(),
        )
        .await?;
        println!("Validator wallet balance: {}", Ever(balance));

        // Deploying depool
        {
            let spinner = Spinner::start("Transferring funds to the DePool contract");

            // Transfer initial funds to the depool (if its balance is not enough)
            if let Some(balance) = depool_initial_balance {
                wallet
                    .call(InternalMessage::empty(depool.address().clone(), balance))
                    .await
                    .context("failed to transfer funds to the DePool contract")?;
            }

            // Call depool constructor
            spinner.set_message("Deploying DePool contract");
            depool
                .deploy(depool::DePoolInitParams {
                    min_stake,
                    validator_assurance,
                    owner: wallet.address().clone(),
                    participant_reward_fraction,
                })
                .await
                .context("failed to deploy DePool")?;
        }

        // Done
        println!("DePool contract was successfully deployed!");
    }

    // Handle stEver depool case
    if let DePoolType::StEver = depool_type {
        // Get allowed participants
        let depool_state = depool.get_state().await?;
        let allowed_participants = depool
            .get_allowed_participants(&depool_state)
            .context("failed to get allowed participant")?;

        if allowed_participants.len() < 2 {
            // Try select stever depool strategy
            let (mut spinner, strategy) = loop {
                match Select::with_theme(theme)
                    .item("Deploy new stEVER DePool strategy")
                    .item("Use existing stEVER DePool strategy")
                    .default(0)
                    .interact()?
                {
                    // Deploy new stever depool strategy
                    0 => {
                        // Determine strategies factory
                        let AddressInput(factory) = Input::with_theme(theme)
                            .with_prompt("Specify strategies factory")
                            .default(AddressInput(default_strategies_factory))
                            .validate_with(|addr: &AddressInput| {
                                let address = addr.0.clone();
                                let subscription = subscription.clone();
                                block_in_place(async move {
                                    strategy_factory::StrategyFactory::new(address, subscription)
                                        .get_details()
                                        .await
                                        .map(|_| ())
                                })
                            })
                            .interact_text()?;

                        // Create factory wrapper
                        let factory = strategy_factory::StrategyFactory::new(factory, subscription);

                        // Prepare deployment message
                        let deployment_message = factory.deploy_strategy(depool.address())?;

                        // Wait until validator wallet will have enough funds
                        wait_for_balance(
                            "Waiting for some funds to deploy stEVER DePool strategy",
                            deployment_message.amount + ONE_EVER,
                            || wallet.get_balance(),
                        )
                        .await?;

                        // Send an internal message to the factory
                        let spinner = Spinner::start("Deploying stEVER DePool strategy");
                        let depool_strategy = wallet
                            .call(deployment_message)
                            .await
                            .and_then(strategy_factory::StrategyFactory::extract_strategy_address)
                            .context("failed to deploy stEVER DePool strategy")?;

                        spinner.println(format!(
                            "{}\n{}\n\n",
                            style("Strategy address:").green().bold(),
                            style(&depool_strategy).bold()
                        ));

                        // Stever depool strategy is now deployed
                        break (Some(spinner), depool_strategy);
                    }
                    _ => {
                        // Ask for existing strategy
                        let OptionalAddressInput(strategy) = Input::with_theme(theme)
                            .with_prompt("Specify strategy address")
                            .allow_empty(true)
                            .validate_with(|addr: &OptionalAddressInput| {
                                let OptionalAddressInput(Some(address)) = addr else { return Ok(()) };

                                // Check whether strategy exists
                                match block_in_place(async {
                                    strategy::Strategy::new(address.clone(), subscription.clone())
                                        .get_details()
                                        .await
                                        .context("failed to get details")
                                }) {
                                    // Check whether strategy is for the same depool
                                    Ok(details) if &details.depool != depool.address() => {
                                        Err("Strategy was deployed for the different DePool".to_owned())
                                    }
                                    Ok(_) => Ok(()),
                                    Err(e) => Err(format!("Invalid strategy: {e:?}")),
                                }
                            })
                            .interact_text()?;

                        match strategy {
                            // Use the specified strategy
                            Some(strategy) => break (None, strategy),
                            // Return back to the selection prompt
                            None => continue,
                        }
                    }
                }
            };

            let spinner = spinner.get_or_insert_with(|| Spinner::start(""));
            spinner.set_message("Updating DePool info");

            // Set stever depool strategy as an allowed participant
            wallet
                .call(depool.set_allowed_participant(&strategy)?)
                .await
                .context("failed to set update DePool strategy")?;

            println!("DePool contract was successfully deployed!");
        }
    }

    // Check validator stake
    steps.next("Checking validator's stake");

    // Get full depool info
    let depool_state = depool.get_state().await?;
    let depool_info = depool
        .get_info(&depool_state)
        .context("failed to get DePool info")?;
    let rounds = depool.get_rounds(&depool_state)?;

    // Check if validator is a depool participant
    let participant_info = depool
        .get_participant_info(&depool_state, wallet.address())
        .context("failed to get participant info")?;

    if let Some(participant_info) = participant_info {
        anyhow::ensure!(rounds.len() == 4, "invalid DePool rounds");

        // Compute remaining validator assurance
        let mut remaining = 0;
        for next_round in rounds.into_keys().skip(2) {
            let validator_stake = participant_info.compute_total_stake(next_round);
            if let Some(diff) = depool_info.validator_assurance.checked_sub(validator_stake) {
                remaining += diff;
            }
        }

        if remaining > 0 {
            let remaining = std::cmp::max(ONE_EVER, remaining as u128) + ONE_EVER;
            if !confirm(
                theme,
                true,
                "Validator stake is too small. Prepare balance for the ordinary stake?",
            )? {
                return Ok(());
            }

            // Toupup validator wallet
            wait_for_balance("Waiting for the stake balance", remaining, || {
                wallet.get_balance()
            })
            .await?;
        }
    } else {
        let target_balance = depool_info.validator_assurance as u128 * 2 + ONE_EVER;

        // Check if validator balance is enough for the validator assurance
        let validator_balance = wallet
            .get_balance()
            .await
            .context("failed to get validator wallet balance")?;

        if !matches!(validator_balance, Some(balance) if balance >= target_balance) {
            if !confirm(
                theme,
                true,
                "Validator is not a DePool participant yet. Prepare ordinary stake?",
            )? {
                return Ok(());
            }

            // Toupup validator wallet
            wait_for_balance("Waiting for the stake balance", target_balance, || {
                wallet.get_balance()
            })
            .await?;
        }
    }

    // Done
    steps.next("Everything is ready for the validation!");

    Ok(())
}

async fn wait_for_balance<T, F>(prompt: T, target: u128, mut f: impl FnMut() -> F) -> Result<u128>
where
    T: std::fmt::Display,
    F: Future<Output = Result<Option<u128>>>,
{
    let spinner = Spinner::start(format!("{prompt}: fetching balance..."));
    let interval = std::time::Duration::from_secs(1);
    loop {
        match f().await? {
            Some(balance) if balance >= target => break Ok(balance),
            Some(balance) => spinner.set_message(format!(
                "{prompt}: account balance is not enough {}",
                note(format!("{} / {} EVER", Ever(balance), Ever(target)))
            )),
            None => spinner.set_message(format!(
                "{prompt}: account does not exist yet {}",
                note(format!("0 / {} EVER", Ever(target)))
            )),
        }
        tokio::time::sleep(interval).await;
    }
}

fn prepare_keys<P: AsRef<Path>>(
    theme: &dyn Theme,
    name: &str,
    path: P,
) -> Result<ed25519_dalek::Keypair> {
    selector_variant!(Action, {
        Existing => "Use existing keys",
        Generate => "Generate new keys",
        Import => "Import seed",
    });

    let path = path.as_ref();

    let mut items = Action::all();
    if !path.exists() {
        items.remove(0);
    }

    let mut select = Select::with_theme(theme);
    select.items(&items).default(0);

    let store_keys = |keys: &StoredKeys| -> Result<bool> {
        if path.exists() && !confirm(theme, false, "Overwrite existing keys?")? {
            return Ok(false);
        }
        keys.store(path)?;
        Ok(true)
    };

    let stored_keys = loop {
        match items[select.interact()?] {
            Action::Existing => match StoredKeys::load(path) {
                Ok(keys) => break keys,
                Err(e) => {
                    print_error(format!("failed to load existing keys: {e:?}"));
                    continue;
                }
            },
            Action::Generate => {
                let keys = StoredKeys::generate()?;
                let true = store_keys(&keys)? else { continue; };
                break keys;
            }
            Action::Import => {
                let seed = Input::with_theme(theme)
                    .with_prompt(name)
                    .validate_with(|phrase: &String| {
                        crypto::validate_phrase(phrase, StoredKeys::DEFAULT_MNEMONIC_TYPE)
                    })
                    .interact_text()?;
                let keys = StoredKeys::from_seed(seed)?;
                let true = store_keys(&keys)? else { continue; };
                break keys;
            }
        }
    };

    Ok(stored_keys.as_keypair())
}
