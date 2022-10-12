use std::net::SocketAddrV4;
use std::sync::Arc;

use anyhow::{Context, Result};
use everscale_network::utils::PackedSocketAddr;
use everscale_network::{adnl, dht, overlay, rldp, NetworkBuilder};
use futures_util::StreamExt;
use parking_lot::Mutex;
use rand::Rng;
use tl_proto::{TlRead, TlWrite};
use ton_block::Deserializable;

use crate::global_config::GlobalConfig;
use crate::node_rpc::{NodeRpc, NodeStats};

mod proto;

pub struct BlockSubscription {
    local_id: adnl::NodeIdShort,
    peer_id: adnl::NodeIdShort,
    query_prefix: Vec<u8>,
    adnl: Arc<adnl::Node>,
    rldp: Arc<rldp::Node>,
    roundtrip: Mutex<u64>,
}

impl BlockSubscription {
    pub async fn new(global_config: GlobalConfig, peer_id: adnl::NodeIdShort) -> Result<Self> {
        let ip_addr = public_ip::addr_v4()
            .await
            .context("failed to resolve public ip")?;

        let keystore = adnl::Keystore::builder()
            .with_tagged_key(rand::thread_rng().gen(), 0)?
            .build();

        let rldp_options = rldp::NodeOptions {
            force_compression: true,
            ..Default::default()
        };

        let (adnl, dht, rldp) = NetworkBuilder::with_adnl(
            SocketAddrV4::new(ip_addr, 30001),
            keystore,
            Default::default(),
        )
        .with_dht(0, Default::default())
        .with_rldp(rldp_options)
        .build()?;

        let mut static_nodes = Vec::new();
        for peer in &global_config.dht_nodes {
            if let Some(peer_id) = dht.add_dht_peer(peer.clone())? {
                static_nodes.push(peer_id);
            }
        }

        adnl.start()?;

        let mut dht_node_count = static_nodes.len();
        dht_node_count += search_dht_nodes(dht.as_ref(), &static_nodes).await?;
        tracing::info!("total static nodes: {dht_node_count}");

        let overlay_id_full =
            overlay::IdFull::for_shard_overlay(-1, global_config.zero_state.file_hash.as_slice());
        let overlay_id = overlay_id_full.compute_short_id();

        let query_prefix = tl_proto::serialize(everscale_network::proto::rpc::OverlayQuery {
            overlay: overlay_id.as_slice(),
        });

        let (peer_ip_address, peer_full_id) = resolve_ip(&dht, &peer_id).await?;

        let local_id = *adnl.key_by_tag(0)?.id();
        adnl.add_peer(
            adnl::NewPeerContext::Dht,
            &local_id,
            &peer_id,
            peer_ip_address,
            peer_full_id,
        )?;

        Ok(Self {
            local_id,
            peer_id,
            query_prefix,
            adnl,
            rldp,
            roundtrip: Default::default(),
        })
    }

    pub async fn get_capabilities(&self) -> Result<proto::Capabilities> {
        self.adnl_query(proto::GetCapabilities, 1000).await
    }

    pub async fn get_block(
        &self,
        block_id: &ton_block::BlockIdExt,
    ) -> Result<Option<ton_block::Block>> {
        // Prepare
        let prepare: proto::Prepared = self
            .adnl_query(proto::PrepareBlock { block_id }, 1000)
            .await?;

        println!("PREPARED: {prepare:?}");

        Ok(None)
    }

    pub async fn get_next_block(
        &self,
        prev_block_id: &ton_block::BlockIdExt,
        attempt: u64,
    ) -> Result<Option<ton_block::Block>> {
        let data = self
            .rldp_query(proto::DownloadNextBlockFull { prev_block_id }, attempt)
            .await?;

        match tl_proto::deserialize::<proto::DataFull>(&data)? {
            proto::DataFull::Found {
                block: block_data, ..
            } => Ok(Some(ton_block::Block::construct_from_bytes(block_data)?)),
            proto::DataFull::Empty => Ok(None),
        }
    }

    async fn adnl_query<Q, R>(&self, query: Q, timeout: u64) -> Result<R>
    where
        Q: TlWrite,
        for<'a> R: TlRead<'a, Repr = tl_proto::Boxed> + 'static,
    {
        self.adnl
            .query_with_prefix(
                &self.local_id,
                &self.peer_id,
                &self.query_prefix,
                query,
                Some(timeout),
            )
            .await?
            .context("timeout")
    }

    async fn rldp_query<Q>(&self, query: Q, attempt: u64) -> Result<Vec<u8>>
    where
        Q: TlWrite,
    {
        const ATTEMPT_INTERVAL: u64 = 50; // milliseconds

        let prefix = &self.query_prefix;
        let mut query_data = Vec::with_capacity(prefix.len() + query.max_size_hint());
        query_data.extend_from_slice(prefix);
        query.write_to(&mut query_data);

        let roundtrip = {
            let roundtrip = *self.roundtrip.lock();
            if roundtrip > 0 {
                Some(roundtrip + attempt * ATTEMPT_INTERVAL)
            } else {
                None
            }
        };

        let (answer, roundtrip) = self
            .rldp
            .query(&self.local_id, &self.peer_id, query_data, roundtrip)
            .await?;

        let answer = answer.context("timeout")?;

        let mut current_roundtrip = self.roundtrip.lock();
        if *current_roundtrip > 0 {
            *current_roundtrip = (*current_roundtrip + roundtrip) / 2;
        } else {
            *current_roundtrip = roundtrip;
        }

        Ok(answer)
    }
}

async fn resolve_ip(
    dht: &Arc<dht::Node>,
    peer_id: &adnl::NodeIdShort,
) -> Result<(PackedSocketAddr, adnl::NodeIdFull)> {
    let mut attempt = 0;
    loop {
        attempt += 1;
        match dht.find_address(peer_id).await {
            Ok(res) => break Ok(res),
            Err(e) if attempt > 2 => break Err(e),
            Err(e) => {
                tracing::warn!("failed to resolve peer IP: {e}");
            }
        }
    }
}

async fn search_dht_nodes(dht: &dht::Node, static_nodes: &[adnl::NodeIdShort]) -> Result<usize> {
    let mut tasks = futures_util::stream::FuturesUnordered::new();
    for peer_id in static_nodes {
        tasks.push(async move {
            let res = dht.query_dht_nodes(peer_id, 10, false).await;
            (peer_id, res)
        });
    }

    let mut node_count = 0;
    while let Some((peer_id, res)) = tasks.next().await {
        match res {
            Ok(nodes) => {
                for node in nodes {
                    node_count += dht.add_dht_peer(node)?.is_some() as usize;
                }
            }
            Err(e) => tracing::warn!("failed to get DHT nodes from {peer_id}: {e:?}"),
        }
    }

    Ok(node_count)
}
