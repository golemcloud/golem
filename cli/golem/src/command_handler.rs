// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use anyhow::{Context as _, anyhow, bail};
use clap_verbosity_flag::Verbosity;
use golem_cli::command::server::{RunArgs, ServerSubcommand};
use golem_cli::command_handler::{CommandHandlerHooks, Handlers};
use golem_cli::config::{DEFAULT_LOCAL_CUSTOM_REQUEST_PORT, DEFAULT_LOCAL_MCP_PORT};
use golem_cli::context::Context;
use golem_cli::error::NonSuccessfulExit;
use golem_cli::fs;
use golem_cli::log::{LogColorize, log_warn_action};
use golem_cli::model::app::ResolvedLocalServer;
use golem_common::model::account_usage::MonthlyPlanAmounts;
use golem_worker_executor::services::golem_config::ResourceUsageMeteringConfig;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::debug;

use crate::compat::map_local_server_startup_error;
use crate::launch::{LaunchArgs, StartupPorts, launch_golem_services};

pub struct ServerCommandHandler;

impl CommandHandlerHooks for ServerCommandHandler {
    async fn handler_server_commands(
        &self,
        ctx: Arc<Context>,
        subcommand: ServerSubcommand,
    ) -> anyhow::Result<()> {
        match subcommand {
            ServerSubcommand::Run { args } => {
                let args = args.with_env_overrides()?;
                let launch_args = launch_args_from_run_args_and_manifest(&args, &ctx)?;

                if !ctx.server_no_limit_change() {
                    let file_limit_increase_result = rlimit::increase_nofile_limit(1000000);
                    debug!(
                        "File limit increase result: {:?}",
                        file_limit_increase_result
                    );
                }

                let data_dir = launch_args.data_dir.clone();
                if args.clean && tokio::fs::metadata(&data_dir).await.is_ok() {
                    clean_data_dir(&ctx, &data_dir).await?;
                };

                let (mut join_set, startup_ports) = launch_golem_services(&launch_args)
                    .await
                    .map_err(|err| map_local_server_startup_error(err, &data_dir))?;

                // Subdomains of the manifest's built-in local environments are expanded from
                // the `localServer` ports or their defaults, so the check applies whenever the
                // manifest has such an environment (with or without a `localServer` section);
                // which environment the CLI has selected is irrelevant to the local server.
                if ctx.manifest_has_builtin_local_environment() {
                    warn_on_subdomain_port_mismatches(ctx.manifest_local_server(), &startup_ports);
                }

                while let Some(res) = join_set.join_next().await {
                    res??;
                }

                Ok(())
            }
            ServerSubcommand::Clean => {
                let data_dir = data_dir_from_local_server(ctx.manifest_local_server())?;
                clean_data_dir(&ctx, &data_dir).await
            }
        }
    }

    async fn run_server() -> anyhow::Result<()> {
        let args = RunArgs::default().with_env_overrides()?;
        let data_dir = default_data_dir()?;
        let local_metering = local_metering_from_env()?;

        let (mut join_set, _) = launch_golem_services(&LaunchArgs {
            system_memory_override: args.system_memory_override,
            router_addr: args.router_addr().to_string(),
            router_port: args.router_port(),
            custom_request_port: args.custom_request_port(),
            mcp_port: args.mcp_port(),
            ports_file: args.ports_file.clone(),
            data_dir: data_dir.clone(),
            agent_filesystem_root: args.agent_filesystem_root.clone(),
            managed_xfs_root_dir: local_metering.managed_xfs_root_dir,
            resource_usage_metering: local_metering.resource_usage_metering,
            monthly_compute_gcu: local_metering.monthly_compute_gcu,
            monthly_memory_gb_seconds: local_metering.monthly_memory_gb_seconds,
            monthly_durable_storage_gb_month: local_metering.monthly_durable_storage_gb_month,
            monthly_ephemeral_storage_gb_month: local_metering.monthly_ephemeral_storage_gb_month,
        })
        .await
        .map_err(|err| map_local_server_startup_error(err, &data_dir))?;

        tokio::spawn(async move {
            while let Some(res) = join_set.join_next().await {
                res.unwrap().unwrap();
            }
        });

        Ok(())
    }

    fn override_verbosity(verbosity: Verbosity) -> Verbosity {
        if verbosity.is_present() {
            verbosity
        } else {
            Verbosity::new(2, 0)
        }
    }

    fn override_pretty_mode() -> bool {
        true
    }
}

fn default_data_dir() -> anyhow::Result<PathBuf> {
    Ok(dirs::data_local_dir()
        .ok_or_else(|| anyhow!("Failed to get data local dir"))?
        .join("golem"))
}

fn launch_args_from_run_args_and_manifest(
    args: &RunArgs,
    ctx: &Context,
) -> anyhow::Result<LaunchArgs> {
    launch_args_from_run_args_and_local_server(
        args,
        ctx.manifest_local_server(),
        local_metering_from_env()?,
    )
}

#[derive(Debug, Default)]
struct LocalMetering {
    resource_usage_metering: ResourceUsageMeteringConfig,
    monthly_compute_gcu: u64,
    monthly_memory_gb_seconds: u64,
    monthly_durable_storage_gb_month: u64,
    monthly_ephemeral_storage_gb_month: u64,
    managed_xfs_root_dir: Option<PathBuf>,
}

const MONTHLY_COMPUTE_GCU: &str = "GOLEM__INITIAL_PLANS__DEFAULT__MONTHLY_COMPUTE_GCU";
const MONTHLY_MEMORY_GB_SECONDS: &str = "GOLEM__INITIAL_PLANS__DEFAULT__MONTHLY_MEMORY_GB_SECONDS";
const MONTHLY_DURABLE_STORAGE_GB_MONTH: &str =
    "GOLEM__INITIAL_PLANS__DEFAULT__MONTHLY_DURABLE_STORAGE_GB_MONTH";
const MONTHLY_EPHEMERAL_STORAGE_GB_MONTH: &str =
    "GOLEM__INITIAL_PLANS__DEFAULT__MONTHLY_EPHEMERAL_STORAGE_GB_MONTH";
const MANAGED_XFS_ROOT_DIR: &str = "GOLEM__FILESYSTEM_STORAGE__MANAGED_XFS_ROOT_DIR";

fn local_metering_from_env() -> anyhow::Result<LocalMetering> {
    local_metering_from(resource_usage_metering_from_env()?, env_value)
}

fn local_metering_from(
    resource_usage_metering: ResourceUsageMeteringConfig,
    mut value: impl FnMut(&str) -> anyhow::Result<Option<String>>,
) -> anyhow::Result<LocalMetering> {
    let monthly_compute_gcu = value(MONTHLY_COMPUTE_GCU)?;
    let monthly_memory_gb_seconds = value(MONTHLY_MEMORY_GB_SECONDS)?;
    let monthly_durable_storage_gb_month = value(MONTHLY_DURABLE_STORAGE_GB_MONTH)?;
    let monthly_ephemeral_storage_gb_month = value(MONTHLY_EPHEMERAL_STORAGE_GB_MONTH)?;
    let managed_xfs_root_dir = value(MANAGED_XFS_ROOT_DIR)?;

    resolve_local_metering(
        resource_usage_metering,
        parse_optional_u64(MONTHLY_COMPUTE_GCU, monthly_compute_gcu.as_deref())?,
        parse_optional_u64(
            MONTHLY_MEMORY_GB_SECONDS,
            monthly_memory_gb_seconds.as_deref(),
        )?,
        parse_optional_u64(
            MONTHLY_DURABLE_STORAGE_GB_MONTH,
            monthly_durable_storage_gb_month.as_deref(),
        )?,
        parse_optional_u64(
            MONTHLY_EPHEMERAL_STORAGE_GB_MONTH,
            monthly_ephemeral_storage_gb_month.as_deref(),
        )?,
        parse_optional_path(MANAGED_XFS_ROOT_DIR, managed_xfs_root_dir.as_deref())?,
    )
}

fn resolve_local_metering(
    resource_usage_metering: ResourceUsageMeteringConfig,
    monthly_compute_gcu: Option<u64>,
    monthly_memory_gb_seconds: Option<u64>,
    monthly_durable_storage_gb_month: Option<u64>,
    monthly_ephemeral_storage_gb_month: Option<u64>,
    managed_xfs_root_dir: Option<PathBuf>,
) -> anyhow::Result<LocalMetering> {
    if resource_usage_metering.compute && monthly_compute_gcu.is_none() {
        bail!("{MONTHLY_COMPUTE_GCU} is required when compute metering is enabled");
    }
    if resource_usage_metering.memory && monthly_memory_gb_seconds.is_none() {
        bail!("{MONTHLY_MEMORY_GB_SECONDS} is required when memory metering is enabled");
    }
    if resource_usage_metering.filesystem {
        if monthly_durable_storage_gb_month.is_none() {
            bail!(
                "{MONTHLY_DURABLE_STORAGE_GB_MONTH} is required when filesystem metering is enabled"
            );
        }
        if monthly_ephemeral_storage_gb_month.is_none() {
            bail!(
                "{MONTHLY_EPHEMERAL_STORAGE_GB_MONTH} is required when filesystem metering is enabled"
            );
        }
        if managed_xfs_root_dir.is_none() {
            bail!("{MANAGED_XFS_ROOT_DIR} is required when filesystem metering is enabled");
        }
    }

    let monthly_compute_gcu = monthly_compute_gcu.unwrap_or(0);
    let monthly_memory_gb_seconds = monthly_memory_gb_seconds.unwrap_or(0);
    let monthly_durable_storage_gb_month = monthly_durable_storage_gb_month.unwrap_or(0);
    let monthly_ephemeral_storage_gb_month = monthly_ephemeral_storage_gb_month.unwrap_or(0);
    MonthlyPlanAmounts {
        compute_gcu: monthly_compute_gcu,
        memory_gb_seconds: monthly_memory_gb_seconds,
        durable_storage_gb_month: monthly_durable_storage_gb_month,
        ephemeral_storage_gb_month: monthly_ephemeral_storage_gb_month,
    }
    .resolve()
    .map_err(|error| anyhow!("Invalid local monthly Plan amount: {error}"))?;

    Ok(LocalMetering {
        resource_usage_metering,
        monthly_compute_gcu,
        monthly_memory_gb_seconds,
        monthly_durable_storage_gb_month,
        monthly_ephemeral_storage_gb_month,
        managed_xfs_root_dir,
    })
}

fn resource_usage_metering_from_env() -> anyhow::Result<ResourceUsageMeteringConfig> {
    Ok(ResourceUsageMeteringConfig {
        compute: metering_dimension_from_env("GOLEM__RESOURCE_USAGE_METERING__COMPUTE")?,
        memory: metering_dimension_from_env("GOLEM__RESOURCE_USAGE_METERING__MEMORY")?,
        filesystem: metering_dimension_from_env("GOLEM__RESOURCE_USAGE_METERING__FILESYSTEM")?,
    })
}

fn metering_dimension_from_env(name: &str) -> anyhow::Result<bool> {
    match std::env::var(name) {
        Ok(value) => parse_metering_dimension(name, &value),
        Err(std::env::VarError::NotPresent) => Ok(false),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("Failed to parse {name}: non-Unicode value")
        }
    }
}

fn env_value(name: &str) -> anyhow::Result<Option<String>> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!("Failed to parse {name}: non-Unicode value")
        }
    }
}

fn parse_optional_u64(name: &str, value: Option<&str>) -> anyhow::Result<Option<u64>> {
    value
        .map(|value| {
            value
                .parse()
                .with_context(|| format!("Failed to parse {name}: {value}"))
        })
        .transpose()
}

fn parse_optional_path(name: &str, value: Option<&str>) -> anyhow::Result<Option<PathBuf>> {
    match value {
        Some("") => bail!("Failed to parse {name}: path is empty"),
        Some(value) => Ok(Some(PathBuf::from(value))),
        None => Ok(None),
    }
}

fn parse_metering_dimension(name: &str, value: &str) -> anyhow::Result<bool> {
    value
        .parse()
        .with_context(|| format!("Failed to parse {name}: {value}"))
}

fn data_dir_from_local_server(
    local_server: Option<&ResolvedLocalServer>,
) -> anyhow::Result<PathBuf> {
    match local_server.and_then(|manifest| manifest.data_dir.clone()) {
        Some(data_dir) => Ok(data_dir),
        None => default_data_dir(),
    }
}

fn launch_args_from_run_args_and_local_server(
    args: &RunArgs,
    local_server: Option<&ResolvedLocalServer>,
    local_metering: LocalMetering,
) -> anyhow::Result<LaunchArgs> {
    let launch_args = LaunchArgs {
        system_memory_override: args
            .system_memory_override
            .or_else(|| local_server.and_then(|manifest| manifest.system_memory_override)),
        router_addr: args
            .router_addr
            .clone()
            .or_else(|| local_server.and_then(|manifest| manifest.router_addr.clone()))
            .unwrap_or_else(|| args.router_addr().to_string()),
        router_port: args
            .router_port
            .or_else(|| local_server.and_then(|manifest| manifest.router_port))
            .unwrap_or_else(|| args.router_port()),
        custom_request_port: args
            .custom_request_port
            .or_else(|| local_server.and_then(|manifest| manifest.custom_request_port))
            .unwrap_or_else(|| args.custom_request_port()),
        mcp_port: args
            .mcp_port
            .or_else(|| local_server.and_then(|manifest| manifest.mcp_port))
            .unwrap_or_else(|| args.mcp_port()),
        ports_file: args
            .ports_file
            .clone()
            .or_else(|| local_server.and_then(|manifest| manifest.ports_file.clone())),
        data_dir: match &args.data_dir {
            Some(data_dir) => data_dir.clone(),
            None => data_dir_from_local_server(local_server)?,
        },
        agent_filesystem_root: args
            .agent_filesystem_root
            .clone()
            .or_else(|| local_server.and_then(|manifest| manifest.agent_filesystem_root.clone())),
        managed_xfs_root_dir: local_metering.managed_xfs_root_dir,
        resource_usage_metering: local_metering.resource_usage_metering,
        monthly_compute_gcu: local_metering.monthly_compute_gcu,
        monthly_memory_gb_seconds: local_metering.monthly_memory_gb_seconds,
        monthly_durable_storage_gb_month: local_metering.monthly_durable_storage_gb_month,
        monthly_ephemeral_storage_gb_month: local_metering.monthly_ephemeral_storage_gb_month,
    };
    launch_args.validate()?;
    Ok(launch_args)
}

/// A local server port that differs from the one the manifest's deployment `subdomain`
/// expansion uses, e.g. because it was overridden with a flag or requested as `0`.
#[derive(Debug, PartialEq, Eq)]
struct SubdomainPortMismatch {
    deployment_kind: &'static str,
    manifest_field: &'static str,
    flag: &'static str,
    expanded_port: u16,
    bound_port: u16,
}

/// Deployment subdomains expand to `<label>.localhost:<port>` using the manifest's
/// `localServer.customRequestPort` / `localServer.mcpPort` (or their defaults), and a request is
/// only routed when its `Host` header matches that expansion exactly. A server bound to a
/// different port cannot serve those deployments, so the ports have to match. `local_server` is
/// `None` when the manifest has no `localServer` section, in which case the defaults apply.
fn subdomain_port_mismatches(
    local_server: Option<&ResolvedLocalServer>,
    startup_ports: &StartupPorts,
) -> Vec<SubdomainPortMismatch> {
    let checks = [
        (
            "HTTP API",
            "localServer.customRequestPort",
            "--custom-request-port",
            local_server
                .and_then(|local_server| local_server.custom_request_port)
                .unwrap_or(DEFAULT_LOCAL_CUSTOM_REQUEST_PORT),
            startup_ports.custom_request_port,
        ),
        (
            "MCP",
            "localServer.mcpPort",
            "--mcp-port",
            local_server
                .and_then(|local_server| local_server.mcp_port)
                .unwrap_or(DEFAULT_LOCAL_MCP_PORT),
            startup_ports.mcp_port,
        ),
    ];

    checks
        .into_iter()
        .filter(|(_, _, _, expanded_port, bound_port)| expanded_port != bound_port)
        .map(
            |(deployment_kind, manifest_field, flag, expanded_port, bound_port)| {
                SubdomainPortMismatch {
                    deployment_kind,
                    manifest_field,
                    flag,
                    expanded_port,
                    bound_port,
                }
            },
        )
        .collect()
}

fn warn_on_subdomain_port_mismatches(
    local_server: Option<&ResolvedLocalServer>,
    startup_ports: &StartupPorts,
) {
    for mismatch in subdomain_port_mismatches(local_server, startup_ports) {
        log_warn_action(
            "Bound",
            format!(
                "{} port {} differs from port {} used by {} deployment subdomains ({}); those deployments are not reachable on this server, use the same value for {} and {}",
                mismatch.deployment_kind,
                mismatch.bound_port.to_string().log_color_highlight(),
                mismatch.expanded_port.to_string().log_color_highlight(),
                mismatch.deployment_kind,
                mismatch.manifest_field.log_color_highlight(),
                mismatch.manifest_field.log_color_highlight(),
                mismatch.flag.log_color_highlight(),
            ),
        );
    }
}

fn resolve_clean_data_dir(data_dir: &Path) -> anyhow::Result<PathBuf> {
    let data_dir = fs::absolute_lexical_path(data_dir)?;
    let Some(parent) = data_dir.parent() else {
        bail!(
            "Refusing to clean filesystem root {}",
            data_dir.display().to_string().log_color_highlight()
        );
    };
    let file_name = data_dir
        .file_name()
        .ok_or_else(|| anyhow!("Data directory {} has no name", data_dir.display()))?;
    let resolved_parent = std::fs::canonicalize(parent).with_context(|| {
        format!(
            "Failed to resolve parent of local server data directory {}",
            data_dir.display()
        )
    })?;

    Ok(resolved_parent.join(file_name))
}

async fn clean_data_dir(ctx: &Arc<Context>, data_dir: &Path) -> anyhow::Result<()> {
    let data_dir = resolve_clean_data_dir(data_dir)?;
    if !ctx
        .interactive_handler()
        .confirm_clean_local_server_data_dir(&data_dir)?
    {
        bail!(NonSuccessfulExit);
    }

    log_warn_action(
        "Cleaning",
        format!(
            "local server data directory {}",
            data_dir.display().to_string().log_color_highlight()
        ),
    );
    tokio::fs::remove_dir_all(&data_dir)
        .await
        .map_err(|err| anyhow!("Failed cleaning data dir ({}): {}", data_dir.display(), err))
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_cli::model::app_raw::LocalServer;
    use std::collections::HashMap;
    use test_r::test;

    fn local_server(value: LocalServer) -> ResolvedLocalServer {
        ResolvedLocalServer::from_raw_with_base_dir(&value, Path::new("/tmp/test-app"))
    }

    #[test]
    fn metering_dimension_values_are_validated() {
        assert!(parse_metering_dimension("METERING", "true").unwrap());
        assert!(!parse_metering_dimension("METERING", "false").unwrap());
        assert!(parse_metering_dimension("METERING", "invalid").is_err());
    }

    #[test]
    fn monthly_amount_values_preserve_presence_and_validate_input() {
        assert_eq!(parse_optional_u64("AMOUNT", None).unwrap(), None);
        assert_eq!(parse_optional_u64("AMOUNT", Some("0")).unwrap(), Some(0));
        assert_eq!(parse_optional_u64("AMOUNT", Some("17")).unwrap(), Some(17));
        assert!(parse_optional_u64("AMOUNT", Some("invalid")).is_err());
        assert!(parse_optional_u64("AMOUNT", Some("-1")).is_err());
    }

    #[test]
    fn local_metering_reads_and_validates_environment_values() {
        let mut values = HashMap::from([
            (MONTHLY_COMPUTE_GCU, "2"),
            (MONTHLY_MEMORY_GB_SECONDS, "3"),
            (MONTHLY_DURABLE_STORAGE_GB_MONTH, "5"),
            (MONTHLY_EPHEMERAL_STORAGE_GB_MONTH, "7"),
            (MANAGED_XFS_ROOT_DIR, "/managed-xfs"),
        ]);

        let resolved = local_metering_from(ResourceUsageMeteringConfig::all_enabled(), |name| {
            Ok(values.get(name).map(ToString::to_string))
        })
        .unwrap();
        assert_eq!(
            resolved.resource_usage_metering,
            ResourceUsageMeteringConfig::all_enabled()
        );
        assert_eq!(resolved.monthly_compute_gcu, 2);
        assert_eq!(resolved.monthly_memory_gb_seconds, 3);
        assert_eq!(resolved.monthly_durable_storage_gb_month, 5);
        assert_eq!(resolved.monthly_ephemeral_storage_gb_month, 7);
        assert_eq!(
            resolved.managed_xfs_root_dir,
            Some(PathBuf::from("/managed-xfs"))
        );

        values.insert(MONTHLY_COMPUTE_GCU, "invalid");
        assert!(
            local_metering_from(ResourceUsageMeteringConfig::all_enabled(), |name| {
                Ok(values.get(name).map(ToString::to_string))
            })
            .is_err()
        );
        values.insert(MONTHLY_COMPUTE_GCU, "2");
        values.insert(MANAGED_XFS_ROOT_DIR, "");
        assert!(
            local_metering_from(ResourceUsageMeteringConfig::all_enabled(), |name| {
                Ok(values.get(name).map(ToString::to_string))
            })
            .is_err()
        );
    }

    #[test]
    fn local_metering_defaults_to_disabled_zero_amounts() {
        let resolved = resolve_local_metering(
            ResourceUsageMeteringConfig::default(),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            resolved.resource_usage_metering,
            ResourceUsageMeteringConfig::default()
        );
        assert_eq!(resolved.monthly_compute_gcu, 0);
        assert_eq!(resolved.monthly_memory_gb_seconds, 0);
        assert_eq!(resolved.monthly_durable_storage_gb_month, 0);
        assert_eq!(resolved.monthly_ephemeral_storage_gb_month, 0);
        assert_eq!(resolved.managed_xfs_root_dir, None);
    }

    #[test]
    fn enabled_metering_requires_its_own_amounts_and_managed_xfs() {
        let compute = ResourceUsageMeteringConfig {
            compute: true,
            ..Default::default()
        };
        assert!(
            resolve_local_metering(compute, None, None, None, None, None)
                .unwrap_err()
                .to_string()
                .contains(MONTHLY_COMPUTE_GCU)
        );

        let memory = ResourceUsageMeteringConfig {
            memory: true,
            ..Default::default()
        };
        assert!(
            resolve_local_metering(memory, None, None, None, None, None)
                .unwrap_err()
                .to_string()
                .contains(MONTHLY_MEMORY_GB_SECONDS)
        );

        let filesystem = ResourceUsageMeteringConfig {
            filesystem: true,
            ..Default::default()
        };
        assert!(
            resolve_local_metering(filesystem, None, None, None, None, None)
                .unwrap_err()
                .to_string()
                .contains(MONTHLY_DURABLE_STORAGE_GB_MONTH)
        );
        assert!(
            resolve_local_metering(filesystem, None, None, Some(1), None, None)
                .unwrap_err()
                .to_string()
                .contains(MONTHLY_EPHEMERAL_STORAGE_GB_MONTH)
        );
        assert!(
            resolve_local_metering(filesystem, None, None, Some(1), Some(2), None)
                .unwrap_err()
                .to_string()
                .contains(MANAGED_XFS_ROOT_DIR)
        );
    }

    #[test]
    fn explicit_zero_is_valid_for_enabled_dimensions() {
        let resolved = resolve_local_metering(
            ResourceUsageMeteringConfig::all_enabled(),
            Some(0),
            Some(0),
            Some(0),
            Some(0),
            Some(PathBuf::from("/xfs")),
        )
        .unwrap();

        assert_eq!(resolved.monthly_compute_gcu, 0);
        assert_eq!(resolved.monthly_memory_gb_seconds, 0);
        assert_eq!(resolved.monthly_durable_storage_gb_month, 0);
        assert_eq!(resolved.monthly_ephemeral_storage_gb_month, 0);
    }

    #[test]
    fn disabled_dimensions_preserve_explicit_independent_amounts() {
        let resolved = resolve_local_metering(
            ResourceUsageMeteringConfig {
                memory: true,
                ..Default::default()
            },
            Some(2),
            Some(3),
            Some(5),
            Some(7),
            Some(PathBuf::from("/xfs")),
        )
        .unwrap();

        assert_eq!(resolved.monthly_compute_gcu, 2);
        assert_eq!(resolved.monthly_memory_gb_seconds, 3);
        assert_eq!(resolved.monthly_durable_storage_gb_month, 5);
        assert_eq!(resolved.monthly_ephemeral_storage_gb_month, 7);
        assert_eq!(resolved.managed_xfs_root_dir, Some(PathBuf::from("/xfs")));
    }

    #[test]
    fn local_metering_rejects_customer_unit_overflow() {
        use golem_common::model::account_usage::{BYTE_SECONDS_PER_GB_MONTH, FUEL_PER_GCU};

        assert!(
            resolve_local_metering(
                ResourceUsageMeteringConfig::default(),
                Some(u64::MAX / FUEL_PER_GCU + 1),
                None,
                None,
                None,
                None,
            )
            .unwrap_err()
            .to_string()
            .contains("compute GCU")
        );
        assert!(
            resolve_local_metering(
                ResourceUsageMeteringConfig::default(),
                None,
                None,
                Some(u64::MAX / BYTE_SECONDS_PER_GB_MONTH + 1),
                None,
                None,
            )
            .unwrap_err()
            .to_string()
            .contains("durable storage GB-month")
        );
        assert!(
            resolve_local_metering(
                ResourceUsageMeteringConfig::default(),
                None,
                None,
                None,
                Some(u64::MAX / BYTE_SECONDS_PER_GB_MONTH + 1),
                None,
            )
            .unwrap_err()
            .to_string()
            .contains("ephemeral storage GB-month")
        );
    }

    #[test]
    fn manifest_local_server_values_are_used_when_cli_args_are_absent() {
        let manifest = local_server(LocalServer {
            system_memory_override: std::num::NonZeroU64::new(2147483648),
            router_addr: Some("127.0.0.1".to_string()),
            router_port: Some(9882),
            custom_request_port: Some(9008),
            mcp_port: Some(9009),
            ports_file: Some(PathBuf::from("/tmp/test-app/.golem/ports.json")),
            data_dir: Some(PathBuf::from("/tmp/test-app/.golem/data")),
            agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
        });

        let args = launch_args_from_run_args_and_local_server(
            &RunArgs::default(),
            Some(&manifest),
            LocalMetering::default(),
        )
        .unwrap();

        assert_eq!(args.router_addr, "127.0.0.1");
        assert_eq!(args.system_memory_override.unwrap().get(), 2147483648);
        assert_eq!(args.router_port, 9882);
        assert_eq!(args.custom_request_port, 9008);
        assert_eq!(args.mcp_port, 9009);
        assert_eq!(
            args.ports_file,
            Some(PathBuf::from("/tmp/test-app/.golem/ports.json"))
        );
        assert_eq!(args.data_dir, PathBuf::from("/tmp/test-app/.golem/data"));
        assert_eq!(
            args.agent_filesystem_root,
            Some(PathBuf::from("/tmp/test-app/.golem/agents"))
        );
    }

    #[test]
    fn subdomain_ports_match_when_bound_ports_equal_manifest_or_default_ports() {
        let manifest = local_server(LocalServer {
            custom_request_port: Some(9008),
            ..LocalServer::default()
        });
        let ports = StartupPorts {
            router_port: 9881,
            custom_request_port: 9008,
            mcp_port: DEFAULT_LOCAL_MCP_PORT,
        };

        assert_eq!(subdomain_port_mismatches(Some(&manifest), &ports), vec![]);
    }

    #[test]
    fn subdomain_ports_use_defaults_without_local_server_section() {
        // e.g. `--custom-request-port 0` in an app whose manifest has no `localServer`
        let ports = StartupPorts {
            router_port: 9881,
            custom_request_port: 41235,
            mcp_port: DEFAULT_LOCAL_MCP_PORT,
        };

        assert_eq!(
            subdomain_port_mismatches(None, &ports),
            vec![SubdomainPortMismatch {
                deployment_kind: "HTTP API",
                manifest_field: "localServer.customRequestPort",
                flag: "--custom-request-port",
                expanded_port: DEFAULT_LOCAL_CUSTOM_REQUEST_PORT,
                bound_port: 41235,
            }]
        );
    }

    #[test]
    fn subdomain_ports_mismatch_when_bound_ports_differ() {
        let manifest = local_server(LocalServer {
            custom_request_port: Some(9008),
            ..LocalServer::default()
        });
        // e.g. `--custom-request-port 0 --mcp-port 0`, OS-assigned ports
        let ports = StartupPorts {
            router_port: 9881,
            custom_request_port: 41235,
            mcp_port: 41236,
        };

        assert_eq!(
            subdomain_port_mismatches(Some(&manifest), &ports),
            vec![
                SubdomainPortMismatch {
                    deployment_kind: "HTTP API",
                    manifest_field: "localServer.customRequestPort",
                    flag: "--custom-request-port",
                    expanded_port: 9008,
                    bound_port: 41235,
                },
                SubdomainPortMismatch {
                    deployment_kind: "MCP",
                    manifest_field: "localServer.mcpPort",
                    flag: "--mcp-port",
                    expanded_port: DEFAULT_LOCAL_MCP_PORT,
                    bound_port: 41236,
                },
            ]
        );
    }

    #[test]
    fn local_server_system_memory_override_uses_detection_when_unset() {
        let args = launch_args_from_run_args_and_local_server(
            &RunArgs::default(),
            None,
            LocalMetering::default(),
        )
        .unwrap();
        assert_eq!(args.system_memory_override, None);
    }

    #[test]
    fn cli_args_override_manifest_local_server_values() {
        let manifest = local_server(LocalServer {
            system_memory_override: std::num::NonZeroU64::new(2147483648),
            router_addr: Some("127.0.0.1".to_string()),
            router_port: Some(9882),
            custom_request_port: Some(9008),
            mcp_port: Some(9009),
            ports_file: Some(PathBuf::from("/tmp/test-app/.golem/ports.json")),
            data_dir: Some(PathBuf::from("/tmp/test-app/.golem/data")),
            agent_filesystem_root: Some(PathBuf::from("/tmp/test-app/.golem/agents")),
        });
        let run_args = RunArgs {
            system_memory_override: std::num::NonZeroU64::new(1073741824),
            router_addr: Some("0.0.0.0".to_string()),
            router_port: Some(10000),
            custom_request_port: Some(10001),
            mcp_port: Some(10002),
            ports_file: Some(PathBuf::from("cli-ports.json")),
            data_dir: Some(PathBuf::from("cli-data")),
            clean: false,
            agent_filesystem_root: Some(PathBuf::from("cli-agents")),
        };

        let args = launch_args_from_run_args_and_local_server(
            &run_args,
            Some(&manifest),
            LocalMetering::default(),
        )
        .unwrap();

        assert_eq!(args.router_addr, "0.0.0.0");
        assert_eq!(args.system_memory_override.unwrap().get(), 1073741824);
        assert_eq!(args.router_port, 10000);
        assert_eq!(args.custom_request_port, 10001);
        assert_eq!(args.mcp_port, 10002);
        assert_eq!(args.ports_file, Some(PathBuf::from("cli-ports.json")));
        assert_eq!(args.data_dir, PathBuf::from("cli-data"));
        assert_eq!(
            args.agent_filesystem_root,
            Some(PathBuf::from("cli-agents"))
        );
    }

    #[test]
    fn clean_rejects_filesystem_root() {
        let current_dir = std::env::current_dir().unwrap();
        let root = current_dir.ancestors().last().unwrap();

        let error = resolve_clean_data_dir(root).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Refusing to clean filesystem root")
        );
    }

    #[test]
    fn clean_resolves_relative_data_dir() {
        let data_dir = resolve_clean_data_dir(Path::new("local-server-data")).unwrap();

        assert!(data_dir.is_absolute());
        assert!(data_dir.ends_with(Path::new("local-server-data")));

        let absolute_data_dir = std::env::current_dir().unwrap().join("local-server-data");
        assert_eq!(
            resolve_clean_data_dir(&absolute_data_dir).unwrap(),
            absolute_data_dir
        );
    }

    #[cfg(unix)]
    #[test]
    fn clean_resolves_intermediate_symlink_without_following_final_symlink() {
        use std::os::unix::fs::symlink;

        let test_root =
            std::env::temp_dir().join(format!("golem-clean-symlink-test-{}", std::process::id()));
        let actual_parent = test_root.join("actual");
        let intermediate_link = test_root.join("intermediate-link");
        let final_link = actual_parent.join("final-link");
        std::fs::create_dir_all(&actual_parent).unwrap();
        symlink(&actual_parent, &intermediate_link).unwrap();
        symlink(actual_parent.join("final-target"), &final_link).unwrap();

        let resolved_intermediate =
            resolve_clean_data_dir(&intermediate_link.join("data")).unwrap();
        let resolved_final = resolve_clean_data_dir(&final_link).unwrap();
        let canonical_parent = std::fs::canonicalize(&actual_parent).unwrap();

        assert_eq!(resolved_intermediate, canonical_parent.join("data"));
        assert_eq!(resolved_final, canonical_parent.join("final-link"));

        std::fs::remove_dir_all(&test_root).unwrap();
    }
}
