use std::borrow::Cow;
use std::path::PathBuf;

use anyhow::{Context, Result};
use argh::FromArgs;

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

    /// path to the config file
    #[argh(option, default = "default_config_path()")]
    config: PathBuf,
}

impl App {
    pub async fn run(self) -> Result<()> {
        tracing::debug!("using config {:?}", self.config);

        let ctx = CliContext {
            config_path: self.config,
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
    config_path: PathBuf,
}

impl CliContext {
    pub fn load_config(&self) -> Result<AppConfig> {
        AppConfig::load(&self.config_path)
    }

    pub fn root_dir(&self) -> Result<PathBuf> {
        let path = if self.config_path.is_absolute() {
            Cow::Borrowed(self.config_path.as_path())
        } else {
            Cow::Owned(
                std::env::current_dir()
                    .context("failed to get working directory")?
                    .join(&self.config_path),
            )
        };

        match path.parent() {
            Some(path) => Ok(path.to_path_buf()),
            None => anyhow::bail!("couldn't determine root dir"),
        }
    }
}
