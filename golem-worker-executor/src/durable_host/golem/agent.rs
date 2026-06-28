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
use chrono::Utc;
use golem_common::model::PromiseId;
use golem_common::model::agent::{
    AgentConfigSource, AgentTypeName, ParsedAgentId, typed_constructor_parameters,
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
use golem_common::schema::agent::wit::{encode_registered_agent_type, wire};
use golem_common::schema::agent::{AgentTypeSchema, RegisteredAgentTypeSchema};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::graph::TypedSchemaValue;
use golem_common::schema::schema_type::{NamedFieldType, SchemaType};
use golem_common::schema::schema_value::{SchemaValue, SecretValuePayload};
use golem_common::schema::validation::subtyping::is_equivalent_cross_graph;
use golem_common::schema::validation::value::validate_value;
use golem_schema::schema::wit::wire as core_wire;
use golem_schema::schema::wit::{
    decode_graph, decode_value_with, encode_typed, encode_value, encode_value_with,
    reject_quota_handles_in_value_tree,
};

fn encode_registered_agent_type_schema_wire(
    schema: RegisteredAgentTypeSchema,
) -> anyhow::Result<wire::RegisteredAgentType> {
    encode_registered_agent_type(&schema)
        .map_err(|e| anyhow!("Failed to encode agent type to wire form: {e}"))
}

/// Convert a guest-supplied `golem:core/types@2.0.0` `schema-value-tree`
/// (whose root encodes the constructor's parameter list) into the
/// schema-native constructor parameter payload stored in [`ParsedAgentId`].
///
/// The decoded value is validated directly against the constructor's
/// [`AgentTypeSchema`] before being paired with that same schema graph. This
/// stays on the hot path without lowering through legacy value carriers.
pub(crate) fn schema_value_tree_to_typed_constructor_parameters<Ctx: WorkerCtx>(
    input: core_wire::SchemaValueTree,
    agent_type: &AgentTypeSchema,
    resolver: &mut DurableWorkerCtx<Ctx>,
) -> Result<TypedSchemaValue, String> {
    // The input is a guest-owned value tree, so it is decoded through the
    // resolver-aware path: this consumes any owned `quota-token` handles it
    // carries (lifting them to trusted snapshots) so none leak. Constructor
    // parameters never legally contain a quota token, so such a value is then
    // rejected by schema validation below.
    let schema_value =
        decode_value_with(input, resolver).map_err(|e| format!("invalid input value tree: {e}"))?;
    validate_constructor_input_value(&schema_value, agent_type)?;
    Ok(typed_constructor_parameters(agent_type, schema_value))
}

fn validate_constructor_input_value(
    input: &SchemaValue,
    agent_type: &AgentTypeSchema,
) -> Result<(), String> {
    let SchemaValue::Record { fields } = input else {
        return Err("expected input parameter record".to_string());
    };

    let fields_schema = agent_type.constructor.input_schema.fields();
    if fields.len() != fields_schema.len() {
        return Err(format!(
            "expected {} parameters, got {}",
            fields_schema.len(),
            fields.len()
        ));
    }

    let record_type = SchemaType::record(
        fields_schema
            .iter()
            .map(|field| NamedFieldType {
                name: field.name.clone(),
                body: field.schema.clone(),
                metadata: field.metadata.clone(),
            })
            .collect(),
    );

    validate_value(&agent_type.schema, &record_type, input).map_err(|errors| {
        format!(
            "invalid input parameter value: {}",
            errors
                .into_iter()
                .map(|error| error.to_string())
                .collect::<Vec<_>>()
                .join("; ")
        )
    })
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    /// Resolve a local agent-config value.
    fn resolve_local_config(
        &self,
        key: &[String],
        key_str: &str,
        expected_graph: &SchemaGraph,
        expected_type: &SchemaType,
        declared_graph: &SchemaGraph,
        declared_type: &SchemaType,
    ) -> anyhow::Result<SchemaValue> {
        let config_value = self.state.agent_config.get(key);

        // Future automatic-update transforms belong here, where both
        // the component-declared type and the guest-expected type are
        // available together with the stored local config value.
        if !schema_types_compatible(declared_graph, declared_type, expected_graph, expected_type) {
            return Err(anyhow!(
                "declared and expected type for config key {key_str} are not compatible"
            ));
        }

        match (
            resolve_schema_ref(expected_graph, expected_type),
            config_value,
        ) {
            (SchemaType::Option { .. }, None) => Ok(SchemaValue::Option { inner: None }),
            // The stored local config is already a schema-native typed value.
            (_, Some(stored)) => Ok(stored.value().clone()),
            (_, None) => Err(anyhow!("required config key {key_str} is missing value")),
        }
    }

    /// Resolve a secret-backed agent-config value. Stored [`AgentSecret`]
    /// schemas describe the plaintext payload type while guest config expects
    /// an opaque `secret<T>` handle. Compatibility is checked against the
    /// expected secret's inner type before a durable secret handle is returned.
    async fn resolve_secret_config(
        &mut self,
        path: Vec<String>,
        path_str: &str,
        expected_graph: SchemaGraph,
        declared_graph: &SchemaGraph,
        declared_type: &SchemaType,
    ) -> anyhow::Result<SchemaValue> {
        // Future automatic-update transforms belong here, where both
        // the component-declared type and the guest-expected type are
        // available together with the resolved secret metadata/value.
        // This deterministic validation must happen before opening the
        // durable function; replay must not be able to skip it and return
        // a previously persisted config value.
        if !schema_types_compatible(
            declared_graph,
            declared_type,
            &expected_graph,
            &expected_graph.root,
        ) {
            return Err(anyhow!(
                "declared and expected type for secret key {path_str} are not compatible"
            ));
        }

        let expected_root = resolve_schema_ref(&expected_graph, &expected_graph.root);
        let (optional, expected_secret_spec) = match expected_root {
            SchemaType::Option { inner, .. } => match resolve_schema_ref(&expected_graph, inner) {
                SchemaType::Secret { spec, .. } => (true, spec.clone()),
                _ => {
                    return Err(anyhow!(
                        "expected type for secret key {path_str} must be secret or option<secret>"
                    ));
                }
            },
            SchemaType::Secret { spec, .. } => (false, spec.clone()),
            _ => {
                return Err(anyhow!(
                    "expected type for secret key {path_str} must be secret or option<secret>"
                ));
            }
        };

        let handle = CallHandle::<GolemAgentGetConfigValue, NotCancellable>::start(
            self,
            HostRequestGolemAgentGetConfigValue {
                path: path.clone(),
                expected_type: expected_graph.clone(),
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

                let result_schema = match agent_secret {
                    None if optional => SchemaValue::Option { inner: None },
                    None => {
                        return Err(anyhow!(
                            "No secret for key {path_str} exists in environment"
                        ));
                    }
                    Some(secret) => {
                        let stored_secret_inner = resolve_schema_ref(
                            &secret.secret_type,
                            &secret.secret_type.root,
                        );
                        if !schema_types_compatible(
                            &secret.secret_type,
                            stored_secret_inner,
                            &expected_graph,
                            &expected_secret_spec.inner,
                        )
                        {
                            return Err(anyhow!(
                                "declared and expected type for config key {path_str} are not compatible"
                            ));
                        }

                        if let Some(secret_value) = &secret.secret_value {
                            validate_value(&secret.secret_type, stored_secret_inner, secret_value)
                                .map_err(|errors| {
                                    anyhow!(
                                        "secret key {path_str} has invalid stored value: {}",
                                        errors
                                            .into_iter()
                                            .map(|error| error.to_string())
                                            .collect::<Vec<_>>()
                                            .join("; ")
                                    )
                                })?;
                        }

                        if secret.secret_value.is_none() {
                            if optional {
                                return Ok(HostResponseGolemAgentGetConfigValue {
                                    result: SchemaValue::Option { inner: None },
                                });
                            }
                            return Err(anyhow!("Secret key {path_str} is missing value"));
                        }

                        let secret_value = SchemaValue::Secret(SecretValuePayload {
                            secret_id: secret.id.into(),
                            config_key: Some(secret.path.0.clone()),
                            version: secret.revision.get(),
                            resolved_at: Utc::now(),
                            category: expected_secret_spec.category.clone(),
                        });

                        if optional {
                            SchemaValue::Option {
                                inner: Some(Box::new(secret_value)),
                            }
                        } else {
                            secret_value
                        }
                    }
                };

                Ok(HostResponseGolemAgentGetConfigValue {
                    result: result_schema,
                })
            })
            .await?;

        validate_secret_config_result_shape(path_str, optional, &persisted.result)?;

        Ok(persisted.result)
    }

    /// Durable lookup of all registered agent types, returning the schema-native
    /// [`RegisteredAgentTypeSchema`] model directly. The WIT wire form is
    /// produced at the host-import boundary in [`Host::get_all_agent_types`].
    pub(crate) async fn get_all_agent_types_model(
        &mut self,
    ) -> anyhow::Result<Vec<RegisteredAgentTypeSchema>> {
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
            Ok(result) => Ok(result),
            Err(err) => Err(anyhow!(err)),
        }
    }

    /// Durable lookup of a single registered agent type by name, returning the
    /// schema-native model persisted in the oplog.
    pub(crate) async fn get_agent_type_schema_model(
        &mut self,
        agent_type_name: AgentTypeName,
    ) -> anyhow::Result<Option<RegisteredAgentTypeSchema>> {
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
            Ok(result) => Ok(result),
            Err(err) => Err(anyhow!(err)),
        }
    }
}

/// Cross-graph structural type equality, resolving any [`SchemaType::Ref`]
/// nodes on each side against its own graph. Used to compare the
/// component-declared config type, the guest-supplied expected type, and the
/// stored secret type, each of which carries its own [`SchemaGraph`].
fn schema_types_compatible(
    graph_a: &SchemaGraph,
    type_a: &SchemaType,
    graph_b: &SchemaGraph,
    type_b: &SchemaType,
) -> bool {
    is_equivalent_cross_graph(graph_a, type_a, graph_b, type_b)
}

/// Follow a chain of [`SchemaType::Ref`] nodes in `graph` to the first
/// non-`Ref` structural type. Cycle-guarded; returns the last seen type if a
/// ref cannot be resolved or a cycle is detected.
fn resolve_schema_ref<'a>(graph: &'a SchemaGraph, mut ty: &'a SchemaType) -> &'a SchemaType {
    let mut seen = std::collections::HashSet::new();
    while let SchemaType::Ref { id, .. } = ty {
        if !seen.insert(id.clone()) {
            break;
        }
        match graph.lookup(id) {
            Some(def) => ty = &def.body,
            None => break,
        }
    }
    ty
}

fn validate_secret_config_result_shape(
    path_str: &str,
    optional: bool,
    value: &SchemaValue,
) -> anyhow::Result<()> {
    match (optional, value) {
        (false, SchemaValue::Secret(_)) => Ok(()),
        (true, SchemaValue::Option { inner: None }) => Ok(()),
        (true, SchemaValue::Option { inner: Some(inner) })
            if matches!(inner.as_ref(), SchemaValue::Secret(_)) =>
        {
            Ok(())
        }
        _ => Err(anyhow!(
            "persisted secret config response for key {path_str} has invalid shape"
        )),
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn get_all_agent_types(&mut self) -> anyhow::Result<Vec<wire::RegisteredAgentType>> {
        self.get_all_agent_types_model()
            .await?
            .into_iter()
            .map(encode_registered_agent_type_schema_wire)
            .collect()
    }

    async fn get_agent_type(
        &mut self,
        agent_type_name: String,
    ) -> anyhow::Result<Option<wire::RegisteredAgentType>> {
        self.get_agent_type_schema_model(AgentTypeName(agent_type_name))
            .await?
            .map(encode_registered_agent_type_schema_wire)
            .transpose()
    }

    async fn make_agent_id(
        &mut self,
        agent_type_name: String,
        input: core_wire::SchemaValueTree,
        phantom_id: Option<core_wire::Uuid>,
    ) -> anyhow::Result<Result<String, wire::AgentError>> {
        DurabilityHost::observe_function_call(self, "golem_agent", "make_agent_id");

        if let Some(registered) = self
            .get_agent_type_schema_model(AgentTypeName(agent_type_name.clone()))
            .await?
        {
            match schema_value_tree_to_typed_constructor_parameters(
                input,
                &registered.agent_type,
                self,
            ) {
                Ok(input) => {
                    let agent_id = ParsedAgentId::try_new(
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
            // Unknown agent type: this returns a non-trapping `AgentError`, so
            // the instance (and its resource table) stay alive. Drain any owned
            // `quota-token` handle the guest smuggled into the unused constructor
            // input so it cannot leak. Constructor parameters never legally carry
            // a quota token, so dropping them here is correct.
            let _ = reject_quota_handles_in_value_tree(input, self);
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
        match ParsedAgentId::parse(agent_id, component_metadata) {
            Ok(agent_id) => {
                let wire_typed = encode_typed(&agent_id.parameters)
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

    async fn create_webhook(&mut self, promise_id: core_wire::PromiseId) -> anyhow::Result<String> {
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
        expected: core_wire::SchemaGraph,
    ) -> anyhow::Result<core_wire::SchemaValueTree> {
        let path_str = path.join(".");
        tracing::debug!("Agent getting config value for key {path_str}");

        let agent_id = self
            .parsed_agent_id()
            .ok_or_else(|| anyhow!("only agentic workers can access agent config"))?;

        // The guest passes the expected type as a self-contained
        // `schema-graph`; the resolvers below operate directly on it, following
        // any [`SchemaType::Ref`] against its `defs`.
        let expected_graph = decode_graph(&expected).map_err(|e| {
            anyhow!("Expected config type for path {path_str} is not a valid schema graph: {e}")
        })?;

        let agent_type = self
            .component_metadata()
            .metadata
            .find_agent_type_by_name(&agent_id.agent_type)
            .expect("Active agent type of agent was not declared in component metadata");

        let declaration = agent_type.config.iter().find(|c| c.path == path);

        let declaration_value_type = declaration.map(|d| d.value_type.clone());

        let (schema_value, uses_resolver): (SchemaValue, bool) = match declaration {
            // Allow reading undeclared optional config keys so that
            // newer agents can run against older component schemas.
            None if matches!(
                resolve_schema_ref(&expected_graph, &expected_graph.root),
                SchemaType::Option { .. }
            ) =>
            {
                (SchemaValue::Option { inner: None }, false)
            }
            None => return Err(anyhow!("No config declared for path {path_str}")),
            Some(declaration) if declaration.source == AgentConfigSource::Local => (
                self.resolve_local_config(
                    &path,
                    &path_str,
                    &expected_graph,
                    &expected_graph.root,
                    &agent_type.schema,
                    declaration_value_type
                        .as_ref()
                        .expect("existing config declaration must have a value type"),
                )?,
                false,
            ),
            Some(declaration) if declaration.source == AgentConfigSource::Secret => (
                self.resolve_secret_config(
                    path,
                    &path_str,
                    expected_graph,
                    &agent_type.schema,
                    declaration_value_type
                        .as_ref()
                        .expect("existing config declaration must have a value type"),
                )
                .await?,
                true,
            ),
            Some(declaration) => {
                return Err(anyhow!(
                    "Unsupported config source {:?} for path {path_str}",
                    declaration.source
                ));
            }
        };

        // Encode the schema-native value into the wire value tree returned
        // across the `golem:agent/host@2.0.0` boundary. Secret-backed config is
        // the only capability-minting source here; local config still uses the
        // pure encoder so a quota token remains a schema/config error.
        if uses_resolver {
            encode_value_with(&schema_value, self)
        } else {
            encode_value(&schema_value)
        }
        .map_err(|e| anyhow!("Failed to encode config value to wire form: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    fn secret_snapshot_value() -> SchemaValue {
        SchemaValue::Secret(SecretValuePayload {
            secret_id: uuid::Uuid::nil(),
            config_key: None,
            version: 1,
            resolved_at: Utc::now(),
            category: None,
        })
    }

    #[test]
    fn secret_config_replay_shape_rejects_plaintext_values() {
        validate_secret_config_result_shape("apiKey", false, &secret_snapshot_value()).unwrap();
        validate_secret_config_result_shape(
            "apiKey",
            true,
            &SchemaValue::Option {
                inner: Some(Box::new(secret_snapshot_value())),
            },
        )
        .unwrap();
        validate_secret_config_result_shape("apiKey", true, &SchemaValue::Option { inner: None })
            .unwrap();

        validate_secret_config_result_shape(
            "apiKey",
            false,
            &SchemaValue::String("plaintext".to_string()),
        )
        .expect_err("required secret config replay must not accept plaintext");
        validate_secret_config_result_shape(
            "apiKey",
            true,
            &SchemaValue::Option {
                inner: Some(Box::new(SchemaValue::String("plaintext".to_string()))),
            },
        )
        .expect_err("optional secret config replay must not accept plaintext");
    }
}
