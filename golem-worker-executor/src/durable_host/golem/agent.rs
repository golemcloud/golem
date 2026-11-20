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

use crate::durable_host::authorization::targets::{
    agent_method_target, agent_owner, config_segments_target,
};
use crate::durable_host::concurrent::{
    AccessClaimOptions, CallReplayOutcome, DurableCallSession, NotCancellable, ResolvedCall,
    SharedDurableContext, authorize_live_permissions_at_serialized_access,
};
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::secrets::secret_hold_target_for_path;
use crate::durable_host::{DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::agent::host::{ConfigValueError, Host, RpcError, WebhookError};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use chrono::Utc;
use golem_common::model::agent::{
    AgentConfigSource, AgentTypeName, RegisteredAgentType, ResolvedOwnerContext,
    typed_constructor_parameters,
};
use golem_common::model::agent_config::CanonicalAgentConfigPath;
use golem_common::model::agent_secret::CanonicalAgentSecretPath;
use golem_common::model::card::AgentVerb;
use golem_common::model::oplog::host_functions::{
    GolemAgentCreateWebhook, GolemAgentGetAgentType, GolemAgentGetAgentTypeByAgentId,
    GolemAgentGetAllAgentTypes, GolemAgentGetConfigValue,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestGolemAgentGetAgentType,
    HostRequestGolemAgentGetAgentTypeByAgentId, HostRequestGolemAgentGetConfigValue,
    HostRequestGolemApiPromiseId, HostRequestNoInput, HostResponseGolemAgentAgentType,
    HostResponseGolemAgentAgentTypes, HostResponseGolemAgentGetConfigValue,
    HostResponseGolemAgentWebhookUrl,
};
use golem_common::model::{AgentId, PromiseId};
use golem_common::schema::agent::wit::{encode_registered_agent_type, wire};
use golem_common::schema::agent::{AgentTypeSchema, RegisteredAgentTypeSchema};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::graph::TypedSchemaValue;
use golem_common::schema::schema_type::{NamedFieldType, SchemaType};
use golem_common::schema::schema_value::{SchemaValue, SecretValuePayload};
use golem_common::schema::validation::subtyping::is_equivalent_cross_graph;
use golem_common::schema::validation::value::validate_value;
use golem_schema::schema::wit::wire as core_wire;
use golem_schema::schema::wit::{decode_graph, decode_value_with, encode_value, encode_value_with};

const WEBHOOK_PERMISSION_DENIED: &str = "permission denied";

fn webhook_error(error: String) -> WebhookError {
    if error == WEBHOOK_PERMISSION_DENIED {
        WebhookError::PermissionDenied
    } else {
        WebhookError::InternalError(error)
    }
}

fn encode_registered_agent_type_schema_wire(
    schema: RegisteredAgentTypeSchema,
) -> anyhow::Result<wire::RegisteredAgentType> {
    encode_registered_agent_type(&schema)
        .map_err(|e| anyhow!("Failed to encode agent type to wire form: {e}"))
}

fn encode_registered_agent_type_wire(
    registered: RegisteredAgentType,
) -> anyhow::Result<wire::RegisteredAgentType> {
    encode_registered_agent_type_schema_wire(RegisteredAgentTypeSchema {
        agent_type: registered.agent_type,
        implemented_by: registered.implemented_by,
    })
}

/// Convert a guest-supplied `golem:core/types@2.0.0` `schema-value-tree`
/// (whose root encodes the constructor's parameter list) into the
/// schema-native constructor parameter payload stored in [`ParsedAgentKey`].
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

    let fields_schema: Vec<_> = agent_type
        .constructor
        .input_schema
        .fields()
        .iter()
        .filter(|field| {
            matches!(
                field.source,
                golem_common::schema::agent::FieldSource::UserSupplied
            )
        })
        .collect();
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
    async fn config_value_denied(
        &mut self,
        path: &[String],
        check_wallet: bool,
    ) -> anyhow::Result<bool> {
        let is_secret_config = self.is_secret_config(path);
        Ok(
            match self.config_permission_targets(path, is_secret_config, check_wallet) {
                Ok(targets) if targets.is_empty() => false,
                Ok(targets) => self.authorize_live_permissions(&targets).await?.is_err(),
                Err(()) => true,
            },
        )
    }

    fn config_permission_targets(
        &self,
        path: &[String],
        is_secret_config: bool,
        check_wallet: bool,
    ) -> Result<Vec<golem_common::model::card::PermissionTarget>, ()> {
        let binding_denied = self.entity_invocation_scope().is_some_and(|scope| {
            let policy = scope.activation().policy();
            !policy
                .config_keys_readable()
                .contains(&CanonicalAgentConfigPath::from_path_in_unknown_casing(path))
                || (is_secret_config
                    && !policy
                        .secret_keys_readable()
                        .contains(&CanonicalAgentSecretPath::from_path_in_unknown_casing(path)))
        });
        if binding_denied {
            return Err(());
        }

        // Snapshot loading executes unpersisted calls without publishing live execution. Normal
        // tail continuation has already published liveness during durable-call resolution.
        if !check_wallet {
            return Ok(Vec::new());
        }

        config_segments_target(agent_owner(self), path)
            .map_err(|_| ())
            .and_then(|target| {
                let mut targets = vec![target];
                if is_secret_config {
                    targets.push(secret_hold_target_for_path(self, path).map_err(|_| ())?);
                }
                Ok(targets)
            })
    }

    fn is_secret_config(&self, path: &[String]) -> bool {
        match self.owner_context() {
            ResolvedOwnerContext::Agent(agent_id) => self
                .owner_component_metadata()
                .metadata
                .find_agent_type_by_name(&agent_id.agent_type)
                .is_some_and(|agent_type| {
                    agent_type.config.iter().any(|entry| {
                        entry.path == path && entry.source == AgentConfigSource::Secret
                    })
                }),
            ResolvedOwnerContext::ComponentBaseline | ResolvedOwnerContext::OplogProcessor => self
                .owner_component_metadata()
                .metadata
                .config_schema()
                .declarations
                .iter()
                .any(|entry| entry.path == path && entry.source == AgentConfigSource::Secret),
        }
    }

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
    fn resolve_secret_config(
        &mut self,
        path: Vec<String>,
        path_str: &str,
        expected_graph: SchemaGraph,
        declared_graph: &SchemaGraph,
        declared_type: &SchemaType,
    ) -> impl Future<Output = anyhow::Result<SchemaValue>> + Send + 'static + use<Ctx> {
        let service = self.state.environment_state_service.clone();
        let environment_id = self.owner_component_metadata().environment_id;
        let path_str = path_str.to_string();
        let declared_graph = declared_graph.clone();
        let declared_type = declared_type.clone();
        async move {
            let canonical_path = CanonicalAgentSecretPath::from_path_in_unknown_casing(&path);

            // Future automatic-update transforms belong here, where both
            // the component-declared type and the guest-expected type are
            // available together with the resolved secret metadata/value.
            // This deterministic validation must happen before opening the
            // durable function; replay must not be able to skip it and return
            // a previously persisted config value.
            if !schema_types_compatible(
                &declared_graph,
                &declared_type,
                &expected_graph,
                &expected_graph.root,
            ) {
                return Err(anyhow!(
                    "declared and expected type for secret key {path_str} are not compatible"
                ));
            }

            let expected_root = resolve_schema_ref(&expected_graph, &expected_graph.root);
            let (optional, expected_secret_spec) = match expected_root {
                SchemaType::Option { inner, .. } => {
                    match resolve_schema_ref(&expected_graph, inner) {
                        SchemaType::Secret { spec, .. } => (true, spec.clone()),
                        _ => {
                            return Err(anyhow!(
                                "expected type for secret key {path_str} must be secret or option<secret>"
                            ));
                        }
                    }
                }
                SchemaType::Secret { spec, .. } => (false, spec.clone()),
                _ => {
                    return Err(anyhow!(
                        "expected type for secret key {path_str} must be secret or option<secret>"
                    ));
                }
            };

            let persisted = {
                let agent_secrets = service.get_agent_secrets(environment_id).await?;

                let agent_secret = agent_secrets.get(&canonical_path);

                match agent_secret {
                    None if optional => SchemaValue::Option { inner: None },
                    None => {
                        return Err(anyhow!(
                            "No secret for key {path_str} exists in environment"
                        ));
                    }
                    Some(secret) => {
                        let stored_secret_inner =
                            resolve_schema_ref(&secret.secret_type, &secret.secret_type.root);
                        if !schema_types_compatible(
                            &secret.secret_type,
                            stored_secret_inner,
                            &expected_graph,
                            &expected_secret_spec.inner,
                        ) {
                            return Err(anyhow!(
                                "declared and expected type for config key {path_str} are not compatible"
                            ));
                        }

                        if let Some(secret_value) = &secret.secret_value {
                            // The validation errors can embed fragments of the
                            // stored plaintext (e.g. an invalid URL value), so
                            // they are never surfaced; the message stays generic.
                            validate_value(&secret.secret_type, stored_secret_inner, secret_value)
                                .map_err(|_| {
                                    anyhow!("secret key {path_str} has invalid stored value")
                                })?;
                        }

                        if secret.secret_value.is_none() {
                            if optional {
                                return Ok(SchemaValue::Option { inner: None });
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
                }
            };

            validate_secret_config_result_shape(&path_str, optional, &persisted)?;

            Ok(persisted)
        }
    }

    fn resolve_config_value(
        &mut self,
        path: Vec<String>,
        expected_graph: SchemaGraph,
    ) -> futures::future::BoxFuture<'static, anyhow::Result<SchemaValue>> {
        let path_str = path.join(".");
        let (declarations, schema) = match self.owner_context() {
            ResolvedOwnerContext::Agent(agent_id) => {
                let agent_type = self
                    .owner_component_metadata()
                    .metadata
                    .find_agent_type_by_name(&agent_id.agent_type)
                    .expect("Active agent type of agent was not declared in component metadata");
                (agent_type.config, agent_type.schema)
            }
            ResolvedOwnerContext::ComponentBaseline | ResolvedOwnerContext::OplogProcessor => {
                let config_schema = self.owner_component_metadata().metadata.config_schema();
                (
                    config_schema.declarations.clone(),
                    config_schema.schema.clone(),
                )
            }
        };
        let result = match declarations.iter().find(|entry| entry.path == path) {
            None if matches!(
                resolve_schema_ref(&expected_graph, &expected_graph.root),
                SchemaType::Option { .. }
            ) =>
            {
                Ok(SchemaValue::Option { inner: None })
            }
            None => Err(anyhow!("No config declared for path {path_str}")),
            Some(declaration) if declaration.source == AgentConfigSource::Local => self
                .resolve_local_config(
                    &path,
                    &path_str,
                    &expected_graph,
                    &expected_graph.root,
                    &schema,
                    &declaration.value_type,
                ),
            Some(declaration) if declaration.source == AgentConfigSource::Secret => {
                return Box::pin(self.resolve_secret_config(
                    path,
                    &path_str,
                    expected_graph,
                    &schema,
                    &declaration.value_type,
                ));
            }
            Some(declaration) => Err(anyhow!(
                "Unsupported config source {:?} for path {path_str}",
                declaration.source
            )),
        };
        Box::pin(std::future::ready(result))
    }

    /// Durable lookup of all registered agent types, returning the schema-native
    /// [`RegisteredAgentTypeSchema`] model directly. The WIT wire form is
    /// produced at the host-import boundary in [`Host::get_all_agent_types`].
    pub(crate) async fn get_all_agent_types_model(
        &mut self,
    ) -> anyhow::Result<Vec<RegisteredAgentTypeSchema>> {
        let mut handle = DurableCallSession::<GolemAgentGetAllAgentTypes, NotCancellable>::start(
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

            let owner_component_id = self.owner_component_metadata().id;
            let result = loop {
                let result = self
                    .agent_types_service()
                    .get_all(
                        self.owned_agent_id.environment_id,
                        owner_component_id,
                        self.owner_component_metadata().revision,
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
    /// The deployed agent type of the name, with its type ID. The lookup is durable, so replay
    /// sees the type that the live call saw.
    pub(crate) async fn get_agent_type_schema_model(
        &mut self,
        agent_type_name: AgentTypeName,
    ) -> anyhow::Result<Option<RegisteredAgentType>> {
        let mut handle = DurableCallSession::<GolemAgentGetAgentType, NotCancellable>::start(
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

            let component_revision = self.owner_component_metadata().revision;
            let owner_component_id = self.owner_component_metadata().id;
            let result = loop {
                let result = self
                    .agent_types_service()
                    .get(
                        self.owned_agent_id.environment_id,
                        owner_component_id,
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
            handle.complete(self, agent_type_response(result)).await?
        };

        registered_agent_type_from_response(response)
    }

    /// The deployed agent type that the type tag of `agent_id` names, whether or not the agent
    /// exists. The lookup is durable.
    pub(crate) async fn get_agent_type_by_agent_id(
        &mut self,
        agent_id: AgentId,
    ) -> anyhow::Result<Option<RegisteredAgentType>> {
        let mut handle =
            DurableCallSession::<GolemAgentGetAgentTypeByAgentId, NotCancellable>::start(
                self,
                HostRequestGolemAgentGetAgentTypeByAgentId { agent_id },
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
                let result = match agent_id.type_id() {
                    Some(type_id) => self
                        .agent_types_service()
                        .get_by_type_id(self.owned_agent_id.environment_id, type_id)
                        .await
                        .map_err(|err| err.to_string()),
                    None => Ok(None),
                };
                match handle
                    .try_trigger_retry_or_loop(self, &result, |_| HostFailureKind::Transient)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };
            handle.complete(self, agent_type_response(result)).await?
        };

        registered_agent_type_from_response(response)
    }
}

fn agent_type_response(
    result: Result<Option<RegisteredAgentType>, String>,
) -> HostResponseGolemAgentAgentType {
    match result {
        Ok(Some(registered)) => HostResponseGolemAgentAgentType {
            type_id: Some(registered.type_id),
            result: Ok(Some(RegisteredAgentTypeSchema {
                agent_type: registered.agent_type,
                implemented_by: registered.implemented_by,
            })),
        },
        Ok(None) => HostResponseGolemAgentAgentType {
            result: Ok(None),
            type_id: None,
        },
        Err(error) => HostResponseGolemAgentAgentType {
            result: Err(error),
            type_id: None,
        },
    }
}

fn registered_agent_type_from_response(
    response: HostResponseGolemAgentAgentType,
) -> anyhow::Result<Option<RegisteredAgentType>> {
    match response.result.map_err(|err| anyhow!(err))? {
        None => Ok(None),
        Some(schema) => {
            let type_id = response.type_id.ok_or_else(|| {
                anyhow!(
                    "recorded agent type {} has no type ID",
                    schema.agent_type.type_name
                )
            })?;
            Ok(Some(RegisteredAgentType {
                type_id,
                agent_type: schema.agent_type,
                implemented_by: schema.implemented_by,
            }))
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
            .map(encode_registered_agent_type_wire)
            .transpose()
    }

    async fn get_agent_type_by_agent_id(
        &mut self,
        agent_id: core_wire::AgentId,
    ) -> anyhow::Result<Option<wire::RegisteredAgentType>> {
        DurableWorkerCtx::get_agent_type_by_agent_id(self, agent_id.into())
            .await?
            .map(encode_registered_agent_type_wire)
            .transpose()
    }

    async fn lookup_agent_id(
        &mut self,
        agent_type_name: String,
        input: core_wire::SchemaValueTree,
    ) -> anyhow::Result<Result<Option<core_wire::AgentId>, RpcError>> {
        self.lookup_keyed_agent_id(agent_type_name, input).await
    }

    async fn create_webhook(
        &mut self,
        promise_id: core_wire::PromiseId,
    ) -> anyhow::Result<Result<String, WebhookError>> {
        let promise_id: PromiseId = promise_id.into();
        let begun = DurableCallSession::<GolemAgentCreateWebhook, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let response = 'result: {
            let mut handle = match begun.resolve(self).await? {
                ResolvedCall::Live(begun) => {
                    let denied = match agent_method_target(
                        agent_owner(self),
                        AgentVerb::Invoke,
                        "create-webhook",
                    ) {
                        Ok(target) => self.authorize_live_permission(&target).await?.is_err(),
                        Err(_) => true,
                    };
                    let request = HostRequestGolemApiPromiseId {
                        promise_id: promise_id.clone(),
                    };
                    if denied {
                        break 'result begun
                            .start_live_denied(self, request, WEBHOOK_PERMISSION_DENIED.to_string())
                            .await?;
                    }
                    begun.start_live(self, request).await?
                }
                ResolvedCall::Replay(handle) => handle,
            };
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'result replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            if promise_id.agent_id.type_id() != self.agent_id().type_id() {
                let error = "Attempted to create a webhook for a promise not created by an agent of the current agent type".to_string();
                break 'result handle
                    .complete(
                        self,
                        HostResponseGolemAgentWebhookUrl { result: Err(error) },
                    )
                    .await?;
            }

            let agent_type = match self.state.owner_context.agent() {
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
                    self.owner_component_metadata().environment_id,
                    &agent_type,
                    &promise_id,
                )
                .await
            {
                Ok(webhook_url) => webhook_url,
                Err(err) => {
                    return Err(handle.trap(err));
                }
            };

            let Some(webhook_url) = webhook_url else {
                return Err(handle.trap(anyhow!(
                    "Agent is not currently deployed as part of an http api. Only deployed agents can create webhook urls"
                )));
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

        Ok(response.result.map_err(webhook_error))
    }

    async fn get_config_value(
        &mut self,
        path: Vec<String>,
        expected: core_wire::SchemaGraph,
    ) -> anyhow::Result<Result<core_wire::SchemaValueTree, ConfigValueError>> {
        let path_str = path.join(".");
        let expected_graph = decode_graph(&expected).map_err(|e| {
            anyhow!("Expected config type for path {path_str} is not a valid schema graph: {e}")
        })?;

        self.process_pending_replay_events().await?;
        let begun = DurableCallSession::<GolemAgentGetConfigValue, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;
        let (handle, denied) = match begun.resolve(self).await? {
            ResolvedCall::Replay(handle) => (handle, None),
            ResolvedCall::Live(begun) => {
                let denied = self
                    .config_value_denied(&path, self.state.is_live())
                    .await?;
                let handle = begun
                    .start_live(
                        self,
                        HostRequestGolemAgentGetConfigValue {
                            path: path.clone(),
                            expected_type: expected_graph.clone(),
                        },
                    )
                    .await?;
                (handle, Some(denied))
            }
        };

        let response = handle
            .run::<_, _, anyhow::Error>(self, async |ctx| {
                let denied = match denied {
                    Some(denied) => denied,
                    None => ctx.config_value_denied(&path, true).await?,
                };
                if denied {
                    return Ok(HostResponseGolemAgentGetConfigValue {
                        result: Err("permission denied".to_string()),
                    });
                }

                let schema_value = ctx
                    .resolve_config_value(path.clone(), expected_graph)
                    .await?;

                Ok(HostResponseGolemAgentGetConfigValue {
                    result: Ok(schema_value),
                })
            })
            .await?;

        let schema_value = match response {
            HostResponseGolemAgentGetConfigValue { result: Ok(value) } => value,
            HostResponseGolemAgentGetConfigValue { result: Err(_) } => {
                return Ok(Err(ConfigValueError::PermissionDenied));
            }
        };

        // Secret-backed config is the only capability-minting source here. Local config uses the
        // pure encoder so a quota token remains a schema/config error.
        let encoded = if self.is_secret_config(&path) {
            encode_value_with(&schema_value, self)
        } else {
            encode_value(&schema_value)
        }
        .map_err(|e| anyhow!("Failed to encode config value to wire form: {e}"))?;
        Ok(Ok(encoded))
    }
}

impl<Ctx: WorkerCtx> SharedDurableContext<Ctx> {
    async fn config_value_denied(
        &self,
        path: &[String],
        check_wallet: bool,
    ) -> Result<bool, golem_service_base::error::worker_executor::WorkerExecutorError> {
        let targets = self.with_ctx(|ctx| {
            ctx.config_permission_targets(path, ctx.is_secret_config(path), check_wallet)
        });
        Ok(match targets {
            Ok(targets) if targets.is_empty() => false,
            Ok(targets) => authorize_live_permissions_at_serialized_access(
                self,
                |ctx| ctx.durable_ctx_mut(),
                &targets,
            )
            .await?
            .is_err(),
            Err(()) => true,
        })
    }

    pub(crate) async fn get_config_value(
        &mut self,
        path: Vec<String>,
        expected: core_wire::SchemaGraph,
    ) -> anyhow::Result<Result<core_wire::SchemaValueTree, ConfigValueError>> {
        let expected_graph = decode_graph(&expected).map_err(|error| {
            anyhow!(
                "Expected config type for path {} is not a valid schema graph: {error}",
                path.join(".")
            )
        })?;
        let store = self.clone();
        let mut denied = None;
        let mut call = DurableCallSession::<GolemAgentGetConfigValue, NotCancellable>::start_context_with_options(
            self, |ctx| ctx.durable_ctx_mut(), DurableFunctionType::ReadRemote, AccessClaimOptions::default(),
            async |start| {
                if start.is_live {
                    denied = Some(store.config_value_denied(&path, store.with_ctx(|ctx| ctx.state.is_live())).await?);
                }
                Ok(HostRequestGolemAgentGetConfigValue { path: path.clone(), expected_type: expected_graph.clone() })
            },
        ).await?;
        let response = 'response: {
            if !call.is_live() {
                match call.replay_context(self).await? {
                    CallReplayOutcome::Replayed(response) => break 'response response,
                    CallReplayOutcome::Incomplete(live) => call = live,
                }
            }
            let result = async {
                let denied = match denied {
                    Some(denied) => denied,
                    None => self.config_value_denied(&path, true).await?,
                };
                if denied {
                    Ok(HostResponseGolemAgentGetConfigValue {
                        result: Err("permission denied".to_string()),
                    })
                } else {
                    let value = self
                        .with_ctx(|ctx| ctx.resolve_config_value(path.clone(), expected_graph))
                        .await?;
                    Ok::<_, anyhow::Error>(HostResponseGolemAgentGetConfigValue {
                        result: Ok(value),
                    })
                }
            }
            .await;
            let response = match result {
                Ok(response) => response,
                Err(error) => return Err(call.trap(error)),
            };
            call.complete(self, response).await?
        };
        let value = match response.result {
            Ok(value) => value,
            Err(_) => return Ok(Err(ConfigValueError::PermissionDenied)),
        };
        let encoded = if self.with_ctx(|ctx| ctx.is_secret_config(&path)) {
            self.with_ctx(|ctx| encode_value_with(&value, ctx))
        } else {
            encode_value(&value)
        }
        .map_err(|error| anyhow!("Failed to encode config value to wire form: {error}"))?;
        Ok(Ok(encoded))
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
