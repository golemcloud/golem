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

use super::*;

impl DefaultWorkerService {
    pub(super) fn seal_read_error(
        owned_agent_id: &OwnedAgentId,
        error: crate::services::oplog::OplogArchiveError,
    ) -> WorkerExecutorError {
        crate::services::oplog::UnreadableOplog::new(
            &owned_agent_id.agent_id,
            crate::services::oplog::OplogReadError::of_archive(
                crate::services::oplog::OplogReadSource::Other("seal record"),
                error,
            ),
        )
        .into()
    }

    pub(super) async fn resolve_agent_identity_with_authority(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        if let Some(seal) = self
            .oplog_service
            .seal_service()
            .filter(|seal| seal.enabled())
        {
            return seal
                .read_with_authority(
                    owned_agent_id,
                    || self.resolve_present_agent_identity(owned_agent_id),
                    |record| {
                        let seal = seal.clone();
                        async move {
                            self.read_sealed_agent_identity(owned_agent_id, &seal, &record)
                                .await
                        }
                    },
                    || Ok(None),
                    |error| Self::seal_read_error(owned_agent_id, error),
                )
                .await;
        }
        self.resolve_present_agent_identity(owned_agent_id).await
    }

    pub(super) async fn resolve_present_agent_identity(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        let hint = self.read_cached_agent_mode(owned_agent_id).await?;
        record_agent_identity_resolution(if hint.is_some() { "hint" } else { "no_hint" });
        let first_mode = hint
            .as_ref()
            .map_or(AgentMode::Durable, |hint| hint.agent_mode);
        let second_mode = match first_mode {
            AgentMode::Durable => AgentMode::Ephemeral,
            AgentMode::Ephemeral => AgentMode::Durable,
        };
        let resolved = match self
            .read_agent_identity_in_mode(owned_agent_id, first_mode)
            .await?
        {
            Some(identity) => Some(identity),
            None => {
                record_agent_identity_resolution("alternate_mode_fallback");
                self.read_agent_identity_in_mode(owned_agent_id, second_mode)
                    .await?
            }
        };
        // Cold readers must not race seal cleanup with a cache write that can outlive the request.
        if self
            .oplog_service
            .seal_service()
            .is_some_and(|seal| seal.enabled())
        {
            return Ok(resolved);
        }
        match &resolved {
            Some(identity) if identity.agent_mode == AgentMode::Durable => {
                let actual = CachedAgentMode {
                    agent_mode: identity.agent_mode,
                    fingerprint: identity.fingerprint,
                };
                if hint.as_ref() != Some(&actual) {
                    match self.write_cached_agent_mode(owned_agent_id, &actual).await {
                        Ok(()) if hint.is_some() => {
                            record_agent_identity_resolution("stale_hint_replacement");
                        }
                        Ok(()) => {}
                        Err(err) => {
                            error!("Failed to cache the agent mode for {owned_agent_id}: {err}");
                        }
                    }
                }
            }
            Some(_) if hint.is_some() => {
                match self.remove_cached_agent_mode(owned_agent_id).await {
                    Ok(()) => record_agent_identity_resolution("stale_hint_replacement"),
                    Err(err) => {
                        error!("Failed to remove stale agent mode for {owned_agent_id}: {err}");
                    }
                }
            }
            _ => {}
        }
        Ok(resolved)
    }

    pub(super) async fn read_sealed_agent_identity(
        &self,
        owned_agent_id: &OwnedAgentId,
        seal: &crate::services::oplog::SealService,
        record: &crate::services::oplog::seal::SealRecord,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        let create_entry = seal.read_initial(owned_agent_id, record).await?;
        if !matches!(&create_entry, OplogEntry::Create { parameters, .. }
            if parameters.agent_id == owned_agent_id.agent_id
                && parameters.environment_id == owned_agent_id.environment_id
                && parameters.agent_mode == record.agent_mode
                && AgentFingerprint(parameters.instance_id) == record.fingerprint)
        {
            return Err(WorkerExecutorError::runtime(format!(
                "agent {owned_agent_id} has a seal without a matching initial Create entry"
            )));
        }
        Ok(Some(ResolvedAgentIdentity {
            agent_mode: record.agent_mode,
            fingerprint: record.fingerprint,
            create_entry,
        }))
    }

    /// Key holding the worker's immutable mode and fingerprint as a hint for identity resolution,
    /// stored separately from `AgentStatusRecord`. Cold resolution populates and repairs durable
    /// hints only when sealing is disabled. The `Create` entry remains authoritative. This lives
    /// in the `Worker` namespace because it has an independent lifecycle from the status fields.
    pub(super) fn agent_mode_key(owned_agent_id: &OwnedAgentId) -> String {
        format!("worker:agent_mode:{}", owned_agent_id.storage_key())
    }

    /// Reads the dedicated mode hint, if present. Invalid formats are cache misses.
    pub(super) async fn read_cached_agent_mode(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<CachedAgentMode>, WorkerExecutorError> {
        let value = self
            .key_value_storage
            .with_entity("worker", "read_cached_agent_mode", "agent_mode")
            .get_raw(
                KeyValueStorageNamespace::Worker {
                    owned_agent_id: owned_agent_id.clone(),
                },
                &Self::agent_mode_key(owned_agent_id),
            )
            .await
            .map_err(|err| {
                WorkerExecutorError::runtime(format!(
                    "failed to get agent mode for {owned_agent_id} from KV storage: {err}"
                ))
            })?;

        Ok(value
            .as_deref()
            .and_then(|bytes| try_deserialize(bytes).ok().flatten()))
    }

    /// Publishes the durable mode hint atomically with its expiry. This is best-effort because the
    /// resolved `Create` entry, not this key, is authoritative.
    pub(super) async fn write_cached_agent_mode(
        &self,
        owned_agent_id: &OwnedAgentId,
        cached: &CachedAgentMode,
    ) -> Result<(), String> {
        let value = serialize(cached)?;
        self.key_value_storage
            .with_entity("worker", "write_cached_agent_mode", "agent_mode")
            .set_raw_with_expiry(
                KeyValueStorageNamespace::Worker {
                    owned_agent_id: owned_agent_id.clone(),
                },
                &Self::agent_mode_key(owned_agent_id),
                &value,
                DERIVED_CACHE_EXPIRY,
            )
            .await
            .map_err(|err| {
                record_derived_cache_publication_failed("mode_hint");
                format!("failed to set agent mode in KV storage: {err}")
            })
    }

    pub(super) async fn remove_cached_agent_mode(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<(), String> {
        self.key_value_storage
            .with("worker", "remove_cached_agent_mode")
            .del(
                KeyValueStorageNamespace::Worker {
                    owned_agent_id: owned_agent_id.clone(),
                },
                &Self::agent_mode_key(owned_agent_id),
            )
            .await
    }

    pub(super) async fn remove_cached_agent_mode_if_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), WorkerExecutorError> {
        let namespace = KeyValueStorageNamespace::Worker {
            owned_agent_id: owned_agent_id.clone(),
        };
        let key = Self::agent_mode_key(owned_agent_id);
        let current = self
            .key_value_storage
            .with_entity("worker", "remove_cached_agent_mode", "agent_mode")
            .get_raw(namespace.clone(), &key)
            .await
            .map_err(WorkerExecutorError::runtime)?;
        let Some(current) = current else {
            return Ok(());
        };
        let matches = try_deserialize::<CachedAgentMode>(&current)
            .ok()
            .flatten()
            .is_some_and(|cached| cached.fingerprint == fingerprint);
        if matches {
            self.key_value_storage
                .with_entity("worker", "remove_cached_agent_mode", "agent_mode")
                .compare_and_mutate_many_raw(
                    namespace,
                    &key,
                    Some(&current),
                    &[],
                    &[&key],
                    DERIVED_CACHE_EXPIRY,
                )
                .await
                .map_err(WorkerExecutorError::runtime)?;
        }
        Ok(())
    }

    pub(super) async fn read_agent_identity_in_mode(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        let entry = self
            .oplog_service
            .read_source(owned_agent_id, agent_mode, OplogIndex::INITIAL, 1)
            .await?
            .remove(&OplogIndex::INITIAL);
        match entry {
            None => Ok(None),
            Some(ref create_entry @ OplogEntry::Create { ref parameters, .. })
                if parameters.agent_mode == agent_mode =>
            {
                Ok(Some(ResolvedAgentIdentity {
                    agent_mode,
                    fingerprint: AgentFingerprint(parameters.instance_id),
                    create_entry: create_entry.clone(),
                }))
            }
            Some(OplogEntry::Create { .. }) => Err(WorkerExecutorError::runtime(format!(
                "agent {owned_agent_id} has a Create entry in the wrong oplog namespace"
            ))),
            Some(_) => Err(WorkerExecutorError::runtime(format!(
                "agent {owned_agent_id} has a malformed oplog without an initial Create entry"
            ))),
        }
    }

    pub(super) async fn read_agent_identity_for_deletion(
        &self,
        owned_agent_id: &OwnedAgentId,
        deleting_mode: AgentMode,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        if let Some(identity) = self
            .read_agent_identity_in_mode(owned_agent_id, deleting_mode)
            .await?
        {
            return Ok(Some(identity));
        }
        let alternate_mode = match deleting_mode {
            AgentMode::Durable => AgentMode::Ephemeral,
            AgentMode::Ephemeral => AgentMode::Durable,
        };
        self.read_agent_identity_in_mode(owned_agent_id, alternate_mode)
            .await
    }
}
