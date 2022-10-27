use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use reqwest::Url;
use tokio::process::Command;

use super::ProjectDirs;

macro_rules! validator_service {
    () => {
        r#"[Unit]
Description=Everscale Validator Node
After=network.target
StartLimitIntervalSec=0

[Service]
Type=simple
Restart=always
RestartSec=1
User={user}
LimitNOFILE=2048000
ExecStart={node_binary} --configs {configs_dir}

[Install]
WantedBy=multi-user.target
"#
    };
}

impl ProjectDirs {
    pub fn prepare_binaries_dir(&self) -> Result<()> {
        let binaries_dir = self.binaries_dir();
        if !binaries_dir.exists() {
            std::fs::create_dir_all(binaries_dir).context("failed to create binaries directory")?;
        }
        Ok(())
    }

    pub async fn install_node_from_repo(&self, repo: &Url) -> Result<()> {
        let git_dir = self.git_cache_dir();
        if !git_dir.exists() {
            std::fs::create_dir_all(&git_dir).context("failed to create git cache directory")?;
        }

        let repo_dir = git_dir.join("ton-labs-node");

        clone_repo(repo, &repo_dir).await?;
        let binary = build_node(repo_dir).await?;

        std::fs::copy(binary, self.node_binary()).context("failed to copy node binary")?;
        Ok(())
    }

    pub fn create_systemd_services(&self, user: &str) -> Result<()> {
        let node = std::fs::canonicalize(self.binaries_dir())
            .context("failed to canonicalize node binary path")?;
        let configs_dir = std::fs::canonicalize(self.node_configs_dir())
            .context("failed to canonicalize node configs path")?;

        let service = format!(
            validator_service!(),
            user = user,
            node_binary = node.display(),
            configs_dir = configs_dir.display()
        );

        Ok(())
    }
}

async fn clone_repo<P: AsRef<Path>>(url: &Url, target: P) -> Result<()> {
    let target = target.as_ref();
    if target.exists() {
        std::fs::remove_dir_all(target).context("failed to remove old git directory")?;
    }

    let mut child = Command::new("git")
        .stdout(Stdio::piped())
        .arg("clone")
        .arg("--recursive")
        .arg(url.to_string())
        .arg(target)
        .spawn()?;

    let status = child
        .wait()
        .await
        .context("child process encountered an error")?;

    if !status.success() {
        anyhow::bail!("failed to clone repo (exit code: {})", status);
    }

    Ok(())
}

async fn build_node<P: AsRef<Path>>(target: P) -> Result<PathBuf> {
    let target = target.as_ref();

    let mut child = Command::new("cargo")
        .current_dir(target)
        .stdout(Stdio::piped())
        .arg("build")
        .arg("--release")
        .spawn()?;

    let status = child
        .wait()
        .await
        .context("child process encountered an error")?;

    if !status.success() {
        anyhow::bail!("failed to build node");
    }

    Ok(target.join("target").join("release").join("ton_node"))
}

async fn get_node_version<P: AsRef<Path>>(node: P) -> Result<String> {
    let child = Command::new(node.as_ref())
        .arg("--version")
        .output()
        .await
        .context("failed to run node binary")?;

    if !child.status.success() {
        std::io::stderr().write_all(&child.stdout)?;
        anyhow::bail!("node finished with exit code {}", child.status);
    }

    parse_node_version(&child.stdout)
        .map(String::from)
        .context("invalid node output during version check")
}

fn parse_node_version(output: &[u8]) -> Option<&str> {
    output
        .strip_prefix(b"TON Node, version ")
        .and_then(|output| output.split(|&ch| ch == b'\n').next())
        .and_then(|output| std::str::from_utf8(output).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_version_parser() {
        const STDOUT: &[u8] = b"TON Node, version 0.51.1
Rust: rustc 1.61.0 (fe5b13d68 2022-05-18)
TON NODE git commit:         Not set
ADNL git commit:             Not set
DHT git commit:              Not set
OVERLAY git commit:          Not set
RLDP git commit:             Not set
TON_BLOCK git commit:        Not set
TON_BLOCK_JSON git commit:   Not set
TON_SDK git commit:          Not set
TON_EXECUTOR git commit:     Not set
TON_TL git commit:           Not set
TON_TYPES git commit:        Not set
TON_VM git commit:           Not set
TON_ABI git commit:     Not set

TON node ";

        assert_eq!(parse_node_version(STDOUT), Some("0.51.1"));
    }
}
