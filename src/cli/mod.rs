use std::path::PathBuf;

use anyhow::Result;
use argh::FromArgs;

use crate::config::*;
use crate::dirs::*;
use crate::util::*;

pub mod contract;
pub mod exporter;
pub mod init;
pub mod node;
pub mod seed;
pub mod validator;

/// All-in-one node management tool
#[derive(FromArgs)]
pub struct App {
    #[argh(subcommand)]
    command: Command,

    /// path to the root directory
    #[argh(option, default = "ProjectDirs::default_root_dir()")]
    root: PathBuf,
}

impl App {
    pub async fn run(self) -> Result<()> {
        tracing::debug!("root dir {:?}", self.root);

        let ctx = CliContext {
            dirs: ProjectDirs::new(self.root),
        };

        match self.command {
            Command::Init(cmd) => invoke_as_cli(cmd.run(ctx)).await,
            Command::Validator(cmd) => cmd.run(ctx).await,
            Command::Contract(cmd) => invoke_as_cli(cmd.run(ctx)).await,
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
    Validator(validator::Cmd),
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
