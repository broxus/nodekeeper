use std::path::{Path, PathBuf};

use anyhow::Result;
use argh::FromArgs;
use home::home_dir;
use once_cell::race::OnceBox;

use crate::config::*;

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
        AppConfig::load(self.dirs.app_config())
    }

    pub fn dirs(&self) -> &ProjectDirs {
        &self.dirs
    }
}

pub struct ProjectDirs {
    app_config: PathBuf,
    node_config: PathBuf,
    global_config: PathBuf,
    node_configs_dir: PathBuf,
    binaries_dir: PathBuf,
    git_cache_dir: PathBuf,
    root: PathBuf,
}

impl ProjectDirs {
    fn new<P: AsRef<Path>>(root_dir: P) -> Self {
        let root = root_dir.as_ref().to_path_buf();
        let node_configs_dir = root.join("node");
        let binaries_dir = root.join("bin");
        let git_cache_dir = root.join("git");

        Self {
            app_config: root.join("config.toml"),
            node_config: node_configs_dir.join("config.json"),
            global_config: node_configs_dir.join("global-config.json"),
            node_configs_dir,
            binaries_dir,
            git_cache_dir,
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn app_config(&self) -> &Path {
        &self.app_config
    }

    pub fn binaries_dir(&self) -> &Path {
        &self.binaries_dir
    }

    pub fn git_cache_dir(&self) -> &Path {
        &self.git_cache_dir
    }

    pub fn node_configs_dir(&self) -> &Path {
        &self.node_configs_dir
    }

    pub fn node_config(&self) -> &Path {
        &self.node_config
    }

    pub fn global_config(&self) -> &Path {
        &self.global_config
    }
}

fn default_root_dir() -> &'static PathBuf {
    const ENV: &str = "STEVER_ROOT";
    const DEFAULT_ROOT_DIR: &str = ".stever";

    static DIRS: OnceBox<PathBuf> = OnceBox::new();
    DIRS.get_or_init(|| {
        Box::new(if let Ok(path) = std::env::var(ENV) {
            PathBuf::from(path)
        } else {
            match home_dir() {
                Some(home) => home.join(DEFAULT_ROOT_DIR),
                None => {
                    panic!(
                        "No valid home directory path could be retrieved from the operating system"
                    )
                }
            }
        })
    })
}
