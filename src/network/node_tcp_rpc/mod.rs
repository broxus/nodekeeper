use std::time::Duration;

use anyhow::Result;
use everscale_crypto::ed25519;
use tl_proto::{IntermediateBytes, TlRead, TlWrite};
use ton_block::Deserializable;

use self::stats::StatsError;
pub use self::stats::{NodeStats, ValidatorSetEntry};
use self::tcp_adnl::{TcpAdnl, TcpAdnlConfig, TcpAdnlError};
use crate::config::AppConfigControl;

mod proto;
mod stats;
mod tcp_adnl;

#[derive(Clone)]
pub struct NodeTcpRpc {
    tcp_adnl: TcpAdnl,
    query_timeout: Duration,
}

impl NodeTcpRpc {
    pub async fn new(config: &AppConfigControl) -> Result<Self> {
        let tcp_adnl = TcpAdnl::connect(TcpAdnlConfig {
            server_address: config.server_address.into(),
            server_pubkey: config.server_pubkey,
            client_secret: config.client_secret,
            connection_timeout: config.connection_timeout,
        })
        .await
        .map_err(NodeRpcError::ConnectionFailed)?;

        let query_timeout = config.query_timeout;

        Ok(Self {
            tcp_adnl,
            query_timeout,
        })
    }

    pub async fn generate_key_pair(&self) -> Result<[u8; 32]> {
        let proto::KeyHash { key_hash } = self.query(proto::GenerateKeyPair).await?;
        Ok(key_hash)
    }

    pub async fn export_public_key(&self, key_hash: &[u8; 32]) -> Result<ed25519::PublicKey> {
        let pubkey: everscale_crypto::tl::PublicKeyOwned =
            self.query(proto::ExportPublicKey { key_hash }).await?;
        ed25519::PublicKey::from_tl(pubkey.as_equivalent_ref())
            .ok_or_else(|| NodeRpcError::InvalidPubkey.into())
    }

    pub async fn sign(&self, key_hash: &[u8; 32], data: &[u8]) -> Result<[u8; 64]> {
        let proto::Signature { signature } = self.query(proto::Sign { key_hash, data }).await?;
        signature
            .try_into()
            .map_err(|_| NodeRpcError::InvalidSignature.into())
    }

    pub async fn add_validator_permanent_key(
        &self,
        key_hash: &[u8; 32],
        election_date: u32,
        ttl: u32,
    ) -> Result<()> {
        self.query(proto::AddValidatorPermanentKey {
            key_hash,
            election_date,
            ttl,
        })
        .await
        .map(expect_success)
    }

    pub async fn add_validator_adnl_address(
        &self,
        permanent_key_hash: &[u8; 32],
        key_hash: &[u8; 32],
        ttl: u32,
    ) -> Result<()> {
        self.query(proto::AddValidatorAdnlAddress {
            permanent_key_hash,
            key_hash,
            ttl,
        })
        .await
        .map(expect_success)
    }

    pub async fn get_stats(&self) -> Result<NodeStats> {
        let stats = self.query::<_, proto::Stats>(proto::GetStats).await?;
        NodeStats::try_from(stats).map_err(|e| NodeRpcError::InvalidStats(e).into())
    }

    pub async fn get_raw_stats(&self) -> Result<serde_json::Value> {
        let stats = self.query::<_, proto::Stats>(proto::GetStats).await?;

        let mut result = serde_json::Map::new();
        for stat in stats.items {
            let key = String::from_utf8_lossy(&stat.key).into_owned();
            let Ok(value) = serde_json::from_slice(&stat.value) else {
                continue;
            };
            result.insert(key, value);
        }

        Ok(serde_json::Value::Object(result))
    }

    pub async fn set_states_gc_interval(&self, interval_ms: u32) -> Result<()> {
        self.query(proto::SetStatesGcInterval { interval_ms })
            .await
            .map(expect_success)
    }

    pub async fn send_message<T: AsRef<[u8]>>(&self, message: T) -> Result<()> {
        // NOTE: proto::Success is used here on purpose instead of SendMsgStatus
        self.query(proto::SendMessage {
            body: message.as_ref(),
        })
        .await
        .map(expect_success)
    }

    pub async fn get_config_all(&self) -> Result<ConfigWithId> {
        let proto::ConfigInfo {
            id, config_proof, ..
        } = self
            .query(proto::GetConfigAll {
                mode: 0,
                id: proto::BlockIdExt::default(),
            })
            .await?;

        Ok(ConfigWithId {
            block_id: convert_proto_to_block_id(id)?,
            config: ton_block::ConfigParams::construct_from_bytes(&config_proof)
                .map_err(|_| NodeRpcError::InvalidBlockchainConfig)?,
        })
    }

    pub async fn get_config_param(&self, param: u32) -> Result<ConfigParamWithId> {
        let proto::ConfigInfo {
            id, config_proof, ..
        } = self
            .query(proto::GetConfigParams {
                mode: 0,
                id: proto::BlockIdExt::default(),
                param_list: std::slice::from_ref(&param),
            })
            .await?;

        Ok(ConfigParamWithId {
            block_id: convert_proto_to_block_id(id)?,
            param: String::from_utf8(config_proof).map_err(|_| NodeRpcError::InvalidString)?,
        })
    }

    pub async fn get_shard_account_state(
        &self,
        address: &ton_block::MsgAddressInt,
    ) -> Result<ton_block::ShardAccount> {
        let shard_account = self
            .query::<_, proto::ShardAccount>(proto::GetShardAccountState {
                address: address.to_string().as_bytes(),
            })
            .await?;

        match shard_account {
            proto::ShardAccount::State(data) => {
                ton_block::ShardAccount::construct_from_bytes(&data)
                    .map_err(|_| NodeRpcError::InvalidAccountState.into())
            }
            proto::ShardAccount::Empty => Ok(ton_block::ShardAccount::default()),
        }
    }

    async fn query<Q, R>(&self, query: Q) -> Result<R>
    where
        Q: TlWrite<Repr = tl_proto::Boxed>,
        for<'a> R: TlRead<'a>,
    {
        match self
            .tcp_adnl
            .query(
                proto::ControlQuery(IntermediateBytes(query)),
                self.query_timeout,
            )
            .await
        {
            Ok(Some(data)) => Ok(data),
            Ok(None) => Err(NodeRpcError::QueryTimeout.into()),
            Err(e) => Err(NodeRpcError::QueryFailed(e).into()),
        }
    }
}

fn convert_proto_to_block_id(
    id: proto::BlockIdExtOwned,
) -> Result<ton_block::BlockIdExt, NodeRpcError> {
    Ok(ton_block::BlockIdExt {
        shard_id: ton_block::ShardIdent::with_tagged_prefix(id.workchain, id.shard)
            .map_err(|_| NodeRpcError::InvalidBlockId)?,
        seq_no: id.seqno,
        root_hash: id.root_hash.into(),
        file_hash: id.file_hash.into(),
    })
}

pub struct ConfigWithId {
    pub block_id: ton_block::BlockIdExt,
    pub config: ton_block::ConfigParams,
}

pub struct ConfigParamWithId {
    pub block_id: ton_block::BlockIdExt,
    pub param: String,
}

fn expect_success(_: proto::Success) {}

#[derive(thiserror::Error, Debug)]
pub enum NodeRpcError {
    #[error("connection failed")]
    ConnectionFailed(#[source] TcpAdnlError),
    #[error("query failed")]
    QueryFailed(#[source] TcpAdnlError),
    #[error("query timeout")]
    QueryTimeout,
    #[error("invalid stats")]
    InvalidStats(#[source] StatsError),
    #[error("invalid pubkey")]
    InvalidPubkey,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid string")]
    InvalidString,
    #[error("invalid account state")]
    InvalidAccountState,
    #[error("invalid block id")]
    InvalidBlockId,
    #[error("invalid blockchain config")]
    InvalidBlockchainConfig,
}
