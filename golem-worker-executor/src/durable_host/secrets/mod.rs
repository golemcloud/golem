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

use crate::durable_host::concurrent::{CallHandle, CallReplayOutcome, NotCancellable};
use crate::durable_host::secrets::types::SecretEntry;
use crate::durable_host::{DurabilityHost, DurableWorkerCtx};
use crate::preview2::golem::secrets::reveal;
use crate::preview2::golem::secrets::types as secret_types;
use crate::preview2::golem::secrets::types::{
    SecretError, SecretId, SecretMetadata, SecretVersion,
};
use crate::workerctx::WorkerCtx;
use anyhow::anyhow;
use chrono::Utc;
use golem_common::model::agent_secret::{AgentSecretRevision, CanonicalAgentSecretPath};
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

fn secret_inner_type<'a>(graph: &'a SchemaGraph) -> anyhow::Result<&'a SchemaType> {
    match resolve_schema_ref(graph, &graph.root) {
        SchemaType::Secret { spec, .. } => Ok(&spec.inner),
        other => Err(anyhow!("stored secret type must be secret, got {other:?}")),
    }
}

fn validate_expected_type(
    secret: &AgentSecret,
    expected_graph: &SchemaGraph,
) -> Result<(), SecretRevealError> {
    let pinned_inner = secret_inner_type(&secret.secret_type)
        .map_err(|err| SecretRevealError::Internal(err.to_string()))?;

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
    let pinned_inner = secret_inner_type(&secret.secret_type)
        .map_err(|err| SecretRevealError::Internal(err.to_string()))?;

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
        let expected_graph = match decode_graph(&expected) {
            Ok(graph) => graph,
            Err(error) => {
                return Ok(Err(SecretError::Internal(format!(
                    "invalid expected schema graph: {error}"
                ))));
            }
        };

        let entry = secret_entry(self, &s)?.clone();
        let mut handle = CallHandle::<GolemSecretsReveal, NotCancellable>::start(
            self,
            HostRequestSecretReveal {
                secret_id: entry.secret_id.0,
                expected_type: expected_graph.clone(),
            },
            DurableFunctionType::ReadRemote,
        )
        .await?;

        let mut live_secret = None;
        let response = 'reveal: {
            if !handle.is_live() {
                match handle.replay(self).await? {
                    CallReplayOutcome::Replayed(replayed) => break 'reveal replayed,
                    CallReplayOutcome::Incomplete(live) => handle = live,
                }
            }

            let result = match self
                .state
                .environment_state_service
                .get_agent_secret_revision(
                    self.state.component_metadata.environment_id,
                    entry.secret_id,
                    match canonical_config_key(&entry) {
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
                    },
                    entry.pinned_revision,
                )
                .await
            {
                Ok(Some(secret)) => {
                    let result = validate_expected_type(&secret, &expected_graph).and_then(|()| {
                        let value = secret.secret_value.as_ref().ok_or_else(|| {
                            SecretRevealError::Unavailable("secret value is missing".to_string())
                        })?;
                        validate_secret_value(&secret, value)
                    });
                    if result.is_ok() {
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
    use golem_common::schema::schema_type::{SchemaType, SecretSpec};
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
            secret_type: SchemaGraph::anonymous(SchemaType::Secret {
                spec: SecretSpec {
                    inner: Box::new(inner),
                    category: None,
                },
                metadata: Default::default(),
            }),
            secret_value: Some(value),
        }
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
}
