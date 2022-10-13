use std::collections::hash_map;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Weak};

use anyhow::Result;
use arc_swap::ArcSwapOption;
use rustc_hash::FxHashMap;
use tokio::sync::{oneshot, Notify};
use ton_block::{Deserializable, Serializable};

use crate::node_tcp_rpc::NodeTcpRpc;
use crate::node_udp_rpc::NodeUdpRpc;
use crate::util::{split_address, BlockStuff, FxDashMap};

pub struct Subscription {
    node_tcp_rpc: NodeTcpRpc,
    node_udp_rpc: Arc<NodeUdpRpc>,
    last_mc_block: ArcSwapOption<StoredMcBlock>,
    pending_message_count: AtomicUsize,
    pending_messages_changed: Arc<Notify>,
    mc_pending_messages: PendingMessages,
    sc_pending_messages: PendingMessages,
}

type PendingMessages = FxDashMap<ton_types::UInt256, AccountPendingMessages>;
type AccountPendingMessages = FxHashMap<ton_types::UInt256, PendingMessage>;

impl Subscription {
    pub fn new(node_tcp_rpc: NodeTcpRpc, node_udp_rpc: Arc<NodeUdpRpc>) -> Arc<Self> {
        let subscription = Arc::new(Self {
            node_tcp_rpc,
            node_udp_rpc,
            last_mc_block: Default::default(),
            pending_message_count: Default::default(),
            pending_messages_changed: Default::default(),
            mc_pending_messages: Default::default(),
            sc_pending_messages: Default::default(),
        });

        // TODO: spawn

        subscription
    }

    pub async fn send_message(
        &self,
        message: &ton_block::Message,
        expire_at: u32,
    ) -> Result<ton_block::Transaction> {
        // Prepare dst address
        let dst = match message.ext_in_header() {
            Some(header) => header.dst.clone(),
            None => anyhow::bail!("expected external message"),
        };
        let (workchain, dst) = split_address(&dst)?;

        // Get message hash
        let message_cell = message.serialize()?;
        let message_hash = message_cell.repr_hash();
        let data = ton_types::serialize_toc(&message_cell)?;

        // Find pending messages map
        let pending_messages = match workchain {
            ton_block::MASTERCHAIN_ID => &self.mc_pending_messages,
            ton_block::BASE_WORKCHAIN_ID => &self.sc_pending_messages,
            _ => anyhow::bail!("unsupported workchain"),
        };

        // Insert pending message
        let rx = {
            let mut pending_messages = pending_messages.entry(dst).or_default();

            let rx = match pending_messages.entry(message_hash) {
                hash_map::Entry::Vacant(entry) => {
                    let (tx, rx) = oneshot::channel();
                    entry.insert(PendingMessage { expire_at, tx });
                    rx
                }
                hash_map::Entry::Occupied(_) => anyhow::bail!("message already sent"),
            };

            // Notify waiters while pending messages is still acquired
            self.pending_message_count.fetch_add(1, Ordering::Release);
            self.pending_messages_changed.notify_waiters();

            // Drop the lock
            rx
        };

        // Send the message
        if let Err(e) = self.node_tcp_rpc.send_message(data).await {
            // Remove pending message from the map before returning an error
            match pending_messages.entry(dst) {
                dashmap::mapref::entry::Entry::Occupied(mut entry) => {
                    entry.get_mut().remove(&message_hash);
                    if entry.get().is_empty() {
                        entry.remove();
                    }
                }
                dashmap::mapref::entry::Entry::Vacant(_) => {
                    tracing::warn!("pending messages entry not found");
                }
            };
            return Err(e.into());
        }

        // Wait message execution
        match rx.await? {
            Some(tx) => Ok(tx),
            None => anyhow::bail!("message expired"),
        }
    }

    async fn make_blocks_step(&self) -> Result<bool> {
        // Get last masterchain block
        let last_mc_block = self.get_last_mc_block().await?;

        // Get next masterchain block
        let next_mc_block = self
            .node_udp_rpc
            .get_next_block(last_mc_block.data.id())
            .await?;
        let next_shard_block_ids = next_mc_block.shard_blocks()?;
        let next_mc_utime = {
            let info = next_mc_block.block().read_info()?;
            info.gen_utime().0
        };

        // Get all shard blocks between these masterchain blocks
        let mut tasks = Vec::with_capacity(next_shard_block_ids.len());
        for (_, id) in next_shard_block_ids {
            let last_mc_block = last_mc_block.clone();
            let rpc = self.node_udp_rpc.clone();
            tasks.push(tokio::spawn(async move {
                let edge = &last_mc_block.shards_edge;
                let mut blocks = Vec::new();

                let mut stack = Vec::from([id]);
                while let Some(id) = stack.pop() {
                    let block = rpc.get_block(&id).await?;
                    let info = block.read_brief_info()?;
                    blocks.push((info.gen_utime, block));

                    if edge.is_before(&info.prev1) {
                        stack.push(info.prev1);
                    }
                    if let Some(prev_id2) = info.prev2 {
                        if edge.is_before(&prev_id2) {
                            stack.push(prev_id2);
                        }
                    }
                }

                // Sort blocks by time (to increase processing locality) and seqno
                blocks.sort_unstable_by_key(|(info, block_data)| (*info, block_data.id().seq_no));

                Ok::<_, anyhow::Error>(blocks)
            }));
        }

        // Wait and process all shard blocks
        for task in tasks {
            let blocks = task.await??;
            for (_, item) in blocks {
                process_block(item.block(), &self.sc_pending_messages)?;
            }
        }
        process_block(next_mc_block.block(), &self.mc_pending_messages)?;

        // Remove expired messages
        remove_expired_messages(&self.mc_pending_messages, next_mc_utime);
        remove_expired_messages(&self.sc_pending_messages, next_mc_utime);

        // Done
        Ok(!self.mc_pending_messages.is_empty() || self.sc_pending_messages.is_empty())
    }

    async fn get_last_mc_block(&self) -> Result<Arc<StoredMcBlock>> {
        if let Some(last_mc_block) = self.last_mc_block.load_full() {
            return Ok(last_mc_block);
        }

        let stats = self.node_tcp_rpc.get_stats().await?;
        let last_mc_block = stats.try_into_running()?.last_mc_block;
        let data = self.node_udp_rpc.get_block(&last_mc_block).await?;

        let gen_utime = {
            let info = data.block().read_info()?;
            info.gen_utime().0
        };

        let shards_edge = Edge(data.shard_blocks_seq_no()?);

        let block = Arc::new(StoredMcBlock {
            gen_utime,
            data,
            shards_edge,
        });
        self.last_mc_block.store(Some(block.clone()));
        Ok(block)
    }
}

fn remove_expired_messages(pending_messages: &PendingMessages, utime: u32) {
    pending_messages.retain(|_, pending_messages| {
        pending_messages.retain(|_, message| message.expire_at > utime);
        !pending_messages.is_empty()
    });
}

fn process_block(block: &ton_block::Block, pending_messages: &PendingMessages) -> Result<()> {
    use ton_block::HashmapAugType;

    let extra = block.read_extra()?;
    let account_blocks = extra.read_account_blocks()?;

    account_blocks.iterate_with_keys(|address, account_block| {
        let mut pending_messages = match pending_messages.get_mut(&address) {
            Some(pending_messages) => pending_messages,
            None => return Ok(true),
        };

        account_block
            .transactions()
            .iterate_slices_with_keys(|_, tx| {
                let cell = tx.reference(0)?;
                let repr_hash = cell.repr_hash();
                let tx = ton_block::Transaction::construct_from_cell(cell)?;
                let in_msg_hash = match &tx.in_msg {
                    Some(in_msg) => in_msg.hash(),
                    None => return Ok(true),
                };

                let pending_message = match pending_messages.remove(&in_msg_hash) {
                    Some(pending_message) => pending_message,
                    None => return Ok(true),
                };

                pending_message.tx.send(Some(tx)).ok();
                Ok(true)
            })?;

        Ok(true)
    })?;

    Ok(())
}

struct StoredMcBlock {
    gen_utime: u32,
    data: BlockStuff,
    shards_edge: Edge,
}

struct Edge(FxHashMap<ton_block::ShardIdent, u32>);

impl Edge {
    pub fn is_before(&self, id: &ton_block::BlockIdExt) -> bool {
        match self.0.get(&id.shard_id) {
            Some(&top_seq_no) => top_seq_no < id.seq_no,
            None => self
                .0
                .iter()
                .find(|&(shard, _)| id.shard_id.intersect_with(shard))
                .map(|(_, &top_seq_no)| top_seq_no < id.seq_no)
                .unwrap_or_default(),
        }
    }
}

struct PendingMessage {
    expire_at: u32,
    tx: oneshot::Sender<Option<ton_block::Transaction>>,
}
