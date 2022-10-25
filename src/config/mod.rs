use std::path::PathBuf;

use home::home_dir;
use once_cell::race::OnceBox;

pub use self::app_config::{AppConfig, AppConfigAdnl, AppConfigControl};
pub use self::global_config::GlobalConfig;
pub use self::node_config::{NodeConfig, NodeConfigAdnl, NodeConfigControlServer};

mod app_config;
mod global_config;
mod node_config;

pub fn default_config_path() -> PathBuf {
    // Search local config first
    for local in ["./config.json", "./config.toml"] {
        let path = PathBuf::from(local);
        if path.exists() {
            return path;
        }
    }

    // Use default folder
    default_root_dir().join("config.toml")
}

pub fn default_root_dir() -> &'static PathBuf {
    const DEFAULT_ROOT_DIR: &str = ".stever";

    static DIRS: OnceBox<PathBuf> = OnceBox::new();
    DIRS.get_or_init(|| match home_dir() {
        Some(dirs) => Box::new(dirs.join(DEFAULT_ROOT_DIR)),
        None => {
            panic!("No valid home directory path could be retrieved from the operating system")
        }
    })
}
