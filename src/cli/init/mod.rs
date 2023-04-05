use std::net::Ipv4Addr;
use std::path::PathBuf;

use anyhow::{Context, Result};
use argh::FromArgs;
use broxus_util::{const_bool, serde_optional_string, serde_string_or_number};
use serde::{Deserialize, Serialize};

use super::{CliContext, ProjectDirs};
use crate::config::{AppConfig, AppConfigDePoolDeploymentParams, DePoolType, NodeConfig};
use crate::defaults;
use crate::util::{is_terminal, print_output};

mod contracts;
mod node;
#[cfg(not(feature = "packaged"))]
mod systemd;

#[derive(FromArgs)]
/// Prepares configs and binaries
#[argh(subcommand, name = "init")]
pub struct Cmd {
    #[argh(subcommand)]
    subcommand: Option<SubCmd>,
    /// path to the params template
    #[argh(option)]
    template: Option<PathBuf>,
    /// force download and build the latest node
    #[argh(switch)]
    rebuild: bool,
}

impl Cmd {
    pub async fn run(self, ctx: CliContext) -> Result<()> {
        fn load_template(template: Option<PathBuf>) -> Result<Option<Template>> {
            let Some(path) = &template else { return Ok(None) };

            let data = std::fs::read_to_string(path).context("failed to read template")?;
            let template = if matches!(path.extension(), Some(ext) if ext == "toml") {
                let mut td = toml::Deserializer::new(&data);
                serde_path_to_error::deserialize(&mut td)
                    .context("failed to parse template as TOML")?
            } else {
                let mut jd = serde_json::Deserializer::from_str(&data);
                serde_path_to_error::deserialize(&mut jd)
                    .context("failed to parse template as JSON")?
            };

            Ok(Some(template))
        }

        let theme = &dialoguer::theme::ColorfulTheme::default();
        match self.subcommand {
            None => {
                let template = load_template(self.template)?;

                let node = node::Cmd {
                    rebuild: self.rebuild,
                }
                .run(theme, &ctx, &template)
                .await?;

                let contracts = contracts::Cmd {}.run(theme, &ctx, &template).await?;

                if template.is_some() && !is_terminal() {
                    print_output(serde_json::json!({
                        "node": node,
                        "contracts": contracts,
                    }));
                }

                Ok(())
            }
            Some(SubCmd::Node(cmd)) => {
                let template = load_template(self.template)?;

                let node = cmd.run(theme, &ctx, &template).await?;

                if template.is_some() && !is_terminal() {
                    print_output(serde_json::to_value(node).unwrap());
                }

                Ok(())
            }
            Some(SubCmd::Contracts(cmd)) => {
                let template = load_template(self.template)?;

                let contracts = cmd.run(theme, &ctx, &template).await?;

                if template.is_some() && !is_terminal() {
                    print_output(serde_json::to_value(contracts).unwrap());
                }

                Ok(())
            }
            #[cfg(not(feature = "packaged"))]
            Some(SubCmd::Systemd(cmd)) => {
                anyhow::ensure!(
                    self.template.is_none(),
                    "Template is not supported for `systemd` command"
                );
                cmd.run(theme, &ctx).await
            }
        }
    }
}

#[derive(FromArgs)]
#[argh(subcommand)]
enum SubCmd {
    Node(node::Cmd),
    Contracts(contracts::Cmd),
    #[cfg(not(feature = "packaged"))]
    Systemd(systemd::Cmd),
}

impl ProjectDirs {
    fn store_app_config(&self, app_config: &AppConfig) -> Result<()> {
        app_config.store(&self.app_config)
    }

    fn store_node_config(&self, node_config: &NodeConfig) -> Result<()> {
        node_config.store(&self.node_config)
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Template {
    /// General settings
    general: TemplateGeneral,

    /// Control server settings.
    #[serde(default)]
    control: TemplateControl,

    /// ADNL settings.
    #[serde(default)]
    adnl: TemplateAdnl,

    /// Optional validation params.
    #[serde(default)]
    validator: Option<TemplateValidator>,
}

fn default_global_config() -> Option<String> {
    Some("ever_mainnet".to_owned())
}

#[derive(Deserialize, Serialize)]
struct TemplateGeneral {
    /// Whether to create a root directory. Default: `true`.
    #[serde(default = "const_bool::<true>")]
    create_root_dir: bool,

    /// Path, url or name of the global config. Default: `ever_mainnet`.
    ///
    /// NOTE: Tries to use the existing one if None.
    #[serde(default = "default_global_config")]
    global_config: Option<String>,

    /// Whether to reset node logger settings. Default: `false`.
    #[serde(default)]
    reset_logger_config: bool,

    /// Whether to overwrite the existing node config. Default: `false`.
    #[serde(default)]
    reset_node_config: bool,

    /// Whether to reset the existing app config. Default: `false`.
    #[serde(default)]
    reset_app_config: bool,

    /// Path to the root directory for the node DB.
    node_db_path: PathBuf,

    /// Node repository info.
    #[serde(default)]
    node_repo: TemplateNodeRepo,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields, rename_all = "lowercase", tag = "type")]
enum TemplateValidator {
    Single(TemplateValidatorSingle),
    DePool(Box<TemplateValidatorDePool>),
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TemplateValidatorSingle {
    /// Whether to overwrite existing validation config. Default: `false`.
    #[serde(default)]
    overwrite: bool,

    /// Whether to overwrite existing validator keys. Default: `false`.
    #[serde(default)]
    overwrite_validator_keys: bool,

    /// Stake per round in nano EVERs.
    #[serde(with = "serde_string_or_number")]
    stake_per_round: u64,

    /// Optional stake factor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stake_factor: Option<u32>,
}

#[derive(Deserialize, Serialize)]
struct TemplateValidatorDePool {
    /// Whether to overwrite existing validation config. Default: `false`.
    #[serde(default)]
    overwrite: bool,

    /// Whether to overwrite existing validator keys. Default: `false`.
    #[serde(default)]
    overwrite_validator_keys: bool,

    /// Whether to overwrite existing DePool keys. Default: `false`.
    #[serde(default)]
    overwrite_depool_keys: bool,

    /// DePool type.
    depool_type: DePoolType,

    /// Optional stake factor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stake_factor: Option<u32>,

    /// stEVER cluster address.
    #[serde(
        default,
        with = "serde_optional_string",
        skip_serializing_if = "Option::is_none"
    )]
    cluster: Option<ton_block::MsgAddressInt>,

    /// DePool deployment params.
    #[serde(flatten)]
    deploy: AppConfigDePoolDeploymentParams,
}

#[derive(Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct TemplateControl {
    /// Control server TCP port. Default: `None`.
    ///
    /// NOTE: Tries to use the default one or an existing from the config.
    port: Option<u16>,

    /// What to do with new node keys: `append` or `replace`. Default: `replace`.
    node_key_behavior: TemplateNodeKeyBehavior,

    /// On which address control server will be listening for requests.
    /// Default: `127.0.0.1`.
    listen_addr: Ipv4Addr,
}

impl Default for TemplateControl {
    fn default() -> Self {
        Self {
            port: None,
            node_key_behavior: TemplateNodeKeyBehavior::default(),
            listen_addr: Ipv4Addr::LOCALHOST,
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TemplateAdnl {
    /// ADNL UDP port. Default: `None`.
    ///
    /// NOTE: Tries to use the default one or an existing from the config.
    #[serde(default)]
    port: Option<u16>,

    /// Public IP. Default: `None`.
    ///
    /// NOTE: Tries to resolve public ip if not specified.
    #[serde(default)]
    public_ip: Option<Ipv4Addr>,
}

struct TemplateNodeRepo {
    /// Node repository URL.
    url: reqwest::Url,
    /// Optional branch.
    branch: Option<String>,
    /// Features which will be used during node build.
    features: Vec<String>,
}

impl Default for TemplateNodeRepo {
    fn default() -> Self {
        Self {
            url: defaults::DEFAULT_NODE_REPO.parse().unwrap(),
            branch: None,
            features: Vec::new(),
        }
    }
}

impl Serialize for TemplateNodeRepo {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        #[derive(Serialize)]
        struct Helper<'a> {
            url: &'a reqwest::Url,
            branch: &'a Option<String>,
            features: &'a Vec<String>,
        }

        if self.branch.is_none() && self.features.is_empty() {
            self.url.serialize(serializer)
        } else {
            Helper {
                url: &self.url,
                branch: &self.branch,
                features: &self.features,
            }
            .serialize(serializer)
        }
    }
}

impl<'de> Deserialize<'de> for TemplateNodeRepo {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged, deny_unknown_fields)]
        enum Helper {
            Url(reqwest::Url),
            Full {
                url: reqwest::Url,
                branch: Option<String>,
                features: Vec<String>,
            },
        }

        Ok(match Helper::deserialize(deserializer)? {
            Helper::Url(git) => Self {
                url: git,
                branch: None,
                features: Vec::new(),
            },
            Helper::Full {
                url,
                branch,
                features,
            } => Self {
                url,
                branch,
                features,
            },
        })
    }
}

#[derive(Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
enum TemplateNodeKeyBehavior {
    Append,
    #[default]
    Replace,
}

impl TemplateNodeKeyBehavior {
    pub fn is_append(&self) -> bool {
        matches!(self, Self::Append)
    }
}
