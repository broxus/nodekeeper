use std::io::Write;

use super::ExporterTarget;

pub struct StdoutExporterTarget;

impl ExporterTarget for StdoutExporterTarget {
    fn target_name(&self) -> &'static str {
        "stdout_exporter"
    }

    fn write(&self, metrics: &dyn std::fmt::Display) -> anyhow::Result<()> {
        write!(std::io::stdout(), "{metrics}")?;
        Ok(())
    }
}
