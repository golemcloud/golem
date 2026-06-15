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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::agent::host::Host;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::model::PromiseId;
use golem_common::model::agent::bindings::golem::agent::common::{
    AgentError, DataValue, RegisteredAgentType,
};
use golem_common::model::agent::{
    AgentConfigDeclaration, AgentConfigSource, AgentTypeName, LegacyParsedAgentId,
};
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::oplog::host_functions::{
    GolemAgentCreateWebhook, GolemAgentGetAgentType, GolemAgentGetAllAgentTypes,
    GolemAgentGetConfigValue,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemAgentGetAgentType, HostRequestGolemAgentGetConfigValue,
    HostRequestGolemApiPromiseId, HostRequestNoInput, HostResponseGolemAgentAgentType,
    HostResponseGolemAgentAgentTypes, HostResponseGolemAgentGetConfigValue,
    HostResponseGolemAgentWebhookUrl,
};
use golem_common::schema::adapters::analysed_type::{
    analysed_type_to_schema_type_inline, schema_type_to_analysed_type,
};
use golem_common::schema::adapters::value::schema_value_to_value;
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::SchemaValue;
use golem_common::schema::validation::subtyping::is_assignable;
use golem_wasm::analysis::AnalysedType;
use golem_wasm::{NodeBuilder, WitType, WitValue, WitValueBuilderExtensions};

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Resolve a local agent-config value.
    fn resolve_local_config(
        &self,
        key: &[String],
        key_str: &str,
        expected_type: &SchemaType,
        declared_type: &SchemaType,
    ) -> anyhow::Result<WitValue> {
        let config_value = self.state.agent_config.get(key);

        // Future automatic-update transforms belong here, where both
        // the component-declared type and the guest-expected type are
        // available together with the stored local config value.
        if declared_type != expected_type {
            return Err(anyhow!(
                "declared and expected type for config key {key_str} are not compatible"
            ));
        }

        let result = match (expected_type, config_value) {
            (SchemaType::Option { .. }, None) => WitValue::builder().option_none(),
            (_, Some(value)) => value.value.clone().into(),
            (_, None) => return Err(anyhow!("required config key {key_str} is missing value")),
        };

        Ok(result)
    }

    /// Resolve a secret-backed agent-config value. The stored
    /// [`AgentSecret`] carries its own [`SchemaGraph`] (with possibly
    /// recursive named types reached via [`SchemaType::Ref`]); the
    /// guest-supplied `expected_type` is inline (no refs).
    /// Compatibility between the two is checked via
    /// [`schema_types_compatible`], which resolves refs against the
    /// secret's graph.
    async fn resolve_secret_config(
        &mut self,
        path: Vec<String>,
        path_str: &str,
        expected_type: SchemaType,
        declared_type: &SchemaType,
    ) -> anyhow::Result<WitValue> {
        // Future automatic-update transforms belong here, where both
        // the component-declared type and the guest-expected type are
        // available together with the resolved secret metadata/value.
        // This deterministic validation must happen before opening the
        // durable function; replay must not be able to skip it and return
        // a previously persisted config value.
        if declared_type != &expected_type {
            return Err(anyhow!(
                "declared and expected type for secret key {path_str} are not compatible"
            ));
        }

        let handle = CallHandle::<GolemAgentGetConfigValue, NotCancellable>::start(
            self,
            HostRequestGolemAgentGetConfigValue {
                path: path.clone(),
                expected_type: schema_type_to_analysed_type(
                    &SchemaGraph::anonymous(expected_type.clone()),
                    &expected_type,
                )
                .map_err(|e| {
                    anyhow!(
                        "Expected secret type for key {path_str} is not representable as AnalysedType: {e}"
                    )
                })?,
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let persisted = handle
            .run(self, async |ctx| -> anyhow::Result<_> {
                let agent_secrets = ctx
                    .state
                    .environment_state_service
                    .get_agent_secrets(ctx.state.component_metadata.environment_id)
                    .await?;

                let canonical_agent_secret_path =
                    CanonicalAgentSecretPath::from_path_in_unknown_casing(&path);
                let agent_secret = agent_secrets.get(&canonical_agent_secret_path);

                let result_schema = match (&expected_type, agent_secret) {
                    // No secret stored; `Option<_>` resolves to `None`.
                    (SchemaType::Option { .. }, None) => SchemaValue::Option { inner: None },

                    // No secret stored and a non-optional expected type.
                    (_, None) => {
                        return Err(anyhow!(
                            "declared and expected type for secret key {path_str} are not compatible"
                        ));
                    }

                    // Secret exists. Compatibility uses the secret's own
                    // graph so any [`SchemaType::Ref`] in the secret's root
                    // resolves through `secret.secret_type` — including a
                    // ref to `Option<T>` matched against an inline
                    // `Option<T>` expected type.
                    (expected_type, Some(secret)) => {
                        if !schema_types_compatible(
                            &secret.secret_type,
                            expected_type,
                            &secret.secret_type.root,
                        ) {
                            return Err(anyhow!(
                                "declared and expected type for config key {path_str} are not compatible"
                            ));
                        }

                    match (expected_type, &secret.secret_value) {
                        // Missing-value secrets with an `Option<_>`
                        // expected type collapse to `None`.
                        (SchemaType::Option { .. }, None) => SchemaValue::Option { inner: None },
                        (_, None) => {
                            return Err(anyhow!("Secret key {path_str} is missing value"));
                        }
                        (_, Some(value)) => value.clone(),
                    }
                }
            };

                // The oplog payload and guest return value cross the
                // durability / WIT-bindgen boundary as `Value` /
                // `AnalysedType`. When the secret has its own graph it is
                // used directly; otherwise the inline expected type stands
                // in as a self-contained anonymous graph.
                let boundary_graph = if let Some(sec) = agent_secret {
                    sec.secret_type.clone()
                } else {
                    SchemaGraph::anonymous(expected_type.clone())
                };
                let result = schema_value_to_value(
                    &boundary_graph,
                    &boundary_graph.root,
                    &result_schema,
                )
                .map_err(|e| {
                    anyhow!(
                        "Resolved secret value for key {path_str} is not representable as Value: {e}"
                    )
                })?;

                Ok(HostResponseGolemAgentGetConfigValue { result })
            })
            .await?;

        Ok(persisted.result.into())
    }
}

/// Structural type equality, resolving any [`SchemaType::Ref`] nodes
/// against `graph`. Bidirectional [`is_assignable`] collapses to type
/// equality on the same graph; the guest-supplied inline side has no
/// refs to resolve, while the secret's `Ref`s are followed via `graph`.
fn schema_types_compatible(graph: &SchemaGraph, left: &SchemaType, right: &SchemaType) -> bool {
    is_assignable(graph, left, right) && is_assignable(graph, right, left)
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_agent_types(&mut self) -> anyhow::Result<Vec<RegisteredAgentType>> {
        let mut handle = CallHandle::<GolemAgentGetAllAgentTypes, NotCancellable>::start(
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
                    .agent_types_service()
                    .get_all(
                        self.owned_agent_id.environment_id,
                        self.owned_agent_id.agent_id.component_id,
                        self.state.component_metadata.revision,
                    )
                    .await
                    .map_err(|err| err.to_string());
                match handle
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            handle
                .complete(self, HostResponseGolemAgentAgentTypes { result })
                .await?
        };

        match response.result {
            Ok(result) => Ok(result.into_iter().map(|r| r.into()).collect()),
            Err(err) => Err(anyhow!(err)),
        }
    }

    async fn get_agent_type(
        &mut self,
        agent_type_name: String,
    ) -> anyhow::Result<Option<RegisteredAgentType>> {
        let agent_type_name = AgentTypeName(agent_type_name);
        let mut handle = CallHandle::<GolemAgentGetAgentType, NotCancellable>::start(
            self,
            HostRequestGolemAgentGetAgentType {
                agent_type_name: agent_type_name.clone(),
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

            let component_revision = self.state.component_metadata.revision;
            let result = loop {
                let result = self
                    .agent_types_service()
                    .get(
                        self.owned_agent_id.environment_id,
                        self.owned_agent_id.agent_id.component_id,
                        component_revision,
                        &agent_type_name,
                    )
                    .await
                    .map_err(|err| err.to_string());
                match handle
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            handle
                .complete(self, HostResponseGolemAgentAgentType { result })
                .await?
        };

        match response.result {
            Ok(result) => Ok(result.map(|r| r.into())),
            Err(err) => Err(anyhow!(err)),
        }
    }

    async fn make_agent_id(
        &mut self,
        agent_type_name: String,
        input: DataValue,
        phantom_id: Option<golem_wasm::Uuid>,
    ) -> anyhow::Result<Result<String, AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "make_agent_id");

        if let Some(agent_type) = self.get_agent_type(agent_type_name.clone()).await? {
            match golem_common::model::agent::DataValue::try_from_bindings(
                input,
                agent_type.agent_type.constructor.input_schema,
            ) {
                Ok(input) => {
                    let agent_id = LegacyParsedAgentId::new(
                        AgentTypeName(agent_type_name),
                        input,
                        phantom_id.map(|id| id.into()),
                    )
                    .map_err(|e| anyhow::anyhow!("{e}"))?;
                    Ok(Ok(agent_id.to_string()))
                }
                Err(err) => Ok(Err(AgentError::InvalidInput(err))),
            }
        } else {
            Ok(Err(AgentError::InvalidType(agent_type_name)))
        }
    }

    async fn parse_agent_id(
        &mut self,
        agent_id: String,
    ) -> anyhow::Result<Result<(String, DataValue, Option<golem_wasm::Uuid>), AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "parse_agent_id");

        let component_metadata = &self.component_metadata().metadata;
        match LegacyParsedAgentId::parse(agent_id, component_metadata) {
            Ok(agent_id) => Ok(Ok((
                agent_id.agent_type.to_string(),
                agent_id.parameters.into(),
                agent_id.phantom_id.map(|id| id.into()),
            ))),
            Err(error) => Ok(Err(AgentError::InvalidAgentId(error))),
        }
    }

    async fn create_webhook(
        &mut self,
        promise_id: crate::preview2::golem_api_1_x::host::PromiseId,
    ) -> anyhow::Result<String> {
        let promise_id: PromiseId = promise_id.clone().into();
        let mut handle = CallHandle::<GolemAgentCreateWebhook, NotCancellable>::start(
            self,
            HostRequestGolemApiPromiseId {
                promise_id: promise_id.clone(),
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

            if promise_id.agent_id.component_id != self.state.component_metadata.id {
                let error = "Attempted to create a webhook for a promise not created by the current component".to_string();
                break 'result handle
                    .complete(
                        self,
                        HostResponseGolemAgentWebhookUrl { result: Err(error) },
                    )
                    .await?;
            }

            let agent_type = match self.state.agent_id.as_ref() {
                Some(agent_id) => agent_id.agent_type.clone(),
                None => {
                    let error = "Creating webhook urls is only supported for agentic components"
                        .to_string();
                    break 'result handle
                        .complete(
                            self,
                            HostResponseGolemAgentWebhookUrl { result: Err(error) },
                        )
                        .await?;
                }
            };

            let webhook_url = match self
                .state
                .agent_webhooks_service
                .get_agent_webhook_url_for_promise(
                    self.state.component_metadata.environment_id,
                    &agent_type,
                    &promise_id,
                )
                .await
            {
                Ok(webhook_url) => webhook_url,
                Err(err) => {
                    handle.abandon_for_trap();
                    return Err(err.into());
                }
            };

            let Some(webhook_url) = webhook_url else {
                handle.abandon_for_trap();
                return Err(anyhow!(
                    "Agent is not currently deployed as part of an http api. Only deployed agents can create webhook urls"
                ));
            };

            handle
                .complete(
                    self,
                    HostResponseGolemAgentWebhookUrl {
                        result: Ok(webhook_url),
                    },
                )
                .await?
        };

        response.result.map_err(|e| anyhow!(e))
    }

    async fn get_config_value(
        &mut self,
        path: Vec<String>,
        expected_type: WitType,
    ) -> anyhow::Result<WitValue> {
        let path_str = path.join(".");
        tracing::debug!("Agent getting config value for key {path_str}");

        let agent_id = self
            .parsed_agent_id()
            .ok_or_else(|| anyhow!("only agentic workers can access agent config"))?;

        // The guest passes the expected type as a `WitType` through
        // wit-bindgen. Lift it to a `SchemaType` once so the resolvers
        // below operate on a single type representation.
        let expected_type_legacy = AnalysedType::from(expected_type);
        let expected_type = analysed_type_to_schema_type_inline(&expected_type_legacy)
            .map_err(|e| anyhow!(
                "Expected config type for path {path_str} is not representable as SchemaType: {e}"
            ))?;

        let agent_type = self
            .component_metadata()
            .metadata
            .find_agent_type_by_name(&agent_id.agent_type)
            .expect("Active agent type of agent was not declared in component metadata");

        let declaration = agent_type.config.iter().find(|c| c.path == path);

        let declaration_value_type = declaration
            .map(|d| {
                analysed_type_to_schema_type_inline(&d.value_type).map_err(|e| {
                    anyhow!(
                        "Declared config type for path {path_str} is not representable as SchemaType: {e}"
                    )
                })
            })
            .transpose()?;

        match declaration {
            // Allow reading undeclared optional config keys so that
            // newer agents can run against older component schemas.
            None if matches!(expected_type, SchemaType::Option { .. }) => {
                Ok(WitValue::builder().option_none())
            }
            None => Err(anyhow!("No config declared for path {path_str}")),
            Some(AgentConfigDeclaration {
                source: AgentConfigSource::Local,
                ..
            }) => self.resolve_local_config(
                &path,
                &path_str,
                &expected_type,
                declaration_value_type
                    .as_ref()
                    .expect("existing config declaration must have converted value type"),
            ),
            Some(AgentConfigDeclaration {
                source: AgentConfigSource::Secret,
                ..
            }) => {
                self.resolve_secret_config(
                    path,
                    &path_str,
                    expected_type,
                    declaration_value_type
                        .as_ref()
                        .expect("existing config declaration must have converted value type"),
                )
                .await
            }
        }
    }
}
