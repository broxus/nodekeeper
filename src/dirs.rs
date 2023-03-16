use std::path::{Path, PathBuf};

const ENV: &str = "STEVER_ROOT";

pub const VALIDATOR_SERVICE: &str = "ever-validator";
pub const VALIDATOR_MANAGER_SERVICE: &str = "ever-validator-manager";

pub struct ProjectDirs {
    pub app_config: PathBuf,
    pub node_config: PathBuf,
    pub node_log_config: PathBuf,
    pub global_config: PathBuf,
    pub node_configs_dir: PathBuf,
    pub binaries_dir: PathBuf,
    pub node_binary: PathBuf,
    pub default_node_db_dir: PathBuf,
    pub git_cache_dir: PathBuf,
    pub keys_dir: PathBuf,
    pub validator_keys: PathBuf,
    pub depool_keys: PathBuf,
    pub root: PathBuf,
    pub validator_service: PathBuf,
    pub validator_manager_service: PathBuf,
}

impl ProjectDirs {
    pub fn new<P: AsRef<Path>>(root_dir: P) -> Self {
        let root = root_dir.as_ref().to_path_buf();
        let node_configs_dir = root.join("node");
        let binaries_dir = root.join("bin");
        let git_cache_dir = root.join("git");

        let node_binary = binaries_dir.join("node");

        let systemd_root = PathBuf::from("/etc/systemd/system");
        let validator_service = systemd_root.join(format!("{VALIDATOR_SERVICE}.service"));
        let validator_manager_service =
            systemd_root.join(format!("{VALIDATOR_MANAGER_SERVICE}.service"));

        let keys_dir = root.join("keys");
        let validator_keys = keys_dir.join("vld.keys.json");
        let depool_keys = keys_dir.join("depool.keys.json");

        #[cfg(feature = "packaged")]
        let default_node_db_dir = root.join("db");
        #[cfg(not(feature = "packaged"))]
        let default_node_db_dir = PathBuf::from("/var/ever/rnode");

        Self {
            app_config: root.join("config.toml"),
            node_config: node_configs_dir.join("config.json"),
            node_log_config: node_configs_dir.join("log_cfg.yml"),
            global_config: node_configs_dir.join("global-config.json"),
            node_configs_dir,
            binaries_dir,
            node_binary,
            default_node_db_dir,
            git_cache_dir,
            keys_dir,
            validator_keys,
            depool_keys,
            root,
            validator_service,
            validator_manager_service,
        }
    }

    pub fn default_root_dir() -> PathBuf {
        if let Ok(path) = std::env::var(ENV) {
            PathBuf::from(path)
        } else {
            default_root_dir()
        }
    }
}

#[cfg(feature = "packaged")]
fn default_root_dir() -> PathBuf {
    PathBuf::from("/var/stever")
}

#[cfg(not(feature = "packaged"))]
fn default_root_dir() -> PathBuf {
    use crate::util::system;

    const DEFAULT_ROOT_DIR: &str = ".stever";

    let home_dir = if let Some(uid) = system::get_sudo_uid().unwrap() {
        system::home_dir(uid)
    } else {
        home::home_dir()
    };

    match home_dir {
        Some(home) => home.join(DEFAULT_ROOT_DIR),
        None => {
            panic!("No valid home directory path could be retrieved from the operating system")
        }
    }
}
