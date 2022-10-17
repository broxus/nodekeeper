use std::path::PathBuf;

use anyhow::{Context, Result};
use argh::FromArgs;
use nekoton_abi::FunctionExt;

use super::CliContext;
use crate::global_config::GlobalConfig;
use crate::node_tcp_rpc::{NodeStats, NodeTcpRpc};
use crate::node_udp_rpc::NodeUdpRpc;
use crate::subscription::Subscription;
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
        let node_rpc = NodeTcpRpc::new(&config).await?;

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
    node_rpc: &NodeTcpRpc,
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
    async fn run(self, node_rpc: NodeTcpRpc) -> Result<serde_json::Value> {
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

    /// message expiration timeout
    #[argh(option, long = "timeout", default = "60")]
    timeout: u32,

    /// seed phrase or path to the keys
    #[argh(option)]
    sign: Option<PathBuf>,

    /// base64 encoded state init
    #[argh(option)]
    state_init: Option<String>,

    /// path to the global config
    #[argh(option)]
    global_config: PathBuf,
}

impl CmdSend {
    async fn run(self, node_tcp_rpc: NodeTcpRpc) -> Result<serde_json::Value> {
        let address = parse_address(&self.address)?;

        let abi = parse_contract_abi(&self.abi)?;
        let method = abi
            .functions
            .get(&self.method)
            .with_context(|| format!("method `{}` not found", self.method))?;

        let input = nekoton_abi::parse_abi_tokens(&method.inputs, self.args)?;
        let keys = parse_keys(self.sign)?;
        let state_init = parse_optional_state_init(self.state_init)?;

        let (expire_at, headers) =
            make_default_headers(keys.as_ref().map(|keypair| keypair.public), self.timeout);

        let body = method.encode_input(
            &headers,
            &input,
            false,
            keys.as_ref(),
            Some(address.clone()),
        )?;

        let mut message =
            ton_block::Message::with_ext_in_header(ton_block::ExternalInboundMessageHeader {
                dst: address,
                ..Default::default()
            });

        message.set_body(body.into());

        if let Some(state_init) = state_init {
            message.set_state_init(state_init);
        }

        let global_config = GlobalConfig::load(self.global_config)?;

        let stats = match node_tcp_rpc.get_stats().await? {
            NodeStats::Running(stats) => stats,
            NodeStats::NotReady => anyhow::bail!("node is not ready"),
        };

        let node_udp_rpc = NodeUdpRpc::new(
            global_config,
            everscale_network::adnl::NodeIdShort::new(stats.overlay_adnl_id),
        )
        .await?;

        let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);
        let TransactionWithHash {
            hash: tx_hash,
            data: tx,
        } = subscription
            .send_message(&message, expire_at)
            .await?
            .context("message expired")?;

        let msg_hash = tx
            .in_msg
            .context("external inbound message not found")?
            .hash();

        let mut output = None;
        let mut events = Vec::new();

        tx.out_msgs.iterate(|ton_block::InRefValue(msg)| {
            match parse_message(&abi, method, &msg)? {
                Some(ParsedData::Output(data)) => output = Some(data),
                Some(ParsedData::Event(event)) => events.push(event),
                None => {}
            }
            Ok(true)
        })?;

        let output = if method.has_output() {
            nekoton_abi::make_abi_tokens(&output.context("method output not found")?)?
        } else {
            serde_json::Value::Object(Default::default())
        };

        Ok(serde_json::json!({
            "tx_hash": tx_hash.to_hex_string(),
            "msg_hash": msg_hash.to_hex_string(),
            "output": output,
            "events": events,
        }))
    }
}

fn parse_message<'a, 'b: 'a>(
    abi: &'b ton_abi::Contract,
    method: &'b ton_abi::Function,
    msg: &'a ton_block::Message,
) -> Result<Option<ParsedData<'b>>> {
    if !matches!(msg.header(), ton_block::CommonMsgInfo::ExtOutMsgInfo(_)) {
        return Ok(None);
    }

    let mut body = match msg.body() {
        Some(body) => body,
        None => return Ok(None),
    };

    let function_id = match body.get_next_u32() {
        Ok(id) => id,
        Err(_) => return Ok(None),
    };

    if function_id == method.output_id {
        let output =
            ton_abi::TokenValue::decode_params(&method.outputs, body, &method.abi_version, false)?;
        Ok(Some(ParsedData::Output(output)))
    } else {
        let mut event = match abi.events.values().find(|event| event.id == function_id) {
            Some(event) => event,
            None => return Ok(None),
        };

        let data =
            ton_abi::TokenValue::decode_params(&event.inputs, body, &event.abi_version, false)?;
        Ok(Some(ParsedData::Event(ParsedEvent {
            name: &event.name,
            data,
        })))
    }
}

enum ParsedData<'a> {
    Output(Vec<ton_abi::Token>),
    Event(ParsedEvent<'a>),
}

struct ParsedEvent<'a> {
    name: &'a str,
    data: Vec<ton_abi::Token>,
}

impl serde::Serialize for ParsedEvent<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeStruct};

        let data = nekoton_abi::make_abi_tokens(&self.data).map_err(Error::custom)?;
        let mut s = serializer.serialize_struct("Event", 2)?;
        s.serialize_field("name", self.name)?;
        s.serialize_field("data", &data)?;
        s.end()
    }
}

fn default_args() -> serde_json::Value {
    serde_json::Value::Object(Default::default())
}
