use std::net::SocketAddrV4;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use everscale_network::utils::PackedSocketAddr;
use everscale_network::{adnl, dht, overlay, rldp, NetworkBuilder};
use parking_lot::Mutex;
use rand::Rng;
use tl_proto::{TlRead, TlWrite};

use crate::global_config::GlobalConfig;
use crate::util::BlockStuff;

mod proto;

#[derive(Clone)]
pub struct NodeUdpRpc {
    inner: Arc<NodeInner>,
}

impl NodeUdpRpc {
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

        for peer in global_config.dht_nodes {
            dht.add_dht_peer(peer.clone())?;
        }

        adnl.start()?;

        let dht_node_count = dht.find_more_dht_nodes().await?;
        tracing::info!("total DHT nodes: {dht_node_count}");

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
            inner: Arc::new(NodeInner {
                local_id,
                peer_id,
                query_prefix,
                adnl,
                rldp,
                roundtrip: Default::default(),
            }),
        })
    }

    pub async fn get_next_block(
        &self,
        prev_block_id: &ton_block::BlockIdExt,
    ) -> Result<BlockStuff> {
        let mut timeouts = BLOCK_TIMEOUTS;

        let mut attempt = 0;
        loop {
            let data = self
                .inner
                .rldp_query(proto::DownloadNextBlockFull { prev_block_id }, attempt)
                .await
                .context("rldp query failed")?;

            match data.as_deref().map(tl_proto::deserialize) {
                // Received valid block
                Some(Ok(proto::DataFull::Found {
                    block_id, block, ..
                })) => break BlockStuff::new(block, block_id),
                // Received invalid response
                Some(Err(e)) => break Err(e.into()),
                // Received empty response or nothing (due to timeout)
                Some(Ok(proto::DataFull::Empty)) | None => {
                    tracing::debug!("next block not found");
                    timeouts.sleep_and_update().await;
                    attempt += 1;
                }
            }
        }
    }

    pub async fn get_block(&self, block_id: &ton_block::BlockIdExt) -> Result<BlockStuff> {
        let mut timeouts = BLOCK_TIMEOUTS;
        loop {
            match self
                .inner
                .adnl_query(proto::PrepareBlock { block_id }, 1000)
                .await?
            {
                proto::Prepared::Found => break,
                proto::Prepared::NotFound => {
                    tracing::debug!("block not found");
                    timeouts.sleep_and_update().await;
                }
            }
        }

        timeouts = BLOCK_TIMEOUTS;
        let mut attempt = 0;
        loop {
            let data = self
                .inner
                .rldp_query(proto::RpcDownloadBlock { block_id }, attempt)
                .await?;

            match data {
                Some(block) => break BlockStuff::new(&block, block_id.clone()),
                None => {
                    tracing::debug!("block receiver timeout");
                    timeouts.sleep_and_update().await;
                    attempt += 1;
                }
            }
        }
    }
}

struct NodeInner {
    local_id: adnl::NodeIdShort,
    peer_id: adnl::NodeIdShort,
    query_prefix: Vec<u8>,
    adnl: Arc<adnl::Node>,
    rldp: Arc<rldp::Node>,
    roundtrip: Mutex<u64>,
}

impl NodeInner {
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

    async fn rldp_query<Q>(&self, query: Q, attempt: u64) -> Result<Option<Vec<u8>>>
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

        if answer.is_some() {
            let mut current_roundtrip = self.roundtrip.lock();
            if *current_roundtrip > 0 {
                *current_roundtrip = (*current_roundtrip + roundtrip) / 2;
            } else {
                *current_roundtrip = roundtrip;
            }
        }

        Ok(answer)
    }
}

const BLOCK_TIMEOUTS: DownloaderTimeouts = DownloaderTimeouts {
    initial: 200,
    max: 1000,
    multiplier: 1.2,
};

#[derive(Debug, Copy, Clone)]
pub struct DownloaderTimeouts {
    /// Milliseconds
    pub initial: u64,
    /// Milliseconds
    pub max: u64,

    pub multiplier: f64,
}

impl DownloaderTimeouts {
    async fn sleep_and_update(&mut self) {
        tokio::time::sleep(Duration::from_millis(self.initial)).await;
        self.update();
    }

    fn update(&mut self) -> u64 {
        self.initial = std::cmp::min(self.max, (self.initial as f64 * self.multiplier) as u64);
        self.initial
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
