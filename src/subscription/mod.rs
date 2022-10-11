use std::net::SocketAddrV4;
use std::sync::Arc;

use anyhow::{Context, Result};
use everscale_network::utils::PackedSocketAddr;
use everscale_network::{adnl, dht, overlay, rldp, NetworkBuilder};
use futures_util::StreamExt;
use rand::Rng;

use crate::global_config::GlobalConfig;
use crate::node_rpc::{NodeRpc, NodeStats};

mod proto;

pub struct BlockSubscription {
    node_rpc: NodeRpc,
    local_id: adnl::NodeIdShort,
    peer_id: adnl::NodeIdShort,
    query_prefix: Vec<u8>,
    adnl: Arc<adnl::Node>,
    dht: Arc<dht::Node>,
    rldp: Arc<rldp::Node>,
}

impl BlockSubscription {
    pub async fn new(node_rpc: NodeRpc, global_config: GlobalConfig) -> Result<Self> {
        let stats = match node_rpc.get_stats().await? {
            NodeStats::Running(stats) => stats,
            NodeStats::NotReady => anyhow::bail!("node is not ready"),
        };

        let ip_addr = public_ip::addr_v4()
            .await
            .context("failed to resolve public ip")?;

        let keystore = adnl::Keystore::builder()
            .with_tagged_key(rand::thread_rng().gen(), 0)?
            .build();

        let (adnl, dht, rldp) = NetworkBuilder::with_adnl(
            SocketAddrV4::new(ip_addr, 30001),
            keystore,
            Default::default(),
        )
        .with_dht(0, Default::default())
        .with_rldp(Default::default())
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

        let peer_id = adnl::NodeIdShort::new(stats.overlay_adnl_id);
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
            node_rpc,
            local_id,
            peer_id,
            query_prefix,
            adnl,
            rldp,
            dht,
        })
    }

    pub async fn get_capabilities(&self) -> Result<proto::Capabilities> {
        self.adnl
            .query_with_prefix(
                &self.local_id,
                &self.peer_id,
                &self.query_prefix,
                proto::GetCapabilities,
                None,
            )
            .await?
            .context("query timeout")
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
