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

use super::{
    Arc, CallReplayOutcome, ClassifiedHostError, Context, DiscoveredTool, DurableCallSession,
    DurableFunctionType, DurableWorkerCtx, GolemSpecificWasmTrap, GolemToolGetAllTools,
    GolemToolGetTool, HostFailureKind, HostRequestGolemToolGetTool, HostRequestNoInput,
    HostResponseGolemToolTool, HostResponseGolemToolTools, InternalRetryResult, NotCancellable,
    ResolvedOwnerContext, SerializableToolDiscoverySnapshot, ToolBindingOwner, ToolDeploymentState,
    ToolDiscoveryError, ToolName, WitRegisteredTool, WitTool, WorkerCtx,
    get_accessible_tool_from_deployment, get_accessible_tools_from_deployment,
};

impl TryFrom<&DiscoveredTool> for WitRegisteredTool {
    type Error = anyhow::Error;

    fn try_from(value: &DiscoveredTool) -> Result<Self, Self::Error> {
        let name = value.definition.name().unwrap_or("<unnamed tool>");
        let definition = WitTool::try_from(&value.definition).with_context(|| {
            format!(
                "failed to encode discovered tool '{name}' implemented by component {}",
                value.implemented_by
            )
        })?;

        Ok(Self {
            lookup_name: value.lookup_name.clone(),
            definition,
            implemented_by: value.implemented_by.into(),
        })
    }
}

pub(super) fn classify_tool_discovery_error(error: &ToolDiscoveryError) -> HostFailureKind {
    match error {
        ToolDiscoveryError::Retrieval(_) => HostFailureKind::Transient,
        ToolDiscoveryError::Mcp(error) => match error {
            golem_service_base::clients::registry::RegistryServiceError::InternalServerError(_)
            | golem_service_base::clients::registry::RegistryServiceError::InternalClientError(_) => {
                HostFailureKind::Transient
            }
            _ => HostFailureKind::Permanent,
        },
        ToolDiscoveryError::AgentContextRequired
        | ToolDiscoveryError::MissingDeploymentRevision { .. }
        | ToolDiscoveryError::InconsistentSnapshot { .. } => HostFailureKind::Permanent,
    }
}

pub(super) fn terminal_tool_discovery_error(message: String) -> anyhow::Error {
    anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Permanent,
        message,
    })
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(super) async fn observe_mcp_tools(
        &mut self,
        deployment: Option<&ToolDeploymentState>,
        name: Option<&ToolName>,
    ) -> Result<
        Vec<golem_common::model::oplog::payload::types::SerializableMcpImportDiscovery>,
        ToolDiscoveryError,
    > {
        use golem_common::model::mcp_import::{McpImportSource, mcp_import_bridge_source};
        use golem_common::model::oplog::payload::types::{
            SerializableDiscoveredTools, SerializableMcpImportDiscovery,
        };

        let Some(deployment) = deployment else {
            return Ok(Vec::new());
        };
        if deployment.mcp_imports.is_empty()
            || name.is_some_and(|name| deployment.registered_tools.contains_key(name))
        {
            return Ok(Vec::new());
        }
        let auth = self
            .capture_agent_auth_ctx_at_boundary()
            .await?
            .ok_or(ToolDiscoveryError::AgentContextRequired)?;
        let mut observations = Vec::new();
        for index in 0..deployment.mcp_imports.len() {
            let source = McpImportSource {
                environment_id: self.state.owned_agent_id.environment_id,
                deployment_revision: deployment.deployment_revision,
                import_index: index.try_into().map_err(|_| {
                    ToolDiscoveryError::InconsistentSnapshot {
                        details: "too many MCP imports".into(),
                    }
                })?,
                upstream_tool_name: String::new(),
            };
            let observation = self
                .state
                .environment_state_service
                .resolve_mcp_import(&source, &auth, false)
                .await
                .map_err(ToolDiscoveryError::Mcp)?;
            let found = name.is_some_and(|name| {
                observation
                    .tools
                    .iter()
                    .any(|tool| tool.definition.name() == Some(name.as_str()))
            });
            observations.push(SerializableMcpImportDiscovery {
                import_index: source.import_index,
                tools: SerializableDiscoveredTools(
                    observation
                        .tools
                        .into_iter()
                        .map(|tool| {
                            DiscoveredTool::new(tool.definition, mcp_import_bridge_source())
                        })
                        .collect(),
                ),
                exclusions: observation
                    .diagnostics
                    .into_iter()
                    .map(|diagnostic| (diagnostic.upstream_name, diagnostic.reason))
                    .collect(),
            });
            if found {
                break;
            }
        }
        Ok(observations)
    }

    pub(super) async fn rehydrate_tool_discovery(
        &mut self,
        revision: Option<u64>,
        live: Option<Arc<ToolDeploymentState>>,
    ) -> anyhow::Result<Option<Arc<ToolDeploymentState>>> {
        let Some(revision) = revision else {
            return Ok(None);
        };
        let revision = revision.try_into().map_err(|error| {
            terminal_tool_discovery_error(format!(
                "recorded invalid tool deployment revision {revision}: {error}"
            ))
        })?;
        if let Some(deployment) = live.filter(|state| state.deployment_revision == revision) {
            return Ok(Some(deployment));
        }
        self.state
            .environment_state_service
            .get_tool_deployment_state_at_revision(
                self.state.owned_agent_id.environment_id,
                revision,
            )
            .await
            .map(Some)
            .map_err(|error| {
                anyhow::Error::new(ClassifiedHostError {
                    kind: classify_tool_discovery_error(&error),
                    message: format!(
                        "failed to rehydrate recorded tool deployment revision {revision}: {error}"
                    ),
                })
            })
    }

    pub(crate) async fn get_all_tools_model(&mut self) -> anyhow::Result<Vec<Arc<DiscoveredTool>>> {
        self.process_pending_replay_events().await?;
        let environment_id = self.state.owned_agent_id.environment_id;
        let owner_component_metadata = self.owner_component_metadata();
        let component_id = owner_component_metadata.id;
        let component_revision = owner_component_metadata.revision;
        let binding_owner = match self.owner_context() {
            ResolvedOwnerContext::Agent(agent) => ToolBindingOwner::AgentType {
                agent_type_name: agent.agent_type.clone(),
            },
            ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                ToolBindingOwner::ComponentBaseline { component_id }
            }
        };

        let mut handle = DurableCallSession::<GolemToolGetAllTools, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let mut live_deployment = None;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = {
                loop {
                    let result = self
                        .state
                        .environment_state_service
                        .get_live_tool_deployment_state(
                            environment_id,
                            component_id,
                            component_revision,
                        )
                        .await;
                    let result = async {
                        let deployment = result?;
                        get_accessible_tools_from_deployment(
                            deployment.as_deref(),
                            &binding_owner,
                        )?;
                        let dynamic_tools =
                            self.observe_mcp_tools(deployment.as_deref(), None).await?;
                        let deployment_revision = deployment
                            .as_ref()
                            .map(|deployment| deployment.deployment_revision.get());
                        live_deployment = deployment;
                        Ok(SerializableToolDiscoverySnapshot {
                            deployment_revision,
                            dynamic_tools,
                        })
                    }
                    .await;
                    if matches!(&result, Err(ToolDiscoveryError::Mcp(golem_service_base::clients::registry::RegistryServiceError::LimitExceeded(_)))) {
                        return Err(handle.trap(GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted));
                    }
                    match handle
                        .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                        .await?
                    {
                        InternalRetryResult::Persist => break result,
                        InternalRetryResult::RetryInternally => continue,
                    }
                }
            };

            handle
                .complete(
                    self,
                    HostResponseGolemToolTools {
                        result: result.map_err(|error| error.to_string()),
                    },
                )
                .await?
        };

        let snapshot = response.result.map_err(|error| {
            terminal_tool_discovery_error(format!(
                "failed to discover tools for owner '{binding_owner:?}' in environment '{environment_id}': {error}"
            ))
        })?;
        let deployment = self
            .rehydrate_tool_discovery(snapshot.deployment_revision, live_deployment)
            .await?;
        merge_discovered_tools(deployment.as_deref(), &binding_owner, None, snapshot)
            .map_err(|error| terminal_tool_discovery_error(error.to_string()))
    }

    pub(crate) async fn get_tool_model(
        &mut self,
        tool_name: String,
    ) -> anyhow::Result<Option<Arc<DiscoveredTool>>> {
        self.process_pending_replay_events().await?;
        let valid_tool_name = ToolName::try_from(tool_name.as_str()).ok();
        let environment_id = self.state.owned_agent_id.environment_id;
        let owner_component_metadata = self.owner_component_metadata();
        let component_id = owner_component_metadata.id;
        let component_revision = owner_component_metadata.revision;
        let binding_owner = match self.owner_context() {
            ResolvedOwnerContext::Agent(agent) => ToolBindingOwner::AgentType {
                agent_type_name: agent.agent_type.clone(),
            },
            ResolvedOwnerContext::ComponentWorker | ResolvedOwnerContext::ComponentBaseline => {
                ToolBindingOwner::ComponentBaseline { component_id }
            }
        };

        let mut handle = DurableCallSession::<GolemToolGetTool, NotCancellable>::start(
            self,
            HostRequestGolemToolGetTool {
                name: tool_name.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let mut live_deployment = None;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = {
                if valid_tool_name.is_none() {
                    Ok(SerializableToolDiscoverySnapshot {
                        deployment_revision: None,
                        dynamic_tools: Vec::new(),
                    })
                } else {
                    loop {
                        let result = self
                            .state
                            .environment_state_service
                            .get_live_tool_deployment_state(
                                environment_id,
                                component_id,
                                component_revision,
                            )
                            .await;
                        let result = async {
                            let deployment = result?;
                            if let Some(valid_tool_name) = &valid_tool_name {
                                get_accessible_tool_from_deployment(
                                    deployment.as_deref(),
                                    &binding_owner,
                                    valid_tool_name,
                                )?;
                            }
                            let dynamic_tools = self
                                .observe_mcp_tools(deployment.as_deref(), valid_tool_name.as_ref())
                                .await?;
                            let deployment_revision = deployment
                                .as_ref()
                                .map(|deployment| deployment.deployment_revision.get());
                            live_deployment = deployment;
                            Ok(SerializableToolDiscoverySnapshot {
                                deployment_revision,
                                dynamic_tools,
                            })
                        }
                        .await;
                        if matches!(&result, Err(ToolDiscoveryError::Mcp(golem_service_base::clients::registry::RegistryServiceError::LimitExceeded(_)))) {
                            return Err(handle.trap(GolemSpecificWasmTrap::WorkerMonthlyHttpCallBudgetExhausted));
                        }
                        match handle
                            .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                            .await?
                        {
                            InternalRetryResult::Persist => break result,
                            InternalRetryResult::RetryInternally => continue,
                        }
                    }
                }
            };

            handle
                .complete(
                    self,
                    HostResponseGolemToolTool {
                        result: result.map_err(|error| error.to_string()),
                    },
                )
                .await?
        };

        let snapshot = response.result.map_err(|error| {
            terminal_tool_discovery_error(format!(
                "failed to discover tool '{}' for owner '{binding_owner:?}' in environment '{environment_id}': {error}",
                tool_name
            ))
        })?;
        let Some(valid_tool_name) = valid_tool_name else {
            return Ok(None);
        };
        let deployment = self
            .rehydrate_tool_discovery(snapshot.deployment_revision, live_deployment)
            .await?;
        Ok(merge_discovered_tools(
            deployment.as_deref(),
            &binding_owner,
            Some(&valid_tool_name),
            snapshot,
        )
        .map_err(|error| terminal_tool_discovery_error(error.to_string()))?
        .into_iter()
        .find(|tool| tool.lookup_name == valid_tool_name.as_str()))
    }
}

pub(super) fn merge_discovered_tools(
    deployment: Option<&ToolDeploymentState>,
    owner: &ToolBindingOwner,
    selected_name: Option<&ToolName>,
    snapshot: SerializableToolDiscoverySnapshot,
) -> Result<Vec<Arc<DiscoveredTool>>, ToolDiscoveryError> {
    let mut tools = match selected_name {
        Some(name) => get_accessible_tool_from_deployment(deployment, owner, name)?
            .into_iter()
            .collect(),
        None => get_accessible_tools_from_deployment(deployment, owner)?,
    };
    // A native name remains reserved even when it is not bound to this agent.
    let mut names = deployment
        .into_iter()
        .flat_map(|state| state.registered_tools.keys())
        .map(|name| name.as_str().to_owned())
        .collect::<std::collections::BTreeSet<_>>();
    for observation in snapshot.dynamic_tools {
        for mut tool in observation.tools.0 {
            if let Some(name) = tool.definition.name()
                && selected_name.is_none_or(|selected| selected.as_str() == name)
                && names.insert(name.to_owned())
            {
                let tool_name = ToolName::try_from(name)
                    .map_err(|details| ToolDiscoveryError::InconsistentSnapshot { details })?;
                if let Some(deployment) = deployment {
                    let configuration = &deployment.tool_middleware_configuration;
                    let environment_binding = configuration.environment_bindings.get(&tool_name);
                    let owner_binding = match owner {
                        ToolBindingOwner::AgentType { agent_type_name } => configuration
                            .agent_bindings
                            .get(agent_type_name)
                            .and_then(|bindings| bindings.get(&tool_name)),
                        ToolBindingOwner::ComponentBaseline { .. } => None,
                    };
                    let (config, readable, revealable) =
                        crate::services::environment_state::mcp_binding_scopes(
                            environment_binding,
                            owner_binding,
                        );
                    let compiled = golem_common::model::tool_middleware::compile::compile_discovered_tool_middleware_chain(
                        deployment.deployment_revision,
                        &tool.definition,
                        owner,
                        &tool_name,
                        &config,
                        &readable,
                        &revealable,
                        &deployment.registered_tool_middlewares.values().cloned().collect::<Vec<_>>(),
                        &configuration.universal,
                        environment_binding,
                        owner_binding,
                        configuration.compatibility_mode,
                    );
                    if !compiled.errors.is_empty() {
                        return Err(ToolDiscoveryError::InconsistentSnapshot {
                            details: format!(
                                "middleware for dynamic tool '{tool_name}' is incompatible: {}",
                                compiled
                                    .errors
                                    .iter()
                                    .map(|diagnostic| diagnostic.message.as_str())
                                    .collect::<Vec<_>>()
                                    .join("; ")
                            ),
                        });
                    }
                    if let Some(chain) = compiled.chains.into_iter().next() {
                        tool.definition = chain.effective_definition;
                    }
                }
                tools.push(Arc::new(tool));
            }
        }
    }
    Ok(tools)
}
