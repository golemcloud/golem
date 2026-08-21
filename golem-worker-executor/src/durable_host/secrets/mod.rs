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

pub mod types;

use crate::durable_host::authorization::targets::secret_target;
use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::durability::HostFailureKind;
use crate::durable_host::secrets::types::SecretEntry;
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, InternalRetryResult};
use crate::preview2::golem::secrets::reveal;
use crate::preview2::golem::secrets::types as secret_types;
use crate::preview2::golem::secrets::types::{
    SecretError, SecretId, SecretMetadata, SecretVersion,
};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use chrono::Utc;
use golem_common::model::agent_secret::{AgentSecretRevision, CanonicalAgentSecretPath};
use golem_common::model::card::PermissionTarget;
use golem_common::model::card::SecretVerb;
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::oplog::DurableFunctionType;
use golem_common::model::oplog::host_functions::GolemSecretsReveal;
use golem_common::model::oplog::payload::types::{
    SecretRevealAudit, SecretRevealError, SerializableDateTime,
};
use golem_common::model::oplog::payload::{HostRequestSecretReveal, HostResponseSecretRevealed};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::schema_value::{SchemaValue, SecretValuePayload};
use golem_common::schema::validation::subtyping::is_equivalent_cross_graph;
use golem_common::schema::validation::value::validate_value;
use golem_schema::schema::wit::wire::{HostSecret, SchemaValueTree};
use golem_schema::schema::wit::{SecretHandleRep, SecretResolver, decode_graph, encode_value_with};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::agent_secret::AgentSecret;
use wasmtime::component::Resource;

fn secret_entry<'a, Ctx: WorkerCtx>(
    ctx: &'a mut DurableWorkerCtx<Ctx>,
    secret: &Resource<SecretHandleRep>,
) -> anyhow::Result<&'a SecretEntry> {
    ctx.table()
        .get(secret)?
        .downcast_ref::<SecretEntry>()
        .ok_or_else(|| anyhow!("secret resource had unexpected payload type"))
}

fn secret_id_bytes(entry: &SecretEntry) -> SecretId {
    let mut bytes = Vec::with_capacity(24);
    bytes.extend_from_slice(entry.secret_id.0.as_bytes());
    bytes.extend_from_slice(&entry.pinned_revision.get().to_be_bytes());
    SecretId { bytes }
}

fn secret_version_bytes(revision: AgentSecretRevision) -> SecretVersion {
    SecretVersion {
        bytes: revision.get().to_be_bytes().to_vec(),
    }
}

fn secret_metadata(entry: &SecretEntry) -> SecretMetadata {
    SecretMetadata {
        config_key: entry.config_key.clone(),
        version: Some(secret_version_bytes(entry.pinned_revision)),
        resolved_at: SerializableDateTime::from(entry.resolved_at).into(),
        category: entry.category.clone(),
    }
}

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

fn secret_inner_type(graph: &SchemaGraph) -> &SchemaType {
    resolve_schema_ref(graph, &graph.root)
}

fn validate_expected_type(
    secret: &AgentSecret,
    expected_graph: &SchemaGraph,
) -> Result<(), SecretRevealError> {
    let pinned_inner = secret_inner_type(&secret.secret_type);

    if is_equivalent_cross_graph(
        &secret.secret_type,
        pinned_inner,
        expected_graph,
        &expected_graph.root,
    ) {
        Ok(())
    } else {
        Err(SecretRevealError::Unavailable(
            "expected reveal type is not compatible with the secret's pinned inner type"
                .to_string(),
        ))
    }
}

fn validate_secret_value(
    secret: &AgentSecret,
    value: &SchemaValue,
) -> Result<(), SecretRevealError> {
    let pinned_inner = secret_inner_type(&secret.secret_type);

    validate_value(&secret.secret_type, pinned_inner, value)
        .map_err(|_| SecretRevealError::Internal("stored secret value is invalid".to_string()))
}

fn canonical_config_key(
    entry: &SecretEntry,
) -> Result<CanonicalAgentSecretPath, SecretRevealError> {
    entry
        .config_key
        .clone()
        .map(CanonicalAgentSecretPath)
        .ok_or_else(|| {
            SecretRevealError::Unavailable(
                "secret handle is not backed by a versioned config key".to_string(),
            )
        })
}

fn canonical_secret_resource_segments(
    segments: Option<&[String]>,
) -> Result<String, SecretRevealError> {
    let segments = segments.ok_or_else(|| {
        SecretRevealError::Unavailable(
            "secret handle is not backed by a versioned config key".to_string(),
        )
    })?;
    if segments.is_empty()
        || segments
            .iter()
            .any(|segment| segment.is_empty() || segment.contains('.') || segment.contains('*'))
    {
        return Err(SecretRevealError::Internal(
            "secret handle has an invalid config key".to_string(),
        ));
    }
    Ok(segments.join("."))
}

fn canonical_secret_resource(entry: &SecretEntry) -> Result<String, SecretRevealError> {
    canonical_secret_resource_segments(entry.config_key.as_deref())
}

fn environment_owner<Ctx: WorkerCtx>(ctx: &DurableWorkerCtx<Ctx>) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: ctx.state.component_metadata.account_email.clone(),
        application: ctx.state.component_metadata.application_name.clone(),
        environment: ctx.state.component_metadata.environment_name.clone(),
    }
}

pub(crate) fn secret_hold_target_for_path<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    segments: &[String],
) -> Result<PermissionTarget, WorkerExecutorError> {
    let resource = canonical_secret_resource_segments(Some(segments))
        .map_err(|_| WorkerExecutorError::runtime("secret handle has an invalid config key"))?;
    secret_target(environment_owner(ctx), SecretVerb::Hold, &resource)
        .map_err(|_| WorkerExecutorError::runtime("secret handle has an invalid config key"))
}

fn secret_paths_for_value(value: &SchemaValue) -> Result<Vec<&[String]>, WorkerExecutorError> {
    fn collect<'a>(
        value: &'a SchemaValue,
        paths: &mut Vec<&'a [String]>,
    ) -> Result<(), WorkerExecutorError> {
        match value {
            SchemaValue::Record { fields }
            | SchemaValue::Tuple { elements: fields }
            | SchemaValue::List { elements: fields }
            | SchemaValue::FixedList { elements: fields } => {
                for value in fields {
                    collect(value, paths)?;
                }
            }
            SchemaValue::Variant(payload) => {
                if let Some(value) = &payload.payload {
                    collect(value, paths)?;
                }
            }
            SchemaValue::Map { entries } => {
                for (key, value) in entries {
                    collect(key, paths)?;
                    collect(value, paths)?;
                }
            }
            SchemaValue::Option { inner } => {
                if let Some(value) = inner {
                    collect(value, paths)?;
                }
            }
            SchemaValue::Result(result) => match result {
                golem_common::schema::schema_value::ResultValuePayload::Ok { value }
                | golem_common::schema::schema_value::ResultValuePayload::Err { value } => {
                    if let Some(value) = value {
                        collect(value, paths)?;
                    }
                }
            },
            SchemaValue::Union(payload) => collect(&payload.body, paths)?,
            SchemaValue::Secret(snapshot) => {
                SecretEntry::from_snapshot(snapshot)
                    .map_err(|_| WorkerExecutorError::runtime("invalid secret handle snapshot"))?;
                let path = snapshot.config_key.as_ref().ok_or_else(|| {
                    WorkerExecutorError::runtime(
                        "secret handle is not backed by a versioned config key",
                    )
                })?;
                paths.push(path);
            }
            _ => {}
        }
        Ok(())
    }

    let mut paths = Vec::new();
    collect(value, &mut paths)?;
    Ok(paths)
}

pub(crate) fn secret_hold_targets_for_value<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    value: &SchemaValue,
) -> Result<Vec<PermissionTarget>, WorkerExecutorError> {
    secret_paths_for_value(value)?
        .into_iter()
        .map(|path| secret_hold_target_for_path(ctx, path))
        .collect()
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    pub(crate) async fn secret_holds_allowed_for_value(
        &mut self,
        value: &SchemaValue,
    ) -> Result<bool, WorkerExecutorError> {
        if !self.state.is_live() {
            return Ok(true);
        }
        let targets = secret_hold_targets_for_value(self, value)?;
        if targets.is_empty() {
            Ok(true)
        } else {
            Ok(self.authorize_live_permissions(&targets).await?.is_ok())
        }
    }
}

fn permission_denied() -> SecretRevealError {
    SecretRevealError::Unavailable("secret permission denied".to_string())
}

fn classify_secret_revision_error(error: &WorkerExecutorError) -> HostFailureKind {
    match error {
        WorkerExecutorError::InvalidRequest { .. }
        | WorkerExecutorError::AgentAlreadyExists { .. }
        | WorkerExecutorError::AgentNotFound { .. }
        | WorkerExecutorError::PromiseNotFound { .. }
        | WorkerExecutorError::PromiseDropped { .. }
        | WorkerExecutorError::PromiseAlreadyCompleted { .. }
        | WorkerExecutorError::ParamTypeMismatch { .. }
        | WorkerExecutorError::NoValueInMessage
        | WorkerExecutorError::ValueMismatch { .. }
        | WorkerExecutorError::UnexpectedOplogEntry { .. }
        | WorkerExecutorError::InvalidAccount
        | WorkerExecutorError::PreviousInvocationFailed { .. }
        | WorkerExecutorError::PreviousInvocationExited
        | WorkerExecutorError::ComponentNotFound { .. } => HostFailureKind::Permanent,
        _ => HostFailureKind::Transient,
    }
}

fn reveal_error_to_wit(error: SecretRevealError) -> SecretError {
    match error {
        SecretRevealError::Unavailable(message) => SecretError::Unavailable(message),
        SecretRevealError::VersionNotFound(revision) => {
            SecretError::VersionNotFound(SecretVersion {
                bytes: revision.to_be_bytes().to_vec(),
            })
        }
        SecretRevealError::Internal(message) => SecretError::Internal(message),
    }
}

impl<Ctx: WorkerCtx> HostSecret for DurableWorkerCtx<Ctx> {
    async fn drop(&mut self, rep: Resource<SecretHandleRep>) -> anyhow::Result<()> {
        DurabilityHost::observe_function_call(self, "golem::core::secret", "drop");
        self.table().delete(rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> SecretResolver for DurableWorkerCtx<Ctx> {
    type Error = WorkerExecutorError;

    fn snapshot_secret_handle(
        &mut self,
        handle: Resource<SecretHandleRep>,
    ) -> Result<SecretValuePayload, Self::Error> {
        let entry = self
            .table()
            .delete(handle)
            .map_err(|e| WorkerExecutorError::runtime(format!("invalid secret handle: {e}")))?
            .into_payload::<SecretEntry>()
            .map_err(|_| {
                WorkerExecutorError::runtime("secret resource had unexpected payload type")
            })?;

        Ok(entry.to_snapshot())
    }

    fn secret_handle_from_snapshot(
        &mut self,
        snapshot: &SecretValuePayload,
    ) -> Result<Resource<SecretHandleRep>, Self::Error> {
        let entry = SecretEntry::from_snapshot(snapshot)
            .map_err(|e| WorkerExecutorError::runtime(format!("invalid secret snapshot: {e}")))?;
        self.table().push(SecretHandleRep::new(entry)).map_err(|e| {
            WorkerExecutorError::runtime(format!("failed to create secret handle: {e}"))
        })
    }

    fn drop_secret_handle(&mut self, handle: Resource<SecretHandleRep>) {
        let _ = self.table().delete(handle);
    }
}

impl<Ctx: WorkerCtx> secret_types::Host for DurableWorkerCtx<Ctx> {
    async fn id(&mut self, s: Resource<SecretHandleRep>) -> anyhow::Result<SecretId> {
        DurabilityHost::observe_function_call(self, "golem::secrets::types", "id");
        Ok(secret_id_bytes(secret_entry(self, &s)?))
    }

    async fn metadata(&mut self, s: Resource<SecretHandleRep>) -> anyhow::Result<SecretMetadata> {
        DurabilityHost::observe_function_call(self, "golem::secrets::types", "metadata");
        Ok(secret_metadata(secret_entry(self, &s)?))
    }
}

impl<Ctx: WorkerCtx> reveal::Host for DurableWorkerCtx<Ctx> {
    async fn reveal(
        &mut self,
        s: Resource<SecretHandleRep>,
        expected: golem_schema::schema::wit::wire::SchemaGraph,
    ) -> anyhow::Result<Result<SchemaValueTree, SecretError>> {
        let entry = secret_entry(self, &s)?.clone();
        let (denied, mut expected_graph) = if self.state.is_live() {
            let denied = match canonical_secret_resource(&entry) {
                Ok(resource) => {
                    match secret_target(environment_owner(self), SecretVerb::Reveal, &resource) {
                        Ok(target) => self
                            .authorize_live_permission(&target)
                            .await?
                            .err()
                            .map(|_| permission_denied()),
                        Err(_) => Some(permission_denied()),
                    }
                }
                Err(_) => Some(permission_denied()),
            };
            let expected_graph = match decode_graph(&expected) {
                Ok(graph) => graph,
                Err(error) => {
                    return Ok(Err(SecretError::Internal(format!(
                        "invalid expected schema graph: {error}"
                    ))));
                }
            };
            (denied, Some(expected_graph))
        } else {
            (None, None)
        };
        let begun = CallHandle::<GolemSecretsReveal, NotCancellable>::begin(
            self,
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let mut handle = if begun.is_live() {
            begun
                .start_live(
                    self,
                    HostRequestSecretReveal {
                        secret_id: entry.secret_id.0,
                        expected_type: expected_graph
                            .as_ref()
                            .expect("live secret reveal has a decoded expected graph")
                            .clone(),
                    },
                )
                .await?
        } else {
            begun.start_replay(self).await?
        };

        let mut live_secret = None;
        let response = 'reveal: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'reveal replayed,
                    CallReplayOutcome::Incomplete(live) => {
                        handle = live;
                        expected_graph =
                            Some(decode_graph(&expected).map_err(|error| {
                                anyhow!("invalid expected schema graph: {error}")
                            })?);
                    }
                }
            }

            if let Some(error) = denied {
                break 'reveal handle
                    .complete(
                        self,
                        HostResponseSecretRevealed {
                            secret_id: entry.secret_id.0,
                            pinned_revision: entry.pinned_revision.get(),
                            resolved_at: entry.resolved_at.into(),
                            result: Err(error),
                            audit: SecretRevealAudit {
                                calling_agent: self.owned_agent_id.agent_id.clone(),
                                config_key: None,
                                timestamp: Utc::now().into(),
                            },
                        },
                    )
                    .await?;
            }

            let expected_graph = expected_graph
                .as_ref()
                .expect("live secret reveal has a decoded expected graph");

            let config_key = match canonical_config_key(&entry) {
                Ok(path) => path,
                Err(error) => {
                    break 'reveal handle
                        .complete(
                            self,
                            HostResponseSecretRevealed {
                                secret_id: entry.secret_id.0,
                                pinned_revision: entry.pinned_revision.get(),
                                resolved_at: entry.resolved_at.into(),
                                result: Err(error),
                                audit: SecretRevealAudit {
                                    calling_agent: self.owned_agent_id.agent_id.clone(),
                                    config_key: entry.config_key.clone(),
                                    timestamp: Utc::now().into(),
                                },
                            },
                        )
                        .await?;
                }
            };

            let secret = loop {
                let result = self
                    .state
                    .environment_state_service
                    .get_agent_secret_revision(
                        self.state.component_metadata.environment_id,
                        entry.secret_id,
                        config_key.clone(),
                        entry.pinned_revision,
                    )
                    .await;
                match handle
                    .try_trigger_retry_or_loop(self, &result, classify_secret_revision_error)
                    .await?
                {
                    InternalRetryResult::Persist => break result,
                    InternalRetryResult::RetryInternally => continue,
                }
            };

            let result = match secret {
                Ok(Some(secret)) => {
                    let mut result =
                        validate_expected_type(&secret, expected_graph).and_then(|()| {
                            let value = secret.secret_value.as_ref().ok_or_else(|| {
                                SecretRevealError::Unavailable(
                                    "secret value is missing".to_string(),
                                )
                            })?;
                            validate_secret_value(&secret, value)
                        });
                    if result.is_ok()
                        && let Some(value) = &secret.secret_value
                        && !self.secret_holds_allowed_for_value(value).await?
                    {
                        result = Err(permission_denied());
                    } else if result.is_ok() {
                        live_secret = Some(secret);
                    }
                    result
                }
                Ok(None) => Err(SecretRevealError::VersionNotFound(
                    entry.pinned_revision.get(),
                )),
                Err(error) => Err(SecretRevealError::Internal(error.to_string())),
            };

            handle
                .complete(
                    self,
                    HostResponseSecretRevealed {
                        secret_id: entry.secret_id.0,
                        pinned_revision: entry.pinned_revision.get(),
                        resolved_at: entry.resolved_at.into(),
                        result,
                        audit: SecretRevealAudit {
                            calling_agent: self.owned_agent_id.agent_id.clone(),
                            config_key: entry.config_key.clone(),
                            timestamp: Utc::now().into(),
                        },
                    },
                )
                .await?
        };

        if response.secret_id != entry.secret_id.0
            || response.pinned_revision != entry.pinned_revision.get()
        {
            return Ok(Err(SecretError::Internal(
                "persisted secret reveal response does not match the requested secret".to_string(),
            )));
        }

        if let Err(error) = response.result {
            return Ok(Err(reveal_error_to_wit(error)));
        }

        let expected_graph = match expected_graph {
            Some(graph) => graph,
            None => decode_graph(&expected)
                .map_err(|error| anyhow!("invalid expected schema graph during replay: {error}"))?,
        };

        let secret = match live_secret {
            Some(secret) => secret,
            None => match self
                .state
                .environment_state_service
                .get_agent_secret_revision(
                    self.state.component_metadata.environment_id,
                    entry.secret_id,
                    match canonical_config_key(&entry) {
                        Ok(path) => path,
                        Err(error) => return Ok(Err(reveal_error_to_wit(error))),
                    },
                    entry.pinned_revision,
                )
                .await
            {
                Ok(Some(secret)) => secret,
                Ok(None) => {
                    return Err(anyhow!(
                        "pinned secret revision {} is no longer available after a successful reveal was persisted",
                        entry.pinned_revision.get()
                    ));
                }
                Err(error) => {
                    return Err(anyhow!(
                        "failed to re-materialize pinned secret revision after a successful reveal was persisted: {error}"
                    ));
                }
            },
        };

        validate_expected_type(&secret, &expected_graph).map_err(|error| {
            anyhow!("pinned secret revision no longer matches persisted reveal success: {error:?}")
        })?;

        let secret_value = secret.secret_value.as_ref().ok_or_else(|| {
            anyhow!("pinned secret revision has no value after a successful reveal was persisted")
        })?;

        validate_secret_value(&secret, secret_value).map_err(|error| {
            anyhow!("pinned secret value no longer matches persisted reveal success: {error:?}")
        })?;

        encode_value_with(secret_value, self)
            .map(Ok)
            .map_err(|e| anyhow!("Failed to encode revealed secret value: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::graph::SchemaGraph;
    use golem_common::schema::schema_type::SchemaType;
    use golem_service_base::model::agent_secret::AgentSecret;
    use test_r::test;

    fn secret_with_inner(inner: SchemaType, value: SchemaValue) -> AgentSecret {
        AgentSecret {
            id: golem_common::model::agent_secret::AgentSecretId(uuid::Uuid::nil()),
            environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::nil()),
            path: golem_common::model::agent_secret::CanonicalAgentSecretPath(vec![
                "apiKey".to_string(),
            ]),
            revision: AgentSecretRevision::INITIAL,
            secret_type: SchemaGraph::anonymous(inner),
            secret_value: Some(value),
        }
    }

    fn secret_value(path: &[&str]) -> SchemaValue {
        SchemaValue::Secret(SecretValuePayload {
            secret_id: uuid::Uuid::nil(),
            config_key: Some(path.iter().map(|segment| (*segment).to_string()).collect()),
            version: AgentSecretRevision::INITIAL.get(),
            resolved_at: Utc::now(),
            category: None,
        })
    }

    #[test]
    fn reveal_validation_accepts_matching_inner_type() {
        let secret = secret_with_inner(SchemaType::string(), SchemaValue::String("s3".to_string()));
        let expected = SchemaGraph::anonymous(SchemaType::string());

        validate_expected_type(&secret, &expected).unwrap();
        validate_secret_value(&secret, secret.secret_value.as_ref().unwrap()).unwrap();
    }

    #[test]
    fn reveal_validation_rejects_mismatched_inner_type() {
        let secret = secret_with_inner(SchemaType::string(), SchemaValue::String("s3".to_string()));
        let expected = SchemaGraph::anonymous(SchemaType::u64());

        assert!(matches!(
            validate_expected_type(&secret, &expected),
            Err(SecretRevealError::Unavailable(_))
        ));
    }

    #[test]
    fn secret_id_distinguishes_pinned_revisions() {
        let secret_id = golem_common::model::agent_secret::AgentSecretId(uuid::Uuid::nil());
        let first = SecretEntry {
            secret_id,
            pinned_revision: AgentSecretRevision::INITIAL,
            config_key: Some(vec!["apiKey".to_string()]),
            resolved_at: Utc::now(),
            category: None,
        };
        let second = SecretEntry {
            secret_id,
            pinned_revision: AgentSecretRevision::INITIAL.next().unwrap(),
            config_key: Some(vec!["apiKey".to_string()]),
            resolved_at: Utc::now(),
            category: None,
        };

        assert_ne!(
            secret_id_bytes(&first).bytes,
            secret_id_bytes(&second).bytes,
            "secret ids should identify the pinned secret material/version, not only the stable registry id"
        );
    }

    #[test]
    fn canonical_secret_resource_is_exact_and_dot_separated() {
        let entry = SecretEntry {
            secret_id: golem_common::model::agent_secret::AgentSecretId(uuid::Uuid::nil()),
            pinned_revision: AgentSecretRevision::INITIAL,
            config_key: Some(vec!["service".to_string(), "api-key".to_string()]),
            resolved_at: Utc::now(),
            category: None,
        };

        assert_eq!(
            canonical_secret_resource(&entry).unwrap(),
            "service.api-key"
        );
    }

    #[test]
    fn canonical_secret_resource_rejects_pattern_segments() {
        let entry = SecretEntry {
            secret_id: golem_common::model::agent_secret::AgentSecretId(uuid::Uuid::nil()),
            pinned_revision: AgentSecretRevision::INITIAL,
            config_key: Some(vec!["service".to_string(), "*".to_string()]),
            resolved_at: Utc::now(),
            category: None,
        };

        assert!(canonical_secret_resource(&entry).is_err());
    }

    #[test]
    fn secret_hold_admission_finds_every_nested_secret_handle() {
        let value = SchemaValue::Record {
            fields: vec![
                SchemaValue::Option {
                    inner: Some(Box::new(secret_value(&["first"]))),
                },
                SchemaValue::Map {
                    entries: vec![(
                        SchemaValue::String("key".to_string()),
                        SchemaValue::Variant(
                            golem_common::schema::schema_value::VariantValuePayload {
                                case: 0,
                                payload: Some(Box::new(secret_value(&["second", "nested"]))),
                            },
                        ),
                    )],
                },
                SchemaValue::Result(golem_common::schema::schema_value::ResultValuePayload::Ok {
                    value: Some(Box::new(SchemaValue::Union(
                        golem_common::schema::schema_value::UnionValuePayload {
                            tag: "secret".to_string(),
                            body: Box::new(secret_value(&["third"])),
                        },
                    ))),
                }),
            ],
        };

        let paths = secret_paths_for_value(&value)
            .unwrap()
            .into_iter()
            .map(|path| path.join("."))
            .collect::<Vec<_>>();

        assert_eq!(paths, ["first", "second.nested", "third"]);
    }

    #[test]
    fn secret_hold_admission_rejects_handles_without_a_config_key() {
        let value = SchemaValue::Secret(SecretValuePayload {
            secret_id: uuid::Uuid::nil(),
            config_key: None,
            version: AgentSecretRevision::INITIAL.get(),
            resolved_at: Utc::now(),
            category: None,
        });

        assert!(secret_paths_for_value(&value).is_err());
    }
}
