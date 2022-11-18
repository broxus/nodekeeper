use std::borrow::Cow;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use argh::FromArgs;
use console::style;
use dialoguer::theme::Theme;
use dialoguer::Select;
use tokio::process::Command;

use crate::cli::{CliContext, ProjectDirs, VALIDATOR_MANAGER_SERVICE, VALIDATOR_SERVICE};
use crate::util::*;

#[derive(FromArgs)]
/// Creates systemd services (`ever-validator` and `ever-validator-manager`)
#[argh(subcommand, name = "systemd")]
pub struct Cmd {}

impl Cmd {
    pub async fn run(self, theme: &dyn Theme, ctx: CliContext) -> Result<()> {
        let dirs = ctx.dirs();
        let mut steps = Steps::new(2);

        // Ensure all services are created
        steps.next("Preparing services");
        prepare_services(theme, dirs)?;

        // Reload sysetmd
        steps.next("Reloading systemd configs");
        systemd_daemon_reload().await?;

        // Optionally start services
        steps.next("Systemd services are configured now. Great!");
        start_services(theme).await?;

        Ok(())
    }
}

pub fn prepare_services(theme: &dyn Theme, dirs: &ProjectDirs) -> Result<()> {
    const ROOT_USER: &str = "root";

    // Determine current user id
    let uid = system::user_id();
    // Determine "real" user id (if he runs this app under sudo)
    let other_user = match uid {
        // If current user is root
        0 => match system::get_sudo_uid()? {
            // Root user is running this app under sudo
            Some(0) => None,
            // All other cases (no sudo or real user id)
            uid => uid,
        },
        // Current user is not root
        uid => Some(uid),
    };

    let user = if let Some(uid) = other_user {
        // If there is an option of running services under non-root user,
        // ask user about it
        let other_user = system::user_name(uid).context("failed to get user name")?;
        match Select::with_theme(theme)
            .with_prompt("Select the user from which the service will work")
            .item(&other_user)
            .item("root")
            .default(0)
            .interact()?
        {
            // Running as non-root user
            0 => Cow::Owned(other_user),
            // Running as root
            _ => Cow::Borrowed(ROOT_USER),
        }
    } else {
        // No options available
        system::user_name(uid)
            .map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(ROOT_USER))
    };

    let print_service = |path: &Path| {
        println!(
            "{}",
            style(format!("Created validator service at {}", path.display())).dim()
        );
    };

    // Create validator node service
    dirs.create_systemd_validator_service(&user)?;
    print_service(&dirs.validator_service);

    // Create validator manager service
    dirs.create_systemd_validator_manager_service(&user)?;
    print_service(&dirs.validator_manager_service);

    Ok(())
}

pub async fn start_services(theme: &dyn Theme) -> Result<()> {
    let services = [VALIDATOR_SERVICE, VALIDATOR_MANAGER_SERVICE];
    systemd_set_sercices_enabled(
        services,
        confirm(theme, true, "Enable autostart services at system startup?")?,
    )
    .await?;

    if confirm(theme, true, "Restart systemd services?")? {
        for service in services {
            systemd_restart_service(service).await?;
        }
    }

    Ok(())
}

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

macro_rules! validator_manager_service {
    () => {
        r#"[Unit]
Description=Everscale Validator Manager
After=network.target
StartLimitIntervalSec=0

[Service]
Type=simple
Restart=always
RestartSec=1
User={user}
ExecStart={stever_binary} --root {root_dir} validate

[Install]
WantedBy=multi-user.target
"#
    };
}

impl ProjectDirs {
    fn create_systemd_validator_service(&self, user: &str) -> Result<()> {
        let node = std::fs::canonicalize(&self.node_binary)
            .context("failed to canonicalize node binary path")?;
        let node_configs_dir = std::fs::canonicalize(&self.node_configs_dir)
            .context("failed to canonicalize node configs path")?;

        let validator_service = format!(
            validator_service!(),
            user = user,
            node_binary = node.display(),
            configs_dir = node_configs_dir.display()
        );
        std::fs::write(&self.validator_service, validator_service)
            .context("failed to create systemd validator service")?;

        Ok(())
    }

    fn create_systemd_validator_manager_service(&self, user: &str) -> Result<()> {
        let current_exe = std::env::current_exe()?;
        let root_dir = std::fs::canonicalize(&self.root)
            .context("failed to canonicalize root directory path")?;

        let validator_manager_service = format!(
            validator_manager_service!(),
            user = user,
            stever_binary = current_exe.display(),
            root_dir = root_dir.display(),
        );
        std::fs::write(&self.validator_manager_service, validator_manager_service)
            .context("failed to create systemd validator manager service")?;

        Ok(())
    }
}

async fn systemd_restart_service(service: &str) -> Result<()> {
    exec(
        Command::new("systemctl")
            .stdout(Stdio::piped())
            .arg("restart")
            .arg(service),
    )
    .await
    .with_context(|| format!("failed to restart service {service}"))
}

async fn systemd_set_sercices_enabled<'a, I: IntoIterator<Item = &'a str>>(
    services: I,
    enabled: bool,
) -> Result<()> {
    let mut command = Command::new("systemctl");
    command
        .stdout(Stdio::piped())
        .arg(if enabled { "enable" } else { "disable" });

    for service in services {
        command.arg(service);
    }

    exec(&mut command)
        .await
        .context("failed to enable services")
}

pub async fn systemd_daemon_reload() -> Result<()> {
    exec(
        Command::new("systemctl")
            .stdout(Stdio::piped())
            .arg("daemon-reload"),
    )
    .await
    .context("failed to reload systemd configs")
}
