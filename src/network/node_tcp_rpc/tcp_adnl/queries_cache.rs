use std::sync::{Arc, Weak};

use tokio::sync::oneshot;

use crate::util::FxDashMap;

#[derive(Default)]
pub struct QueriesCache {
    queries: FxDashMap<[u8; 32], DataTx>,
}

impl QueriesCache {
    pub fn add_query(self: &Arc<Self>, query_id: [u8; 32]) -> PendingAdnlQuery {
        let (tx, rx) = oneshot::channel();

        self.queries.insert(query_id, tx);

        PendingAdnlQuery {
            query_id,
            data_rx: Some(rx),
            cache: Arc::downgrade(self),
            finished: false,
        }
    }

    pub fn update_query(&self, query_id: &[u8; 32], answer: &[u8]) {
        if let Some((_, tx)) = self.queries.remove(query_id) {
            tx.send(answer.to_vec()).ok();
        }
    }
}

pub struct PendingAdnlQuery {
    query_id: [u8; 32],
    data_rx: Option<DataRx>,
    cache: Weak<QueriesCache>,
    finished: bool,
}

impl PendingAdnlQuery {
    pub async fn wait(mut self) -> Option<Vec<u8>> {
        // SAFETY: `data_rx` is guaranteed to be `Some`
        let data_rx = unsafe { self.data_rx.take().unwrap_unchecked() };
        let data = data_rx.await.ok();
        self.finished = true;
        data
    }
}

impl Drop for PendingAdnlQuery {
    fn drop(&mut self) {
        if self.finished {
            return;
        }

        if let Some(cache) = self.cache.upgrade() {
            cache.queries.remove(&self.query_id);
        }
    }
}

type DataTx = oneshot::Sender<Vec<u8>>;
type DataRx = oneshot::Receiver<Vec<u8>>;
