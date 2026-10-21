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

use super::running_workers::stale_reason;
use super::*;

#[async_trait]
impl WorkerService for DefaultWorkerService {
    async fn lookup_durable_stream_producer_metadata(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        keys: Vec<ProducerMetadataKey>,
    ) -> Result<(OplogIndex, Vec<Option<ProducerMetadataRow>>), SessionError> {
        self.stream_session_index
            .lookup_producer_metadata(owned_agent_id, agent_mode, fingerprint, keys)
            .await
    }

    async fn lookup_durable_stream_recovery_metadata(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
    ) -> Result<DurableStreamRecoveryMetadata, SessionError> {
        self.stream_session_index
            .lookup_recovery_metadata(owned_agent_id, agent_mode, fingerprint)
            .await
    }

    async fn lookup_durable_stream_resume_offset(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
        attempt: golem_common::model::durable_stream::AttemptId,
    ) -> Result<Option<OplogIndex>, SessionError> {
        self.stream_session_index
            .lookup_resume_offset(owned_agent_id, agent_mode, fingerprint, key, attempt)
            .await
    }

    async fn lookup_durable_stream_control_metadata(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
    ) -> Result<SessionControlMetadata, SessionError> {
        self.stream_session_index
            .lookup_control_metadata(owned_agent_id, agent_mode, fingerprint, key)
            .await
    }

    async fn read_durable_stream_consumer_page(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        key: &StreamSessionKey,
        reader: golem_common::model::durable_stream::LocalStreamReaderId,
        page: u64,
    ) -> Result<Vec<OplogIndex>, SessionError> {
        self.stream_session_index
            .read_consumer_page(owned_agent_id, fingerprint, key, reader, page)
            .await
    }

    #[tracing::instrument(name = "worker_metadata.get", level = "debug", skip_all)]
    async fn get(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<GetWorkerMetadataResult>, WorkerExecutorError> {
        let lifecycle_gate = self.lifecycle_gate(owned_agent_id);
        let _lifecycle_guard = lifecycle_gate.gate.read().await;
        record_worker_call("get");

        let Some(identity) = self.resolve_agent_identity(owned_agent_id).await? else {
            return Ok(None);
        };
        let agent_mode = identity.agent_mode;
        let resolved_fingerprint = identity.fingerprint;
        let initial_oplog_entry = Some((OplogIndex::INITIAL, identity.create_entry));

        debug!("Found initial oplog entry for worker: {initial_oplog_entry:?}");

        match initial_oplog_entry {
            None => Ok(None),
            Some((
                _,
                OplogEntry::Create {
                    timestamp,
                    parameters,
                },
            )) => {
                let golem_common::model::oplog::CreateParameters {
                    agent_id,
                    owner_kind,
                    agent_mode: persisted_agent_mode,
                    component_revision,
                    env,
                    environment_id,
                    created_by,
                    parent,
                    component_size,
                    initial_total_linear_memory_size,
                    initial_active_plugins,
                    local_agent_config,
                    original_phantom_id,
                    instance_id,
                } = *parameters;
                owner_kind
                    .validate_instance_name(&agent_id.agent_id)
                    .unwrap_or_else(|error| {
                        panic!("invalid authoritative owner metadata for {owned_agent_id}: {error}")
                    });
                debug_assert_eq!(persisted_agent_mode, agent_mode);
                debug_assert_eq!(AgentFingerprint(instance_id), resolved_fingerprint);
                let agent_mode = persisted_agent_mode;
                let component_metadata = self
                    .component_service
                    .get_metadata(agent_id.component_id, Some(component_revision))
                    .await
                    .map_or_else(
                        |e| match e {
                            WorkerExecutorError::ComponentNotFound { .. } => Ok(None),
                            other => Err(other),
                        },
                        |v| Ok(Some(v)),
                    )
                    .unwrap_or_else(|err| {
                        panic!("failed to get component metadata for {owned_agent_id}: {err}")
                    });
                let Some(component_metadata) = component_metadata else {
                    return Ok(None);
                };
                let agent_type_name = (matches!(
                    owner_kind,
                    golem_common::model::agent::OwnerKind::ComponentAgent
                ) && component_metadata.metadata.is_agent())
                .then(|| ParsedAgentId::parse_agent_type_name(&agent_id.agent_id))
                .transpose()
                .unwrap_or_else(|error| {
                    panic!("invalid agent type in authoritative owner metadata for {owned_agent_id}: {error}")
                });

                let config = local_agent_config
                    .into_iter()
                    .map(|lac| {
                        lac.enrich_with_type(
                            &component_metadata.metadata,
                            owner_kind,
                            agent_type_name.as_ref(),
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap_or_else(|err| {
                        panic!("failed enriching local agent config for {owned_agent_id}: {err}")
                    });

                let initial_worker_metadata = AgentMetadata {
                    agent_id,
                    owner_kind,
                    env,
                    config,
                    environment_id,
                    created_by,
                    created_by_email: component_metadata.account_email,
                    created_at: timestamp,
                    parent,
                    last_known_status: AgentStatusRecord {
                        component_revision,
                        component_revision_for_replay: component_revision,
                        component_size,
                        total_linear_memory_size: initial_total_linear_memory_size,
                        active_plugins: initial_active_plugins,
                        invocation_results: self.config.invocation_results.membership(),
                        export_fork_admissions: golem_common::model::ExportForkAdmissions {
                            owner_fingerprint: Some(AgentFingerprint(instance_id)),
                            ..Default::default()
                        },
                        agent_mode,
                        ..AgentStatusRecord::default()
                    },
                    original_phantom_id,
                    fingerprint: AgentFingerprint(instance_id),
                    agent_mode,
                };

                let fingerprint = AgentFingerprint(instance_id);
                let last_known_status = match self
                    .read_cached_status(owned_agent_id, fingerprint)
                    .await?
                {
                    Some(mut status) => {
                        // `agent_mode` is `#[transient]` and therefore not part of the status
                        // blob; restore it from the authoritative value resolved above so the
                        // returned record carries the correct mode instead of the `Durable`
                        // deserialization default.
                        status.agent_mode = agent_mode;
                        Some(status)
                    }
                    // No cached status (cache miss, missing for ephemeral workers, or stale
                    // format) -> recompute from oplog, preferring to fold forward from the clean
                    // checkpoint (if any) over a full re-read.
                    None => {
                        let last_known_status = calculate_last_known_status_with_checkpoint_reader(
                            self,
                            owned_agent_id,
                            agent_mode,
                            None,
                            || async {
                                // The checkpoint is only a fold baseline: a read failure here
                                // costs a full recompute, not correctness, and the function has
                                // no way to report it. The live cached status above is the read
                                // that propagates.
                                self.read_status_checkpoint(owned_agent_id, fingerprint, agent_mode)
                                    .await
                                    .unwrap_or_else(|err| {
                                        error!(
                                            "Failed to read the status checkpoint for {owned_agent_id}: {err}"
                                        );
                                        None
                                    })
                            },
                        )
                        .await;

                        let last_known_status = match last_known_status {
                            Ok(Some(status)) => status,
                            Ok(None) => return Ok(None),
                            Err(error) => {
                                tracing::error!(
                                    agent_id = %owned_agent_id,
                                    %error,
                                    "Failed to recompute cold worker status"
                                );
                                // The Create entry still proves the worker exists. Leave status
                                // unresolved so typed reconstruction callers report the failure,
                                // rather than treating a corrupt status payload as a missing worker.
                                return Ok(Some(GetWorkerMetadataResult {
                                    initial_worker_metadata,
                                    last_known_status: None,
                                }));
                            }
                        };

                        // Cold path: no in-memory previous, reconcile against stored fields.
                        self.write_cached_status(
                            owned_agent_id,
                            fingerprint,
                            None,
                            last_known_status.clone(),
                        )
                        .await?;

                        Some(last_known_status)
                    }
                };

                Ok(Some(GetWorkerMetadataResult {
                    initial_worker_metadata,
                    last_known_status,
                }))
            }
            Some(_) => panic!("Encountered malformed oplog without create oplog entry"),
        }
    }

    async fn get_running_workers_in_shards(
        &self,
        on_stale: OnStale<'_>,
    ) -> Result<Vec<GetWorkerMetadataResult>, WorkerExecutorError> {
        let shard_assignment = self.shard_service.try_get_current_assignment();
        let mut result = Vec::new();
        if let Some(shard_assignment) = shard_assignment {
            for shard_id in shard_assignment.shard_ids() {
                let key = Self::running_in_shard_key(&shard_id);
                let mut shard = self.enum_workers_at_key(&key, on_stale).await?;
                result.append(&mut shard);
            }
        }
        Ok(result)
    }

    async fn remove(
        &self,
        lifecycle: &mut OplogLifecycleGuard,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        expected_epoch: Option<ShardEpoch>,
        after_oplog_delete: &(dyn Fn(AgentFingerprint) + Send + Sync),
    ) -> Result<(), WorkerExecutorError> {
        lifecycle.assert_agent(&owned_agent_id.agent_id);
        let lifecycle_gate = self.lifecycle_gate(owned_agent_id);
        let _lifecycle_guard = lifecycle_gate.gate.write().await;
        record_worker_call("remove");
        let current_identity = self
            .read_agent_identity_for_deletion(owned_agent_id, agent_mode)
            .await?;
        let delete_current_oplog = match current_identity.as_ref() {
            Some(identity)
                if identity.fingerprint == fingerprint && identity.agent_mode != agent_mode =>
            {
                return Err(WorkerExecutorError::runtime(format!(
                    "agent {owned_agent_id} fingerprint matches the deleting incarnation but its mode does not"
                )));
            }
            Some(identity) => {
                identity.fingerprint == fingerprint && identity.agent_mode == agent_mode
            }
            // A previous deletion attempt may already have removed the Create entry while deeper
            // archive storage still needs cleanup. The lifecycle fence excludes a concurrent new
            // incarnation, so absence means this deletion must continue rather than declare the
            // partial cleanup complete.
            None => true,
        };

        // The oplog goes after everything it can rebuild, so a crash part-way leaves an oplog
        // behind: the next attempt (or load) re-derives whatever of the state below is missing and
        // finishes from there. Removed first, a crash would leave orphaned keys that a retry reads
        // as an agent already gone. The recovery-index entry comes after it: one a crash leaves
        // behind names no oplog, and the next recovery scan drops it.
        //
        // The epoch is confirmed before anything is removed, so a delete turned away by the fence
        // removes nothing. A new owner claiming the shard between this check and the oplog delete
        // is still refused by that delete, which keeps the oplog: the state removed in between is
        // what the new owner rebuilds from it.
        if delete_current_oplog && let Some(epoch) = expected_epoch {
            self.oplog_service
                .assert_owning_epoch(owned_agent_id, agent_mode, epoch)
                .await?;
        }

        self.remove_cached_status(owned_agent_id, fingerprint)
            .await?;
        self.remove_all_fields(
            owned_agent_id,
            Self::invocation_result_index_namespace(&owned_agent_id.agent_id, fingerprint),
            "invocation-result index",
        )
        .await?;
        self.stream_session_index
            .clear(owned_agent_id, fingerprint)
            .await
            .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        self.key_value_storage
            .with("worker", "remove_rejected_periodic_snapshot")
            .del(
                Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id),
                &Self::rejected_periodic_snapshots_field(fingerprint),
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;

        // Only this incarnation's oplog: a recreated agent's is not the deleting one's to remove.
        if delete_current_oplog {
            self.oplog_service
                .delete(lifecycle, owned_agent_id, agent_mode, expected_epoch)
                .await?;
        }
        after_oplog_delete(fingerprint);

        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assigment is not ready");
        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);

        self.remove_running_worker_member(
            &Self::running_in_shard_key(&shard_id),
            &RunningWorker {
                owned_agent_id: owned_agent_id.clone(),
                fingerprint,
            },
        )
        .await
    }

    async fn remove_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<(), WorkerExecutorError> {
        record_worker_call("remove_cached_status");
        self.remove_split_status(
            owned_agent_id,
            Self::status_namespace(&owned_agent_id.agent_id, fingerprint),
        )
        .await?;
        self.remove_split_status(
            owned_agent_id,
            Self::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
        )
        .await?;
        self.remove_cached_agent_mode_if_fingerprint(owned_agent_id, fingerprint)
            .await
    }

    async fn get_rejected_periodic_snapshots(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
    ) -> Result<HashSet<OplogIndex>, WorkerExecutorError> {
        let value: Option<Result<Vec<OplogIndex>, String>> = self
            .key_value_storage
            .with_entity("worker", "get_rejected_periodic_snapshots", "oplog_indexes")
            .get_attempt_deserialize(
                Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id),
                &Self::rejected_periodic_snapshots_field(fingerprint),
            )
            .await
            .map_err(WorkerExecutorError::runtime)?;
        value
            .transpose()
            .map(|indexes| indexes.into_iter().flatten().collect())
            .map_err(WorkerExecutorError::runtime)
    }

    async fn reject_periodic_snapshots(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
        oplog_indexes: &HashSet<OplogIndex>,
    ) -> Result<(), WorkerExecutorError> {
        let namespace = Self::rejected_periodic_snapshots_namespace(&owned_agent_id.agent_id);
        let field = Self::rejected_periodic_snapshots_field(fingerprint);
        loop {
            let current = self
                .key_value_storage
                .with_entity(
                    "worker",
                    "read_rejected_periodic_snapshots",
                    "oplog_indexes",
                )
                .get_raw(namespace.clone(), &field)
                .await
                .map_err(WorkerExecutorError::runtime)?;
            let current_indexes = match &current {
                Some(current) => {
                    deserialize::<Vec<OplogIndex>>(current).map_err(WorkerExecutorError::runtime)?
                }
                None => Vec::new(),
            };
            let Some(to_store) = rejections_to_store(current_indexes, status, oplog_indexes) else {
                return Ok(());
            };
            let encoded = serialize(&to_store).map_err(WorkerExecutorError::runtime)?;
            let updated = self
                .key_value_storage
                .with_entity("worker", "reject_periodic_snapshots", "oplog_indexes")
                .compare_and_set_many_raw(
                    namespace.clone(),
                    &field,
                    current.as_deref(),
                    &[],
                    &[(field.as_str(), encoded.as_slice())],
                )
                .await
                .map_err(WorkerExecutorError::runtime)?;
            if updated {
                return Ok(());
            }
        }
    }

    async fn lookup_durable_stream_session(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
        key: &IdempotencyKey,
    ) -> Result<Option<DurableStreamSessionStatus>, SessionError> {
        if let Some(value) = status.durable_stream_sessions.get(key) {
            return Ok(Some(value.clone()));
        }
        if !status.durable_stream_sessions.has_history() {
            return Ok(None);
        }
        self.stream_session_index
            .lookup_persisted_offsets(
                owned_agent_id,
                agent_mode,
                fingerprint,
                status.oplog_idx,
                key,
            )
            .await
    }

    async fn lookup_durable_stream_public_binding(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        public_session_id: &str,
    ) -> Result<Option<DurableStreamPublicBinding>, SessionError> {
        self.stream_session_index
            .lookup_public_binding(owned_agent_id, agent_mode, fingerprint, public_session_id)
            .await
    }

    async fn catch_up_invocation_result_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
    ) -> Result<(), WorkerExecutorError> {
        if agent_mode == AgentMode::Ephemeral {
            return Ok(());
        }

        let lock = self.invocation_result_index_lock(owned_agent_id);
        let _guard = lock.inner.lock().await;
        async {
            let namespace =
                Self::invocation_result_index_namespace(&owned_agent_id.agent_id, fingerprint);
            let status_generation = status.invocation_results.revert_generation();
            'reload: loop {
                let observed_metadata = self
                    .key_value_storage
                    .with_entity("worker", "read_invocation_result_index", "metadata")
                    .get_raw(
                        namespace.clone(),
                        INVOCATION_RESULT_INDEX_METADATA_FIELD,
                    )
                    .await.map_err(WorkerExecutorError::runtime)?
                    .map(|value| value.to_vec());
                let persisted = observed_metadata
                    .as_deref()
                    .map(deserialize::<InvocationResultIndexMetadata>)
                    .transpose();
                let (mut metadata, mut expected_metadata) = match &persisted {
                    Ok(Some(metadata)) if metadata.revert_generation > status_generation => {
                        return Ok(());
                    }
                    Ok(Some(metadata)) if metadata.revert_generation == status_generation => {
                        if metadata.covered_through >= status.oplog_idx {
                            return Ok(());
                        }
                        (metadata.clone(), observed_metadata)
                    }
                    Ok(Some(_)) | Ok(None) | Err(_) => {
                        // Only a reset needs all mapping field names. Re-evaluate the generation
                        // from this atomic snapshot because another publisher may have advanced it
                        // after the metadata-only read above.
                        let snapshot = self
                            .key_value_storage
                            .with_entity("worker", "snapshot_invocation_result_index", "metadata")
                            .get_all_raw(namespace.clone())
                            .await.map_err(WorkerExecutorError::runtime)?;
                        let snapshot_metadata = snapshot
                            .iter()
                            .find(|(field, _)| field == INVOCATION_RESULT_INDEX_METADATA_FIELD)
                            .map(|(_, value)| value.to_vec());
                        let snapshot_decoded = snapshot_metadata
                            .as_deref()
                            .map(deserialize::<InvocationResultIndexMetadata>)
                            .transpose();
                        match snapshot_decoded {
                            Ok(Some(metadata))
                                if metadata.revert_generation > status_generation =>
                            {
                                return Ok(());
                            }
                            Ok(Some(metadata))
                                if metadata.revert_generation == status_generation =>
                            {
                                if metadata.covered_through >= status.oplog_idx {
                                    return Ok(());
                                }
                                (metadata, snapshot_metadata)
                            }
                            Ok(Some(_)) | Ok(None) | Err(_) => {
                                let metadata = InvocationResultIndexMetadata {
                                    covered_through: OplogIndex::NONE,
                                    revert_generation: status_generation,
                                    current_idempotency_key: None,
                                    cancelled_idempotency_key: None,
                                };
                                let encoded = serialize(&metadata).map_err(WorkerExecutorError::runtime)?;
                                let deletions: Vec<&str> = snapshot
                                    .iter()
                                    .map(|(field, _)| field.as_str())
                                    .filter(|field| {
                                        *field != INVOCATION_RESULT_INDEX_METADATA_FIELD
                                    })
                                    .collect();
                                let installed = self
                                    .key_value_storage
                                    .with_entity(
                                        "worker",
                                        "reset_invocation_result_index",
                                        "metadata",
                                    )
                                    .compare_and_mutate_many_raw(
                                        namespace.clone(),
                                        INVOCATION_RESULT_INDEX_METADATA_FIELD,
                                        snapshot_metadata.as_deref(),
                                        &[(
                                            INVOCATION_RESULT_INDEX_METADATA_FIELD,
                                            encoded.as_slice(),
                                        )],
                                        &deletions,
                                        DERIVED_CACHE_EXPIRY,
                                    )
                                    .await
                                    .inspect_err(|_err| {
                                        record_derived_cache_publication_failed(
                                            "invocation_result_index",
                                        );
                                    }).map_err(WorkerExecutorError::runtime)?;
                                if !installed {
                                    continue 'reload;
                                }
                                (metadata, Some(encoded))
                            }
                        }
                    }
                };

                while metadata.covered_through < status.oplog_idx {
                    let remaining =
                        status.oplog_idx.as_u64() - metadata.covered_through.as_u64();
                    let count = remaining
                        .min(
                            self.config
                                .invocation_results
                                .physical_index_catch_up_chunk_size,
                        )
                        .max(1);
                    let entries = self
                        .oplog_service
                        .read_exact(
                            owned_agent_id,
                            agent_mode,
                            metadata.covered_through.next(),
                            count,
                        )
                        .await?;
                    if entries.is_empty() {
                        return Err(WorkerExecutorError::runtime(format!(
                            "failed to advance invocation result index for {owned_agent_id}: oplog range starting at {} was empty",
                            metadata.covered_through.next()
                        )));
                    }

                    let mut mappings = HashMap::new();
                    let mut advanced = metadata.clone();
                    fold_invocation_result_entries(
                        &mut advanced.current_idempotency_key,
                        &mut advanced.cancelled_idempotency_key,
                        &status.deleted_regions,
                        &entries,
                        |key, index| {
                            mappings.insert(key.clone(), index);
                        },
                    );
                    advanced.covered_through = *entries.keys().max().unwrap();

                    let mut fields = Vec::with_capacity(mappings.len() + 1);
                    for (key, oplog_index) in mappings {
                        fields.push((
                            invocation_result_index_field(&key),
                            serialize(&PersistedInvocationResult {
                                revert_generation: advanced.revert_generation,
                                oplog_index,
                            }).map_err(WorkerExecutorError::runtime)?,
                        ));
                    }
                    fields.push((
                        INVOCATION_RESULT_INDEX_METADATA_FIELD.to_string(),
                        serialize(&advanced).map_err(WorkerExecutorError::runtime)?,
                    ));
                    let pairs: Vec<(&str, &[u8])> = fields
                        .iter()
                        .map(|(field, value)| (field.as_str(), value.as_slice()))
                        .collect();
                    let updated = self
                        .key_value_storage
                        .with_entity(
                            "worker",
                            "advance_invocation_result_index",
                            "invocation_result",
                        )
                        .compare_and_mutate_many_raw(
                            namespace.clone(),
                            INVOCATION_RESULT_INDEX_METADATA_FIELD,
                            expected_metadata.as_deref(),
                            &pairs,
                            &[],
                            DERIVED_CACHE_EXPIRY,
                        )
                        .await
                        .inspect_err(|_err| {
                            record_derived_cache_publication_failed("invocation_result_index");
                        }).map_err(WorkerExecutorError::runtime)?;
                    if !updated {
                        continue 'reload;
                    }
                    expected_metadata = fields.last().map(|(_, value)| value.clone());
                    metadata = advanced;
                    crate::metrics::workers::record_invocation_result_index_catch_up(entries.len());
                }

                return Ok(());
            }
        }
        .await
    }

    async fn lookup_invocation_result_index(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        status: &AgentStatusRecord,
        idempotency_key: &IdempotencyKey,
    ) -> Result<InvocationResultIndexLookup, String> {
        let values = self
            .key_value_storage
            .with_entity(
                "worker",
                "lookup_invocation_result_index",
                "invocation_result",
            )
            .get_many_raw(
                Self::invocation_result_index_namespace(&owned_agent_id.agent_id, fingerprint),
                vec![
                    INVOCATION_RESULT_INDEX_METADATA_FIELD.to_string(),
                    invocation_result_index_field(idempotency_key),
                ]
                .into(),
            )
            .await?;
        let Some(metadata) = values.first().and_then(Option::as_ref) else {
            return Ok(InvocationResultIndexLookup::Incomplete);
        };
        let metadata: InvocationResultIndexMetadata = deserialize(metadata)?;
        if metadata.revert_generation != status.invocation_results.revert_generation() {
            return Ok(InvocationResultIndexLookup::Incomplete);
        }

        let complete = metadata.covered_through >= status.oplog_idx
            || status
                .invocation_results
                .oldest_retained_index()
                .is_some_and(|oldest| oldest <= metadata.covered_through);
        if !complete {
            return Ok(InvocationResultIndexLookup::Incomplete);
        }

        let Some(value) = values.get(1).and_then(Option::as_ref) else {
            return Ok(InvocationResultIndexLookup::DefinitiveMiss);
        };
        let value: PersistedInvocationResult = deserialize(value)?;
        if value.revert_generation != metadata.revert_generation
            || status
                .deleted_regions
                .is_in_deleted_region(value.oplog_index)
        {
            return Ok(InvocationResultIndexLookup::DefinitiveMiss);
        }

        Ok(InvocationResultIndexLookup::Found(value.oplog_index))
    }

    async fn resolve_agent_identity(
        &self,
        owned_agent_id: &OwnedAgentId,
    ) -> Result<Option<ResolvedAgentIdentity>, WorkerExecutorError> {
        record_worker_call("resolve_agent_identity");
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

    async fn write_cached_status(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        previous_status: Option<&AgentStatusRecord>,
        status_value: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, WorkerExecutorError> {
        record_worker_call("write_status");

        debug!("Writing cached agent status for {owned_agent_id} to {status_value:?}");

        if status_value.has_durable_stream_history {
            self.stream_session_index
                .catch_up(
                    owned_agent_id,
                    status_value.agent_mode,
                    fingerprint,
                    status_value.oplog_idx,
                )
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        }

        self.catch_up_invocation_result_index(
            owned_agent_id,
            status_value.agent_mode,
            fingerprint,
            &status_value,
        )
        .await?;
        self.write_split_status(
            owned_agent_id,
            Self::status_namespace(&owned_agent_id.agent_id, fingerprint),
            previous_status,
            status_value,
        )
        .await
        .map_err(WorkerExecutorError::runtime)
    }

    async fn read_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        agent_mode: AgentMode,
    ) -> Result<Option<AgentStatusRecord>, WorkerExecutorError> {
        record_worker_call("read_status_checkpoint");

        let status = self
            .read_split_status(
                owned_agent_id,
                Self::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
            )
            .await?;
        // `agent_mode` is transient (not part of `core`); restore the authoritative value.
        Ok(status.map(|mut status| {
            status.agent_mode = agent_mode;
            status
        }))
    }

    async fn write_status_checkpoint(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        previous_checkpoint: Option<&AgentStatusRecord>,
        checkpoint: AgentStatusRecord,
    ) -> Result<AgentStatusRecord, WorkerExecutorError> {
        record_worker_call("write_status_checkpoint");

        debug!(
            "Writing clean status checkpoint for {owned_agent_id} at oplog index {}",
            checkpoint.oplog_idx
        );

        if checkpoint.has_durable_stream_history {
            self.stream_session_index
                .catch_up(
                    owned_agent_id,
                    checkpoint.agent_mode,
                    fingerprint,
                    checkpoint.oplog_idx,
                )
                .await
                .map_err(|error| error.into_worker_executor_error(WorkerExecutorError::runtime))?;
        }
        self.write_split_status(
            owned_agent_id,
            Self::checkpoint_namespace(&owned_agent_id.agent_id, fingerprint),
            previous_checkpoint,
            checkpoint,
        )
        .await
        .map_err(WorkerExecutorError::runtime)
    }

    async fn set_assignment_tracking(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        status_value: &AgentStatusRecord,
    ) -> Result<(), String> {
        record_worker_call("set_assignment_tracking");

        // Ephemeral workers are never tracked for recovery (mirrors `write_cached_status`).
        if status_value.agent_mode == AgentMode::Ephemeral {
            return Ok(());
        }

        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assignment is not ready");

        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);
        let running_worker = RunningWorker {
            owned_agent_id: owned_agent_id.clone(),
            fingerprint,
        };

        if Self::should_track_for_assignment_recovery(status_value) {
            debug!("Adding worker to the set of running workers in shard {shard_id}");

            self.key_value_storage
                .with_entity("worker", "add", "agent_id")
                .add_to_set(
                    KeyValueStorageNamespace::RunningWorkers,
                    &Self::running_in_shard_key(&shard_id),
                    &running_worker,
                )
                .await
                .map_err(|err| {
                    format!(
                        "failed to add worker to the set of running workers per shard ids on KV storage: {err}"
                    )
                })
        } else {
            debug!("Removing worker from the set of running workers in shard {shard_id}");

            self.key_value_storage
                .with_entity("worker", "remove", "agent_id")
                .remove_from_set(
                    KeyValueStorageNamespace::RunningWorkers,
                    &Self::running_in_shard_key(&shard_id),
                    &running_worker,
                )
                .await
                .map_err(|err| {
                    format!(
                        "failed to remove worker from the set of running worker ids per shard on KV storage: {err}"
                    )
                })
        }
    }

    async fn remove_if_stale(
        &self,
        owned_agent_id: &OwnedAgentId,
        fingerprint: AgentFingerprint,
        on_stale: OnStale<'_>,
    ) -> Result<bool, WorkerExecutorError> {
        let identity = self.resolve_agent_identity(owned_agent_id).await?;
        let Some(reason) = stale_reason(identity.as_ref(), fingerprint) else {
            return Ok(false);
        };
        let shard_assignment = self
            .shard_service
            .current_assignment()
            .expect("sharding assignment is not ready");
        let shard_id =
            ShardId::from_agent_id(&owned_agent_id.agent_id, shard_assignment.number_of_shards);
        self.drop_stale_member(
            &Self::running_in_shard_key(&shard_id),
            &RunningWorker {
                owned_agent_id: owned_agent_id.clone(),
                fingerprint,
            },
            reason,
            on_stale,
        )
        .await?;
        Ok(true)
    }
}
