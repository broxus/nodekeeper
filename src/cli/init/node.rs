use std::borrow::Cow;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::FromStr;

use anyhow::{Context, Result};
use argh::FromArgs;
use dialoguer::console::style;
use dialoguer::theme::Theme;
use dialoguer::{Completion, Input, Select};
use reqwest::Url;
use serde::Serialize;
use tokio::process::Command;

use super::Template;
use crate::cli::{CliContext, ProjectDirs};
use crate::config::*;
use crate::defaults;
use crate::util::*;

#[derive(FromArgs)]
/// Prepares configs and binaries
#[argh(subcommand, name = "node")]
pub struct Cmd {
    /// force download and build the latest node
    #[argh(switch)]
    pub rebuild: bool,
}

impl Cmd {
    pub async fn run(
        self,
        theme: &dyn Theme,
        ctx: &CliContext,
        template: &Option<Template>,
    ) -> Result<Output> {
        let dirs = ctx.dirs();

        // Compute steps len
        let mut steps = Steps::new(2);

        steps.next("Preparing configs");

        let mut output = Output::default();

        // Ensure root dir exists
        if !prepare_root_dir(theme, dirs, template)? {
            return Ok(output);
        }

        // Ensure that global config exists
        let global_config = load_global_config(theme, dirs, template, &mut output).await?;
        // Ensure that node config exists
        let mut node_config = load_node_config(dirs, template, &mut output)?;
        // Ensure that app config exists
        let mut app_config = load_app_config(dirs, template, &mut output)?;

        // Configure control server
        if !setup_control_server(
            theme,
            dirs,
            template,
            &mut app_config,
            &mut node_config,
            &mut output,
        )? {
            return Ok(output);
        }

        // Configure udp rpc
        if !setup_adnl(
            theme,
            dirs,
            template,
            &mut app_config,
            &mut node_config,
            &global_config,
            &mut output,
        )
        .await?
        {
            return Ok(output);
        }

        // Configure node config
        setup_node_config_paths(theme, dirs, template, &mut node_config, &mut output)?;

        // Clone and build the node
        steps.next("Preparing binary");
        if !setup_binary(
            theme,
            dirs,
            &app_config,
            template,
            self.rebuild,
            &mut output,
        )
        .await?
        {
            return Ok(output);
        }

        // Notify user about creating systemd services
        steps.next("Node is configured now. Great!");

        #[cfg(not(feature = "packaged"))]
        check_systemd_service(dirs)?;

        // Check cpu/memory/disk
        check_resources(&node_config, &mut output)?;

        Ok(output)
    }
}

#[derive(Debug, Default, Serialize)]
pub struct Output {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub global_config_updated: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub logger_config_reset: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_config_reset: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_config_reset: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_config_updated: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_config_updated: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_control_addr: Option<SocketAddrV4>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_public_addr: Option<SocketAddrV4>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub zerostate_file_hash: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_db_path: Option<PathBuf>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_binary_updated: Option<bool>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub node_version: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_is_suitable: Option<bool>,
}

fn prepare_root_dir(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    template: &Option<Template>,
) -> Result<bool> {
    let root = &dirs.root;
    if root.exists() {
        // Do nothing if root directory exists
        return Ok(true);
    }

    let create_root_dir = match template {
        Some(template) => template.general.create_root_dir,
        None => confirm(
            theme,
            root.is_absolute(),
            format!("Create root directory? {}", note(root.display())),
        )?,
    };

    if !create_root_dir {
        return Ok(false);
    }

    // Create root dir
    std::fs::create_dir_all(root).context("failed create root directory")?;
    Ok(true)
}

async fn load_global_config(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    template: &Option<Template>,
    output: &mut Output,
) -> Result<GlobalConfig> {
    #[derive(Clone)]
    enum PathOrUrl {
        Path(PathBuf),
        Url(Url),
    }

    impl std::fmt::Display for PathOrUrl {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            match self {
                Self::Path(path) => std::fmt::Display::fmt(&path.display(), f),
                Self::Url(url) => std::fmt::Display::fmt(url, f),
            }
        }
    }

    impl FromStr for PathOrUrl {
        type Err = anyhow::Error;

        fn from_str(s: &str) -> Result<Self> {
            let make_path = |path: &str| -> Result<PathBuf> {
                let path = system::make_shell_path(path);
                anyhow::ensure!(path.exists(), "specified path does not exist");
                Ok(path)
            };

            if s.starts_with("http") || s.starts_with("file") {
                let url: Url = s.parse()?;
                if url.scheme() == "file" {
                    if let Ok(path) = url.to_file_path() {
                        if let Some(path) = path.as_os_str().to_str() {
                            return make_path(path).map(Self::Path);
                        }
                    }
                    anyhow::bail!("invalid file url")
                }

                Ok(Self::Url(url))
            } else {
                make_path(s).map(Self::Path)
            }
        }
    }

    async fn download_config(url: PathOrUrl) -> Result<String> {
        match url {
            PathOrUrl::Path(path) => {
                std::fs::read_to_string(path).context("failed to load global config from file")
            }
            PathOrUrl::Url(url) => reqwest::get(url)
                .await
                .context("failed to download global config")?
                .text()
                .await
                .context("failed to download global config"),
        }
    }

    let overwrite_config = matches!(template, Some(t) if t.general.global_config.is_some());

    let global_config = &dirs.global_config;
    if !global_config.exists() || overwrite_config {
        let data = match template {
            Some(template) => match template.general.global_config.as_deref() {
                None | Some("ever_mainnet") => Cow::Borrowed(GlobalConfig::MAINNET),
                Some("ever_testnet") => Cow::Borrowed(GlobalConfig::MAINNET),
                Some(url) => {
                    let url = url.parse().context("invalid global config URL")?;
                    download_config(url).await.map(Cow::Owned)?
                }
            },
            None => {
                selector_variant!(Action, {
                    Mainnet => "Everscale mainnet",
                    Testnet => "Everscale testnet",
                    Other => "other",
                });

                // Select network static nodes config
                let items = Action::all();
                match items[Select::with_theme(theme)
                    .with_prompt("Select network")
                    .items(&items)
                    .default(0)
                    .interact()?]
                {
                    Action::Mainnet => Cow::Borrowed(GlobalConfig::MAINNET),
                    Action::Testnet => Cow::Borrowed(GlobalConfig::TESTNET),
                    // Try to download config
                    Action::Other => loop {
                        let completion = PathCompletion::new().with_dirs().with_files();
                        let url: PathOrUrl = Input::with_theme(theme)
                            .with_prompt("Config path or URL")
                            .completion_with(&completion)
                            .interact_text()?;

                        match download_config(url).await {
                            Ok(config) => break Cow::Owned(config),
                            Err(e) => {
                                print_error(format!("invalid config: {e}"));
                                continue;
                            }
                        }
                    },
                }
            }
        };

        std::fs::create_dir_all(&dirs.node_configs_dir)
            .context("failed to create node configs dir")?;
        dirs.store_global_config(data)?;
        output.global_config_updated = Some(true);
    }

    GlobalConfig::load(global_config)
}

fn load_node_config(
    dirs: &ProjectDirs,
    template: &Option<Template>,
    output: &mut Output,
) -> Result<NodeConfig> {
    // Generate default log config if it doesn't exist
    let node_log_config = &dirs.node_log_config;
    if !node_log_config.exists() || matches!(template, Some(t) if t.general.reset_logger_config) {
        dirs.store_node_log_config(&NodeLogConfig::generate())?;
        output.logger_config_reset = Some(true);
        if template.is_some() {
            eprintln!("Logger config overwritten");
        }
    }

    let node_config = &dirs.node_config;
    if !node_config.exists() || matches!(template, Some(t) if t.general.reset_node_config) {
        // Generate and save default node config
        let node_config = NodeConfig::generate()?;
        dirs.store_node_config(&node_config)?;
        output.node_config_reset = Some(true);
        if template.is_some() {
            eprintln!("Node config overwritten");
        }
        return Ok(node_config);
    }

    // Load node config if it already exists
    NodeConfig::load(node_config)
}

fn load_app_config(
    dirs: &ProjectDirs,
    template: &Option<Template>,
    output: &mut Output,
) -> Result<AppConfig> {
    let app_config = &dirs.app_config;
    if !app_config.exists() || matches!(template, Some(t) if t.general.reset_app_config) {
        // Generate and save default app config
        let app_config = AppConfig::default();
        dirs.store_app_config(&app_config)?;
        output.app_config_reset = Some(true);
        if template.is_some() {
            eprintln!("App config overwritten");
        }
        return Ok(app_config);
    }

    // Load app config if it already exists
    AppConfig::load(app_config)
}

fn setup_control_server(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    template: &Option<Template>,
    app_config: &mut AppConfig,
    node_config: &mut NodeConfig,
    output: &mut Output,
) -> Result<bool> {
    use everscale_crypto::ed25519;

    let rng = &mut rand::thread_rng();

    // Compute default control port
    let control_port = node_config
        .get_suggested_control_port()
        .unwrap_or(defaults::DEFAULT_CONTROL_PORT);

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
                let port = if template.is_some() {
                    eprintln!("Using server control port: {server_port}");
                    server_port
                } else {
                    match Select::with_theme(theme)
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
                    }
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
                if template.is_none() && !confirm(theme, true, "Server pubkey mismatch. Update?")? {
                    return Ok(false);
                }

                if template.is_some() {
                    eprintln!("Control server pubkey updated");
                }

                // Update public key
                existing_client.server_pubkey = server_pubkey;
                client_changed = true;
            }

            // Ensure that node clients config has our app in it
            if let Some(clients) = &mut existing_server.clients {
                let client_pubkey = ed25519::PublicKey::from(&existing_client.client_secret);
                if !clients.contains(&client_pubkey) {
                    let append = if clients.is_empty() {
                        true
                    } else if let Some(template) = template {
                        template.control.node_key_behavior.is_append()
                    } else {
                        Select::with_theme(theme)
                            .with_prompt("Node config has some clients specified. What to do?")
                            .item("append")
                            .item("replace")
                            .default(0)
                            .interact()?
                            == 0
                    };

                    if template.is_some() && !append {
                        eprintln!("Control client keys replaced");
                    }

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
                output.app_config_updated = Some(true);
            }
            if server_changed {
                node_config.set_control_server(&existing_server)?;
                dirs.store_node_config(node_config)?;
                output.node_config_updated = Some(true);
            }
        }
        // Only node config entry exists
        (None, Some(mut existing_server)) => {
            if template.is_none()
                && !confirm(
                    theme,
                    true,
                    "App config doesn't have control server entry. Create?",
                )?
            {
                return Ok(false);
            }

            // Generate client keys
            let client_key = ed25519::SecretKey::generate(rng);

            // Update node config clients entry
            let node_config_changed = match &mut existing_server.clients {
                // Explicitly ask about allowing any client to connect
                None if template.is_some() || !confirm(theme, false, "Allow any clients?")? => {
                    existing_server.clients = Some(vec![ed25519::PublicKey::from(&client_key)]);
                    eprintln!("Generated new client keys");
                    true
                }
                None => false,
                Some(clients) => {
                    let append = if clients.is_empty() {
                        true
                    } else if let Some(template) = template {
                        template.control.node_key_behavior.is_append()
                    } else {
                        Select::with_theme(theme)
                            .with_prompt("Node config has some clients specified. What to do?")
                            .item("append")
                            .item("replace")
                            .default(0)
                            .interact()?
                            == 0
                    };

                    if template.is_some() && !append {
                        eprintln!("Control client keys replaced");
                    }

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
            output.app_config_updated = Some(true);

            if template.is_some() {
                eprintln!("Control server entry created");
            }

            if node_config_changed {
                node_config.set_control_server(&existing_server)?;
                dirs.store_node_config(node_config)?;
                output.node_config_updated = Some(true);
            }
        }
        // Server config entry doesn't exist
        (existing_client, None) => {
            if template.is_none()
                && !confirm(
                    theme,
                    true,
                    "Node config doesn't have control server entry. Create?",
                )?
            {
                return Ok(false);
            }

            if template.is_none()
                && existing_client.is_some()
                && !confirm(theme, false, "Overwrite app control server config?")?
            {
                return Ok(false);
            }

            // Configure control server
            let listen_addr = match template {
                Some(template) => template.control.listen_addr,
                None => {
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
                    LISTEN_ADDR_ITEMS[listen_addr].1
                }
            };

            // Select control port
            let control_port = match template {
                Some(template) => template.control.port.unwrap_or(control_port),
                None => Input::with_theme(theme)
                    .with_prompt("Specify control port")
                    .with_initial_text(control_port.to_string())
                    .interact()?,
            };

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

            output.app_config_updated = Some(true);
            output.node_config_updated = Some(true);

            if template.is_some() {
                eprintln!("Control server entry overwritten");
            }
        }
    }

    if let Some(control) = &app_config.control {
        output.server_control_addr = Some(control.server_address);
    }

    Ok(true)
}

async fn setup_adnl(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    template: &Option<Template>,
    app_config: &mut AppConfig,
    node_config: &mut NodeConfig,
    global_config: &GlobalConfig,
    output: &mut Output,
) -> Result<bool> {
    // Compute default adnl port
    let adnl_port = node_config
        .get_suggested_adnl_port()
        .unwrap_or(defaults::DEFAULT_ADNL_PORT);

    // Get our public ip
    let mut public_ip = public_ip::addr_v4().await;

    // Get zerostate file hash from global config
    let zerostate_file_hash = *global_config.zero_state.file_hash.as_array();

    // Check current configs state
    match (&mut app_config.adnl, node_config.get_adnl_node()?) {
        // App and node configs were already touched
        (Some(adnl_client), Some(mut adnl_node)) => {
            if let Some(template) = template {
                if let Some(explicit_ip) = template.adnl.public_ip {
                    public_ip = Some(explicit_ip);
                }
            }

            if let Some(public_ip) = public_ip {
                // Update node ip address if it differs from the public ip
                if adnl_node.ip_address.ip() != &public_ip
                    && (template.is_some()
                        || confirm(
                            theme,
                            false,
                            "Your public IP is different from the configured one. Update?",
                        )?)
                {
                    adnl_node.ip_address.set_ip(public_ip);
                    node_config.set_adnl_node(&adnl_node)?;
                    dirs.store_node_config(node_config)?;
                    output.node_config_updated = Some(true);

                    if template.is_some() {
                        eprintln!("Updated public IP");
                    }
                }
            }

            // Update client config if it differs from the node config
            let server_pubkey = adnl_node.overlay_pubkey()?;
            if adnl_client.server_address != adnl_node.ip_address
                || adnl_client.server_pubkey != server_pubkey
                || adnl_client.zerostate_file_hash != zerostate_file_hash
            {
                if template.is_none()
                    && !confirm(theme, false, "ADNL node configuration mismatch. Update?")?
                {
                    return Ok(false);
                }

                adnl_client.server_address = adnl_node.ip_address;
                adnl_client.server_pubkey = server_pubkey;
                adnl_client.zerostate_file_hash = zerostate_file_hash;

                dirs.store_app_config(app_config)?;
                output.app_config_updated = Some(true);

                if template.is_some() {
                    eprintln!("ADNL config overwritten");
                }
            }
        }
        // Only node config entry exists
        (None, Some(adnl_node)) => {
            // Create client config
            app_config.adnl = Some(AppConfigAdnl {
                client_port: defaults::DEFAULT_LOCAL_ADNL_PORT,
                server_address: adnl_node.ip_address,
                server_pubkey: adnl_node.overlay_pubkey()?,
                zerostate_file_hash,
            });

            dirs.store_app_config(app_config)?;
            output.app_config_updated = Some(true);
        }
        // Node config entry doesn't exist
        (_, None) => {
            // Ask for the public ip
            let addr: Ipv4Addr = match template {
                Some(template) => template
                    .adnl
                    .public_ip
                    .or(public_ip)
                    .context("failed to resolve public ip")?,
                None => {
                    let mut input = Input::with_theme(theme);
                    if let Some(public_ip) = public_ip {
                        input.with_initial_text(public_ip.to_string());
                    }
                    input.with_prompt("Enter public ip").interact_text()?
                }
            };

            // Ask for the adnl port
            let adnl_port = match template {
                Some(template) => template.adnl.port.unwrap_or(adnl_port),
                None => Input::with_theme(theme)
                    .with_prompt("Specify server ADNL port")
                    .with_initial_text(adnl_port.to_string())
                    .interact()?,
            };

            // Update and save configs
            let adnl_node = NodeConfigAdnl::from_addr_and_keys(
                SocketAddrV4::new(addr, adnl_port),
                NodeConfigAdnl::generate_keys(),
            );
            node_config.set_adnl_node(&adnl_node)?;

            app_config.adnl = Some(AppConfigAdnl {
                client_port: defaults::DEFAULT_LOCAL_ADNL_PORT,
                server_address: adnl_node.ip_address,
                server_pubkey: adnl_node.overlay_pubkey()?,
                zerostate_file_hash,
            });

            dirs.store_app_config(app_config)?;
            dirs.store_node_config(node_config)?;
            output.app_config_updated = Some(true);
            output.node_config_updated = Some(true);
        }
    }

    output.zerostate_file_hash = Some(base64::encode(zerostate_file_hash));

    if let Some(adnl) = &app_config.adnl {
        output.node_public_addr = Some(adnl.server_address);
    }

    Ok(true)
}

fn setup_node_config_paths(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    template: &Option<Template>,
    node_config: &mut NodeConfig,
    output: &mut Output,
) -> Result<()> {
    const DB_PATH_FALLBACK: &str = "node_db";

    // Update global config path
    node_config.set_global_config_path(&dirs.global_config)?;

    // Check if internal db path was already configured
    let old_path = if let Some(db_path) = node_config.get_internal_db_path()? {
        if template.is_none() && db_path != PathBuf::from(DB_PATH_FALLBACK) {
            dirs.store_node_config(node_config)?;
            std::fs::create_dir_all(db_path.clone())?;
            output.node_db_path = Some(db_path);
            return Ok(());
        }
        db_path
    } else {
        Default::default()
    };

    // Ask for the internal db path
    let path = match template {
        Some(template) => template.general.node_db_path.clone(),
        None => {
            let completion = &PathCompletion::new().with_dirs();
            let mut path = Input::with_theme(theme);
            path.with_prompt("Specify node DB path")
                .default(dirs.default_node_db_dir.to_string_lossy().to_string())
                .completion_with(completion)
                .validate_with(|input: &String| {
                    let path = PathBuf::from(input);
                    if path.is_absolute() {
                        Ok(())
                    } else {
                        Err("Node DB path must be an absolute")
                    }
                })
                .interact_text()?
                .into()
        }
    };

    // Ensure that db directory exists
    std::fs::create_dir_all(&path)?;

    // Update and save node config
    node_config.set_internal_db_path(&path)?;
    dirs.store_node_config(node_config)?;

    if template.is_some() && path != old_path {
        eprintln!("Node DB path updated");
    }

    output.node_config_updated = Some(true);
    output.node_db_path = Some(path);

    Ok(())
}

async fn setup_binary(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    app_config: &AppConfig,
    template: &Option<Template>,
    force: bool,
    output: &mut Output,
) -> Result<bool> {
    async fn get_node_version<P: AsRef<Path>>(node: P) -> Result<String> {
        use std::io::Write;

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
        const OLD_PREFIX: &[u8] = b"TON Node, version ";
        const NEW_PREFIX: &[u8] = b"EVER Node, version ";

        output
            .strip_prefix(NEW_PREFIX)
            .or_else(|| output.strip_prefix(OLD_PREFIX))
            .and_then(|output| output.split(|&ch| ch == b'\n').next())
            .and_then(|output| std::str::from_utf8(output).ok())
    }

    if !force && dirs.node_binary.exists() {
        // Only check version if binary exists
        let node_version = get_node_version(&dirs.node_binary).await?;
        output.node_version = Some(node_version);
        return Ok(true);
    }

    // Ensure that binaries directory exists
    dirs.prepare_binaries_dir()?;

    let (repo, branch, features) = match template {
        Some(template) => {
            let node_repo = &template.general.node_repo;
            (
                node_repo.url.clone(),
                node_repo.branch.clone(),
                node_repo.features.clone(),
            )
        }
        None => {
            // Ask for the node repo and features
            let args: String = Input::with_theme(theme)
                .with_prompt("Node repo URL and features")
                .with_initial_text(app_config.node_repo())
                .interact_text()?;

            // Parse args:
            // -b,--branch <branch>
            // -f,--features <feature_name>+
            let mut args = args.split(' ');
            let repo = args.next().context("Url expected")?.parse::<Url>()?;

            let mut branch = None;
            let mut features = Vec::new();
            'args: loop {
                match args.next() {
                    Some("-b" | "--branch") => {
                        branch = Some(
                            args.next()
                                .map(ToOwned::to_owned)
                                .context("Expected branch name")?,
                        );
                    }
                    Some("-f" | "--features") => {
                        for feature in args.by_ref() {
                            if feature.starts_with('-') {
                                continue 'args;
                            }
                            features.push(feature.to_owned());
                        }
                        anyhow::ensure!(!features.is_empty(), "Expected features list");
                    }
                    Some(name) => anyhow::bail!("Unknown argument: {name}"),
                    None => break,
                }
            }

            (repo, branch, features)
        }
    };

    dirs.install_node_from_repo(&repo, &branch, &features)
        .await?;

    let node_version = get_node_version(&dirs.node_binary).await?;
    output.node_binary_updated = Some(true);
    output.node_version = Some(node_version);

    Ok(true)
}

async fn clone_repo<P: AsRef<Path>>(url: &Url, branch: &Option<String>, target: P) -> Result<()> {
    // Remove old repo if it exists
    let target = target.as_ref();
    if target.exists() {
        std::fs::remove_dir_all(target).context("failed to remove old git directory")?;
    }

    let mut command = Command::new("git");
    command
        .stdout(Stdio::null())
        .arg("clone")
        .arg("--recursive");

    if let Some(branch) = branch {
        eprintln!("Using branch: {branch}");
        command.arg("--branch").arg(branch);
    }

    // git clone to the target folder
    exec(command.arg(url.to_string()).arg(target))
        .await
        .context("failed to clone repo")
}

async fn build_node<P: AsRef<Path>>(target: P, features: &[String]) -> Result<PathBuf> {
    let target = target.as_ref();

    if let Err(e) = exec(Command::new("cargo").stdout(Stdio::null()).arg("-V")).await {
        print_error(
            "Failed to check `cargo` version. Rust is not installed properly.\n  \
               Please use the following command to install:\n  \
            curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh\n",
        );
        return Err(e);
    }

    let mut command = Command::new("cargo");
    command
        .current_dir(target)
        .stdout(Stdio::null())
        .arg("build")
        .arg("--release");

    if !features.is_empty() {
        eprintln!("Using features: {features:?}");
        command.arg("--features").arg(features.join(" "));
    }

    // cargo build in the target folder
    exec(&mut command).await.context("failed to build node")?;

    // Return the path to the freshly built binary
    for binary_name in ["ton_node", "ever-node"] {
        let path = target.join("target").join("release").join(binary_name);
        if path.exists() {
            return Ok(path);
        }
    }

    anyhow::bail!("node binary not found")
}

#[cfg(not(feature = "packaged"))]
fn check_systemd_service(dirs: &ProjectDirs) -> Result<()> {
    if dirs.validator_service.exists() && dirs.validator_manager_service.exists() {
        // Do nothing if all services exist
        return Ok(());
    }

    // Get current exe path
    let current_exe = std::env::current_exe()?;

    eprintln!(
        "\nTo configure systemd services, run:\n  sudo {} init systemd",
        current_exe.display()
    );
    Ok(())
}

fn check_resources(node_config: &NodeConfig, output: &mut Output) -> Result<()> {
    use sysinfo::System;

    fn format_gb(bytes: u64) -> String {
        let gb = (bytes / (1 << 20)) as f64 / 1024.0f64;
        format!("{:.1} GB", gb)
    }

    fn make_entry<D, F, R>(name: &str, value: D, f: F) -> (bool, String)
    where
        D: std::fmt::Display,
        F: FnOnce() -> Option<R>,
        R: std::fmt::Display,
    {
        if let Some(r) = f() {
            let res = format!(
                "{name}: {value} {}",
                style(format!("(recommended {r})")).yellow()
            );
            (false, res)
        } else {
            (true, format!("{name}: {value}"))
        }
    }

    const SUGGESTED_CORE_COUNT: usize = 12;
    const SUGGESTED_MEMORY: u64 = 30 << 30;
    const SUGGESTED_DISK: u64 = 100 << 30;

    let mut system = System::new_all();
    system.refresh_all();

    let mut entries = Vec::new();

    let vcpu = system.cpus().len();
    entries.push(make_entry("vCPU", vcpu, || {
        (vcpu < SUGGESTED_CORE_COUNT).then_some(SUGGESTED_CORE_COUNT)
    }));

    let memory = system.total_memory();
    entries.push(make_entry("Memory", format_gb(memory), || {
        (memory < SUGGESTED_MEMORY).then(|| format_gb(SUGGESTED_MEMORY))
    }));

    if let Some(node_db) = node_config.get_internal_db_path()? {
        match system::statvfs(node_db) {
            Ok(stats) => {
                let disk = stats.available_space;
                entries.push(make_entry("Disk", format_gb(disk), || {
                    (disk < SUGGESTED_DISK).then(|| format_gb(SUGGESTED_DISK))
                }));
            }
            Err(e) => print_error(format!("failed to check node DB disk usage: {e:?}")),
        }
    }

    let mut without_warnings = true;
    eprintln!("\nSystem info:");
    for (is_ok, item) in entries {
        without_warnings &= is_ok;
        eprintln!("  • {item}");
    }

    if !without_warnings {
        eprintln!(
            "{}",
            style(
                "\nThe system configuration does not meet the recommended host requirements.\
                \nThe node may be unstable or not work at all."
            )
            .yellow()
        );
    }

    output.system_is_suitable = Some(without_warnings);
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

    pub async fn install_node_from_repo(
        &self,
        repo: &Url,
        branch: &Option<String>,
        features: &[String],
    ) -> Result<()> {
        // Create git cache directory if it doesn't exist
        let git_dir = &self.git_cache_dir;
        if !git_dir.exists() {
            std::fs::create_dir_all(git_dir).context("failed to create git cache directory")?;
        }

        let repo_dir = git_dir.join("ever-node");

        // Clone repo
        clone_repo(repo, branch, &repo_dir).await?;
        let binary = build_node(repo_dir, features).await?;

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

#[derive(Default)]
struct PathCompletion {
    show_dirs: bool,
    show_files: bool,
}

impl PathCompletion {
    fn new() -> Self {
        Self::default()
    }

    fn with_dirs(mut self) -> Self {
        self.show_dirs = true;
        self
    }

    fn with_files(mut self) -> Self {
        self.show_files = true;
        self
    }

    fn get_entries(&self, path: &dyn AsRef<Path>) -> Vec<String> {
        match std::fs::read_dir(path) {
            Ok(entires) => entires
                .filter_map(|entry| match entry {
                    Ok(entry) => {
                        let metadata = entry.metadata().ok()?;
                        if self.show_dirs && metadata.is_dir()
                            || self.show_files && metadata.is_file()
                        {
                            entry.file_name().into_string().ok()
                        } else {
                            None
                        }
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
        let real_path = system::make_shell_path(input);
        let original_path = PathBuf::from(input);

        match real_path.metadata() {
            Ok(metadata) if metadata.is_dir() => {
                if with_separator {
                    let dir = self.get_entries(&real_path).into_iter().min()?;
                    return Some(original_path.join(dir).to_str()?.to_string());
                }
            }
            Ok(_) if !self.show_files => return None,
            _ => {}
        }

        let parent = real_path.parent()?;
        let name = real_path.file_name()?.to_str()?;

        let mut entires = self.get_entries(&parent);
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

        Some(original_path.parent()?.join(name).to_str()?.to_string())
    }
}
