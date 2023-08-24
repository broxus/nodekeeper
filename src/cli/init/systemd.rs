use std::borrow::Cow;
use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use argh::FromArgs;
use dialoguer::console::style;
use dialoguer::theme::Theme;
use dialoguer::Select;
use tokio::process::Command;

use crate::cli::{CliContext, ProjectDirs};
use crate::dirs::{VALIDATOR_EXPORTER_SERVICE, VALIDATOR_MANAGER_SERVICE, VALIDATOR_SERVICE};
use crate::util::*;

#[derive(FromArgs)]
/// Creates systemd services
#[argh(subcommand, name = "systemd")]
pub struct Cmd {
    /// which user to use for systemd services.
    #[argh(option)]
    user: Option<String>,
    /// whether to enable services for auto-start.
    #[argh(switch)]
    enable: Option<bool>,
    /// whether to immediately start services.
    #[argh(switch)]
    start: Option<bool>,
}

impl Cmd {
    pub async fn run(self, theme: &dyn Theme, ctx: &CliContext) -> Result<()> {
        if self.user.is_none() && !is_terminal() {
            anyhow::bail!("`user` param is required when running without tty");
        }

        let dirs = ctx.dirs();
        let mut steps = Steps::new(2);

        // Ensure all services are created
        steps.next("Preparing services");
        prepare_services(theme, dirs, &self.user)?;

        // Reload sysetmd
        steps.next("Reloading systemd configs");
        systemd_daemon_reload().await?;

        // Optionally start services
        steps.next("Systemd services are configured now. Great!");
        start_services(theme, self.enable, self.start).await?;

        Ok(())
    }
}

pub fn prepare_services(
    theme: &dyn Theme,
    dirs: &ProjectDirs,
    user: &Option<String>,
) -> Result<()> {
    const ROOT_USER: &str = "root";

    let user = match user {
        Some(user) => Cow::Borrowed(user.as_str()),
        None => {
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

            if let Some(uid) = other_user {
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
            }
        }
    };

    let print_service = |path: &Path| {
        eprintln!(
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

    // Create validator exporter service
    dirs.create_systemd_validator_exporter_service(&user)?;
    print_service(&dirs.validator_exporter_service);

    Ok(())
}

pub async fn start_services(
    theme: &dyn Theme,
    enable: Option<bool>,
    start: Option<bool>,
) -> Result<()> {
    let services = [
        VALIDATOR_SERVICE,
        VALIDATOR_MANAGER_SERVICE,
        VALIDATOR_EXPORTER_SERVICE,
    ];

    let enabled = match enable {
        Some(enable) => enable,
        None => confirm(theme, true, "Enable autostart services at system startup?")?,
    };
    systemd_set_services_enabled(services, enabled).await?;

    let start = match start {
        Some(start) => start,
        None => confirm(theme, true, "Restart systemd services?")?,
    };
    if start {
        for service in services {
            systemd_restart_service(service).await?;
        }
    }

    Ok(())
}

macro_rules! validator_service {
    () => {
        r#"[Unit]
Description=Validator Node
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
Description=Validator Manager
After=network.target
StartLimitIntervalSec=0

[Service]
Type=simple
Restart=always
RestartSec=1
User={user}
ExecStart={nodekeeper_binary} --root {root_dir} validator run

[Install]
WantedBy=multi-user.target
"#
    };
}

macro_rules! validator_exporter_service {
    () => {
        r#"[Unit]
Description=Validator Metrics Exporter
After=network.target
StartLimitIntervalSec=0

[Service]
Type=simple
Restart=always
RestartSec=1
User={user}
Environment=PORT=10000
Environment=INTERVAL=10
ExecStart={nodekeeper_binary} exporter \
    --addr 0.0.0.0:${{PORT}} \
    --interval ${{INTERVAL}}

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
            nodekeeper_binary = current_exe.display(),
            root_dir = root_dir.display(),
        );
        std::fs::write(&self.validator_manager_service, validator_manager_service)
            .context("failed to create systemd validator manager service")?;

        Ok(())
    }

    fn create_systemd_validator_exporter_service(&self, user: &str) -> Result<()> {
        let current_exe = std::env::current_exe()?;

        let validator_exporter_service = format!(
            validator_exporter_service!(),
            user = user,
            nodekeeper_binary = current_exe.display(),
        );
        std::fs::write(&self.validator_exporter_service, validator_exporter_service)
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

async fn systemd_set_services_enabled<'a, I: IntoIterator<Item = &'a str>>(
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
