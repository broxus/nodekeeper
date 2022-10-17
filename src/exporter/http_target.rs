use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;

use super::ExporterTarget;

pub struct HttpExporterTarget {
    state: Arc<parking_lot::RwLock<Option<String>>>,
    _exporter: Arc<pomfrit::MetricsExporter>,
}

impl HttpExporterTarget {
    pub async fn new(addr: SocketAddr) -> Result<Self> {
        let (exporter, writer) = pomfrit::create_exporter(Some(pomfrit::Config {
            collection_interval_sec: 1,
            listen_address: addr,
            metrics_path: None,
        }))
        .await?;

        let state = Arc::new(parking_lot::RwLock::default());
        writer.spawn({
            let state = state.clone();
            move |writer| {
                let metrics = state.read();
                if let Some(metrics) = &*metrics {
                    writer.write_str(metrics);
                }
            }
        });

        Ok(Self {
            state,
            _exporter: exporter,
        })
    }
}

impl ExporterTarget for HttpExporterTarget {
    fn target_name(&self) -> &'static str {
        "http_exporter"
    }

    fn write(&self, metrics: &dyn std::fmt::Display) -> Result<()> {
        let mut state = self.state.write();
        *state = Some(metrics.to_string());
        Ok(())
    }
}
