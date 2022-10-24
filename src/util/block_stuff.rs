use anyhow::{Context, Result};
use rustc_hash::FxHashMap;
use ton_block::Deserializable;

pub struct BlockStuff {
    id: ton_block::BlockIdExt,
    block: ton_block::Block,
}

impl BlockStuff {
    pub fn new(mut data: &[u8], id: ton_block::BlockIdExt) -> Result<Self> {
        let file_hash = ton_types::UInt256::calc_file_hash(data);
        anyhow::ensure!(id.file_hash() == file_hash, "wrong file_hash for {id}");

        let root = ton_types::deserialize_tree_of_cells(&mut data)?;
        anyhow::ensure!(
            id.root_hash() == root.repr_hash(),
            "wrong root hash for {id}"
        );

        let block = ton_block::Block::construct_from(&mut root.into())?;
        Ok(Self { id, block })
    }

    #[inline(always)]
    pub fn id(&self) -> &ton_block::BlockIdExt {
        &self.id
    }

    #[inline(always)]
    pub fn block(&self) -> &ton_block::Block {
        &self.block
    }

    pub fn read_brief_info(&self) -> Result<BriefBlockInfo> {
        let info = self.block.read_info()?;

        let (prev1, prev2) = match info.read_prev_ref()? {
            ton_block::BlkPrevInfo::Block { prev } => {
                let shard_id = if info.after_split() {
                    info.shard().merge()?
                } else {
                    *info.shard()
                };

                let id = ton_block::BlockIdExt {
                    shard_id,
                    seq_no: prev.seq_no,
                    root_hash: prev.root_hash,
                    file_hash: prev.file_hash,
                };

                (id, None)
            }
            ton_block::BlkPrevInfo::Blocks { prev1, prev2 } => {
                let prev1 = prev1.read_struct()?;
                let prev2 = prev2.read_struct()?;
                let (shard1, shard2) = info.shard().split()?;

                let id1 = ton_block::BlockIdExt {
                    shard_id: shard1,
                    seq_no: prev1.seq_no,
                    root_hash: prev1.root_hash,
                    file_hash: prev1.file_hash,
                };

                let id2 = ton_block::BlockIdExt {
                    shard_id: shard2,
                    seq_no: prev2.seq_no,
                    root_hash: prev2.root_hash,
                    file_hash: prev2.file_hash,
                };

                (id1, Some(id2))
            }
        };

        Ok(BriefBlockInfo {
            gen_utime: info.gen_utime().0,
            prev1,
            prev2,
        })
    }

    pub fn shard_blocks(&self) -> Result<FxHashMap<ton_block::ShardIdent, ton_block::BlockIdExt>> {
        let mut shards = FxHashMap::default();
        self.block()
            .read_extra()?
            .read_custom()?
            .context("given block is not a masterchain block")?
            .hashes()
            .iterate_shards(|ident, descr| {
                let last_shard_block = ton_block::BlockIdExt {
                    shard_id: ident,
                    seq_no: descr.seq_no,
                    root_hash: descr.root_hash,
                    file_hash: descr.file_hash,
                };
                shards.insert(ident, last_shard_block);
                Ok(true)
            })?;

        Ok(shards)
    }

    pub fn shard_blocks_seq_no(&self) -> Result<FxHashMap<ton_block::ShardIdent, u32>> {
        let mut shards = FxHashMap::default();
        self.block()
            .read_extra()?
            .read_custom()?
            .context("given block is not a masterchain block")?
            .hashes()
            .iterate_shards(|ident, descr| {
                shards.insert(ident, descr.seq_no);
                Ok(true)
            })?;

        Ok(shards)
    }
}

#[derive(Clone)]
pub struct BriefBlockInfo {
    pub gen_utime: u32,
    pub prev1: ton_block::BlockIdExt,
    pub prev2: Option<ton_block::BlockIdExt>,
}
