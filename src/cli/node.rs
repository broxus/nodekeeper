use anyhow::Result;
use argh::FromArgs;
use serde::Serialize;
use ton_block::Serializable;

use super::CliContext;
use crate::node_tcp_rpc::{ConfigParamWithId, ConfigWithId, NodeTcpRpc};
use crate::util::*;

#[derive(FromArgs)]
/// Raw node tools operations
#[argh(subcommand, name = "node")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: SubCmd,
}

impl Cmd {
    pub async fn run(self, mut ctx: CliContext) -> Result<()> {
        let config = ctx.load_config()?;
        let rpc_node = NodeTcpRpc::new(&config).await?;

        let response = match self.subcommand {
            SubCmd::GenKey(_) => {
                let hey_hash = rpc_node.generate_key_pair().await?;
                serde_json::json!({
                    "key_hash": hex::encode(hey_hash),
                })
            }
            SubCmd::ExportPubKey(cmd) => {
                let key_hash = parse_key_hash(&cmd.key_hash)?;
                let public = rpc_node.export_public_key(&key_hash).await?;
                serde_json::json!({
                    "public": hex::encode(public.as_bytes())
                })
            }
            SubCmd::Sign(cmd) => {
                let key_hash = parse_key_hash(&cmd.key_hash)?;
                let data = parse_optional_input(cmd.data, false)?;
                let signature = rpc_node.sign(&key_hash, &data).await?;
                serde_json::json!({
                    "signature": base64::encode(signature),
                })
            }
            SubCmd::AddPermKey(cmd) => {
                let key_hash = parse_key_hash(&cmd.key_hash)?;
                rpc_node
                    .add_validator_permanent_key(&key_hash, cmd.election_id, cmd.ttl)
                    .await?;
                serde_json::json!({})
            }
            SubCmd::AddValidatorAddr(cmd) => {
                let permanent_key_hash = parse_key_hash(&cmd.permanent_key_hash)?;
                let key_hash = parse_key_hash(&cmd.key_hash)?;
                rpc_node
                    .add_validator_adnl_address(&permanent_key_hash, &key_hash, cmd.ttl)
                    .await?;
                serde_json::json!({})
            }
            SubCmd::GetStats(_) => {
                let stats = rpc_node.get_stats().await?;
                serde_json::to_value(stats)?
            }
            SubCmd::SetStatesGcInterval(cmd) => {
                rpc_node.set_states_gc_interval(cmd.interval).await?;
                serde_json::json!({})
            }
            SubCmd::GetConfig(_) => {
                #[derive(Serialize)]
                struct Response {
                    #[serde(with = "serde_block_id")]
                    block_id: ton_block::BlockIdExt,
                    config: String,
                }

                let ConfigWithId { block_id, config } = rpc_node.get_config_all().await?;
                serde_json::to_value(Response {
                    block_id,
                    config: base64::encode(ton_types::serialize_toc(&config.serialize()?)?),
                })?
            }
            SubCmd::GetConfigParam(cmd) => {
                #[derive(Serialize)]
                struct Response {
                    #[serde(with = "serde_block_id")]
                    block_id: ton_block::BlockIdExt,
                    value: serde_json::Value,
                }

                let ConfigParamWithId { block_id, param } =
                    rpc_node.get_config_param(cmd.param).await?;
                let param = serde_json::from_str::<serde_json::Value>(&param)?;
                let value = match param {
                    serde_json::Value::Object(params) => params
                        .into_iter()
                        .map(|(_, value)| value)
                        .next()
                        .unwrap_or_default(),
                    _ => serde_json::Value::default(),
                };
                serde_json::to_value(Response { block_id, value })?
            }
            SubCmd::GetAccount(cmd) => {
                let address = parse_address(&cmd.address)?;
                let param = rpc_node.get_shard_account_state(&address).await?;
                serde_json::json!({
                    "state": base64::encode(ton_types::serialize_toc(&param.serialize()?)?)
                })
            }
            SubCmd::SendMessage(cmd) => {
                let data = parse_optional_input(cmd.data, false)?;
                rpc_node.send_message(&data).await?;
                serde_json::json!({})
            }
        };

        print_output(response);
        Ok(())
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    GenKey(CmdNodeGenKey),
    ExportPubKey(CmdNodeExportPubKey),
    Sign(CmdNodeSign),
    AddPermKey(CmdAddPermKey),
    AddValidatorAddr(CmdAddValidatorAddr),
    GetStats(CmdGetStats),
    SetStatesGcInterval(CmdSetStatesGcInterval),
    GetConfig(CmdGetConfig),
    GetConfigParam(CmdGetConfigParam),
    GetAccount(CmdGetAccount),
    SendMessage(CmdSendMessage),
}

#[derive(FromArgs)]
/// Generates a new keypair and returns the hash of it.
#[argh(subcommand, name = "genkey")]
struct CmdNodeGenKey {}

#[derive(FromArgs)]
/// Exports public key of the specified key pair.
#[argh(subcommand, name = "exportkey")]
struct CmdNodeExportPubKey {
    /// exported keypair hash (hex encoded string)
    #[argh(positional)]
    key_hash: String,
}

#[derive(FromArgs)]
/// Signs arbitrary data with the specified hash
#[argh(subcommand, name = "sign")]
struct CmdNodeSign {
    /// keypair hash (hex encoded string)
    #[argh(positional)]
    key_hash: String,

    /// base64 encoded data or empty for input from stdin
    #[argh(positional)]
    data: Option<String>,
}

#[derive(FromArgs)]
/// Adds validator permanent key
#[argh(subcommand, name = "addpermkey")]
struct CmdAddPermKey {
    /// keypair hash (hex encoded string)
    #[argh(positional)]
    key_hash: String,

    /// election id (unix timestamp)
    #[argh(option, short = 'e')]
    election_id: u32,

    /// key ttl (in seconds)
    #[argh(option, short = 't')]
    ttl: u32,
}

#[derive(FromArgs)]
/// Adds validator ADNL address
#[argh(subcommand, name = "addvalidatoraddr")]
struct CmdAddValidatorAddr {
    /// permanent keypair hash (hex encoded string)
    #[argh(positional)]
    permanent_key_hash: String,

    /// keypair hash (hex encoded string)
    #[argh(positional)]
    key_hash: String,

    /// address ttl (in seconds)
    #[argh(option, short = 't')]
    ttl: u32,
}

#[derive(FromArgs)]
/// Get validator node stats
#[argh(subcommand, name = "getstats")]
struct CmdGetStats {}

#[derive(FromArgs)]
/// Sets states GC interval
#[argh(subcommand, name = "setgcinterval")]
struct CmdSetStatesGcInterval {
    /// states GC interval (in seconds)
    #[argh(positional)]
    interval: u32,
}

#[derive(FromArgs)]
/// Gets raw blockchain config
#[argh(subcommand, name = "getconfig")]
struct CmdGetConfig {}

#[derive(FromArgs)]
/// Gets config param by id
#[argh(subcommand, name = "getparam")]
struct CmdGetConfigParam {
    /// param id
    #[argh(positional)]
    param: u32,
}

#[derive(FromArgs)]
/// Gets account state
#[argh(subcommand, name = "getaccount")]
struct CmdGetAccount {
    /// address (raw format)
    #[argh(positional)]
    address: String,
}

#[derive(FromArgs)]
/// Broadcasts external message
#[argh(subcommand, name = "sendmessage")]
struct CmdSendMessage {
    /// base64 encoded message data or empty for input from stdin
    #[argh(positional)]
    data: Option<String>,
}
