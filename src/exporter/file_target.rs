use std::io::Write;
use std::os::unix::prelude::OpenOptionsExt;
use std::path::PathBuf;

use anyhow::Result;

use super::ExporterTarget;

pub struct FileExporterTarget {
    file_path: PathBuf,
    temp_file_path: PathBuf,
}

impl FileExporterTarget {
    pub fn new(path: PathBuf) -> Self {
        let mut temp_extension = path.extension().unwrap_or_default().to_os_string();
        temp_extension.push(std::ffi::OsString::from("temp"));

        let mut temp_file_path = path.clone();
        temp_file_path.set_extension(temp_extension);

        Self {
            file_path: path,
            temp_file_path,
        }
    }
}

impl ExporterTarget for FileExporterTarget {
    fn target_name(&self) -> &'static str {
        "file_exporter"
    }

    fn write(&self, metrics: &dyn std::fmt::Display) -> Result<()> {
        let mut temp_file = std::fs::OpenOptions::new()
            .write(true)
            .truncate(true)
            .create(true)
            .mode(0o644)
            .open(&self.temp_file_path)?;

        write!(temp_file, "{metrics}")?;
        drop(temp_file);

        std::fs::rename(&self.temp_file_path, &self.file_path)?;
        Ok(())
    }
}
