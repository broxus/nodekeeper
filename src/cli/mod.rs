use std::path::{Path, PathBuf};

use anyhow::Result;
use argh::FromArgs;
use home::home_dir;

use crate::config::*;
use crate::util::*;

pub mod contract;
pub mod exporter;
pub mod init;
pub mod node;
pub mod seed;

/// All-in-one node management tool with support for the upcoming stEVER
#[derive(FromArgs)]
pub struct App {
    #[argh(subcommand)]
    command: Command,

    /// path to the root directory
    #[argh(option, default = "default_root_dir().clone()")]
    root: PathBuf,
}

impl App {
    pub async fn run(self) -> Result<()> {
        tracing::debug!("root dir {:?}", self.root);

        let ctx = CliContext {
            dirs: ProjectDirs::new(self.root),
        };

        match self.command {
            Command::Init(cmd) => cmd.run(ctx).await,
            Command::Contract(cmd) => cmd.run(ctx).await,
            Command::Exporter(cmd) => cmd.run(ctx).await,
            Command::Node(cmd) => cmd.run(ctx).await,
            Command::Seed(cmd) => cmd.run(),
        }
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum Command {
    Init(init::Cmd),
    Contract(contract::Cmd),
    Exporter(exporter::Cmd),
    Node(node::Cmd),
    Seed(seed::Cmd),
}

pub struct CliContext {
    dirs: ProjectDirs,
}

impl CliContext {
    pub fn load_config(&self) -> Result<AppConfig> {
        AppConfig::load(&self.dirs.app_config)
    }

    pub fn dirs(&self) -> &ProjectDirs {
        &self.dirs
    }
}

pub const VALIDATOR_SERVICE: &str = "ever-validator";
pub const VALIDATOR_MANAGER_SERVICE: &str = "ever-validator-manager";

pub struct ProjectDirs {
    app_config: PathBuf,
    node_config: PathBuf,
    node_log_config: PathBuf,
    global_config: PathBuf,
    node_configs_dir: PathBuf,
    binaries_dir: PathBuf,
    node_binary: PathBuf,
    git_cache_dir: PathBuf,
    keys_dir: PathBuf,
    validator_keys: PathBuf,
    depool_keys: PathBuf,
    root: PathBuf,
    validator_service: PathBuf,
    validator_manager_service: PathBuf,
}

impl ProjectDirs {
    fn new<P: AsRef<Path>>(root_dir: P) -> Self {
        let root = root_dir.as_ref().to_path_buf();
        let node_configs_dir = root.join("node");
        let binaries_dir = root.join("bin");
        let git_cache_dir = root.join("git");

        let node_binary = binaries_dir.join("node");

        let systemd_root = PathBuf::from("/etc/systemd/system");
        let validator_service = systemd_root.join(format!("{VALIDATOR_SERVICE}.service"));
        let validator_manager_service =
            systemd_root.join(format!("{VALIDATOR_MANAGER_SERVICE}.service"));

        let keys_dir = root.join("keys");
        let validator_keys = keys_dir.join("vld.keys.json");
        let depool_keys = keys_dir.join("depool.keys.json");

        Self {
            app_config: root.join("config.toml"),
            node_config: node_configs_dir.join("config.json"),
            node_log_config: node_configs_dir.join("log_cfg.yml"),
            global_config: node_configs_dir.join("global-config.json"),
            node_configs_dir,
            binaries_dir,
            node_binary,
            git_cache_dir,
            keys_dir,
            validator_keys,
            depool_keys,
            root,
            validator_service,
            validator_manager_service,
        }
    }
}

fn default_root_dir() -> &'static PathBuf {
    const ENV: &str = "STEVER_ROOT";
    const DEFAULT_ROOT_DIR: &str = ".stever";

    once!(PathBuf, || {
        if let Ok(path) = std::env::var(ENV) {
            PathBuf::from(path)
        } else {
            let home_dir = if let Some(uid) = system::get_sudo_uid().unwrap() {
                system::home_dir(uid)
            } else {
                home_dir()
            };

            match home_dir {
                Some(home) => home.join(DEFAULT_ROOT_DIR),
                None => {
                    panic!(
                        "No valid home directory path could be retrieved from the operating system"
                    )
                }
            }
        }
    })
}
