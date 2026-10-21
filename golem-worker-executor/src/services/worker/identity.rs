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
    /// Key holding the worker's immutable mode and fingerprint as a hint for identity resolution,
    /// stored separately from `AgentStatusRecord`. Populated lazily for durable workers and never
    /// written on the per-commit hot path. The `Create` entry remains authoritative, so stale hints
    /// are replaced or removed. This lives in the `Worker` namespace because it has an independent
    /// lifecycle from the status fields.
    pub(super) fn agent_mode_key(agent_id: &AgentId) -> String {
        format!("worker:agent_mode:{}", agent_id.to_redis_key())
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
                    agent_id: Arc::new(owned_agent_id.agent_id()),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
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
                    agent_id: Arc::new(owned_agent_id.agent_id()),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
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
                    agent_id: Arc::new(owned_agent_id.agent_id()),
                },
                &Self::agent_mode_key(&owned_agent_id.agent_id),
            )
            .await
    }

    pub(super) async fn remove_cached_agent_mode_if_fingerprint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), WorkerExecutorError> {
        let namespace = KeyValueStorageNamespace::Worker {
            agent_id: Arc::new(owned_agent_id.agent_id()),
        };
        let key = Self::agent_mode_key(&owned_agent_id.agent_id);
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
