use std::path::PathBuf;

use anyhow::{Context, Result};
use argh::FromArgs;

use crate::config::Config;

pub mod exporter;
pub mod node;

/// All-in-one node management tool with support for the upcoming stEVER
#[derive(FromArgs)]
pub struct App {
    #[argh(subcommand)]
    command: Command,

    /// path to the config file
    #[argh(option, default = "PathBuf::from(\"config\")")]
    config: PathBuf,
}

impl App {
    pub async fn run(self) -> Result<()> {
        let ctx = CliContext {
            config_path: self.config,
        };

        match self.command {
            Command::Exporter(cmd) => cmd.run(ctx).await,
            Command::Node(cmd) => cmd.run(ctx).await,
        }
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum Command {
    Exporter(exporter::Cmd),
    Node(node::Cmd),
}

pub struct CliContext {
    config_path: PathBuf,
}

impl CliContext {
    pub fn load_config(&mut self) -> Result<Config> {
        config::Config::builder()
            .add_source(config::File::from(self.config_path.as_path()))
            .add_source(config::Environment::default())
            .build()
            .context("failed to build config")?
            .try_deserialize()
            .context("failed to parse config")
    }
}
