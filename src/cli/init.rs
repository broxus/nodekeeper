use std::collections::HashMap;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use argh::FromArgs;
use console::style;
use dialoguer::theme::Theme;
use dialoguer::{Confirm, Input, Select};

use crate::config::{
    AppConfig, AppConfigAdnl, AppConfigControl, NodeConfig, NodeConfigAdnl, NodeConfigControlServer,
};

use super::CliContext;

#[derive(FromArgs)]
/// Initialize toola environment
#[argh(subcommand, name = "init")]
pub struct Cmd {
    /// path to the validator node config
    #[argh(positional)]
    node_config: PathBuf,
}

impl Cmd {
    pub async fn run(self, ctx: CliContext) -> Result<()> {
        let theme = &dialoguer::theme::ColorfulTheme::default();

        let root_dir = ctx.root_dir()?;

        let mut node_config = load_node_config(&self.node_config)?;
        let mut app_config = match load_app_config(theme, &root_dir, &ctx.config_path)? {
            Some(app_config) => app_config,
            None => return Ok(()),
        };

        setup_control_server(
            theme,
            &ctx.config_path,
            &mut app_config,
            &self.node_config,
            &mut node_config,
        )?;

        setup_adnl(
            theme,
            &ctx.config_path,
            &mut app_config,
            &self.node_config,
            &mut node_config,
        )
        .await?;

        Ok(())
    }
}

fn load_app_config(
    theme: &dyn Theme,
    root_dir: impl AsRef<Path>,
    path: impl AsRef<Path>,
) -> Result<Option<AppConfig>> {
    let path = path.as_ref();
    if path.exists() {
        return Ok(Some(AppConfig::load(path)?));
    }

    if !confirm(
        theme,
        false,
        format!(
            "Config doesn't exist yet. Generate? {}",
            note(path.display())
        ),
    )? {
        return Ok(None);
    }

    std::fs::create_dir_all(root_dir).context("failed create root directory")?;

    let config = AppConfig::default();
    config.store(path)?;

    Ok(Some(config))
}

async fn setup_adnl<P: AsRef<Path>>(
    theme: &dyn Theme,
    app_config_path: P,
    app_config: &mut AppConfig,
    node_config_path: P,
    node_config: &mut NodeConfig,
) -> Result<bool> {
    const DHT_TAG: usize = 1;
    const OVERLAY_TAG: usize = 2;

    let adnl_port = node_config
        .get_suggested_adnl_port()
        .unwrap_or(DEFAULT_ADNL_PORT);

    match (&mut app_config.adnl, node_config.get_adnl_node()?) {
        (Some(adnl_client), Some(adnl_node)) => {
            let server_pubkey = adnl_node.overlay_pubkey()?;
            if adnl_client.server_address != adnl_node.ip_address
                || adnl_client.server_pubkey != server_pubkey
            {
                if !confirm(theme, false, "ADNL node configuration mismatch. Update?")? {
                    return Ok(false);
                }

                adnl_client.server_address = adnl_node.ip_address;
                adnl_client.server_pubkey = server_pubkey;
                // TODO: update zerostate file hash
            }
        }
        (None, Some(adnl_node)) => {
            app_config.adnl = Some(AppConfigAdnl {
                server_address: adnl_node.ip_address,
                server_pubkey: adnl_node.overlay_pubkey()?,
                zerostate_file_hash: Default::default(),
            });

            app_config.store(app_config_path)?;
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
                zerostate_file_hash: Default::default(),
            });

            app_config.store(app_config_path)?;
            node_config.store(node_config_path)?;
        }
    }

    Ok(true)
}

fn setup_control_server<P: AsRef<Path>>(
    theme: &dyn Theme,
    app_config_path: P,
    app_config: &mut AppConfig,
    node_config_path: P,
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
                app_config.store(app_config_path)?;
            }
            if server_changed {
                node_config.set_control_server(&existing_server)?;
                node_config.store(node_config_path)?;
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
            app_config.store(app_config_path)?;

            if node_config_changed {
                node_config.set_control_server(&existing_server)?;
                node_config.store(node_config_path)?;
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

            app_config.store(app_config_path)?;
            node_config.store(node_config_path)?;
        }
    }

    Ok(true)
}

fn load_node_config<P: AsRef<Path>>(path: P) -> Result<NodeConfig> {
    let config = std::fs::read_to_string(path).context("failed to read node config")?;
    let config = serde_json::from_str(&config).context("failed to parse node config")?;
    Ok(config)
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
