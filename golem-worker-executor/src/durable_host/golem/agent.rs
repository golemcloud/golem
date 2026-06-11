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

use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::agent::host::Host;
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use golem_common::model::PromiseId;
use golem_common::model::agent::{
    AgentConfigDeclaration, AgentConfigSource, AgentTypeName, DataSchema, DataValue,
    LegacyParsedAgentId, RegisteredAgentType,
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
use golem_common::schema::adapters::agent::{
    agent_type_to_schema, legacy_data_value_to_typed_schema_value, schema_agent_type_to_legacy,
};
use golem_common::schema::adapters::analysed_type::{
    analysed_type_to_schema_type_inline, schema_type_to_analysed_type,
};
use golem_common::schema::adapters::data_schema::data_schema_to_input_schema;
use golem_common::schema::adapters::untyped::typed_input_to_untyped_data_value;
use golem_common::schema::adapters::value::value_to_schema_value;
use golem_common::schema::agent::RegisteredAgentTypeSchema;
use golem_common::schema::agent::wit::{encode_registered_agent_type, wire};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::SchemaValue;
use golem_common::schema::validation::subtyping::is_assignable;
use golem_common::schema::wit::{decode_graph, decode_value, encode_typed, encode_value};
use golem_wasm::golem_core_2_0_x::types as core_wire;

/// Encode a canonical [`RegisteredAgentType`] into the schema-native
/// `golem:agent/common@2.0.0` wire form returned across the WIT boundary.
/// The service/oplog layers stay on the canonical model type; only the
/// host-import return value is schema-native.
fn encode_registered_agent_type_wire(
    registered: RegisteredAgentType,
) -> anyhow::Result<wire::RegisteredAgentType> {
    let agent_type = agent_type_to_schema(&registered.agent_type)
        .map_err(|e| anyhow!("Failed to convert agent type to schema form: {e}"))?;
    let schema = RegisteredAgentTypeSchema {
        agent_type,
        implemented_by: registered.implemented_by,
    };
    encode_registered_agent_type(&schema)
        .map_err(|e| anyhow!("Failed to encode agent type to wire form: {e}"))
}

/// Project the canonical [`RegisteredAgentType`] model into its schema-native
/// counterpart for schema-native oplog persistence.
fn registered_agent_type_to_schema(
    registered: RegisteredAgentType,
) -> Result<RegisteredAgentTypeSchema, String> {
    let agent_type = agent_type_to_schema(&registered.agent_type).map_err(|e| e.to_string())?;
    Ok(RegisteredAgentTypeSchema {
        agent_type,
        implemented_by: registered.implemented_by,
    })
}

/// Recover the canonical [`RegisteredAgentType`] model from the schema-native
/// form read back from the oplog (live persist round-trip or replay).
fn registered_agent_type_from_schema(
    registered: RegisteredAgentTypeSchema,
) -> Result<RegisteredAgentType, String> {
    let agent_type =
        schema_agent_type_to_legacy(&registered.agent_type).map_err(|e| e.to_string())?;
    Ok(RegisteredAgentType {
        agent_type,
        implemented_by: registered.implemented_by,
    })
}

/// Convert a guest-supplied `golem:core/types@2.0.0` `schema-value-tree`
/// (whose root encodes the constructor's parameter list) into the legacy
/// canonical [`DataValue`] used by the agent-id / service / oplog layers.
///
/// The chain is `SchemaValueTree -> SchemaValue::Record fields ->
/// UntypedDataValue -> DataValue`, driven by the constructor's declared
/// [`DataSchema`]. Failures are returned as a plain `String` so callers can
/// surface them as the agent-domain [`wire::AgentError::InvalidInput`]
/// (matching the previous `try_from_bindings` behaviour) rather than trapping.
pub(crate) fn schema_value_tree_to_data_value(
    input: &core_wire::SchemaValueTree,
    data_schema: &DataSchema,
) -> Result<DataValue, String> {
    let schema_value =
        decode_value(input).map_err(|e| format!("invalid input value tree: {e}"))?;
    let fields = match schema_value {
        SchemaValue::Record { fields } => fields,
        other => {
            return Err(format!(
                "expected a record-valued parameter list as input, got {other:?}"
            ));
        }
    };
    let input_schema = data_schema_to_input_schema(data_schema).map_err(|e| e.to_string())?;
    let untyped =
        typed_input_to_untyped_data_value(&input_schema, &fields).map_err(|e| e.to_string())?;
    DataValue::try_from_untyped(untyped, data_schema.clone())
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Resolve a local agent-config value.
    fn resolve_local_config(
        &self,
        key: &[String],
        key_str: &str,
        expected_type: &SchemaType,
        declared_type: &SchemaType,
    ) -> anyhow::Result<SchemaValue> {
        let config_value = self.state.agent_config.get(key);

        // Future automatic-update transforms belong here, where both
        // the component-declared type and the guest-expected type are
        // available together with the stored local config value.
        if declared_type != expected_type {
            return Err(anyhow!(
                "declared and expected type for config key {key_str} are not compatible"
            ));
        }

        match (expected_type, config_value) {
            (SchemaType::Option { .. }, None) => Ok(SchemaValue::Option { inner: None }),
            // The stored local config is a legacy typed value (its storage is
            // migrated in a later wave); project it into the schema-native
            // value the agent surface works in, driven by its stored type.
            (_, Some(stored)) => {
                value_to_schema_value(&stored.value, &stored.typ).map_err(|e| {
                    anyhow!(
                        "Local config value for key {key_str} is not representable as a schema value: {e}"
                    )
                })
            }
            (_, None) => Err(anyhow!("required config key {key_str} is missing value")),
        }
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
    ) -> anyhow::Result<SchemaValue> {
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

        let durability =
            Durability::<GolemAgentGetConfigValue>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        if durability.is_live() {
            let agent_secrets = self
                .state
                .environment_state_service
                .get_agent_secrets(self.state.component_metadata.environment_id)
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
                        "No secret for key {path_str} exists in environment"
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

            // The oplog payload now stores the resolved value schema-natively;
            // the guest-supplied `expected_type` is self-contained (no refs)
            // and is recorded as the request metadata.
            let persisted = durability
                .persist(
                    self,
                    HostRequestGolemAgentGetConfigValue {
                        path,
                        expected_type: expected_type.clone(),
                    },
                    HostResponseGolemAgentGetConfigValue {
                        result: result_schema,
                    },
                )
                .await?;

            Ok(persisted.result)
        } else {
            let replayed = durability.replay(self).await?;
            Ok(replayed.result)
        }
    }

    /// Durable lookup of all registered agent types, returning the canonical
    /// [`RegisteredAgentType`] model. The schema-native WIT wire form is
    /// produced only at the host-import boundary in [`Host::get_all_agent_types`].
    pub(crate) async fn get_all_agent_types_model(
        &mut self,
    ) -> anyhow::Result<Vec<RegisteredAgentType>> {
        let mut durability =
            Durability::<GolemAgentGetAllAgentTypes>::new(self, DurableFunctionType::ReadRemote)
                .await?;
        let result = if durability.is_live() {
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
                match durability
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.and_then(|types| {
                types
                    .into_iter()
                    .map(registered_agent_type_to_schema)
                    .collect::<Result<Vec<_>, String>>()
            });
            durability
                .persist(
                    self,
                    HostRequestNoInput {},
                    HostResponseGolemAgentAgentTypes { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(result) => result
                .into_iter()
                .map(registered_agent_type_from_schema)
                .collect::<Result<Vec<_>, String>>()
                .map_err(|err| anyhow!(err)),
            Err(err) => Err(anyhow!(err)),
        }
    }

    /// Durable lookup of a single registered agent type by name, returning the
    /// canonical [`RegisteredAgentType`] model. The schema-native WIT wire form
    /// is produced only at the host-import boundary in [`Host::get_agent_type`].
    pub(crate) async fn get_agent_type_model(
        &mut self,
        agent_type_name: AgentTypeName,
    ) -> anyhow::Result<Option<RegisteredAgentType>> {
        let mut durability =
            Durability::<GolemAgentGetAgentType>::new(self, DurableFunctionType::ReadRemote)
                .await?;
        let result = if durability.is_live() {
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
                match durability
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            let result = result.and_then(|maybe_type| {
                maybe_type.map(registered_agent_type_to_schema).transpose()
            });
            durability
                .persist(
                    self,
                    HostRequestGolemAgentGetAgentType { agent_type_name },
                    HostResponseGolemAgentAgentType { result },
                )
                .await
        } else {
            durability.replay(self).await
        }?;

        match result.result {
            Ok(result) => result
                .map(registered_agent_type_from_schema)
                .transpose()
                .map_err(|err| anyhow!(err)),
            Err(err) => Err(anyhow!(err)),
        }
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
    async fn get_all_agent_types(&mut self) -> anyhow::Result<Vec<wire::RegisteredAgentType>> {
        self.get_all_agent_types_model()
            .await?
            .into_iter()
            .map(encode_registered_agent_type_wire)
            .collect()
    }

    async fn get_agent_type(
        &mut self,
        agent_type_name: String,
    ) -> anyhow::Result<Option<wire::RegisteredAgentType>> {
        self.get_agent_type_model(AgentTypeName(agent_type_name))
            .await?
            .map(encode_registered_agent_type_wire)
            .transpose()
    }

    async fn make_agent_id(
        &mut self,
        agent_type_name: String,
        input: core_wire::SchemaValueTree,
        phantom_id: Option<core_wire::Uuid>,
    ) -> anyhow::Result<Result<String, wire::AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "make_agent_id");

        if let Some(agent_type) = self
            .get_agent_type_model(AgentTypeName(agent_type_name.clone()))
            .await?
        {
            match schema_value_tree_to_data_value(
                &input,
                &agent_type.agent_type.constructor.input_schema,
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
                Err(err) => Ok(Err(wire::AgentError::InvalidInput(err))),
            }
        } else {
            Ok(Err(wire::AgentError::InvalidType(agent_type_name)))
        }
    }

    async fn parse_agent_id(
        &mut self,
        agent_id: String,
    ) -> anyhow::Result<
        Result<(String, core_wire::TypedSchemaValue, Option<core_wire::Uuid>), wire::AgentError>,
    > {
        DurabilityHost::observe_function_call(self, "golem_agent", "parse_agent_id");

        let component_metadata = &self.component_metadata().metadata;
        match LegacyParsedAgentId::parse(agent_id, component_metadata) {
            Ok(agent_id) => {
                let typed = legacy_data_value_to_typed_schema_value(&agent_id.parameters)
                    .map_err(|e| anyhow!("Failed to convert agent id parameters to schema: {e}"))?;
                let wire_typed = encode_typed(&typed)
                    .map_err(|e| anyhow!("Failed to encode agent id parameters: {e}"))?;
                Ok(Ok((
                    agent_id.agent_type.to_string(),
                    wire_typed,
                    agent_id.phantom_id.map(|id| id.into()),
                )))
            }
            Err(error) => Ok(Err(wire::AgentError::InvalidAgentId(error))),
        }
    }

    async fn create_webhook(
        &mut self,
        promise_id: core_wire::PromiseId,
    ) -> anyhow::Result<String> {
        let durability =
            Durability::<GolemAgentCreateWebhook>::new(self, DurableFunctionType::ReadRemote)
                .await?;

        if durability.is_live() {
            let promise_id: PromiseId = promise_id.clone().into();
            if promise_id.agent_id.component_id != self.state.component_metadata.id {
                let error = "Attempted to create a webhook for a promise not created by the current component".to_string();
                let persisted = durability
                    .persist(
                        self,
                        HostRequestGolemApiPromiseId { promise_id },
                        HostResponseGolemAgentWebhookUrl { result: Err(error) },
                    )
                    .await?;
                return persisted.result.map_err(|e| anyhow!(e));
            }

            let Some(agent_id) = self.state.agent_id.as_ref() else {
                let error =
                    "Creating webhook urls is only supported for agentic components".to_string();
                let persisted = durability
                    .persist(
                        self,
                        HostRequestGolemApiPromiseId { promise_id },
                        HostResponseGolemAgentWebhookUrl { result: Err(error) },
                    )
                    .await?;
                return persisted.result.map_err(|e| anyhow!(e));
            };

            let webhook_url = self
                .state
                .agent_webhooks_service
                .get_agent_webhook_url_for_promise(
                    self.state.component_metadata.environment_id,
                    &agent_id.agent_type,
                    &promise_id,
                )
                .await?;

            let Some(webhook_url) = webhook_url else {
                return Err(anyhow!(
                    "Agent is not currently deployed as part of an http api. Only deployed agents can create webhook urls"
                ));
            };

            let persisted = durability
                .persist(
                    self,
                    HostRequestGolemApiPromiseId { promise_id },
                    HostResponseGolemAgentWebhookUrl {
                        result: Ok(webhook_url),
                    },
                )
                .await?;

            Ok(persisted.result.map_err(|e| anyhow!(e))?)
        } else {
            Ok(durability
                .replay(self)
                .await?
                .result
                .map_err(|e| anyhow!(e))?)
        }
    }

    async fn get_config_value(
        &mut self,
        path: Vec<String>,
        expected: core_wire::SchemaGraph,
    ) -> anyhow::Result<core_wire::SchemaValueTree> {
        let path_str = path.join(".");
        tracing::debug!("Agent getting config value for key {path_str}");

        let agent_id = self
            .parsed_agent_id()
            .ok_or_else(|| anyhow!("only agentic workers can access agent config"))?;

        // The guest passes the expected type as a `schema-graph`. Lift its
        // root to a single inline `SchemaType` (flattening any refs) so the
        // resolvers below operate on one schema-native type representation.
        let expected_graph = decode_graph(&expected).map_err(|e| {
            anyhow!("Expected config type for path {path_str} is not a valid schema graph: {e}")
        })?;
        let expected_type_flattened =
            schema_type_to_analysed_type(&expected_graph, &expected_graph.root).map_err(|e| {
                anyhow!(
                    "Expected config type for path {path_str} is not representable as a flat type: {e}"
                )
            })?;
        let expected_type =
            analysed_type_to_schema_type_inline(&expected_type_flattened).map_err(|e| {
                anyhow!(
                    "Expected config type for path {path_str} is not representable as SchemaType: {e}"
                )
            })?;

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

        let schema_value: SchemaValue = match declaration {
            // Allow reading undeclared optional config keys so that
            // newer agents can run against older component schemas.
            None if matches!(expected_type, SchemaType::Option { .. }) => {
                SchemaValue::Option { inner: None }
            }
            None => return Err(anyhow!("No config declared for path {path_str}")),
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
            )?,
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
                .await?
            }
        };

        // Encode the schema-native value into the wire value tree returned
        // across the `golem:agent/host@2.0.0` boundary.
        Ok(encode_value(&schema_value))
    }
}
