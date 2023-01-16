use std::borrow::Cow;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use argh::FromArgs;
use dialoguer::theme::Theme;
use dialoguer::{Completion, Input, Select};
use reqwest::Url;
use tokio::process::Command;

use super::systemd::{prepare_services, start_services, systemd_daemon_reload};
use crate::cli::{CliContext, ProjectDirs};
use crate::config::*;
use crate::util::*;

const DEFAULT_CONTROL_PORT: u16 = 5031;
const DEFAULT_LOCAL_ADNL_PORT: u16 = 0;
const DEFAULT_ADNL_PORT: u16 = 30100;
const DEFAULT_NODE_REPO: &str = "https://github.com/tonlabs/ton-labs-node.git";
const DEFAULT_NODE_DB_PATH: &str = "/var/ever/rnode";

#[derive(FromArgs)]
/// Prepares configs and binaries
#[argh(subcommand, name = "node")]
pub struct Cmd {
    /// force download and build the latest node
    #[argh(switch)]
    pub rebuild: bool,
}

impl Cmd {
    pub async fn run(self, theme: &dyn Theme, ctx: &CliContext) -> Result<()> {
        let dirs = ctx.dirs();

        // Compute steps len
        let is_root = system::is_root();
        let mut steps = Steps::new(2 + 2 * (is_root as usize));

        steps.next("Preparing configs");

        // Ensure root dir exists
        if !prepare_root_dir(theme, dirs)? {
            return Ok(());
        }

        // Ensure that global config exists
        let global_config = load_global_config(theme, dirs).await?;
        // Ensure that node config exists
        let mut node_config = load_node_config(dirs)?;
        // Ensure that app config exists
        let mut app_config = load_app_config(dirs)?;

        // Configure control server
        if !setup_control_server(theme, dirs, &mut app_config, &mut node_config)? {
            return Ok(());
        }

        // Configure udp rpc
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

        // Configure node config
        setup_node_config_paths(theme, dirs, &mut node_config)?;

        // Clone and build the node
        steps.next("Preparing binary");
        if !setup_binary(theme, dirs, self.rebuild).await? {
            return Ok(());
        }

        // Configure systemd services if running as root
        if is_root {
            // Ensure all services are created
            steps.next("Preparing services");
            prepare_services(theme, dirs)?;

            // Reload sysetmd
            steps.next("Reloading systemd configs");
            systemd_daemon_reload().await?;

            // Optionally start services
            steps.next("Node is configured now. Great!");
            start_services(theme).await?;
        } else {
            // Ask user about creating systemd services
            steps.next("Node is configured now. Great!");
            check_systemd_service(dirs)?;
        }

        Ok(())
    }
}

fn prepare_root_dir(theme: &dyn Theme, dirs: &ProjectDirs) -> Result<bool> {
    let root = &dirs.root;
    if root.exists() {
        // Do nothing if root directory exists
        return Ok(true);
    }

    if !confirm(
        theme,
        root.is_absolute(),
        format!("Create root directory? {}", note(root.display())),
    )? {
        return Ok(false);
    }

    // Create root dir
    std::fs::create_dir_all(root).context("failed create root directory")?;
    Ok(true)
}

async fn load_global_config(theme: &dyn Theme, dirs: &ProjectDirs) -> Result<GlobalConfig> {
    selector_variant!(Action, {
        Mainnet => "Everscale mainnet",
        Testnet => "Everscale testnet",
        Other => "other",
    });

    let global_config = &dirs.global_config;
    if !global_config.exists() {
        // Select network static nodes config
        let items = Action::all();
        let data = match items[Select::with_theme(theme)
            .with_prompt("Select network")
            .items(&items)
            .default(0)
            .interact()?]
        {
            Action::Mainnet => Cow::Borrowed(GlobalConfig::MAINNET),
            Action::Testnet => Cow::Borrowed(GlobalConfig::TESTNET),
            // Try to download config
            Action::Other => {
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

        std::fs::create_dir_all(&dirs.node_configs_dir)
            .context("failed to create node configs dir")?;
        dirs.store_global_config(data)?;
    }

    GlobalConfig::load(global_config)
}

fn load_node_config(dirs: &ProjectDirs) -> Result<NodeConfig> {
    // Generate default log config if it doesn't exist
    let node_log_config = &dirs.node_log_config;
    if !node_log_config.exists() {
        dirs.store_node_log_config(&NodeLogConfig::generate())?;
    }

    let node_config = &dirs.node_config;
    if node_config.exists() {
        // Load node config if it already exists
        return NodeConfig::load(node_config);
    }

    // Generate and save default node config
    let node_config = NodeConfig::generate()?;
    dirs.store_node_config(&node_config)?;
    Ok(node_config)
}

fn load_app_config(dirs: &ProjectDirs) -> Result<AppConfig> {
    let app_config = &dirs.app_config;
    if app_config.exists() {
        // Load app config if it already exists
        return AppConfig::load(app_config);
    }

    // Generate and save default app config
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

    // Compute default control port
    let control_port = node_config
        .get_suggested_control_port()
        .unwrap_or(DEFAULT_CONTROL_PORT);

    // Check current configs state
    match (&mut app_config.control, node_config.get_control_server()?) {
        // App and node configs were already touched
        (Some(existing_client), Some(mut existing_server)) => {
            let mut server_changed = false;
            let mut client_changed = false;

            // Ensure that control port is the same
            let server_port = existing_server.address.port();
            let client_port = existing_client.server_address.port();
            if existing_client.server_address.port() != existing_server.address.port() {
                let port = match Select::with_theme(theme)
                    .with_prompt("App config has different control port. What to do?")
                    .item(format!(
                        "use control port from the node {}",
                        note(server_port)
                    ))
                    .item(format!(
                        "use control port from this app {}",
                        note(client_port)
                    ))
                    .item("specify custom port")
                    .default(0)
                    .interact()?
                {
                    // Use port from the node config
                    0 => server_port,
                    // Use port from the app config
                    1 => client_port,
                    _ => Input::with_theme(theme)
                        .with_prompt("Specify control port")
                        .interact_text()?,
                };

                client_changed |= port != client_port;
                server_changed |= port != server_port;

                // Update port
                existing_client.server_address.set_port(port);
                existing_server.address.set_port(port);
            }

            // Ensure that control public key is the same
            let server_pubkey = ed25519::PublicKey::from(&existing_server.server_key);
            if server_pubkey != existing_client.server_pubkey {
                if !confirm(theme, true, "Server pubkey mismatch. Update?")? {
                    return Ok(false);
                }

                // Update public key
                existing_client.server_pubkey = server_pubkey;
                client_changed = true;
            }

            // Ensure that node clients config has our app in it
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

                    // Add or replace clients config
                    clients.push(client_pubkey);
                    server_changed = true;
                }
            }

            // Save changed configs
            if client_changed {
                dirs.store_app_config(app_config)?;
            }
            if server_changed {
                node_config.set_control_server(&existing_server)?;
                dirs.store_node_config(node_config)?;
            }
        }
        // Only node config entry exists
        (None, Some(mut existing_server)) => {
            if !confirm(
                theme,
                true,
                "App config doesn't have control server entry. Create?",
            )? {
                return Ok(false);
            }

            // Generate client keys
            let client_key = ed25519::SecretKey::generate(rng);

            // Update node config clients entry
            let node_config_changed = match &mut existing_server.clients {
                // Explicitly ask about allowing any client to connect
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

                    // Add or replace clients config
                    if !append {
                        clients.clear();
                    }

                    clients.push(ed25519::PublicKey::from(&client_key));
                    true
                }
            };

            // Update app config
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
        // Server config entry doesn't exist
        (existing_client, None) => {
            if !confirm(
                theme,
                true,
                "Node config doesn't have control server entry. Create?",
            )? {
                return Ok(false);
            }

            if existing_client.is_some()
                && !confirm(theme, false, "Overwrite app control server config?")?
            {
                return Ok(false);
            }

            // Configure control server
            const LISTEN_ADDR_ITEMS: [(&str, Ipv4Addr); 2] = [
                ("localhost", Ipv4Addr::LOCALHOST),
                ("any", Ipv4Addr::UNSPECIFIED),
            ];

            // Select listen address
            let listen_addr = Select::with_theme(theme)
                .with_prompt("Control server listen address")
                .item(LISTEN_ADDR_ITEMS[0].0)
                .item(LISTEN_ADDR_ITEMS[1].0)
                .default(0)
                .interact()?;
            let listen_addr = LISTEN_ADDR_ITEMS[listen_addr].1;

            // Select control port
            let control_port = Input::with_theme(theme)
                .with_prompt("Specify control port")
                .with_initial_text(control_port.to_string())
                .interact()?;

            let addr = SocketAddrV4::new(listen_addr, control_port);

            // Generate keys
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

            // Save configs
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

    // Compute default adnl port
    let adnl_port = node_config
        .get_suggested_adnl_port()
        .unwrap_or(DEFAULT_ADNL_PORT);

    // Get our public ip
    let public_ip = public_ip::addr_v4().await;

    // Get zerostate file hash from global config
    let zerostate_file_hash = *global_config.zero_state.file_hash.as_array();

    // Check current configs state
    match (&mut app_config.adnl, node_config.get_adnl_node()?) {
        // App and node configs were already touched
        (Some(adnl_client), Some(mut adnl_node)) => {
            if let Some(public_ip) = public_ip {
                // Update node ip address if it differs from the public ip
                if adnl_node.ip_address.ip() != &public_ip
                    && confirm(
                        theme,
                        false,
                        "Your public IP is different from the configured one. Update?",
                    )?
                {
                    adnl_node.ip_address.set_ip(public_ip);
                    node_config.set_adnl_node(&adnl_node)?;
                    dirs.store_node_config(node_config)?;
                }
            }

            // Update client config if it differes from the node config
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
        // Only node config entry exists
        (None, Some(adnl_node)) => {
            // Create client config
            app_config.adnl = Some(AppConfigAdnl {
                client_port: DEFAULT_LOCAL_ADNL_PORT,
                server_address: adnl_node.ip_address,
                server_pubkey: adnl_node.overlay_pubkey()?,
                zerostate_file_hash,
            });

            app_config.store(&dirs.app_config)?;
        }
        // Node config entry doesn't exist
        (_, None) => {
            // Ask for the public ip
            let addr: Ipv4Addr = {
                let mut input = Input::with_theme(theme);
                if let Some(public_ip) = public_ip {
                    input.with_initial_text(public_ip.to_string());
                }
                input.with_prompt("Enter public ip").interact_text()?
            };

            // Ask for the adnl port
            let adnl_port = Input::with_theme(theme)
                .with_prompt("Specify server ADNL port")
                .with_initial_text(adnl_port.to_string())
                .interact()?;

            // Update and save configs
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
    const DB_PATH_FALLBACK: &str = "node_db";

    // Update global config path
    node_config.set_global_config_path(&dirs.global_config)?;

    // Check if internal db path was already configured
    if let Some(db_path) = node_config.get_internal_db_path()? {
        if db_path != PathBuf::from(DB_PATH_FALLBACK) {
            dirs.store_node_config(node_config)?;
            return Ok(());
        }
    }

    // Ask for the internal db path
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

    // Update and save node config
    node_config.set_internal_db_path(&path)?;
    dirs.store_node_config(node_config)
}

async fn setup_binary(theme: &dyn Theme, dirs: &ProjectDirs, force: bool) -> Result<bool> {
    if !force && dirs.node_binary.exists() {
        // Do nothing if binary exists
        // TODO: print version and ask for update?
        return Ok(true);
    }

    // Ensure that binaries directory exists
    dirs.prepare_binaries_dir()?;

    // Ask for the node repo
    let repo: Url = Input::with_theme(theme)
        .with_prompt("Node repo URL")
        .with_initial_text(DEFAULT_NODE_REPO)
        .interact_text()?;

    dirs.install_node_from_repo(&repo).await?;
    Ok(true)
}

async fn clone_repo<P: AsRef<Path>>(url: &Url, target: P) -> Result<()> {
    // Remove old repo if it exists
    let target = target.as_ref();
    if target.exists() {
        std::fs::remove_dir_all(target).context("failed to remove old git directory")?;
    }

    // git clone to the target folder
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

    // cargo build in the target folder
    exec(
        Command::new("cargo")
            .current_dir(target)
            .stdout(Stdio::piped())
            .arg("build")
            .arg("--release"),
    )
    .await
    .context("failed to build node")?;

    // Return the path to the freshly built binary
    Ok(target.join("target").join("release").join("ton_node"))
}

fn check_systemd_service(dirs: &ProjectDirs) -> Result<()> {
    use std::ffi::OsStr;

    if dirs.validator_service.exists() && dirs.validator_manager_service.exists() {
        // Do nothing if all services exist
        return Ok(());
    }

    // Get current exe path
    let current_exe = std::env::current_exe()?;
    let current_exe = current_exe
        .file_name()
        .unwrap_or_else(|| OsStr::new("stever"))
        .to_string_lossy();

    println!(
        "\nTo configure systemd services, run:\n    sudo {} init systemd",
        current_exe
    );
    Ok(())
}

impl ProjectDirs {
    fn store_node_log_config(&self, node_log_config: &NodeLogConfig) -> Result<()> {
        node_log_config.store(&self.node_log_config)
    }

    fn store_global_config<D: AsRef<str>>(&self, global_config: D) -> Result<()> {
        std::fs::write(&self.global_config, global_config.as_ref())
            .context("failed to write global config")
    }

    pub fn prepare_binaries_dir(&self) -> Result<()> {
        let binaries_dir = &self.binaries_dir;
        if !binaries_dir.exists() {
            std::fs::create_dir_all(binaries_dir).context("failed to create binaries directory")?;
        }
        Ok(())
    }

    pub async fn install_node_from_repo(&self, repo: &Url) -> Result<()> {
        // Create git cache directory if it doesn't exist
        let git_dir = &self.git_cache_dir;
        if !git_dir.exists() {
            std::fs::create_dir_all(git_dir).context("failed to create git cache directory")?;
        }

        let repo_dir = git_dir.join("ton-labs-node");

        // Clone repo
        clone_repo(repo, &repo_dir).await?;
        let binary = build_node(repo_dir).await?;

        // Copy the binary to the expected binary path
        match std::fs::copy(&binary, &self.node_binary) {
            Ok(_) => Ok(()),
            Err(e) if matches!(e.raw_os_error(), Some(libc::ETXTBSY)) => {
                std::fs::remove_file(&self.node_binary).context("failed to remove binary")?;
                std::fs::copy(binary, &self.node_binary).context("failed to copy node binary")?;
                Ok(())
            }
            Err(e) => Err(e).context("failed to copy node binary"),
        }
    }
}

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
