// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::command::api::deployment::ApiDeploymentSubcommand;
use crate::command_handler::Handlers;
use crate::context::Context;
use crate::error::service::{AnyhowMapServiceError, ServiceError};
use crate::log::{log_action, log_warn_action, LogColorize, LogIndent};
use crate::model::environment::{EnvironmentResolveMode, ResolvedEnvironmentIdentity};
use crate::model::http_api::{HttpApiDeploymentDeployProperties, McpDeploymentDeployProperties};
use crate::model::text::http_api_deployment::HttpApiDeploymentGetView;
use anyhow::{anyhow, bail};
use golem_client::api::{ApiDeploymentClient, McpDeploymentClient};
use golem_common::cache::SimpleCache;
use golem_common::model::deployment::DeploymentPlanHttpApiDeploymentEntry;
use golem_common::model::diff;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::EnvironmentName;
use golem_common::model::http_api_deployment::{
    HttpApiDeployment, HttpApiDeploymentCreation, HttpApiDeploymentId, HttpApiDeploymentRevision,
    HttpApiDeploymentUpdate,
};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ApiDeploymentCommandHandler {
    ctx: Arc<Context>,
}

impl ApiDeploymentCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle_command(&self, command: ApiDeploymentSubcommand) -> anyhow::Result<()> {
        match command {
            ApiDeploymentSubcommand::Get { domain } => self.cmd_get(Domain(domain)).await,
            ApiDeploymentSubcommand::List => self.cmd_list().await,
        }
    }

    async fn cmd_get(&self, domain: Domain) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let Some(result) = self
            .resolve_http_api_deployment(&environment, &domain, None)
            .await?
        else {
            bail!("Not found!");
        };

        self.ctx
            .log_handler()
            .log_view(&HttpApiDeploymentGetView(result));

        Ok(())
    }

    async fn cmd_list(&self) -> anyhow::Result<()> {
        let environment = self
            .ctx
            .environment_handler()
            .resolve_environment(EnvironmentResolveMode::Any)
            .await?;

        let deployments = environment
            .with_current_deployment_revision_or_default_warn(
                |current_deployment_revision| async move {
                    Ok(self
                        .ctx
                        .golem_clients()
                        .await?
                        .api_deployment
                        .list_http_api_deployments_in_deployment(
                            &environment.environment_id.0,
                            current_deployment_revision.into(),
                        )
                        .await
                        .map_service_error()?
                        .values)
                },
            )
            .await?;

        self.ctx.log_handler().log_view(&deployments);

        Ok(())
    }

    async fn resolve_http_api_deployment(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        domain: &Domain,
        revision: Option<&HttpApiDeploymentRevision>,
    ) -> anyhow::Result<Option<HttpApiDeployment>> {
        environment
            .with_current_deployment_revision_or_default_warn(
                |current_deployment_revision| async move {
                    let clients = self.ctx.golem_clients().await?;

                    let Some(deployment) = clients
                        .api_deployment
                        .get_http_api_deployment_in_deployment(
                            &environment.environment_id.0,
                            current_deployment_revision.get(),
                            &domain.0,
                        )
                        .await
                        .map_service_error_not_found_as_opt()?
                    else {
                        return Ok(None);
                    };

                    let Some(revision) = revision else {
                        return Ok(Some(deployment));
                    };

                    clients
                        .api_deployment
                        .get_http_api_deployment_revision(&deployment.id.0, (*revision).into())
                        .await
                        .map_service_error_not_found_as_opt()
                },
            )
            .await
    }

    pub async fn deployable_manifest_api_deployments(
        &self,
        environment_name: &EnvironmentName,
    ) -> anyhow::Result<BTreeMap<Domain, HttpApiDeploymentDeployProperties>> {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        Ok(app_ctx
            .application()
            .http_api_deployments(environment_name)
            .map(|deployments| {
                deployments
                    .iter()
                    .map(|(name, source)| (name.clone(), source.value.clone()))
                    .collect::<BTreeMap<_, _>>()
            })
            .unwrap_or_default())
    }

    pub async fn deployable_manifest_mcp_deployments(
        &self,
        environment_name: &EnvironmentName,
    ) -> anyhow::Result<BTreeMap<Domain, crate::model::http_api::McpDeploymentDeployProperties>>
    {
        let app_ctx = self.ctx.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        Ok(app_ctx
            .application()
            .mcp_deployments(environment_name)
            .map(
                |deployments: &BTreeMap<
                    golem_common::model::domain_registration::Domain,
                    crate::model::app::WithSource<
                        crate::model::http_api::McpDeploymentDeployProperties,
                    >,
                >| {
                    deployments
                        .iter()
                        .map(|(domain, mcp_deployment)| {
                            (domain.clone(), mcp_deployment.value.clone())
                        })
                        .collect::<BTreeMap<_, _>>()
                },
            )
            .unwrap_or_default())
    }

    pub async fn get_http_api_deployment_revision_by_id(
        &self,
        http_api_deployment_id: &HttpApiDeploymentId,
        revision: HttpApiDeploymentRevision,
    ) -> anyhow::Result<HttpApiDeployment> {
        self.ctx
            .caches()
            .http_api_deployment_revision
            .get_or_insert_simple(&(*http_api_deployment_id, revision), {
                let ctx = self.ctx.clone();
                async move || {
                    ctx.golem_clients()
                        .await?
                        .api_deployment
                        .get_http_api_deployment_revision(
                            &http_api_deployment_id.0,
                            revision.into(),
                        )
                        .await
                        .map_service_error()
                        .map_err(Arc::new)
                }
            })
            .await
            .map_err(|err| anyhow!(err))
    }

    pub async fn create_staged_http_api_deployment(
        &self,
        environment: &ResolvedEnvironmentIdentity,
        domain: &Domain,
        deployable_http_api_deployment: &HttpApiDeploymentDeployProperties,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        log_action(
            "Creating",
            format!("HTTP API deployment {}", domain.0.log_color_highlight()),
        );
        let _indent = LogIndent::new();

        let create = async || {
            clients
                .api_deployment
                .create_http_api_deployment(
                    &environment.environment_id.0,
                    &HttpApiDeploymentCreation {
                        domain: domain.clone(),
                        webhooks_url: deployable_http_api_deployment.webhooks_url.clone(),
                        agents: deployable_http_api_deployment.agents.clone(),
                    },
                )
                .await
                .map_err(ServiceError::from)
        };

        let deployment = match create().await {
            Ok(result) => Ok(result),
            Err(err) if err.is_domain_is_not_registered() => {
                self.ctx
                    .api_domain_handler()
                    .register_missing_domain(&environment.environment_id, domain)
                    .await?;
                create().await
            }
            Err(err) => Err(err),
        }?;

        log_action(
            "Created",
            format!(
                "HTTP API deployment revision: {} {}",
                deployment.domain.0.log_color_highlight(),
                deployment.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn delete_staged_http_api_deployment(
        &self,
        http_api_deployment: &DeploymentPlanHttpApiDeploymentEntry,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Deleting",
            format!(
                "HTTP API deployment {}",
                http_api_deployment.domain.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        self.ctx
            .golem_clients()
            .await?
            .api_deployment
            .delete_http_api_deployment(
                &http_api_deployment.id.0,
                http_api_deployment.revision.into(),
            )
            .await
            .map_service_error()?;

        log_action(
            "Deleted",
            format!(
                "HTTP API deployment revision: {} {}",
                http_api_deployment.domain.0.log_color_highlight(),
                http_api_deployment
                    .revision
                    .to_string()
                    .log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn update_staged_http_api_deployment(
        &self,
        http_api_deployment: &DeploymentPlanHttpApiDeploymentEntry,
        deployable_http_api_deployment: &HttpApiDeploymentDeployProperties,
        diff: &diff::DiffForHashOf<diff::HttpApiDeployment>,
    ) -> anyhow::Result<()> {
        log_action(
            "Updating",
            format!(
                "HTTP API deployment {}",
                http_api_deployment.domain.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        let webhook_url_changed = match diff {
            diff::DiffForHashOf::HashDiff { .. } => true,
            diff::DiffForHashOf::ValueDiff { diff } => diff.webhooks_url_changed,
        };

        let agents_changed = match diff {
            diff::DiffForHashOf::HashDiff { .. } => true,
            diff::DiffForHashOf::ValueDiff { diff } => !diff.agents_changes.is_empty(),
        };

        let deployment = self
            .ctx
            .golem_clients()
            .await?
            .api_deployment
            .update_http_api_deployment(
                &http_api_deployment.id.0,
                &HttpApiDeploymentUpdate {
                    current_revision: http_api_deployment.revision,
                    webhook_url: if webhook_url_changed {
                        Some(deployable_http_api_deployment.webhooks_url.clone())
                    } else {
                        None
                    },
                    agents: if agents_changed {
                        Some(deployable_http_api_deployment.agents.clone())
                    } else {
                        None
                    },
                },
            )
            .await
            .map_service_error()?;

        log_action(
            "Created",
            format!(
                "HTTP API deployment revision: {} {}",
                deployment.domain.0.log_color_highlight(),
                deployment.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn create_staged_mcp_deployment(
        &self,
        environment_id: &ResolvedEnvironmentIdentity,
        domain: &Domain,
        deployable_mcp_deployment: &McpDeploymentDeployProperties,
    ) -> anyhow::Result<()> {
        let clients = self.ctx.golem_clients().await?;

        log_action(
            "Creating",
            format!("MCP deployment {}", domain.0.log_color_highlight()),
        );
        let _indent = LogIndent::new();

        let agents = deployable_mcp_deployment
            .agents
            .iter()
            .map(|(k, _v)| (k.clone(), golem_common::model::mcp_deployment::McpDeploymentAgentOptions::default()))
            .collect();

        let mcp_creation = golem_common::model::mcp_deployment::McpDeploymentCreation {
            domain: domain.clone(),
            agents,
        };

        let create = async || {
            clients
                .mcp_deployment
                .create_mcp_deployment(&environment_id.environment_id.0, &mcp_creation)
                .await
                .map_err(ServiceError::from)
        };

        let deployment = match create().await {
            Ok(result) => Ok(result),
            Err(err) if err.is_domain_is_not_registered() => {
                self.ctx
                    .api_domain_handler()
                    .register_missing_domain(&environment_id.environment_id, domain)
                    .await?;

                create().await
            }
            Err(err) => Err(err),
        }?;

        log_action(
            "Created",
            format!(
                "MCP deployment revision: {} {}",
                deployment.domain.0.log_color_highlight(),
                deployment.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn delete_staged_mcp_deployment(
        &self,
        mcp_deployment: &golem_common::model::deployment::DeploymentPlanMcpDeploymentEntry,
    ) -> anyhow::Result<()> {
        log_warn_action(
            "Deleting",
            format!(
                "MCP deployment {}",
                mcp_deployment.domain.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        self.ctx
            .golem_clients()
            .await?
            .mcp_deployment
            .delete_mcp_deployment(&mcp_deployment.id.0, mcp_deployment.revision.into())
            .await
            .map_service_error()?;

        log_action(
            "Deleted",
            format!(
                "MCP deployment revision: {} {}",
                mcp_deployment.domain.0.log_color_highlight(),
                mcp_deployment.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }

    pub async fn update_staged_mcp_deployment(
        &self,
        mcp_deployment: &golem_common::model::deployment::DeploymentPlanMcpDeploymentEntry,
        update: &golem_common::model::mcp_deployment::McpDeploymentUpdate,
        diff: &diff::DiffForHashOf<diff::McpDeployment>,
    ) -> anyhow::Result<()> {
        log_action(
            "Updating",
            format!(
                "MCP deployment {}",
                mcp_deployment.domain.0.log_color_highlight()
            ),
        );
        let _indent = LogIndent::new();

        let agents_changed = match diff {
            diff::DiffForHashOf::HashDiff { .. } => true,
            diff::DiffForHashOf::ValueDiff { diff } => !diff.agents_changes.is_empty(),
        };

        let deployment = self
            .ctx
            .golem_clients()
            .await?
            .mcp_deployment
            .update_mcp_deployment(
                &mcp_deployment.id.0,
                &golem_common::model::mcp_deployment::McpDeploymentUpdate {
                    current_revision: update.current_revision,
                    domain: update.domain.clone(),
                    agents: if agents_changed {
                        update.agents.clone()
                    } else {
                        None
                    },
                },
            )
            .await
            .map_service_error()?;

        log_action(
            "Updated",
            format!(
                "MCP deployment revision: {} {}",
                deployment.domain.0.log_color_highlight(),
                deployment.revision.to_string().log_color_highlight()
            ),
        );

        Ok(())
    }
}
