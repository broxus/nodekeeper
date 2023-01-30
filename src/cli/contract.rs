use std::path::PathBuf;

use anyhow::{Context, Result};
use argh::FromArgs;
use nekoton_abi::FunctionExt;
use ton_block::{Deserializable, Serializable};

use super::CliContext;
use crate::config::{AppConfig, StoredKeys};
use crate::network::{NodeTcpRpc, NodeUdpRpc, Subscription};
use crate::util::*;

#[derive(FromArgs)]
/// Contract interaction stuff
#[argh(subcommand, name = "contract")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: SubCmd,
}

impl Cmd {
    pub async fn run(self, ctx: CliContext) -> Result<()> {
        let response = match self.subcommand {
            SubCmd::StateInit(cmd) => cmd.run()?,
            SubCmd::Call(cmd) => cmd.run(ctx.load_config()?).await?,
            SubCmd::Send(cmd) => cmd.run(ctx.load_config()?).await?,
        };

        print_output(response);
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
    StateInit(CmdStateInit),
    Call(CmdCall),
    Send(CmdSend),
}

#[derive(FromArgs)]
/// Computes contract address
#[argh(subcommand, name = "stateinit")]
struct CmdStateInit {
    /// base64 encoded TVC or empty for input from stdin
    #[argh(positional)]
    tvc: Option<String>,

    /// workchain
    #[argh(option, short = 'w', default = "0")]
    workchain: i8,

    /// static variables
    #[argh(option, short = 'd', long = "data")]
    data: Option<serde_json::Value>,

    /// path to the JSON ABI file
    #[argh(option, short = 'a')]
    abi: Option<PathBuf>,

    /// explicit contract pubkey
    #[argh(option, short = 'p')]
    pubkey: Option<String>,
}

impl CmdStateInit {
    fn run(self) -> Result<serde_json::Value> {
        let mut state_init = {
            let tvc = parse_optional_input(self.tvc, false)?;
            ton_block::StateInit::construct_from_bytes(&tvc).context("invalid TVC")?
        };

        if let Some(abi) = self.abi {
            let abi = parse_contract_abi(abi)?;

            if let Some(pubkey) = parse_optional_pubkey(self.pubkey)? {
                let data = state_init.data().context("TVC doesn't contain data")?;
                let data = ton_abi::Contract::insert_pubkey(data.into(), pubkey.as_bytes())
                    .context("failed to insert pubkey")?;
                state_init.set_data(data.into_cell());
            }

            if let Some(tokens) = self.data {
                let params = abi
                    .data
                    .values()
                    .map(|item| item.value.clone())
                    .collect::<Vec<_>>();
                let static_params = nekoton_abi::parse_abi_tokens(&params, tokens)?;

                let data = state_init.data().context("TVC doesn't contain data")?;
                let data = abi
                    .update_data(data.into(), &static_params)
                    .context("failed to update TVC static data")?;
                state_init.set_data(data.into_cell());
            }
        } else {
            anyhow::ensure!(
                self.data.is_none() && self.pubkey.is_none(),
                "`abi` param is required to alter the provided TVC"
            );
        }

        let tvc = state_init.serialize()?;
        let address = format!("{}:{:x}", self.workchain, tvc.repr_hash());
        let tvc = base64::encode(ton_types::serialize_toc(&tvc)?);

        Ok(serde_json::json!({
            "address": address,
            "tvc": tvc,
        }))
    }
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
    #[argh(option, short = 'a')]
    abi: PathBuf,

    /// contract address
    #[argh(option, short = 'd', long = "addr")]
    address: String,

    /// execute method as responsible. (NOTE: requires first argument of type `uint32`)
    #[argh(switch, short = 'r')]
    responsible: bool,
}

impl CmdCall {
    async fn run(self, config: AppConfig) -> Result<serde_json::Value> {
        let node_rpc = NodeTcpRpc::new(config.control()?).await?;

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
    #[argh(option, short = 'a')]
    abi: PathBuf,

    /// contract address
    #[argh(option, short = 'd', long = "addr")]
    address: String,

    /// message expiration timeout
    #[argh(option, short = 't', default = "60")]
    timeout: u32,

    /// seed phrase or path to the keys
    #[argh(option, short = 's')]
    sign: Option<PathBuf>,

    /// base64 encoded state init
    #[argh(option, short = 'i')]
    state_init: Option<String>,
}

impl CmdSend {
    async fn run(self, config: AppConfig) -> Result<serde_json::Value> {
        // Prepare RPC clients
        let node_tcp_rpc = NodeTcpRpc::new(config.control()?)
            .await
            .context("failed to build node TCP client")?;
        let node_udp_rpc = NodeUdpRpc::new(config.adnl()?)
            .await
            .context("failed to build node UDP client")?;

        // Parse arguments
        let address = parse_address(&self.address)?;

        let abi = parse_contract_abi(&self.abi)?;
        let method = abi
            .functions
            .get(&self.method)
            .with_context(|| format!("method `{}` not found", self.method))?;

        let input = nekoton_abi::parse_abi_tokens(&method.inputs, self.args)?;
        let keys = self.sign.map(StoredKeys::load_as_keypair).transpose()?;
        let state_init = parse_optional_state_init(self.state_init)?;

        // Check whether the node is running
        node_tcp_rpc.get_stats().await?.try_into_running()?;

        // Create subscription
        let subscription = Subscription::new(node_tcp_rpc, node_udp_rpc);
        let signature_id = subscription.get_signature_id().await?;

        // Prepare external message
        let (expire_at, headers) =
            make_default_headers(keys.as_ref().map(|keypair| keypair.public), self.timeout);

        let body = method.encode_input(
            &headers,
            &input,
            false,
            keys.as_ref().map(|keypair| (keypair, signature_id)),
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

        // Send external message and wait until it is delivered
        let TransactionWithHash {
            hash: tx_hash,
            data: tx,
        } = subscription
            .send_message(&message, expire_at)
            .await?
            .context("message expired")?;

        // Parse transaction
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

        // Done
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

    let Some(mut body) = msg.body() else {
        return Ok(None)
    };

    let Ok(function_id) = body.get_next_u32() else {
        return Ok(None);
    };

    if function_id == method.output_id {
        let output =
            ton_abi::TokenValue::decode_params(&method.outputs, body, &method.abi_version, false)?;
        Ok(Some(ParsedData::Output(output)))
    } else {
        let Some(event) = abi.events.values().find(|event| event.id == function_id) else {
            return Ok(None);
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
