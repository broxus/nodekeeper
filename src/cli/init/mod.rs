use std::borrow::Cow;
use std::net::{Ipv4Addr, SocketAddrV4};

use anyhow::{Context, Result};
use argh::FromArgs;
use console::style;
use dialoguer::theme::Theme;
use dialoguer::{Confirm, Input, Select};
use reqwest::Url;

use super::{CliContext, ProjectDirs};
use crate::config::*;

mod node_manager;

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

        println!("{} Preparing configs.", step(0, 2));

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

        println!("{} Preparing binary.", step(1, 2));

        if !setup_binary(theme, dirs).await? {
            return Ok(());
        }

        println!(r"{} Validator node is configured now. Great!", step(2, 2));

        check_systemd_service(dirs)?;

        Ok(())
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    Systemd(CmdInitSystemd),
}

#[derive(FromArgs)]
/// Creates systemd services (`ever-validator` and `ever-validator-manager`)
#[argh(subcommand, name = "systemd")]
struct CmdInitSystemd {}

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

            if existing_client.is_some() {
                if !confirm(theme, false, "Overwrite stEVER control server config?")? {
                    return Ok(false);
                }
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
                .with_prompt("Specify ADNL port")
                .with_initial_text(adnl_port.to_string())
                .interact()?;

            let adnl_node = NodeConfigAdnl::from_addr_and_keys(
                SocketAddrV4::new(addr, adnl_port),
                NodeConfigAdnl::generate_keys(),
            );
            node_config.set_adnl_node(&adnl_node)?;

            app_config.adnl = Some(AppConfigAdnl {
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
        .unwrap_or(OsStr::new("stever"))
        .to_string_lossy();

    if !dirs.validator_service().exists() || !dirs.validator_manager_service().exists() {
        println!(
            "\nTo configure systemd services, run:\n    sudo {} init systemd",
            current_exe
        );
    }
    Ok(())
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

fn note(text: impl std::fmt::Display) -> impl std::fmt::Display {
    style(format!("({text})")).dim()
}

fn step(i: usize, total: usize) -> impl std::fmt::Display {
    style(format!("[{i}/{total}]")).bold().dim()
}

const DEFAULT_CONTROL_PORT: u16 = 5031;
const DEFAULT_ADNL_PORT: u16 = 30100;
const DEFAULT_NODE_REPO: &str = "https://github.com/tonlabs/ton-labs-node.git";
