use std::borrow::Cow;
use std::future::Future;
use std::io::Write;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use argh::FromArgs;
use console::style;
use dialoguer::theme::Theme;
use dialoguer::{Confirm, Input, Select};
use reqwest::Url;
use tokio::process::Command;

use super::{CliContext, ProjectDirs, VALIDATOR_MANAGER_SERVICE, VALIDATOR_SERVICE};
use crate::config::*;
use crate::crypto;
use crate::node_tcp_rpc::NodeTcpRpc;
use crate::node_udp_rpc::NodeUdpRpc;
use crate::subscription::Subscription;
use crate::util::*;

const DEFAULT_CONTROL_PORT: u16 = 5031;
const DEFAULT_LOCAL_ADNL_PORT: u16 = 5032;
const DEFAULT_ADNL_PORT: u16 = 30100;
const DEFAULT_NODE_REPO: &str = "https://github.com/tonlabs/ton-labs-node.git";
const DEFAULT_NODE_DB_PATH: &str = "/var/ever/db";

const DEFAULT_STAKE_FACTOR: f64 = 3.0;

const DEFAULT_MIN_STAKE: u64 = 10;
const DEFAULT_VALIDATOR_ASSURANCE: u64 = 10_000;
const DEFAULT_PARTICIPANT_REWARD_FRACTION: u8 = 95;

const MNEMONIC_TYPE: crypto::MnemonicType = crypto::MnemonicType::Bip39;
const DERIVATION_PATH: &str = crypto::DEFAULT_PATH;

#[derive(FromArgs)]
/// Prepares configs and binaries
#[argh(subcommand, name = "init")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: Option<SubCmd>,
}

impl Cmd {
    pub async fn run(self, ctx: CliContext) -> Result<()> {
        let theme = &dialoguer::theme::ColorfulTheme::default();
        let dirs = ctx.dirs();

        match self.subcommand {
            None => CmdInit.run(theme, dirs).await,
            Some(SubCmd::Systemd(cmd)) => cmd.run(theme, dirs).await,
            Some(SubCmd::Contracts(cmd)) => cmd.run(ctx, theme).await,
        }
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    Systemd(CmdInitSystemd),
    Contracts(CmdInitContracts),
}

struct CmdInit;

impl CmdInit {
    const STEPS: usize = 2;

    async fn run(self, theme: &dyn Theme, dirs: &ProjectDirs) -> Result<()> {
        let is_root = system::is_root();
        let mut steps = Steps::new(Self::STEPS + CmdInitSystemd::STEPS * (is_root as usize));

        steps.next("Preparing configs");

        if !prepare_root_dir(theme, dirs)? {
            return Ok(());
        }

        let global_config = load_global_config(theme, dirs).await?;
        let mut node_config = load_node_config(dirs)?;
        let mut app_config = load_app_config(dirs)?;

        if !setup_control_server(theme, dirs, &mut app_config, &mut node_config)? {
            return Ok(());
        }

        if !setup_adnl(
            theme,
            dirs,
            &mut app_config,
            &mut node_config,
            &global_config,
        )
        .await?
        {
            return Ok(());
        }

        setup_node_config_paths(theme, dirs, &mut node_config)?;

        steps.next("Preparing binary");

        if !setup_binary(theme, dirs).await? {
            return Ok(());
        }

        if is_root {
            steps.next("Preparing services");
            prepare_services(theme, dirs)?;

            steps.next("Reloading systemd configs");
            systemd_daemon_reload().await?;

            steps.next("Validator node is configured now. Great!");
            start_services(theme).await?;
        } else {
            steps.next("Validator node is configured now. Great!");
            check_systemd_service(dirs)?;
        }

        Ok(())
    }
}

#[derive(FromArgs)]
/// Creates systemd services (`ever-validator` and `ever-validator-manager`)
#[argh(subcommand, name = "systemd")]
struct CmdInitSystemd {}

impl CmdInitSystemd {
    const STEPS: usize = 2;

    async fn run(self, theme: &dyn Theme, dirs: &ProjectDirs) -> Result<()> {
        let mut steps = Steps::new(Self::STEPS);

        steps.next("Preparing services");
        prepare_services(theme, dirs)?;

        steps.next("Reloading systemd configs");
        systemd_daemon_reload().await?;

        steps.next("Systemd services are configured now. Great!");
        start_services(theme).await?;

        Ok(())
    }
}

#[derive(FromArgs)]
/// Deploys contracts required for validation
#[argh(subcommand, name = "contracts")]
struct CmdInitContracts {}

impl CmdInitContracts {
    async fn run(self, ctx: CliContext, theme: &dyn Theme) -> Result<()> {
        let mut config = ctx.load_config()?;
        let dirs = ctx.dirs();

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

        // Check node status
        node_tcp_rpc.get_stats().await?.try_into_running()?;

        // Create subscription
        let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);

        match Select::with_theme(theme)
            .with_prompt("Select validator type")
            .item("Single")
            .item("DePool")
            .default(0)
            .interact()?
        {
            0 => prepare_single_validator(theme, dirs, &mut config, subscription).await,
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

    // Float conversion factor
    const F: f64 = 65536.0;

    fn to_factor_repr(factor: f64) -> u32 {
        (factor * F) as u32
    }

    fn from_factor_repr(factor: u32) -> f64 {
        factor as f64 / F
    }

    // Fetch blockchain stakes config for input validation
    let stakes_config = subscrioption
        .get_blockchain_config()
        .await
        .context("failed to get blockchain config")?
        .stakes_config()
        .context("failed to get stakes config")?;
    let min_stake = num::integer::div_ceil(stakes_config.min_stake.0, ONE_EVER);
    let max_stake = num::integer::div_floor(stakes_config.max_stake.0, ONE_EVER);
    let max_stake_factor = stakes_config.max_stake_factor;

    let mut steps = Steps::new(3);

    // Prepare validator wallet
    steps.next("Creating validator wallet");
    let keypair = prepare_seed(theme, "Validator wallet seed phrase")?;

    let wallet = wallet::Wallet::new(-1, keypair, subscrioption)
        .context("failed to create validator wallet")?;

    print_important_value("Validator wallet address", wallet.address());

    // Configure stake params
    steps.next("Configuring the stake");

    let stake_per_round: u64 = Input::with_theme(theme)
        .with_prompt("Stake per round (EVER)")
        .validate_with(|stake: &u64| match *stake as u128 {
            x if x > max_stake => Err(format!("Too big stake (max stake is {max_stake} EVER)")),
            x if x < min_stake => Err(format!("Too small stake (min stake is {min_stake} EVER)")),
            _ => Ok(()),
        })
        .interact_text()?;
    let stake_per_round = stake_per_round.saturating_mul(ONE_EVER as u64);

    let stake_factor: f64 = Input::with_theme(theme)
        .with_prompt("Stake factor")
        .with_initial_text(DEFAULT_STAKE_FACTOR.to_string())
        .validate_with(|factor: &f64| match to_factor_repr(*factor) {
            x if x > max_stake_factor => Err(format!(
                "Too big stake factor (max is {})",
                from_factor_repr(max_stake_factor)
            )),
            x if x < to_factor_repr(1.0) => Err("Too small stake factor (min is 1.0)".to_owned()),
            _ => Ok::<_, String>(()),
        })
        .interact_text()?;
    let stake_factor = std::cmp::min(to_factor_repr(stake_factor), max_stake_factor);

    // Save config
    app_config.validation = Some(AppConfigValidation::Single {
        address: wallet.address().clone(),
        stake_per_round,
        stake_factor,
    });
    dirs.store_app_config(app_config)?;

    // Wait until validator wallet will have enough funds for staking
    steps.next("Replenishing the balance");

    let balance = wait_for_balance(
        "Waiting for the initial balance",
        stake_per_round as u128 * 2 + 10 * ONE_EVER,
        || async { wallet.get_balance().await },
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
    subscrioption: Arc<Subscription>,
) -> Result<()> {
    use crate::contracts::*;

    let mut steps = Steps::new(4);

    // Prepare validator wallet
    steps.next("Creating validator wallet");
    let wallet_keypair = prepare_seed(theme, "Validator wallet seed phrase")?;

    let wallet = wallet::Wallet::new(0, wallet_keypair, subscrioption.clone())
        .context("failed to create validator wallet")?;

    print_important_value("Validator wallet address", wallet.address());

    // Create depool
    steps.next("Creating depool");
    let depool_keypair = prepare_seed(theme, "DePool seed phrase")?;

    let depool_type = match Select::with_theme(theme)
        .with_prompt("Select DePool type")
        .item("Default DePoolV3")
        .item("StEver")
        .default(0)
        .interact()?
    {
        0 => DePoolType::DefaultV3,
        _ => DePoolType::StEver,
    };

    let depool = depool::DePool::new(depool_type, depool_keypair, subscrioption)
        .context("failed to create DePool")?;

    print_important_value("DePool address", depool.address());

    // Save config
    app_config.validation = Some(AppConfigValidation::DePool {
        owner: wallet.address().clone(),
        depool: depool.address().clone(),
        depool_type,
    });
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
        let min_stake: u64 = Input::with_theme(theme)
            .with_prompt("Minimal participant stake (EVER)")
            .default(DEFAULT_MIN_STAKE)
            .validate_with(|value: &u64| match *value {
                x if x < 10 => Err("Minimal stake is too small (< 10 EVER)"),
                _ => Ok(()),
            })
            .interact_text()?;
        let min_stake = min_stake.saturating_mul(ONE_EVER as u64);

        let validator_assurance: u64 = Input::with_theme(theme)
            .with_prompt("Validator assurance (EVER)")
            .default(DEFAULT_VALIDATOR_ASSURANCE)
            .validate_with(|value: &u64| match *value {
                x if x < 10 => Err("Too small validator assurance (< 10 EVER)"),
                _ => Ok(()),
            })
            .interact_text()?;
        let validator_assurance = validator_assurance.saturating_mul(ONE_EVER as u64);

        let participant_reward_fraction: u8 = Input::with_theme(theme)
            .with_prompt("Participant reward fraction (%, 1..99)")
            .default(DEFAULT_PARTICIPANT_REWARD_FRACTION)
            .validate_with(|value: &u8| match *value {
                x if x < 1 => Err("Too small fraction (< 1%)"),
                x if x > 99 => Err("Too big fraction (> 99%)"),
                _ => Ok(()),
            })
            .interact_text()?;

        const VALIDATOR_MIN_BALANCE: u128 = 10 * ONE_EVER;
        const DEPOOL_MIN_BALANCE: u128 = 30 * ONE_EVER;

        let balance = wait_for_balance(
            "Waiting for the initial validator balance",
            DEPOOL_MIN_BALANCE + VALIDATOR_MIN_BALANCE,
            || async { wallet.get_balance().await },
        )
        .await?;
        println!("Validator wallet balance: {}", Ever(balance));

        {
            let spinner = Spinner::start("Transferring funds to the DePool contract");
            wallet
                .transfer(InternalMessage::empty(
                    depool.address().clone(),
                    DEPOOL_MIN_BALANCE,
                ))
                .await
                .context("failed to transfer funds to the DePool contract")?;

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
        println!("DePool contract was successfully deployed!");

        // TODO
    }

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

fn prepare_seed(theme: &dyn Theme, name: &str) -> Result<ed25519_dalek::Keypair> {
    fn derive(phrase: &dyn AsRef<str>) -> Result<ed25519_dalek::Keypair> {
        crypto::derive_from_phrase(phrase.as_ref(), MNEMONIC_TYPE, DERIVATION_PATH)
    }

    let seed = match Select::with_theme(theme)
        .item("Generate new seed")
        .item("Import seed")
        .default(0)
        .interact()?
    {
        0 => {
            let seed = crypto::generate_seed(MNEMONIC_TYPE);
            print_important_value(name, &seed);
            seed
        }
        _ => Input::with_theme(theme)
            .with_prompt(name)
            .validate_with(|input: &String| match derive(input) {
                Ok(_) => Ok(()),
                Err(e) => Err(e.to_string()),
            })
            .interact_text()?,
    };
    derive(&seed)
}

fn prepare_root_dir(theme: &dyn Theme, dirs: &ProjectDirs) -> Result<bool> {
    let root = dirs.root();
    if root.exists() {
        return Ok(true);
    }

    if !confirm(
        theme,
        root.is_absolute(),
        format!("Create stEVER root directory? {}", note(root.display())),
    )? {
        return Ok(false);
    }

    std::fs::create_dir_all(root).context("failed create root directory")?;
    Ok(true)
}

async fn load_global_config(theme: &dyn Theme, dirs: &ProjectDirs) -> Result<GlobalConfig> {
    let global_config = dirs.global_config();
    if !global_config.exists() {
        let data = match Select::with_theme(theme)
            .with_prompt("Select network")
            .item("Everscale mainnet")
            .item("Everscale testnet")
            .item("other")
            .default(0)
            .interact()?
        {
            0 => Cow::Borrowed(GlobalConfig::MAINNET),
            1 => Cow::Borrowed(GlobalConfig::TESTNET),
            _ => {
                let url: Url = Input::with_theme(theme)
                    .with_prompt("Config URL")
                    .interact_text()?;

                reqwest::get(url)
                    .await
                    .context("failed to download global config")?
                    .text()
                    .await
                    .context("failed to download global config")
                    .map(Cow::Owned)?
            }
        };

        std::fs::create_dir_all(dirs.node_configs_dir())
            .context("failed to create node configs dir")?;
        dirs.store_global_config(data)?;
    }

    GlobalConfig::load(global_config)
}

fn load_node_config(dirs: &ProjectDirs) -> Result<NodeConfig> {
    let node_config = dirs.node_config();
    if node_config.exists() {
        return NodeConfig::load(node_config);
    }

    let node_config = NodeConfig::generate()?;
    dirs.store_node_config(&node_config)?;
    Ok(node_config)
}

fn load_app_config(dirs: &ProjectDirs) -> Result<AppConfig> {
    let app_config = dirs.app_config();
    if app_config.exists() {
        return AppConfig::load(app_config);
    }

    let app_config = AppConfig::default();
    dirs.store_app_config(&app_config)?;
    Ok(app_config)
}

fn setup_control_server(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    app_config: &mut AppConfig,
    node_config: &mut NodeConfig,
) -> Result<bool> {
    use everscale_crypto::ed25519;

    let rng = &mut rand::thread_rng();

    let control_port = node_config
        .get_suggested_control_port()
        .unwrap_or(DEFAULT_CONTROL_PORT);

    match (&mut app_config.control, node_config.get_control_server()?) {
        (Some(existing_client), Some(mut existing_server)) => {
            let mut server_changed = false;
            let mut client_changed = false;

            let server_port = existing_server.address.port();
            let client_port = existing_client.server_address.port();
            if existing_client.server_address.port() != existing_server.address.port() {
                let port = match Select::with_theme(theme)
                    .with_prompt("stEVER config has different control port. What to do?")
                    .item(format!(
                        "use control port from the node {}",
                        note(server_port)
                    ))
                    .item(format!(
                        "use control port from stEVER {}",
                        note(client_port)
                    ))
                    .item("specify custom port")
                    .default(0)
                    .interact()?
                {
                    0 => server_port,
                    1 => client_port,
                    _ => Input::with_theme(theme)
                        .with_prompt("Specify control port")
                        .interact_text()?,
                };

                client_changed |= port != client_port;
                server_changed |= port != server_port;

                existing_client.server_address.set_port(port);
                existing_server.address.set_port(port);
            }

            let server_pubkey = ed25519::PublicKey::from(&existing_server.server_key);
            if server_pubkey != existing_client.server_pubkey {
                if !confirm(theme, true, "Server pubkey mismatch. Update?")? {
                    return Ok(false);
                }

                existing_client.server_pubkey = server_pubkey;
                client_changed = true;
            }

            if let Some(clients) = &mut existing_server.clients {
                let client_pubkey = ed25519::PublicKey::from(&existing_client.client_secret);
                if !clients.contains(&client_pubkey) {
                    let append = clients.is_empty()
                        || Select::with_theme(theme)
                            .with_prompt("Node config has some clients specified. What to do?")
                            .item("append")
                            .item("replace")
                            .default(0)
                            .interact()?
                            == 0;

                    if !append {
                        clients.clear();
                    }

                    clients.push(client_pubkey);
                    server_changed = true;
                }
            }

            if client_changed {
                dirs.store_app_config(app_config)?;
            }
            if server_changed {
                node_config.set_control_server(&existing_server)?;
                dirs.store_node_config(node_config)?;
            }
        }
        (None, Some(mut existing_server)) => {
            if !confirm(
                theme,
                true,
                "stEVER config doesn't have control server entry. Create?",
            )? {
                return Ok(false);
            }

            let client_key = ed25519::SecretKey::generate(rng);

            let node_config_changed = match &mut existing_server.clients {
                None if !confirm(theme, false, "Allow any clients?")? => {
                    existing_server.clients = Some(vec![ed25519::PublicKey::from(&client_key)]);
                    println!("Generated new client keys");
                    true
                }
                None => false,
                Some(clients) => {
                    let append = clients.is_empty()
                        || Select::with_theme(theme)
                            .with_prompt("Node config has some clients specified. What to do?")
                            .item("append")
                            .item("replace")
                            .default(0)
                            .interact()?
                            == 0;

                    if !append {
                        clients.clear();
                    }

                    clients.push(ed25519::PublicKey::from(&client_key));
                    true
                }
            };

            let addr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, existing_server.address.port());

            app_config.control = Some(AppConfigControl::from_addr_and_keys(
                addr,
                ed25519::PublicKey::from(&existing_server.server_key),
                client_key,
            ));
            dirs.store_app_config(app_config)?;

            if node_config_changed {
                node_config.set_control_server(&existing_server)?;
                dirs.store_node_config(node_config)?;
            }
        }
        (existing_client, None) => {
            if !confirm(
                theme,
                true,
                "Node config doesn't have control server entry. Create?",
            )? {
                return Ok(false);
            }

            if existing_client.is_some()
                && !confirm(theme, false, "Overwrite stEVER control server config?")?
            {
                return Ok(false);
            }

            const LISTEN_ADDR_ITEMS: [(&str, Ipv4Addr); 2] = [
                ("localhost", Ipv4Addr::LOCALHOST),
                ("any", Ipv4Addr::UNSPECIFIED),
            ];

            let listen_addr = Select::with_theme(theme)
                .with_prompt("Control server listen address")
                .item(LISTEN_ADDR_ITEMS[0].0)
                .item(LISTEN_ADDR_ITEMS[1].0)
                .default(0)
                .interact()?;
            let listen_addr = LISTEN_ADDR_ITEMS[listen_addr].1;

            let control_port = Input::with_theme(theme)
                .with_prompt("Specify control port")
                .with_initial_text(control_port.to_string())
                .interact()?;

            let addr = SocketAddrV4::new(listen_addr, control_port);

            let server_key = ed25519::SecretKey::generate(rng);
            let client_key = ed25519::SecretKey::generate(rng);

            app_config.control = Some(AppConfigControl::from_addr_and_keys(
                addr,
                ed25519::PublicKey::from(&server_key),
                client_key,
            ));

            node_config.set_control_server(&NodeConfigControlServer::from_addr_and_keys(
                addr,
                server_key,
                ed25519::PublicKey::from(&client_key),
            ))?;

            dirs.store_app_config(app_config)?;
            dirs.store_node_config(node_config)?;
        }
    }

    Ok(true)
}

async fn setup_adnl(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    app_config: &mut AppConfig,
    node_config: &mut NodeConfig,
    global_config: &GlobalConfig,
) -> Result<bool> {
    const DHT_TAG: usize = 1;
    const OVERLAY_TAG: usize = 2;

    let adnl_port = node_config
        .get_suggested_adnl_port()
        .unwrap_or(DEFAULT_ADNL_PORT);

    let zerostate_file_hash = *global_config.zero_state.file_hash.as_array();

    match (&mut app_config.adnl, node_config.get_adnl_node()?) {
        (Some(adnl_client), Some(adnl_node)) => {
            let server_pubkey = adnl_node.overlay_pubkey()?;
            if adnl_client.server_address != adnl_node.ip_address
                || adnl_client.server_pubkey != server_pubkey
                || adnl_client.zerostate_file_hash != zerostate_file_hash
            {
                if !confirm(theme, false, "ADNL node configuration mismatch. Update?")? {
                    return Ok(false);
                }

                adnl_client.server_address = adnl_node.ip_address;
                adnl_client.server_pubkey = server_pubkey;
                adnl_client.zerostate_file_hash = zerostate_file_hash;

                dirs.store_app_config(app_config)?;
            }
        }
        (None, Some(adnl_node)) => {
            app_config.adnl = Some(AppConfigAdnl {
                client_port: DEFAULT_LOCAL_ADNL_PORT,
                server_address: adnl_node.ip_address,
                server_pubkey: adnl_node.overlay_pubkey()?,
                zerostate_file_hash,
            });

            app_config.store(dirs.app_config())?;
        }
        (_, None) => {
            let addr: Ipv4Addr = {
                let public_ip = public_ip::addr_v4().await;
                let mut input = Input::with_theme(theme);
                if let Some(public_ip) = public_ip {
                    input.with_initial_text(public_ip.to_string());
                }
                input.with_prompt("Enter public ip").interact_text()?
            };

            let adnl_port = Input::with_theme(theme)
                .with_prompt("Specify server ADNL port")
                .with_initial_text(adnl_port.to_string())
                .interact()?;

            let adnl_node = NodeConfigAdnl::from_addr_and_keys(
                SocketAddrV4::new(addr, adnl_port),
                NodeConfigAdnl::generate_keys(),
            );
            node_config.set_adnl_node(&adnl_node)?;

            app_config.adnl = Some(AppConfigAdnl {
                client_port: DEFAULT_LOCAL_ADNL_PORT,
                server_address: adnl_node.ip_address,
                server_pubkey: adnl_node.overlay_pubkey()?,
                zerostate_file_hash,
            });

            dirs.store_app_config(app_config)?;
            dirs.store_node_config(node_config)?;
        }
    }

    Ok(true)
}

fn setup_node_config_paths(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    node_config: &mut NodeConfig,
) -> Result<()> {
    use dialoguer::Completion;

    const DB_PATH_FALLBACK: &str = "node_db";

    struct PathCompletion;

    impl PathCompletion {
        fn get_directories(&self, path: &dyn AsRef<Path>) -> Vec<String> {
            match std::fs::read_dir(path) {
                Ok(entires) => entires
                    .filter_map(|entry| match entry {
                        Ok(entry) if entry.metadata().ok()?.is_dir() => {
                            entry.file_name().into_string().ok()
                        }
                        _ => None,
                    })
                    .collect(),
                Err(_) => Vec::new(),
            }
        }
    }

    impl Completion for PathCompletion {
        fn get(&self, input: &str) -> Option<String> {
            let with_separator = input.ends_with(std::path::is_separator);
            let path = PathBuf::from(input);

            match path.metadata() {
                Ok(metadata) if metadata.is_dir() => {
                    if with_separator {
                        let dir = self.get_directories(&path).into_iter().min()?;
                        return Some(path.join(dir).to_str()?.to_string());
                    }
                }
                Ok(_) => return None,
                Err(_) => {}
            }

            let parent = path.parent()?;
            let name = path.file_name()?.to_str()?;

            let mut entires = self.get_directories(&parent);
            entires.sort_unstable();

            let mut entires_iter = entires.iter().skip_while(|item| item.as_str() < name);
            let first_matches = entires_iter.next()?;

            let name = if first_matches == name {
                entires_iter.chain(entires.first()).next()
            } else if name.len() < first_matches.len() && first_matches.starts_with(name) {
                Some(first_matches)
            } else {
                None
            }?;

            Some(parent.join(name).to_str()?.to_string())
        }
    }

    node_config.set_global_config_path(dirs.global_config())?;

    if let Some(db_path) = node_config.get_internal_db_path()? {
        if db_path != PathBuf::from(DB_PATH_FALLBACK) {
            dirs.store_node_config(node_config)?;
            return Ok(());
        }
    }

    let completion = &PathCompletion;

    let path: String = Input::with_theme(theme)
        .with_prompt("Specify node DB path")
        .default(DEFAULT_NODE_DB_PATH.to_owned())
        .completion_with(completion)
        .validate_with(|input: &String| {
            let path = PathBuf::from(input);
            if path.is_absolute() {
                Ok(())
            } else {
                Err("Node DB path must be an absolute")
            }
        })
        .interact_text()?;

    node_config.set_internal_db_path(&path)?;
    dirs.store_node_config(node_config)
}

async fn setup_binary(theme: &dyn Theme, dirs: &ProjectDirs) -> Result<bool> {
    if dirs.node_binary().exists() {
        return Ok(true);
    }
    dirs.prepare_binaries_dir()?;

    let repo: Url = Input::with_theme(theme)
        .with_prompt("Node repo URL")
        .with_initial_text(DEFAULT_NODE_REPO)
        .interact_text()?;

    dirs.install_node_from_repo(&repo).await?;
    Ok(true)
}

fn check_systemd_service(dirs: &ProjectDirs) -> Result<()> {
    use std::ffi::OsStr;

    let current_exe = std::env::current_exe()?;
    let current_exe = current_exe
        .file_name()
        .unwrap_or_else(|| OsStr::new("stever"))
        .to_string_lossy();

    if !dirs.validator_service().exists() || !dirs.validator_manager_service().exists() {
        println!(
            "\nTo configure systemd services, run:\n    sudo {} init systemd",
            current_exe
        );
    }
    Ok(())
}

fn prepare_services(theme: &dyn Theme, dirs: &ProjectDirs) -> Result<()> {
    const ROOT_USER: &str = "root";

    let uid = system::user_id();
    let other_user = match uid {
        0 => match system::get_sudo_uid()? {
            Some(0) => None,
            uid => uid,
        },
        uid => Some(uid),
    };

    let user = if let Some(uid) = other_user {
        let other_user = system::user_name(uid).context("failed to get user name")?;
        match Select::with_theme(theme)
            .with_prompt("Select the user from which the service will work")
            .item(&other_user)
            .item("root")
            .default(0)
            .interact()?
        {
            0 => Cow::Owned(other_user),
            _ => Cow::Borrowed(ROOT_USER),
        }
    } else {
        system::user_name(uid)
            .map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(ROOT_USER))
    };

    let print_service = |path: &Path| {
        println!(
            "{}",
            style(format!("Created validator service at {}", path.display())).dim()
        );
    };

    dirs.create_systemd_validator_service(&user)?;
    print_service(dirs.validator_service());

    dirs.create_systemd_validator_manager_service(&user)?;
    print_service(dirs.validator_manager_service());

    Ok(())
}

async fn start_services(theme: &dyn Theme) -> Result<()> {
    let services = [VALIDATOR_SERVICE, VALIDATOR_MANAGER_SERVICE];
    systemd_set_sercices_enabled(
        services,
        confirm(theme, true, "Enable autostart services at system startup?")?,
    )
    .await?;

    if confirm(theme, true, "Restart systemd services?")? {
        for service in services {
            systemd_restart_service(service).await?;
        }
    }

    Ok(())
}

macro_rules! validator_service {
    () => {
        r#"[Unit]
Description=Everscale Validator Node
After=network.target
StartLimitIntervalSec=0

[Service]
Type=simple
Restart=always
RestartSec=1
User={user}
LimitNOFILE=2048000
ExecStart={node_binary} --configs {configs_dir}

[Install]
WantedBy=multi-user.target
"#
    };
}

macro_rules! validator_manager_service {
    () => {
        r#"[Unit]
Description=Everscale Validator Manager
After=network.target
StartLimitIntervalSec=0

[Service]
Type=simple
Restart=always
RestartSec=1
User={user}
ExecStart={stever_binary} --root {root_dir} validate

[Install]
WantedBy=multi-user.target
"#
    };
}

impl ProjectDirs {
    fn store_app_config(&self, app_config: &AppConfig) -> Result<()> {
        app_config.store(self.app_config())
    }

    fn store_node_config(&self, node_config: &NodeConfig) -> Result<()> {
        node_config.store(self.node_config())
    }

    fn store_global_config<D: AsRef<str>>(&self, global_config: D) -> Result<()> {
        std::fs::write(self.global_config(), global_config.as_ref())
            .context("failed to write global config")
    }

    pub fn prepare_binaries_dir(&self) -> Result<()> {
        let binaries_dir = self.binaries_dir();
        if !binaries_dir.exists() {
            std::fs::create_dir_all(binaries_dir).context("failed to create binaries directory")?;
        }
        Ok(())
    }

    pub async fn install_node_from_repo(&self, repo: &Url) -> Result<()> {
        let git_dir = self.git_cache_dir();
        if !git_dir.exists() {
            std::fs::create_dir_all(git_dir).context("failed to create git cache directory")?;
        }

        let repo_dir = git_dir.join("ton-labs-node");

        clone_repo(repo, &repo_dir).await?;
        let binary = build_node(repo_dir).await?;

        std::fs::copy(binary, self.node_binary()).context("failed to copy node binary")?;
        Ok(())
    }

    pub fn create_systemd_validator_service(&self, user: &str) -> Result<()> {
        let node = std::fs::canonicalize(self.node_binary())
            .context("failed to canonicalize node binary path")?;
        let node_configs_dir = std::fs::canonicalize(self.node_configs_dir())
            .context("failed to canonicalize node configs path")?;

        let validator_service = format!(
            validator_service!(),
            user = user,
            node_binary = node.display(),
            configs_dir = node_configs_dir.display()
        );
        std::fs::write(self.validator_service(), validator_service)
            .context("failed to create systemd validator service")?;

        Ok(())
    }

    pub fn create_systemd_validator_manager_service(&self, user: &str) -> Result<()> {
        let current_exe = std::env::current_exe()?;
        let root_dir = std::fs::canonicalize(self.root())
            .context("failed to canonicalize root directory path")?;

        let validator_manager_service = format!(
            validator_manager_service!(),
            user = user,
            stever_binary = current_exe.display(),
            root_dir = root_dir.display(),
        );
        std::fs::write(self.validator_manager_service(), validator_manager_service)
            .context("failed to create systemd validator manager service")?;

        Ok(())
    }
}

async fn systemd_restart_service(service: &str) -> Result<()> {
    exec(
        Command::new("systemctl")
            .stdout(Stdio::piped())
            .arg("restart")
            .arg(service),
    )
    .await
    .with_context(|| format!("failed to restart service {service}"))
}

async fn systemd_set_sercices_enabled<'a, I: IntoIterator<Item = &'a str>>(
    services: I,
    enabled: bool,
) -> Result<()> {
    let mut command = Command::new("systemctl");
    command
        .stdout(Stdio::piped())
        .arg(if enabled { "enable" } else { "disable" });

    for service in services {
        command.arg(service);
    }

    exec(&mut command)
        .await
        .context("failed to enable services")
}

async fn systemd_daemon_reload() -> Result<()> {
    exec(
        Command::new("systemctl")
            .stdout(Stdio::piped())
            .arg("daemon-reload"),
    )
    .await
    .context("failed to reload systemd configs")
}

async fn clone_repo<P: AsRef<Path>>(url: &Url, target: P) -> Result<()> {
    let target = target.as_ref();
    if target.exists() {
        std::fs::remove_dir_all(target).context("failed to remove old git directory")?;
    }

    exec(
        Command::new("git")
            .stdout(Stdio::piped())
            .arg("clone")
            .arg("--recursive")
            .arg(url.to_string())
            .arg(target),
    )
    .await
    .context("failed to clone repo")
}

async fn build_node<P: AsRef<Path>>(target: P) -> Result<PathBuf> {
    let target = target.as_ref();

    exec(
        Command::new("cargo")
            .current_dir(target)
            .stdout(Stdio::piped())
            .arg("build")
            .arg("--release"),
    )
    .await
    .context("failed to build node")?;

    Ok(target.join("target").join("release").join("ton_node"))
}

async fn exec(command: &mut Command) -> Result<()> {
    let mut child = command.spawn()?;

    let status = child
        .wait()
        .await
        .context("child process encountered an error")?;

    anyhow::ensure!(
        status.success(),
        "child process failed with exit code {status}"
    );
    Ok(())
}

async fn get_node_version<P: AsRef<Path>>(node: P) -> Result<String> {
    let child = Command::new(node.as_ref())
        .arg("--version")
        .output()
        .await
        .context("failed to run node binary")?;

    if !child.status.success() {
        std::io::stderr().write_all(&child.stdout)?;
        anyhow::bail!("node finished with exit code {}", child.status);
    }

    parse_node_version(&child.stdout)
        .map(String::from)
        .context("invalid node output during version check")
}

fn parse_node_version(output: &[u8]) -> Option<&str> {
    output
        .strip_prefix(b"TON Node, version ")
        .and_then(|output| output.split(|&ch| ch == b'\n').next())
        .and_then(|output| std::str::from_utf8(output).ok())
}

fn confirm<T>(theme: &dyn Theme, default: bool, text: T) -> std::io::Result<bool>
where
    T: Into<String>,
{
    Confirm::with_theme(theme)
        .with_prompt(text)
        .default(default)
        .interact()
}

fn print_important_value(title: impl std::fmt::Display, value: impl std::fmt::Display) {
    println!(
        "{}\n{}\n",
        style(format!("{title}:")).green().bold(),
        style(value).bold()
    );
}

fn note(text: impl std::fmt::Display) -> impl std::fmt::Display {
    style(format!("({text})")).dim()
}

struct Steps {
    total: usize,
    current: usize,
}

impl Steps {
    fn new(total: usize) -> Self {
        Self { total, current: 0 }
    }

    fn next(&mut self, text: impl std::fmt::Display) {
        println!(
            "{} {text}",
            style(format!("[{}/{}]", self.current, self.total))
                .bold()
                .dim()
        );
        self.current += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_version_parser() {
        const STDOUT: &[u8] = b"TON Node, version 0.51.1
Rust: rustc 1.61.0 (fe5b13d68 2022-05-18)
TON NODE git commit:         Not set
ADNL git commit:             Not set
DHT git commit:              Not set
OVERLAY git commit:          Not set
RLDP git commit:             Not set
TON_BLOCK git commit:        Not set
TON_BLOCK_JSON git commit:   Not set
TON_SDK git commit:          Not set
TON_EXECUTOR git commit:     Not set
TON_TL git commit:           Not set
TON_TYPES git commit:        Not set
TON_VM git commit:           Not set
TON_ABI git commit:     Not set

TON node ";

        assert_eq!(parse_node_version(STDOUT), Some("0.51.1"));
    }
}
