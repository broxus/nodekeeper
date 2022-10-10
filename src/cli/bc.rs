use std::path::PathBuf;

use anyhow::{Context, Result};
use argh::FromArgs;
use nekoton_abi::FunctionExt;
use nekoton_utils::Clock;

use super::CliContext;
use crate::node_rpc::NodeRpc;
use crate::util::*;

#[derive(FromArgs)]
/// Blockchain interaction stuff
#[argh(subcommand, name = "bc")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: SubCmd,
}

impl Cmd {
    pub async fn run(self, mut ctx: CliContext) -> Result<()> {
        let config = ctx.load_config()?;
        let node_rpc = NodeRpc::new(&config).await?;

        let clock = nekoton_utils::SimpleClock;

        let response = match self.subcommand {
            SubCmd::Run(cmd) => {
                let address = parse_address(&cmd.address)?;
                let contract = parse_contract_abi(cmd.abi)?;
                let method = contract
                    .function(&cmd.method)
                    .ok()
                    .with_context(|| format!("method `{}` not found", cmd.method))?;
                let input = nekoton_abi::parse_abi_tokens(&method.inputs, cmd.args)?;

                let nekoton_abi::ExecutionOutput {
                    result_code,
                    tokens,
                } = run_local(&node_rpc, &clock, &address, method, &input).await?;

                let output = tokens
                    .as_deref()
                    .map(nekoton_abi::make_abi_tokens)
                    .transpose()?;

                serde_json::json!({
                    "code": result_code,
                    "output": output,
                })
            }
        };

        println!("{}", serde_json::to_string_pretty(&response)?);
        Ok(())
    }
}

pub async fn run_local(
    node_rpc: &NodeRpc,
    clock: &dyn Clock,
    address: &ton_block::MsgAddressInt,
    method: &ton_abi::Function,
    input: &[ton_abi::Token],
) -> Result<nekoton_abi::ExecutionOutput> {
    let state = node_rpc.get_shard_account_state(address).await?;
    let account = match state.read_account()? {
        ton_block::Account::Account(account) => account,
        ton_block::Account::AccountNone => anyhow::bail!("account not deployed"),
    };
    method.run_local(clock, account, input)
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    Run(CmdRun),
}

#[derive(FromArgs)]
/// Run get method
#[argh(subcommand, name = "run")]
struct CmdRun {
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
}

fn default_args() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}
