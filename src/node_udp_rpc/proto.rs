use tl_proto::{TlError, TlPacket, TlRead, TlResult, TlWrite};

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "tonNode.prepareBlock", scheme = "proto.tl")]
pub struct PrepareBlock<'tl> {
    #[tl(with = "tl_block_id")]
    pub block_id: &'tl ton_block::BlockIdExt,
}

#[derive(Clone, TlWrite)]
#[tl(boxed, id = "tonNode.downloadBlock", scheme = "proto.tl")]
pub struct RpcDownloadBlock<'tl> {
    #[tl(with = "tl_block_id")]
    pub block_id: &'tl ton_block::BlockIdExt,
}

#[derive(Copy, Clone, TlWrite)]
#[tl(boxed, id = "tonNode.downloadNextBlockFull", scheme = "proto.tl")]
pub struct DownloadNextBlockFull<'tl> {
    #[tl(with = "tl_block_id")]
    pub prev_block_id: &'tl ton_block::BlockIdExt,
}

#[derive(Clone, TlRead)]
#[tl(boxed, scheme = "proto.tl")]
pub enum DataFull<'tl> {
    #[tl(id = "tonNode.dataFull")]
    Found {
        #[tl(with = "tl_block_id")]
        block_id: ton_block::BlockIdExt,
        proof: &'tl [u8],
        block: &'tl [u8],
        is_link: bool,
    },
    #[tl(id = "tonNode.dataFullEmpty")]
    Empty,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, TlRead)]
#[tl(boxed, scheme = "proto.tl")]
pub enum Prepared {
    #[tl(id = "tonNode.notFound")]
    NotFound,
    #[tl(id = "tonNode.prepared")]
    Found,
}

mod tl_block_id {
    use super::*;

    pub const SIZE_HINT: usize = 80;

    pub const fn size_hint(_: &ton_block::BlockIdExt) -> usize {
        SIZE_HINT
    }

    pub fn write<P: TlPacket>(block: &ton_block::BlockIdExt, packet: &mut P) {
        packet.write_i32(block.shard_id.workchain_id());
        packet.write_u64(block.shard_id.shard_prefix_with_tag());
        packet.write_u32(block.seq_no);
        packet.write_raw_slice(block.root_hash.as_slice());
        packet.write_raw_slice(block.file_hash.as_slice());
    }

    pub fn read(packet: &[u8], offset: &mut usize) -> TlResult<ton_block::BlockIdExt> {
        let shard_id = ton_block::ShardIdent::with_tagged_prefix(
            i32::read_from(packet, offset)?,
            u64::read_from(packet, offset)?,
        )
        .map_err(|_| TlError::InvalidData)?;
        let seq_no = u32::read_from(packet, offset)?;
        let root_hash = <[u8; 32]>::read_from(packet, offset)?;
        let file_hash = <[u8; 32]>::read_from(packet, offset)?;

        Ok(ton_block::BlockIdExt {
            shard_id,
            seq_no,
            root_hash: root_hash.into(),
            file_hash: file_hash.into(),
        })
    }
}
