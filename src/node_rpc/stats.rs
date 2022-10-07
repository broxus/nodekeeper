use std::str::FromStr;

use broxus_util::{serde_base64_array, serde_hex_array};
use serde::{Deserialize, Serialize};

use super::proto;
use crate::util::serde_block_id;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum NodeStats {
    NotReady,
    Running(RunningStats),
}

#[derive(Clone, Debug, Serialize)]
pub struct RunningStats {
    pub node_version: NodeVersion,
    pub mc_time: u32,
    pub mc_time_diff: i32,
    pub sc_time_diff: i32,
    #[serde(with = "serde_block_id")]
    pub last_mc_block: ton_block::BlockIdExt,
    pub in_current_vset: ValidatorSetEntry,
    pub in_next_vset: ValidatorSetEntry,
}

impl TryFrom<proto::Stats> for NodeStats {
    type Error = StatsError;

    fn try_from(stats: proto::Stats) -> Result<Self, Self::Error> {
        let mut sync_status = None;
        let mut mc_time = None;
        let mut mc_time_diff = None;
        let mut sc_time_diff = None;
        let mut node_version = None;
        let mut in_current_vset = None;
        let mut current_vset_adnl = None;
        let mut in_next_vset = None;
        let mut next_vset_adnl = None;
        let mut last_mc_block = None;

        #[inline]
        fn parse_stat<'de, T: Deserialize<'de>>(value: &'de [u8]) -> Result<T, StatsError> {
            serde_json::from_slice::<T>(value).map_err(|_| StatsError::InvalidValue)
        }

        #[derive(Debug, Deserialize)]
        struct Adnl(#[serde(with = "serde_base64_array")] [u8; 32]);

        for item in stats.items {
            match item.key.as_slice() {
                STATS_SYNC_STATUS => {
                    sync_status = Some(parse_stat::<SyncStatus>(&item.value)?);
                }
                STATS_MC_BLOCK_TIME => {
                    mc_time = Some(parse_stat::<u32>(&item.value)?);
                }
                STATS_NODE_VERSION => {
                    let str = parse_stat::<String>(&item.value)?;
                    node_version = Some(NodeVersion::from_str(&str)?);
                }
                STATS_TIMEDIFF => {
                    mc_time_diff = Some(parse_stat::<i32>(&item.value)?);
                }
                STATS_SHARDS_TIMEDIFF => {
                    #[derive(Deserialize)]
                    #[serde(untagged)]
                    enum ShardsTimeDiff<'a> {
                        Unknown(&'a str),
                        Known(i32),
                    }
                    sc_time_diff = match parse_stat(&item.value)? {
                        ShardsTimeDiff::Unknown(..) => None,
                        ShardsTimeDiff::Known(diff) => Some(diff),
                    };
                }
                STATS_IN_CURRENT_VSET => {
                    in_current_vset = Some(parse_stat::<bool>(&item.value)?);
                }
                STATS_CURRENT_VSET_ADNL => {
                    current_vset_adnl = Some(parse_stat(&item.value)?);
                }
                STATS_IN_NEXT_VSET => {
                    in_next_vset = Some(parse_stat::<bool>(&item.value)?);
                }
                STATS_NEXT_VSET_ADNL => {
                    next_vset_adnl = Some(parse_stat(&item.value)?);
                }
                STATS_LAST_APPLIED_MC_BLOCK => {
                    #[derive(Debug, Deserialize)]
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
                            let wc = i32::from_str(wc).map_err(|_| StatsError::InvalidValue)?;
                            let shard = u64::from_str_radix(shard, 16)
                                .map_err(|_| StatsError::InvalidValue)?;
                            ton_block::ShardIdent::with_tagged_prefix(wc, shard)
                                .map_err(|_| StatsError::InvalidValue)?
                        }
                        _ => return Err(StatsError::InvalidValue),
                    };

                    last_mc_block = Some(ton_block::BlockIdExt {
                        shard_id,
                        seq_no: block.seq_no,
                        file_hash: block.rh.into(),
                        root_hash: block.fh.into(),
                    });
                }
                _ => {}
            }
        }

        if !matches!(sync_status, Some(SyncStatus::SynchronizationFinished)) {
            return Ok(Self::NotReady);
        }

        match (
            mc_time,
            mc_time_diff,
            sc_time_diff,
            last_mc_block,
            node_version,
        ) {
            (
                Some(mc_time),
                Some(mc_time_diff),
                Some(sc_time_diff),
                Some(last_mc_block),
                Some(node_version),
            ) => {
                let in_current_vset = match (in_current_vset, current_vset_adnl) {
                    (Some(true), Some(Adnl(adnl))) => ValidatorSetEntry::Validator(adnl),
                    (Some(true), None) => return Err(StatsError::FieldsMissing),
                    _ => ValidatorSetEntry::None,
                };

                let in_next_vset = match (in_next_vset, next_vset_adnl) {
                    (Some(true), Some(Adnl(adnl))) => ValidatorSetEntry::Validator(adnl),
                    (Some(true), None) => return Err(StatsError::FieldsMissing),
                    _ => ValidatorSetEntry::None,
                };

                Ok(Self::Running(RunningStats {
                    mc_time,
                    mc_time_diff,
                    sc_time_diff,
                    last_mc_block,
                    in_current_vset,
                    in_next_vset,
                    node_version,
                }))
            }
            _ => Err(StatsError::FieldsMissing),
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize)]
pub struct NodeVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl FromStr for NodeVersion {
    type Err = StatsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut parts = s.split('.');

        fn parse_part(part: &str) -> Result<u32, StatsError> {
            u32::from_str(part).map_err(|_| StatsError::InvalidValue)
        }

        match (parts.next(), parts.next(), parts.next()) {
            (Some(major), Some(minor), Some(patch)) => Ok(Self {
                major: parse_part(major)?,
                minor: parse_part(minor)?,
                patch: parse_part(patch)?,
            }),
            _ => Err(StatsError::InvalidValue),
        }
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ValidatorSetEntry {
    None,
    Validator([u8; 32]),
}

impl Serialize for ValidatorSetEntry {
    fn serialize<S: serde::ser::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::None => serializer.serialize_none(),
            Self::Validator(adnl) => serializer.serialize_some(&hex::encode(adnl)),
        }
    }
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

#[derive(thiserror::Error, Debug)]
pub enum StatsError {
    #[error("invalid value")]
    InvalidValue,
    #[error("fields missing")]
    FieldsMissing,
}

const STATS_SYNC_STATUS: &[u8] = b"sync_status";
const STATS_MC_BLOCK_TIME: &[u8] = b"masterchainblocktime";
const STATS_NODE_VERSION: &[u8] = b"node_version";
const STATS_TIMEDIFF: &[u8] = b"timediff";
const STATS_SHARDS_TIMEDIFF: &[u8] = b"shards_timediff";
const STATS_IN_CURRENT_VSET: &[u8] = b"in_current_vset_p34";
const STATS_CURRENT_VSET_ADNL: &[u8] = b"current_vset_p34_adnl_id";
const STATS_IN_NEXT_VSET: &[u8] = b"in_next_vset_p36";
const STATS_NEXT_VSET_ADNL: &[u8] = b"next_vset_p36_adnl_id";
const STATS_LAST_APPLIED_MC_BLOCK: &[u8] = b"last_applied_masterchain_block_id";
