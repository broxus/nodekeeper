use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use argh::FromArgs;
use console::style;
use dialoguer::theme::Theme;
use dialoguer::{Input, Select};

use super::{Template, TemplateValidator, TemplateValidatorDePool, TemplateValidatorSingle};
use crate::cli::{CliContext, ProjectDirs};
use crate::config::*;
use crate::contracts::*;
use crate::crypto;
use crate::util::*;

const DEFAULT_STAKE_FACTOR: f64 = 3.0;

const DEFAULT_MIN_STAKE: u64 = 10;
const DEFAULT_VALIDATOR_ASSURANCE: u64 = 10_000;
const DEFAULT_PARTICIPANT_REWARD_FRACTION: u8 = 95;

const DEFAULT_STRATEGY_FACTORY: &str =
    "0:519a1205bd021e5e0aa4b64f5ab689bc383efb4f94f283eac78926da71cfe100";

#[derive(FromArgs)]
/// Deploys contracts required for validation
#[argh(subcommand, name = "contracts")]
pub struct Cmd {}

impl Cmd {
    pub async fn run(
        self,
        theme: &dyn Theme,
        ctx: &CliContext,
        template: &Option<Template>,
    ) -> Result<()> {
        let template = match template {
            Some(template) => match &template.validator {
                Some(validator) => Some(validator),
                None => {
                    println!("`validator` info is empty in the provided template");
                    return Ok(());
                }
            },
            None => None,
        };

        let mut config = ctx.load_config()?;
        let dirs = ctx.dirs();

        // Create keys directory if it doesn't exist
        if !dirs.keys_dir.exists() {
            std::fs::create_dir_all(&dirs.keys_dir).context("failed to create keys dir")?;
        }

        // Check whether validation was already configured
        if config.validator.is_some() {
            let overwrite = match template {
                Some(TemplateValidator::Single(t)) => t.overwrite,
                Some(TemplateValidator::DePool(t)) => t.overwrite,
                None => confirm(
                    theme,
                    false,
                    "Validator is already configured. Update config?",
                )?,
            };
            if !overwrite {
                return Ok(());
            }

            if template.is_some() && overwrite {
                println!("Overwriting validator config");
            }
        }

        match template {
            Some(TemplateValidator::Single(template)) => {
                prepare_single_validator(theme, dirs, Some(template), &mut config)
            }
            Some(TemplateValidator::DePool(template)) => {
                prepare_depool_validator(theme, dirs, Some(template), &mut config)
            }
            // Select validator type
            None => match Select::with_theme(theme)
                .with_prompt("Select validator type")
                .item("Single")
                .item("DePool")
                .default(0)
                .interact()?
            {
                // Prepare validator as a single node
                0 => prepare_single_validator(theme, dirs, None, &mut config),
                // Prepare validator as a depool
                _ => prepare_depool_validator(theme, dirs, None, &mut config),
            },
        }
    }
}

fn prepare_single_validator(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    template: Option<&TemplateValidatorSingle>,
    app_config: &mut AppConfig,
) -> Result<()> {
    use crate::contracts::*;

    const MIN_STAKE: u64 = 10_000 * ONE_EVER as u64;
    const MAX_STAKE: u64 = 10_000_000 * ONE_EVER as u64;

    let mut steps = Steps::new(2);

    // Prepare validator wallet
    steps.next("Creating validator wallet");
    let (_, keypair) = KeysSelector {
        theme,
        prompt: "Validator wallet",
        path: &dirs.validator_keys,
        allow_new: true,
    }
    .interact(template.map(|t| t.overwrite_validator_keys))?;

    let wallet_address = wallet::compute_wallet_address(-1, &keypair.public);

    // Configure stake params
    steps.next("Configuring the stake");

    // Configure stake per round
    let stake_per_round = match template {
        Some(template) => {
            let stake = template.stake_per_round;
            if stake > MAX_STAKE {
                anyhow::bail!("Too big stake (max stake is {} EVER)", Ever(MAX_STAKE));
            } else if stake < MIN_STAKE {
                anyhow::bail!("Too small stake (min stake is {} EVER)", Ever(MIN_STAKE));
            }
            stake
        }
        None => Input::with_theme(theme)
            .with_prompt("Stake per round (EVER)")
            .validate_with(|stake: &u64| match stake.saturating_mul(ONE_EVER as u64) {
                x if x > MAX_STAKE => Err(format!(
                    "Too big stake (max stake is {} EVER)",
                    Ever(MAX_STAKE)
                )),
                x if x < MIN_STAKE => Err(format!(
                    "Too small stake (min stake is {} EVER)",
                    Ever(MIN_STAKE)
                )),
                _ => Ok(()),
            })
            .interact_text()?
            .saturating_mul(ONE_EVER as u64),
    };

    // Configure stake factor
    let stake_factor = configure_stake_factor(theme, template.map(|t| t.stake_factor))?;

    // Save config
    app_config.validator = Some(AppConfigValidator::Single(AppConfigValidatorSingle {
        address: wallet_address.clone(),
        stake_per_round,
        stake_factor: Some(stake_factor),
    }));
    dirs.store_app_config(app_config)?;

    // Done
    steps.next("Validator configured successfully. Great!");

    let target_balance = stake_per_round as u128 * 2 + Wallet::INITIAL_BALANCE;

    println!(
        "\n{}\n{}\n\n{} {}{}\n\n{}\n{}",
        console::style("Validator wallet address:").green().bold(),
        console::style(wallet_address).bold(),
        console::style("Required validator wallet balance:")
            .green()
            .bold(),
        console::style(format!("{} EVER", Ever(target_balance))).bold(),
        console::style(format!(
            "\n  • {} EVER, maintenance balance\
             \n  • 2 x {} EVER, stakes for each round",
            Ever(Wallet::INITIAL_BALANCE),
            Ever(target_balance)
        ))
        .dim(),
        console::style("Make sure you back up your keys:")
            .yellow()
            .bold(),
        console::style(dirs.validator_keys.display()).bold()
    );

    Ok(())
}

fn prepare_depool_validator(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    template: Option<&TemplateValidatorDePool>,
    app_config: &mut AppConfig,
) -> Result<()> {
    use crate::contracts::*;

    let (mut steps, params) = match template {
        Some(template) => prepare_new_depool(theme, dirs, Some(template))?,
        None => match Select::with_theme(theme)
            .item("Deploy new DePool")
            .item("Use existing DePool")
            .default(0)
            .interact()?
        {
            0 => prepare_new_depool(theme, dirs, None)?,
            _ => prepare_existing_depool(theme, dirs)?,
        },
    };

    // Save config
    app_config.validator = Some(AppConfigValidator::DePool(Box::new(params.clone())));
    dirs.store_app_config(app_config)?;

    // Done
    steps.next("Everything is ready for the validation!");

    println!(
        "\n{}\n{}\n\n{}\n{}",
        style("Validator wallet address:").green().bold(),
        style(params.owner).bold(),
        style("DePool address:").green().bold(),
        style(params.depool).bold(),
    );

    if let Some(deployment) = params.deploy {
        let strategy_fee = StrategyFactory::DEPLOYMENT_FEE;

        let mut target_balance = deployment.validator_assurance as u128 * 2
            + Wallet::INITIAL_BALANCE
            + DePool::INITIAL_BALANCE;

        let mut factory_deployment_note = "".to_owned();
        if params.strategy_factory.is_some() {
            target_balance += strategy_fee;
            factory_deployment_note =
                format!("\n  • {} EVER, strategy deployment fee", Ever(strategy_fee));
        }

        println!(
            "\n{} {}{}",
            style("Required validator wallet balance:").green().bold(),
            style(format!("{} EVER", Ever(target_balance))).bold(),
            style(format!(
                "\n  • {} EVER, maintenance balance\
                 \n  • {} EVER, DePool deployment fee\
                 {factory_deployment_note}\
                 \n  • 2 x {} EVER, stakes for each round",
                Ever(Wallet::INITIAL_BALANCE),
                Ever(DePool::INITIAL_BALANCE),
                Ever(deployment.validator_assurance),
            ))
            .dim()
        );
    }

    println!(
        "\n{}\n{}\n{}",
        console::style("Make sure you back up your keys:")
            .yellow()
            .bold(),
        console::style(dirs.validator_keys.display()).bold(),
        console::style(dirs.depool_keys.display()).bold(),
    );

    Ok(())
}

fn prepare_new_depool(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    template: Option<&TemplateValidatorDePool>,
) -> Result<(Steps, AppConfigValidatorDePool)> {
    let mut steps = Steps::new(2);

    // Prepare validator wallet
    steps.next("Creating validator wallet");

    let (is_new_wallet, wallet_keypair) = KeysSelector {
        theme,
        prompt: "Validator wallet",
        path: &dirs.validator_keys,
        allow_new: true,
    }
    .interact(template.map(|t| t.overwrite_validator_keys))?;

    // TODO: Select wallet type

    let wallet_address = wallet::compute_wallet_address(0, &wallet_keypair.public);

    // Create depool
    steps.next("Creating DePool");

    // Generate depool keys
    let (is_new_depool, depool_keypair) = KeysSelector {
        theme,
        prompt: "DePool",
        path: &dirs.depool_keys,
        allow_new: true,
    }
    .interact(template.map(|t| t.overwrite_depool_keys))?;

    // Select depool type
    let depool_type = match template {
        Some(template) => template.depool_type,
        None => match Select::with_theme(theme)
            .with_prompt("Select DePool type")
            .item("stEVER")
            .item("DePoolV3")
            .default(0)
            .interact()?
        {
            0 => DePoolType::LATEST_STEVER,
            _ => DePoolType::DefaultV3,
        },
    };

    // Compute depool address
    let depool_address = depool_type
        .compute_depool_address(&depool_keypair.public)
        .context("failed to compute DePool address")?;

    // Configure min participants stake
    let min_stake = match template {
        Some(template) => {
            let stake = template.deploy.min_stake;
            anyhow::ensure!(
                stake >= 10 * ONE_EVER as u64,
                "Minimum stake is too small (< 10 EVER)"
            );
            stake
        }
        None => Input::with_theme(theme)
            .with_prompt("Minimum participant stake (EVER)")
            .default(DEFAULT_MIN_STAKE)
            .validate_with(|value: &u64| match *value {
                x if x < 10 => Err("Minimum stake is too small (< 10 EVER)"),
                _ => Ok(()),
            })
            .interact_text()?
            .saturating_mul(ONE_EVER as u64),
    };

    // Configure validator assurance
    let validator_assurance = match template {
        Some(template) => {
            let assurance = template.deploy.validator_assurance;
            anyhow::ensure!(
                assurance >= 10 * ONE_EVER as u64,
                "Too small validator assurance (< 10 EVER)"
            );
            anyhow::ensure!(
                assurance >= min_stake,
                "Validator assurance is less than minimum stake"
            );
            assurance
        }
        None => Input::with_theme(theme)
            .with_prompt("Validator assurance (EVER)")
            .default(DEFAULT_VALIDATOR_ASSURANCE)
            .validate_with(|value: &u64| match *value {
                x if x < 10 => Err("Too small validator assurance (< 10 EVER)"),
                x if x.saturating_mul(ONE_EVER as u64) < min_stake => {
                    Err("Validator assurance is less than minimum stake")
                }
                _ => Ok(()),
            })
            .interact_text()?
            .saturating_mul(ONE_EVER as u64),
    };

    // Configure participant reward fraction
    let participant_reward_fraction = match template {
        Some(template) => {
            let frac = template.deploy.participant_reward_fraction;
            anyhow::ensure!(frac > 0, "Too small fraction (< 1%)");
            anyhow::ensure!(frac < 100, "Too big fraction (> 99%)");
            frac
        }
        None => Input::with_theme(theme)
            .with_prompt("Participant reward fraction (%, 1..99)")
            .default(DEFAULT_PARTICIPANT_REWARD_FRACTION)
            .validate_with(|value: &u8| match *value {
                x if x < 1 => Err("Too small fraction (< 1%)"),
                x if x > 99 => Err("Too big fraction (> 99%)"),
                _ => Ok(()),
            })
            .interact_text()?,
    };

    // Configure stake factor
    let stake_factor = configure_stake_factor(theme, template.map(|t| t.stake_factor))?;

    let mut params = AppConfigValidatorDePool {
        owner: wallet_address,
        depool: depool_address,
        depool_type,
        stake_factor: Some(stake_factor),
        strategy_factory: None,
        strategy: None,
        deploy: Some(AppConfigDePoolDeploymentParams {
            min_stake,
            validator_assurance,
            participant_reward_fraction,
        }),
    };

    // Configure stEVER strategies stuff
    if depool_type.is_stever() {
        match template {
            Some(template) => {
                params.strategy_factory = Some(
                    template
                        .strategy_factory
                        .clone()
                        .unwrap_or(DEFAULT_STRATEGY_FACTORY.parse().unwrap()),
                );

                if !is_new_depool {
                    params.strategy = template.strategy.clone();
                }
            }
            None => {
                let strategy = if is_new_wallet || is_new_depool {
                    // Always deploy new strategy is new keys were generated
                    StrategyAction::DeployNew.run(theme)?
                } else {
                    // Allow specifying existing strategy otherwise
                    let items = [StrategyAction::DeployNew, StrategyAction::SetExisting];
                    let action = Select::with_theme(theme)
                        .items(&items)
                        .default(0)
                        .interact()?;
                    items[action].run(theme)?
                };
                params.strategy_factory = strategy.factory;
                params.strategy = strategy.existing;
            }
        }
    }

    // Done
    Ok((steps, params))
}

fn prepare_existing_depool(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
) -> Result<(Steps, AppConfigValidatorDePool)> {
    let mut steps = Steps::new(2);

    // Prepare validator wallet
    steps.next("Creating validator wallet");

    let (_, wallet_keypair) = KeysSelector {
        theme,
        prompt: "Validator wallet seed phrase",
        path: &dirs.validator_keys,
        allow_new: false,
    }
    .interact(None)?;

    // TODO: Select wallet type

    let wallet_address = wallet::compute_wallet_address(0, &wallet_keypair.public);

    // Prepare validator wallet
    steps.next("Creating DePool");

    // Configure existing depool address
    let AddressInput(depool_address) = Input::with_theme(theme)
        .with_prompt("Specify existing DePool address")
        .interact_text()?;

    // Generate depool keys
    let (_, depool_keypair) = KeysSelector {
        theme,
        prompt: "DePool seed phrase",
        path: &dirs.depool_keys,
        allow_new: false,
    }
    .interact(None)?;

    // Guess depool type from pubkey and the specified address
    let depool_type = DePoolType::guess(&depool_address, &depool_keypair.public)
        .context("failed to guess depool type")?
        .context("invalid keys or unknown DePool contract")?;

    // Configure stake factor
    let stake_factor = configure_stake_factor(theme, None)?;

    // Done
    let mut params = AppConfigValidatorDePool {
        owner: wallet_address,
        depool: depool_address,
        depool_type,
        stake_factor: Some(stake_factor),
        strategy_factory: None,
        strategy: None,
        deploy: None,
    };

    // Configure stEVER strategies stuff
    if depool_type.is_stever() {
        let items = StrategyAction::all();
        let action = Select::with_theme(theme)
            .items(&items)
            .default(0)
            .interact()?;
        let strategy = items[action].run(theme)?;
        params.strategy_factory = strategy.factory;
        params.strategy = strategy.existing;
    }

    // Done
    Ok((steps, params))
}

impl DePoolType {
    fn guess(
        address: &ton_block::MsgAddressInt,
        pubkey: &ed25519_dalek::PublicKey,
    ) -> Result<Option<Self>> {
        for ty in [Self::DefaultV3, Self::StEverV1, Self::StEverV2] {
            if address == &ty.compute_depool_address(pubkey)? {
                return Ok(Some(ty));
            }
        }
        Ok(None)
    }
}

fn configure_stake_factor(theme: &dyn Theme, template: Option<Option<u32>>) -> Result<u32> {
    const MIN_STAKE_FACTOR: f64 = 1.0;
    const MAX_STAKE_FACTOR: f64 = 3.0;
    const MIN_STAKE_FACTOR_REPR: u32 = 65536;
    const MAX_STAKE_FACTOR_REPR: u32 = 3 * 65536;

    fn to_factor_repr(factor: f64) -> u32 {
        (factor * 65536.0) as u32
    }

    fn clamp_stake_factor(factor: u32) -> u32 {
        std::cmp::min(
            std::cmp::max(factor, MIN_STAKE_FACTOR_REPR),
            MAX_STAKE_FACTOR_REPR,
        )
    }

    Ok(match template {
        Some(None) => MAX_STAKE_FACTOR_REPR,
        Some(Some(stake_factor)) => clamp_stake_factor(stake_factor),
        // Configure factor
        None => Input::with_theme(theme)
            .with_prompt("Stake factor")
            .with_initial_text(DEFAULT_STAKE_FACTOR.to_string())
            .validate_with(|factor: &f64| match *factor {
                x if x > MAX_STAKE_FACTOR => {
                    Err(format!("Too big stake factor (max is {MAX_STAKE_FACTOR})"))
                }
                x if x < MIN_STAKE_FACTOR => Err(format!(
                    "Too small stake factor (min is {MIN_STAKE_FACTOR})"
                )),
                _ => Ok::<_, String>(()),
            })
            .interact_text()
            .map(to_factor_repr)
            .map(clamp_stake_factor)?,
    })
}

selector_variant!(StrategyAction, {
    Skip => "Leave as is",
    DeployNew => "Deploy new stEVER DePool strategy",
    SetExisting => "Set existing stEVER DePool strategy",
});

impl StrategyAction {
    fn run(self, theme: &dyn Theme) -> Result<Strategy> {
        let default_strategy_factory: ton_block::MsgAddressInt =
            DEFAULT_STRATEGY_FACTORY.parse().unwrap();

        Ok(match self {
            Self::Skip => Strategy {
                factory: None,
                existing: None,
            },
            Self::DeployNew => {
                let AddressInput(factory) = Input::with_theme(theme)
                    .with_prompt("Specify stEVER strategy factory")
                    .default(AddressInput(default_strategy_factory))
                    .interact_text()?;

                Strategy {
                    factory: Some(factory),
                    existing: None,
                }
            }
            Self::SetExisting => {
                println!(
                    "NOTE: Specified strategy address must be deployed \
                    for the current DePool"
                );

                let AddressInput(existing) = Input::with_theme(theme)
                    .with_prompt("Specify strategy address")
                    .interact_text()?;

                Strategy {
                    factory: None,
                    existing: Some(existing),
                }
            }
        })
    }
}

struct Strategy {
    factory: Option<ton_block::MsgAddressInt>,
    existing: Option<ton_block::MsgAddressInt>,
}

struct KeysSelector<'a, P> {
    theme: &'a dyn Theme,
    prompt: &'a str,
    path: P,
    allow_new: bool,
}

impl<P: AsRef<Path>> KeysSelector<'_, P> {
    fn interact(
        self,
        overwrite_from_template: Option<bool>,
    ) -> Result<(bool, ed25519_dalek::Keypair)> {
        match overwrite_from_template {
            Some(overwrite) => self.interact_from_template(overwrite),
            None => self.interact_with_cli(),
        }
    }

    fn interact_from_template(self, overwrite: bool) -> Result<(bool, ed25519_dalek::Keypair)> {
        let path = self.path.as_ref();
        let exists = path.exists();
        anyhow::ensure!(exists || self.allow_new, "Keys not found");

        Ok(if exists && !overwrite {
            let keys = StoredKeys::load(path).context("failed to load existing keys")?;
            (false, keys.as_keypair())
        } else {
            let keys = StoredKeys::generate()?;
            keys.store(path)?;
            (true, keys.as_keypair())
        })
    }

    fn interact_with_cli(self) -> Result<(bool, ed25519_dalek::Keypair)> {
        selector_variant!(Action, {
            Existing => "Use existing keys",
            Generate => "Generate new keys",
            Import => "Import seed",
        });

        let path = self.path.as_ref();

        // Helper method
        let store_keys = |keys: &StoredKeys| -> Result<bool> {
            if path.exists() && !confirm(self.theme, false, "Overwrite existing keys?")? {
                return Ok(false);
            }
            keys.store(path)?;
            Ok(true)
        };

        // Construct selector
        let mut items = Vec::new();
        if path.exists() {
            items.push(Action::Existing);
        }
        if self.allow_new {
            items.push(Action::Generate);
        }
        items.push(Action::Import);

        let mut select = Select::with_theme(self.theme);
        select.items(&items).default(0);

        // Try asking user until he selects a correct variant
        let (is_new, stored_keys) = loop {
            // Determine input action
            let action = if items.len() > 1 {
                items[select.interact()?]
            } else {
                items[0]
            };

            // Do action
            match action {
                Action::Existing => match StoredKeys::load(path) {
                    Ok(keys) => break (false, keys),
                    Err(e) => {
                        print_error(format!("failed to load existing keys: {e:?}"));
                        continue;
                    }
                },
                Action::Generate => {
                    let keys = StoredKeys::generate()?;
                    if !store_keys(&keys)? {
                        continue;
                    }
                    break (true, keys);
                }
                Action::Import => {
                    let seed: SeedOrSecretInput = Input::with_theme(self.theme)
                        .with_prompt(format!("{} seed phrase or secret", self.prompt))
                        .interact_text()?;
                    let keys = seed.try_into_stored_keys()?;
                    if !store_keys(&keys)? {
                        continue;
                    }
                    break (false, keys);
                }
            }
        };

        Ok((is_new, stored_keys.as_keypair()))
    }
}

#[derive(Clone)]
pub enum SeedOrSecretInput {
    Seed(String),
    Secret([u8; 32]),
}

impl SeedOrSecretInput {
    fn try_into_stored_keys(self) -> Result<StoredKeys> {
        match self {
            Self::Seed(seed) => StoredKeys::from_seed(seed),
            Self::Secret(secret) => StoredKeys::from_secret(secret),
        }
    }
}

impl std::fmt::Display for SeedOrSecretInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Seed(seed) => seed.fmt(f),
            Self::Secret(secret) => hex::encode(secret).fmt(f),
        }
    }
}

impl FromStr for SeedOrSecretInput {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();

        if !s.contains(' ') {
            let hex = match s.len() {
                64 => Some(hex::decode(s).ok()),
                44 => Some(base64::decode(s).ok()),
                _ => None,
            };

            if let Some(hex) = hex {
                let data = hex
                    .and_then(|data| data.try_into().ok())
                    .context("invalid secret key")?;
                return Ok(Self::Secret(data));
            }
        }

        crypto::validate_phrase(s, StoredKeys::DEFAULT_MNEMONIC_TYPE)?;
        Ok(Self::Seed(s.to_owned()))
    }
}
