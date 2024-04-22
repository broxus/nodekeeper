use std::net::Ipv4Addr;

use anyhow::{Context, Result};
use argh::FromArgs;
use serde::Serialize;
use tl_proto::BoxedConstructor;
use ton_block::Serializable;

use super::CliContext;
use crate::config::NodeConfig;
use crate::network::{ConfigParamWithId, ConfigWithId, NodeTcpRpc};
use crate::util::*;

#[derive(FromArgs)]
/// Raw node tools operations
#[argh(subcommand, name = "node")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: SubCmd,
}

impl Cmd {
    pub async fn run(self, ctx: CliContext) -> Result<()> {
        let response = match self.subcommand {
            SubCmd::GenKey(_) => {
                let rpc_node = ctx.create_rpc_node().await?;
                let hey_hash = rpc_node.generate_key_pair().await?;
                serde_json::json!({
                    "key_hash": hex::encode(hey_hash),
                })
            }
            SubCmd::ExportPubKey(cmd) => {
                let key_hash = parse_key_hash(&cmd.key_hash)?;

                let rpc_node = ctx.create_rpc_node().await?;
                let public = rpc_node.export_public_key(&key_hash).await?;
                serde_json::json!({
                    "public": hex::encode(public.as_bytes())
                })
            }
            SubCmd::Sign(cmd) => {
                let key_hash = parse_key_hash(&cmd.key_hash)?;
                let data = parse_optional_input(cmd.data, false)?;
                let data = ton_abi::extend_signature_with_id(&data, cmd.signature_id);

                let rpc_node = ctx.create_rpc_node().await?;
                let signature = rpc_node.sign(&key_hash, &data).await?;
                serde_json::json!({
                    "signature": base64::encode(signature),
                })
            }
            SubCmd::AddPermKey(cmd) => {
                let key_hash = parse_key_hash(&cmd.key_hash)?;

                let rpc_node = ctx.create_rpc_node().await?;
                rpc_node
                    .add_validator_permanent_key(&key_hash, cmd.election_id, cmd.ttl)
                    .await?;
                serde_json::json!({})
            }
            SubCmd::AddValidatorAddr(cmd) => {
                let permanent_key_hash = parse_key_hash(&cmd.permanent_key_hash)?;
                let key_hash = parse_key_hash(&cmd.key_hash)?;

                let rpc_node = ctx.create_rpc_node().await?;
                rpc_node
                    .add_validator_adnl_address(&permanent_key_hash, &key_hash, cmd.ttl)
                    .await?;
                serde_json::json!({})
            }
            SubCmd::GetStats(_) => {
                let rpc_node = ctx.create_rpc_node().await?;
                let stats = rpc_node.get_stats().await?;
                serde_json::to_value(stats)?
            }
            SubCmd::GetRawStats(_) => {
                let rpc_node = ctx.create_rpc_node().await?;
                rpc_node.get_raw_stats().await?
            }
            SubCmd::SetStatesGcInterval(cmd) => {
                let rpc_node = ctx.create_rpc_node().await?;
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

                let rpc_node = ctx.create_rpc_node().await?;
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

                let rpc_node = ctx.create_rpc_node().await?;
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

                let rpc_node = ctx.create_rpc_node().await?;
                let param = rpc_node.get_shard_account_state(&address).await?;
                serde_json::json!({
                    "state": base64::encode(ton_types::serialize_toc(&param.serialize()?)?)
                })
            }
            SubCmd::SendMessage(cmd) => {
                let data = parse_optional_input(cmd.data, false)?;

                let rpc_node = ctx.create_rpc_node().await?;
                rpc_node.send_message(&data).await?;
                serde_json::json!({})
            }
            SubCmd::GenDht(cmd) => {
                use everscale_crypto::ed25519;
                use everscale_network::proto;

                let now = cmd.time.unwrap_or_else(broxus_util::now);

                let node_config = NodeConfig::load(&ctx.dirs.node_config)?;
                let adnl = node_config
                    .get_adnl_node()
                    .context("failed to load ADNL node info")?
                    .context("ADNL node info not found")?;

                let mut address = adnl.ip_address;
                if let Some(ip) = cmd.ip {
                    address.set_ip(ip);
                }
                if let Some(port) = cmd.port {
                    address.set_port(port);
                }

                let dht_key = adnl.dht_key().context("DHT key not found")?;
                let public_key = ed25519::PublicKey::from(dht_key);

                let signature = dht_key.expand().sign(
                    proto::dht::Node {
                        id: public_key.as_tl(),
                        addr_list: proto::adnl::AddressList {
                            address: Some(proto::adnl::Address::from(&address)),
                            version: now,
                            reinit_date: now,
                            expire_at: 0,
                        },
                        version: now,
                        signature: &[],
                    }
                    .as_boxed(),
                    &public_key,
                );

                serde_json::json!({
                    "@type": "dht.node",
                    "id": {
                        "@type": "pub.ed25519",
                        "key": base64::encode(public_key.as_bytes())
                    },
                    "addr_list": {
                        "@type": "adnl.addressList",
                        "addrs": [
                            {
                                "@type": "adnl.address.udp",
                                "ip": u32::from(*address.ip()) as i32,
                                "port": address.port()
                            }
                        ],
                        "version": now,
                        "reinit_date": now,
                        "priority": 0i32,
                        "expire_at": 0i32
                    },
                    "version": now,
                    "signature": base64::encode(signature)
                })
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
    GetRawStats(CmdGetRawStats),
    SetStatesGcInterval(CmdSetStatesGcInterval),
    GetConfig(CmdGetConfig),
    GetConfigParam(CmdGetConfigParam),
    GetAccount(CmdGetAccount),
    SendMessage(CmdSendMessage),
    GenDht(CmdNodeGenDht),
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
    /// optional signature id (network id).
    #[argh(option)]
    signature_id: Option<i32>,

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
/// Get validator node stats as is
#[argh(subcommand, name = "getrawstats")]
struct CmdGetRawStats {}

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

#[derive(FromArgs)]
/// Generates signed DHT entry for this node
#[argh(subcommand, name = "gendht")]
struct CmdNodeGenDht {
    /// overwrite public IP
    #[argh(option)]
    ip: Option<Ipv4Addr>,
    /// overwrite ADNL port
    #[argh(option)]
    port: Option<u16>,
    /// explicit time
    #[argh(option)]
    time: Option<u32>,
}

impl CliContext {
    async fn create_rpc_node(self) -> Result<NodeTcpRpc> {
        let config = self.load_config()?;
        NodeTcpRpc::new(config.control()?).await
    }
}
