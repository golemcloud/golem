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

//! Host implementation of `golem:tool/host@0.1.0`.
//!
//! The interface is wired into the linker for every component shape so that any
//! agent or tool component may import it and instantiate. Tool discovery is
//! implemented as a durable environment-state read; invocation operations
//! remain placeholders until the tool runtime is implemented.

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::tool::host::{
    Host, HostFutureInvokeResult, HostFutureInvokeResultWithStore, HostToolRpc,
    HostToolRpcWithStore, InvocationResult, RegisteredTool as WitRegisteredTool, RpcError,
    TypedSchemaValue,
};
use crate::services::environment_state::ToolDiscoveryError;
use crate::workerctx::WorkerCtx;
use anyhow::{Context, anyhow};
use golem_common::model::agent::{AgentTypeName, ParsedAgentId};
use golem_common::model::oplog::host_functions::{GolemToolGetAllTools, GolemToolGetTool};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestGolemToolGetTool, HostRequestNoInput,
    HostResponseGolemToolTool, HostResponseGolemToolTools,
};
use golem_common::model::tool::{RegisteredTool, ToolName, ToolSource};
use golem_common::schema::tool::DiscoveredTool;
use golem_common::schema::tool::wit::encode_tool;
use wasmtime::component::{Accessor, HasSelf, Resource, StreamReader};

const NOT_IMPLEMENTED: &str = "golem:tool/host is not yet implemented";

/// Host-side resource table entry backing the `golem:tool/host.tool-rpc`
/// resource. A placeholder until the tool runtime is implemented.
pub struct ToolRpcEntry;

/// Host-side resource table entry backing the
/// `golem:tool/host.future-invoke-result` resource. A placeholder until the tool
/// runtime is implemented.
pub struct FutureInvokeResultEntry;

fn discover_registered_tool(registered_tool: RegisteredTool) -> DiscoveredTool {
    let RegisteredTool {
        definition, source, ..
    } = registered_tool;
    let implemented_by = match source {
        ToolSource::Component { component_id, .. } => component_id,
    };

    DiscoveredTool {
        definition,
        implemented_by,
    }
}

fn encode_discovered_tool(discovered_tool: &DiscoveredTool) -> anyhow::Result<WitRegisteredTool> {
    let name = discovered_tool
        .definition
        .name()
        .unwrap_or("<unnamed tool>");
    let definition = encode_tool(&discovered_tool.definition).with_context(|| {
        format!(
            "failed to encode discovered tool '{name}' implemented by component {}",
            discovered_tool.implemented_by
        )
    })?;

    Ok(WitRegisteredTool {
        definition,
        implemented_by: discovered_tool.implemented_by.into(),
    })
}

fn classify_tool_discovery_error(error: &ToolDiscoveryError) -> HostFailureKind {
    match error {
        ToolDiscoveryError::Retrieval(_) => HostFailureKind::Transient,
        ToolDiscoveryError::InconsistentSnapshot { .. } => HostFailureKind::Permanent,
    }
}

fn terminal_tool_discovery_error(message: String) -> anyhow::Error {
    anyhow::Error::new(ClassifiedHostError {
        kind: HostFailureKind::Permanent,
        message,
    })
}

fn tool_discovery_principal(parsed_agent_id: Option<ParsedAgentId>) -> Option<AgentTypeName> {
    parsed_agent_id.map(|agent_id| agent_id.agent_type)
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) async fn get_all_tools_model(&mut self) -> anyhow::Result<Vec<DiscoveredTool>> {
        let Some(agent_type) = tool_discovery_principal(self.parsed_agent_id()) else {
            self.observe_function_call(
                <GolemToolGetAllTools as HostPayloadPair>::INTERFACE,
                <GolemToolGetAllTools as HostPayloadPair>::FUNCTION,
            );
            return Ok(Vec::new());
        };
        let environment_id = self.state.owned_agent_id.environment_id;

        let mut handle = CallHandle::<GolemToolGetAllTools, NotCancellable>::start(
            self,
            HostRequestNoInput {},
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = loop {
                let result = self
                    .state
                    .environment_state_service
                    .get_accessible_tools(environment_id, &agent_type)
                    .await
                    .map(|tools| {
                        tools
                            .into_iter()
                            .map(discover_registered_tool)
                            .collect::<Vec<_>>()
                    });
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
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

        response.result.map_err(|error| {
            terminal_tool_discovery_error(format!(
                "failed to discover tools for agent type '{}' in environment '{environment_id}': {error}",
                agent_type.0
            ))
        })
    }

    pub(crate) async fn get_tool_model(
        &mut self,
        tool_name: ToolName,
    ) -> anyhow::Result<Option<DiscoveredTool>> {
        let Some(agent_type) = tool_discovery_principal(self.parsed_agent_id()) else {
            self.observe_function_call(
                <GolemToolGetTool as HostPayloadPair>::INTERFACE,
                <GolemToolGetTool as HostPayloadPair>::FUNCTION,
            );
            return Ok(None);
        };
        let environment_id = self.state.owned_agent_id.environment_id;

        let mut handle = CallHandle::<GolemToolGetTool, NotCancellable>::start(
            self,
            HostRequestGolemToolGetTool {
                name: tool_name.as_str().to_string(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let response = 'result: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = loop {
                let result = self
                    .state
                    .environment_state_service
                    .get_accessible_tool(environment_id, &agent_type, &tool_name)
                    .await
                    .map(|tool| tool.map(discover_registered_tool));
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_tool_discovery_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
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

        response.result.map_err(|error| {
            terminal_tool_discovery_error(format!(
                "failed to discover tool '{}' for agent type '{}' in environment '{environment_id}': {error}",
                tool_name, agent_type.0
            ))
        })
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_tools(&mut self) -> anyhow::Result<Vec<WitRegisteredTool>> {
        self.get_all_tools_model()
            .await?
            .iter()
            .map(encode_discovered_tool)
            .collect()
    }

    async fn get_tool(&mut self, name: String) -> anyhow::Result<Option<WitRegisteredTool>> {
        let Ok(tool_name) = ToolName::try_from(name) else {
            self.observe_function_call(
                <GolemToolGetTool as HostPayloadPair>::INTERFACE,
                <GolemToolGetTool as HostPayloadPair>::FUNCTION,
            );
            return Ok(None);
        };

        self.get_tool_model(tool_name)
            .await?
            .as_ref()
            .map(encode_discovered_tool)
            .transpose()
    }
}

impl<Ctx: WorkerCtx> HostToolRpc for DurableWorkerCtx<Ctx> {
    async fn new(&mut self, _tool_name: String) -> anyhow::Result<Resource<ToolRpcEntry>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "new");
        Err(anyhow!(NOT_IMPLEMENTED))
    }

    async fn invoke(
        &mut self,
        _self_: Resource<ToolRpcEntry>,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<StreamReader<u8>>,
    ) -> anyhow::Result<Result<(), RpcError>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "invoke");
        Ok(Err(RpcError::RemoteInternalError(
            NOT_IMPLEMENTED.to_string(),
        )))
    }

    async fn async_invoke_and_await(
        &mut self,
        _self_: Resource<ToolRpcEntry>,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<StreamReader<u8>>,
    ) -> anyhow::Result<Resource<FutureInvokeResultEntry>> {
        self.observe_function_call("golem::tool::host::tool-rpc", "async-invoke-and-await");
        Err(anyhow!(NOT_IMPLEMENTED))
    }

    async fn drop(&mut self, rep: Resource<ToolRpcEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::tool-rpc", "drop");
        let _ = self.table().delete(rep);
        Ok(())
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostToolRpcWithStore<U> for HasSelf<DurableWorkerCtx<Ctx>> {
    async fn invoke_and_await(
        accessor: &Accessor<U, Self>,
        _self_: Resource<ToolRpcEntry>,
        _command_path: Vec<String>,
        _input: TypedSchemaValue,
        _stdin: Option<StreamReader<u8>>,
    ) -> anyhow::Result<Result<InvocationResult, RpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::tool-rpc", "invoke-and-await");
        });
        Ok(Err(RpcError::RemoteInternalError(
            NOT_IMPLEMENTED.to_string(),
        )))
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> HostFutureInvokeResultWithStore<U>
    for HasSelf<DurableWorkerCtx<Ctx>>
{
    async fn get(
        accessor: &Accessor<U, Self>,
        _self_: Resource<FutureInvokeResultEntry>,
    ) -> anyhow::Result<Result<InvocationResult, RpcError>> {
        accessor.with(|mut access| {
            access
                .get()
                .observe_function_call("golem::tool::host::future-invoke-result", "get");
        });
        Ok(Err(RpcError::RemoteInternalError(
            NOT_IMPLEMENTED.to_string(),
        )))
    }
}

impl<Ctx: WorkerCtx> HostFutureInvokeResult for DurableWorkerCtx<Ctx> {
    async fn cancel(&mut self, _self_: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "cancel");
        Ok(())
    }

    async fn drop(&mut self, rep: Resource<FutureInvokeResultEntry>) -> anyhow::Result<()> {
        self.observe_function_call("golem::tool::host::future-invoke-result", "drop");
        let _ = self.table().delete(rep);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        classify_tool_discovery_error, discover_registered_tool, encode_discovered_tool,
        terminal_tool_discovery_error, tool_discovery_principal,
    };
    use crate::durable_host::durability::{ClassifiedHostError, HostFailureKind};
    use crate::services::environment_state::ToolDiscoveryError;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{AgentTypeName, ParsedAgentId};
    use golem_common::model::component::{ComponentId, ComponentName, ComponentRevision};
    use golem_common::model::deployment::DeploymentRevision;
    use golem_common::model::tool::{RegisteredTool, ToolProvisionConfig, ToolSource};
    use golem_common::schema::tool::wit::decode_tool;
    use golem_common::schema::tool::{
        CommandBody, CommandNode, CommandTree, Doc, Globals, Positional, Positionals, Tool,
    };
    use golem_common::schema::{MetadataEnvelope, SchemaGraph, SchemaType, SchemaTypeDef, TypeId};
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use test_r::test;

    fn registered_tool() -> (RegisteredTool, ComponentId) {
        let component_id = ComponentId::new();
        let definition = Tool {
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "search".to_string(),
                    aliases: vec!["find".to_string()],
                    doc: Doc {
                        summary: "Search documents".to_string(),
                        description: "Searches indexed documents".to_string(),
                        examples: Vec::new(),
                    },
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: Some(CommandBody {
                        positionals: Positionals {
                            fixed: vec![Positional {
                                name: "query".to_string(),
                                doc: Doc::default(),
                                value_name: Some("QUERY".to_string()),
                                type_: SchemaType::string().with_metadata(MetadataEnvelope {
                                    doc: Some("Text to search for".to_string()),
                                    aliases: vec!["term".to_string()],
                                    ..Default::default()
                                }),
                                default: None,
                                required: true,
                                accepts_stdio: false,
                            }],
                            tail: None,
                        },
                        options: Vec::new(),
                        flags: Vec::new(),
                        constraints: Vec::new(),
                        stdin: None,
                        stdout: None,
                        result: None,
                        errors: Vec::new(),
                        annotations: None,
                    }),
                }],
            },
            schema: SchemaGraph::empty(),
        };

        (
            RegisteredTool {
                deployment_revision: DeploymentRevision::try_from(1_u64).unwrap(),
                definition,
                provision: ToolProvisionConfig::default(),
                source: ToolSource::Component {
                    component_id,
                    component_revision: ComponentRevision::try_from(7_u64).unwrap(),
                    component_name: ComponentName("search-tools".to_string()),
                },
                owner_account_id: AccountId::new(),
                owner_account_email: AccountEmail::new("owner@example.com"),
                metadata_version: "0.1.0".to_string(),
            },
            component_id,
        )
    }

    #[test]
    fn tool_discovery_principal_fails_closed_without_parsed_agent_id() {
        assert_eq!(tool_discovery_principal(None), None);

        let agent_type = AgentTypeName("AgentA".to_string());
        let parsed_agent_id = ParsedAgentId::new(
            agent_type.clone(),
            golem_common::schema::TypedSchemaValue::new(
                SchemaGraph::empty(),
                golem_common::schema::SchemaValue::Record { fields: Vec::new() },
            ),
            None,
        );
        assert_eq!(
            tool_discovery_principal(Some(parsed_agent_id)),
            Some(agent_type)
        );
    }

    #[test]
    fn registered_component_tool_converts_to_discovery_wit_record() {
        let (registered, component_id) = registered_tool();
        let expected_definition = registered.definition.clone();

        let discovered = discover_registered_tool(registered);
        assert_eq!(discovered.definition, expected_definition);
        assert_eq!(discovered.implemented_by, component_id);

        let wit = encode_discovered_tool(&discovered).unwrap();
        assert_eq!(decode_tool(&wit.definition).unwrap(), expected_definition);
        assert_eq!(ComponentId::from(wit.implemented_by), component_id);
    }

    #[test]
    fn tool_discovery_error_classification_preserves_integrity_semantics() {
        let retrieval = ToolDiscoveryError::Retrieval(WorkerExecutorError::runtime("offline"));
        let inconsistent = ToolDiscoveryError::InconsistentSnapshot {
            details: "dangling binding".to_string(),
        };

        assert_eq!(
            classify_tool_discovery_error(&retrieval),
            HostFailureKind::Transient
        );
        assert_eq!(
            classify_tool_discovery_error(&inconsistent),
            HostFailureKind::Permanent
        );
    }

    #[test]
    fn terminal_tool_discovery_errors_remain_permanent() {
        let error = terminal_tool_discovery_error("discovery failed".to_string());
        let classified = error.downcast_ref::<ClassifiedHostError>().unwrap();

        assert_eq!(classified.kind, HostFailureKind::Permanent);
        assert_eq!(classified.message, "discovery failed");
    }

    #[test]
    fn discovered_tool_wit_encoding_error_has_tool_and_source_context() {
        let (registered, component_id) = registered_tool();
        let mut discovered = discover_registered_tool(registered);
        let duplicate_id = TypeId::new("duplicate");
        let duplicate_definition = SchemaTypeDef {
            id: duplicate_id,
            name: Some("Duplicate".to_string()),
            body: SchemaType::string(),
        };
        discovered.definition.schema.defs =
            vec![duplicate_definition.clone(), duplicate_definition];

        let error = match encode_discovered_tool(&discovered) {
            Ok(_) => panic!("duplicate schema definitions must fail WIT encoding"),
            Err(error) => error,
        };
        let message = format!("{error:#}");

        assert!(message.contains("discovered tool 'search'"));
        assert!(message.contains(&component_id.to_string()));
        assert!(message.contains("duplicate type id: duplicate"));
    }
}
