use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use argh::FromArgs;
use nekoton_abi::FunctionExt;

use super::CliContext;
use crate::global_config::GlobalConfig;
use crate::node_rpc::{NodeRpc, NodeStats};
use crate::subscription::BlockSubscription;
use crate::util::*;

#[derive(FromArgs)]
/// Contract interaction stuff
#[argh(subcommand, name = "contract")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: SubCmd,
}

impl Cmd {
    pub async fn run(self, mut ctx: CliContext) -> Result<()> {
        let config = ctx.load_config()?;
        let node_rpc = NodeRpc::new(&config).await?;

        let response = match self.subcommand {
            SubCmd::Call(cmd) => cmd.run(node_rpc).await?,
            SubCmd::Send(cmd) => cmd.run(node_rpc).await?,
        };

        println!("{}", serde_json::to_string_pretty(&response)?);
        Ok(())
    }
}

fn parse_contract_method(abi: &PathBuf, method: &str) -> Result<ton_abi::Function> {
    parse_contract_abi(abi)?
        .functions
        .remove(method)
        .with_context(|| format!("method `{method}` not found"))
}

async fn get_account_stuff(
    node_rpc: &NodeRpc,
    address: &ton_block::MsgAddressInt,
) -> Result<ton_block::AccountStuff> {
    let state = node_rpc.get_shard_account_state(address).await?;
    match state.read_account()? {
        ton_block::Account::Account(account) => Ok(account),
        ton_block::Account::AccountNone => Err(anyhow::anyhow!("account not deployed")),
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    Call(CmdCall),
    Send(CmdSend),
}

#[derive(FromArgs)]
/// Runs the specified getter method
#[argh(subcommand, name = "call")]
struct CmdCall {
    /// method name
    #[argh(positional)]
    method: String,

    /// method args
    #[argh(positional, default = "default_args()")]
    args: serde_json::Value,

    /// path to the JSON ABI file
    #[argh(option)]
    abi: PathBuf,

    /// contract address
    #[argh(option, long = "addr")]
    address: String,

    /// execute method as responsible. (NOTE: requires first argument of type `uint32`)
    #[argh(switch)]
    responsible: bool,
}

impl CmdCall {
    async fn run(self, node_rpc: NodeRpc) -> Result<serde_json::Value> {
        let clock = nekoton_utils::SimpleClock;

        let address = parse_address(&self.address)?;
        let method = parse_contract_method(&self.abi, &self.method)?;
        let input = nekoton_abi::parse_abi_tokens(&method.inputs, self.args)?;

        let account_stuff = get_account_stuff(&node_rpc, &address).await?;

        let nekoton_abi::ExecutionOutput {
            result_code,
            tokens,
        } = match self.responsible {
            false => method.run_local(&clock, account_stuff, &input)?,
            true => method.run_local_responsible(&clock, account_stuff, &input)?,
        };

        let output = tokens
            .as_deref()
            .map(nekoton_abi::make_abi_tokens)
            .transpose()?;

        Ok(serde_json::json!({
            "code": result_code,
            "output": output,
        }))
    }
}

#[derive(FromArgs)]
/// Sends an external message
#[argh(subcommand, name = "send")]
struct CmdSend {
    /// method name
    #[argh(positional)]
    method: String,

    /// method args
    #[argh(positional, default = "default_args()")]
    args: serde_json::Value,

    /// path to the JSON ABI file
    #[argh(option)]
    abi: PathBuf,

    /// contract address
    #[argh(option, long = "addr")]
    address: String,

    /// seed phrase or path to the keys
    #[argh(option)]
    sign: Option<String>,

    /// path to the global config
    #[argh(option)]
    global_config: PathBuf,
}

impl CmdSend {
    async fn run(self, node_rpc: NodeRpc) -> Result<serde_json::Value> {
        let address = parse_address(&self.address)?;
        let method = parse_contract_method(&self.abi, &self.method)?;
        let input = nekoton_abi::parse_abi_tokens(&method.inputs, self.args)?;

        let global_config = GlobalConfig::load(self.global_config)?;

        let stats = match node_rpc.get_stats().await? {
            NodeStats::Running(stats) => stats,
            NodeStats::NotReady => anyhow::bail!("node is not ready"),
        };

        tracing::info!("STATS: {stats:?}");

        let subscription = BlockSubscription::new(
            global_config,
            everscale_network::adnl::NodeIdShort::new(stats.overlay_adnl_id),
        )
        .await?;

        let mut attempt = 0;
        loop {
            subscription.get_block(&stats.last_mc_block).await?;

            let next = subscription
                .get_next_block(&stats.last_mc_block, attempt)
                .await?;
            tracing::info!("Next block: {next:?}");
            if next.is_some() {
                break;
            }

            tokio::time::sleep(Duration::from_millis(100)).await;
            attempt += 1;
        }

        Ok(Default::default())
    }
}

fn default_args() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}
