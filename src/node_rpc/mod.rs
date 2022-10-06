use std::str::FromStr;
use std::time::Duration;

use anyhow::Result;
use broxus_util::serde_hex_array;
use everscale_crypto::ed25519;
use serde::Deserialize;
use tl_proto::{IntermediateBytes, TlRead, TlWrite};

use crate::config::Config;
use crate::tcp_adnl::{TcpAdnl, TcpAdnlConfig, TcpAdnlError};

mod proto;

pub struct NodeRpc {
    tcp_adnl: TcpAdnl,
    query_timeout: Duration,
}

impl NodeRpc {
    pub async fn new(config: &Config) -> Result<Self, NodeRpcError> {
        let tcp_adnl = TcpAdnl::connect(TcpAdnlConfig {
            server_address: config.server_address,
            server_pubkey: config.server_pubkey,
            client_secret: config.client_secret,
        })
        .await
        .map_err(NodeRpcError::QueryFailed)?;

        let query_timeout = config.query_timeout;

        Ok(Self {
            tcp_adnl,
            query_timeout,
        })
    }

    pub fn connection(&self) -> &TcpAdnl {
        &self.tcp_adnl
    }

    pub async fn generate_key_pair(&self) -> Result<[u8; 32], NodeRpcError> {
        let proto::KeyHash { key_hash } = self.query(proto::GenerateKeyPair).await?;
        Ok(key_hash)
    }

    pub async fn export_public_key(
        &self,
        key_hash: &[u8; 32],
    ) -> Result<ed25519::PublicKey, NodeRpcError> {
        let pubkey: everscale_crypto::tl::PublicKeyOwned =
            self.query(proto::ExportPublicKey { key_hash }).await?;
        ed25519::PublicKey::from_tl(pubkey.as_equivalent_ref()).ok_or(NodeRpcError::InvalidPubkey)
    }

    pub async fn sign(&self, key_hash: &[u8; 32], data: &[u8]) -> Result<[u8; 64], NodeRpcError> {
        let proto::Signature { signature } = self.query(proto::Sign { key_hash, data }).await?;
        signature
            .try_into()
            .map_err(|_| NodeRpcError::InvalidSignature)
    }

    pub async fn add_validator_permanent_key(
        &self,
        key_hash: &[u8; 32],
        election_date: u32,
        ttl: u32,
    ) -> Result<(), NodeRpcError> {
        self.query(proto::AddValidatorPermanentKey {
            key_hash,
            election_date,
            ttl,
        })
        .await
        .map(expect_success)
    }

    pub async fn add_validator_temp_key(
        &self,
        permanent_key_hash: &[u8; 32],
        key_hash: &[u8; 32],
        ttl: u32,
    ) -> Result<(), NodeRpcError> {
        self.query(proto::AddValidatorTempKey {
            permanent_key_hash,
            key_hash,
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
    ) -> Result<(), NodeRpcError> {
        self.query(proto::AddValidatorAdnlAddress {
            permanent_key_hash,
            key_hash,
            ttl,
        })
        .await
        .map(expect_success)
    }

    pub async fn add_adnl_id(
        &self,
        key_hash: &[u8; 32],
        category: u32,
    ) -> Result<(), NodeRpcError> {
        let proto::Success = self.query(proto::AddAdnlId { key_hash, category }).await?;
        Ok(())
    }

    pub async fn get_stats(&self) -> Result<(), NodeRpcError> {
        let stats = self.query::<_, proto::Stats>(proto::GetStats).await?;
        let stats = NodeStats::try_from(stats)?;
        Ok(())
    }

    pub async fn set_states_gc_interval(&self, interval_ms: u32) -> Result<(), NodeRpcError> {
        self.query(proto::SetStatesGcInterval { interval_ms })
            .await
            .map(expect_success)
    }

    pub async fn send_message(&self, message: &[u8]) -> Result<(), NodeRpcError> {
        // NOTE: proto::Success is used here on purpose instead of SendMsgStatus
        self.query(proto::SendMessage { body: message })
            .await
            .map(expect_success)
    }

    pub async fn get_config_all(&self) -> Result<(), NodeRpcError> {
        let proto::ConfigInfo { id, .. } = self
            .query(proto::GetConfigAll {
                mode: 0,
                id: proto::BlockIdExt::default(),
            })
            .await?;

        Ok(())
    }

    pub async fn get_config_param(&self, param: u32) -> Result<(), NodeRpcError> {
        let proto::ConfigInfo { id, .. } = self
            .query(proto::GetConfigParams {
                mode: 0,
                id: proto::BlockIdExt::default(),
                param_list: std::slice::from_ref(&param),
            })
            .await?;

        println!("{id:?}");

        Ok(())
    }

    pub async fn get_shard_account_state(&self, address: &str) -> Result<(), NodeRpcError> {
        let _shard_account = self
            .query::<_, proto::ShardAccount>(proto::GetShardAccountState {
                address: address.as_bytes(),
            })
            .await?;

        todo!()
    }

    async fn query<Q, R>(&self, query: Q) -> Result<R, NodeRpcError>
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
            Ok(None) => Err(NodeRpcError::QueryTimeout),
            Err(e) => Err(NodeRpcError::QueryFailed(e)),
        }
    }
}

pub struct NodeStats {}

impl TryFrom<proto::Stats> for NodeStats {
    type Error = NodeRpcError;

    fn try_from(stats: proto::Stats) -> Result<Self, Self::Error> {
        let mut sync_status = None;
        let mut mc_block_time = None;
        let mut mc_block_seqno = None;
        let mut node_version = None;
        let mut timediff = None;
        let mut shards_timediff = None;
        let mut in_current_vset = None;
        let mut current_vset_adnl = None;
        let mut in_next_vset = None;
        let mut next_vset_adnl = None;
        let mut last_applied_mc_block = None;

        #[inline]
        fn parse_stat<'de, T: Deserialize<'de>>(value: &'de [u8]) -> Result<T, NodeRpcError> {
            serde_json::from_slice::<T>(value).map_err(|_| NodeRpcError::InvalidStats)
        }

        #[derive(Deserialize)]
        struct Adnl(#[serde(with = "serde_hex_array")] [u8; 32]);

        for item in stats.items {
            match item.key.as_slice() {
                STATS_SYNC_STATUS => {
                    sync_status = Some(parse_stat::<SyncStatus>(&item.value)?);
                }
                STATS_MC_BLOCK_TIME => {
                    mc_block_time = Some(parse_stat::<u32>(&item.value)?);
                }
                STATS_MC_BLOCK_SEQNO => {
                    mc_block_seqno = Some(parse_stat::<u32>(&item.value)?);
                }
                STATS_NODE_VERSION => {
                    node_version = Some(parse_stat::<String>(&item.value)?);
                }
                STATS_TIMEDIFF => {
                    timediff = Some(parse_stat::<i32>(&item.value)?);
                }
                STATS_SHARDS_TIMEDIFF => {
                    #[derive(Deserialize)]
                    #[serde(untagged)]
                    enum ShardsTimeDiff<'a> {
                        Unknown(&'a str),
                        Known(i32),
                    }
                    shards_timediff = match parse_stat(&item.value)? {
                        ShardsTimeDiff::Unknown(..) => None,
                        ShardsTimeDiff::Known(diff) => Some(diff),
                    };
                }
                STATS_IN_CURRENT_VSET => {
                    in_current_vset = Some(parse_stat::<bool>(&item.value)?);
                }
                STATS_CURRENT_VSET_ADNL => {
                    let Adnl(adnl) = parse_stat(&item.value)?;
                    current_vset_adnl = Some(adnl);
                }
                STATS_IN_NEXT_VSET => {
                    in_next_vset = Some(parse_stat::<bool>(&item.value)?);
                }
                STATS_NEXT_VSET_ADNL => {
                    let Adnl(adnl) = parse_stat(&item.value)?;
                    next_vset_adnl = Some(adnl);
                }
                STATS_LAST_APPLIED_MC_BLOCK => {
                    #[derive(Deserialize)]
                    struct Block<'a> {
                        shard: &'a str,
                        seq_no: u32,
                        #[serde(with = "serde_hex_array")]
                        rh: [u8; 32],
                        #[serde(with = "serde_hex_array")]
                        fh: [u8; 32],
                    }
                    let block = parse_stat::<Block>(&item.value)?;

                    let mut shard_parts = block.shard.split(':');
                    let shard_id = match (shard_parts.next(), shard_parts.next()) {
                        (Some(wc), Some(shard)) => {
                            let wc = i32::from_str(wc).map_err(|_| NodeRpcError::InvalidStats)?;
                            let shard = u64::from_str_radix(shard, 16)
                                .map_err(|_| NodeRpcError::InvalidStats)?;
                            ton_block::ShardIdent::with_tagged_prefix(wc, shard)
                                .map_err(|_| NodeRpcError::InvalidStats)?
                        }
                        _ => return Err(NodeRpcError::InvalidStats),
                    };

                    last_applied_mc_block = Some(ton_block::BlockIdExt {
                        shard_id,
                        seq_no: block.seq_no,
                        file_hash: block.rh.into(),
                        root_hash: block.fh.into(),
                    });
                }
                _ => {}
            }
        }

        println!("S: {sync_status:?}");
        println!("S: {mc_block_time:?}");
        println!("S: {mc_block_seqno:?}");
        println!("S: {node_version:?}");
        println!("S: {timediff:?}");
        println!("S: {last_applied_mc_block:?}");

        Ok(Self {})
    }
}

const STATS_MC_BLOCK: &[u8] = b"masterchainblock";
const STATS_SYNC_STATUS: &[u8] = b"sync_status";
const STATS_MC_BLOCK_TIME: &[u8] = b"masterchainblocktime";
const STATS_MC_BLOCK_SEQNO: &[u8] = b"masterchainblocknumber";
const STATS_NODE_VERSION: &[u8] = b"node_version";
const STATS_TIMEDIFF: &[u8] = b"timediff";
const STATS_SHARDS_TIMEDIFF: &[u8] = b"shards_timediff";
const STATS_IN_CURRENT_VSET: &[u8] = b"in_current_vset_p34";
const STATS_CURRENT_VSET_ADNL: &[u8] = b"current_vset_p34_adnl_id";
const STATS_IN_NEXT_VSET: &[u8] = b"in_next_vset_p36";
const STATS_NEXT_VSET_ADNL: &[u8] = b"next_vset_p36_adnl_id";
const STATS_LAST_APPLIED_MC_BLOCK: &[u8] = b"last_applied_masterchain_block_id";

#[derive(Copy, Clone, Debug)]
pub enum ValidatorSetEntry {
    None,
    Validator([u8; 32]),
}

#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    StartBoot,
    LoadMasterState,
    LoadShardStates,
    FinishBoot,
    SynchronizationByBlocks,
    SynchronizationFinished,
    CheckingDb,
    DbBroken,
    NoSetStatus,
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
    InvalidStats,
    #[error("invalid pubkey")]
    InvalidPubkey,
    #[error("invalid signature")]
    InvalidSignature,
    #[error("invalid string")]
    InvalidString,
}
