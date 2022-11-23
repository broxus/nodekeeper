use anyhow::Result;
use argh::FromArgs;

use crate::config::{AppConfig, NodeConfig};

use super::{CliContext, ProjectDirs};

mod contracts;
mod node;
mod systemd;

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
        match self.subcommand {
            None => {
                node::Cmd {}.run(theme, &ctx).await?;
                println!();
                contracts::Cmd {}.run(theme, &ctx).await
            }
            Some(SubCmd::Node(cmd)) => cmd.run(theme, &ctx).await,
            Some(SubCmd::Systemd(cmd)) => cmd.run(theme, &ctx).await,
            Some(SubCmd::Contracts(cmd)) => cmd.run(theme, &ctx).await,
        }
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    Node(node::Cmd),
    Systemd(systemd::Cmd),
    Contracts(contracts::Cmd),
}

impl ProjectDirs {
    fn store_app_config(&self, app_config: &AppConfig) -> Result<()> {
        app_config.store(&self.app_config)
    }

    fn store_node_config(&self, node_config: &NodeConfig) -> Result<()> {
        node_config.store(&self.node_config)
    }
}
