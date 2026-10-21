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

impl StreamSession {
    /// Registers result streams and replaces them with session transport mappings.
    pub async fn materialize_result(
        &self,
        value: SchemaValue,
        graph: &SchemaGraph,
        root: &SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<SchemaValue, SessionError> {
        let preparation = self
            .runtime_source
            .as_ref()
            .map(RuntimeSource::external_activity);
        self.materialize_result_accounted(value, graph, root, component_revision, preparation)
            .await
    }

    pub(crate) async fn materialize_result_accounted(
        &self,
        value: SchemaValue,
        graph: &SchemaGraph,
        root: &SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
        preparation: Option<ExternalActivity>,
    ) -> Result<SchemaValue, SessionError> {
        preflight_recursive_stream_value(&value)?;
        let mut next_stream_index = 0u64;
        let retained_bytes =
            encode_recursive_stream_value_with_schema(&value, graph, root, |_stream, _path| {
                let index = next_stream_index;
                next_stream_index = next_stream_index
                    .checked_add(1)
                    .ok_or_else(|| "durable output handle index overflow".to_string())?;
                Ok(index)
            })?
            .encoded_len();
        let session = self.clone();
        let graph = graph.clone();
        let drain_graph = graph.clone();
        let root = root.clone();
        let (result, drain) = self
            .producer
            .run_admitted_accounted(
                None,
                retained_bytes,
                false,
                preparation,
                move |_, admission| async move {
                    let MaterializedResult {
                        value: result,
                        drains,
                    } = session
                        .materialize_result_owned(
                            &admission,
                            value,
                            graph,
                            root,
                            component_revision,
                        )
                        .await?;
                    let coordinator_source = session.runtime_source.clone();
                    let coordinator_activity = coordinator_source
                        .as_ref()
                        .map(RuntimeSource::external_activity);
                    let drain = tokio::spawn(async move {
                        session
                            .drain_materialized_result(
                                drains,
                                Arc::new(drain_graph),
                                coordinator_source,
                                coordinator_activity,
                            )
                            .await
                    });
                    Ok::<_, SessionError>((result, drain))
                },
            )
            .await?;

        // Drains may live for the lifetime of the guest stream, so they must not retain the
        // producer operation permit used for the bounded durable materialization above.
        drain
            .await
            .map_err(|error| format!("durable output drain coordinator failed: {error}"))??;
        Ok(result)
    }

    pub(super) async fn materialize_result_owned(
        &self,
        admission: &Arc<StreamWriteAdmission>,
        value: SchemaValue,
        graph: SchemaGraph,
        root: SchemaType,
        component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<MaterializedResult, SessionError> {
        let session_guard = self.session_lock.lock().await;
        let metadata = self.current_control_metadata().await?;
        let ResultMaterializationState {
            first_result,
            cancel_session,
            deleted_outputs,
            existing_intents,
        } = metadata.materialization_state();
        drop(metadata);
        let cancellation_epoch = if first_result && (cancel_session || !deleted_outputs.is_empty())
        {
            Some(self.authoritative_attachment_state().await?.epoch)
        } else {
            None
        };
        struct PendingOutput<F> {
            path: Vec<StreamValuePathStep>,
            endpoint: Option<LiveStreamEndpoint>,
            forwarded: Option<F>,
            registered_transport_id: Option<u64>,
            registered_role: Option<SessionStreamRole>,
            element_type: SchemaType,
            element_schema_fingerprint: SchemaFingerprintV1,
            cancelled: bool,
        }

        validate_forwarded_durable_input_schemas(
            &value,
            &graph,
            &root,
            "forwarded durable output handle does not match the result stream schema",
        )?;
        let mut pending = Vec::new();
        let encoded =
            encode_recursive_stream_value_with_schema(&value, &graph, &root, |stream, path| {
                let element = stream_element_schema(&graph, &root, path)?;
                let element_schema_fingerprint =
                    schema_fingerprint_v1(&graph, element).map_err(|error| error.to_string())?;
                let forwarded = forwarded_durable_input_reference(stream)?;
                let registered = stream
                    .with_host_endpoint::<RegisteredOutputStream, _>(|output| {
                        (output.transport_stream_id, output.role)
                    })
                    .ok();
                let (registered_transport_id, registered_role) = registered
                    .map(|(id, role)| (Some(id), Some(role)))
                    .unwrap_or((None, None));
                let (endpoint, forwarded) = match forwarded {
                    Some(forwarded) => (None, Some(forwarded.take(stream)?)),
                    None if registered_transport_id.is_some() => (None, None),
                    None => (
                        Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?),
                        None,
                    ),
                };
                let canonical_handle_index = u64::try_from(pending.len())
                    .map_err(|_| "durable output handle index overflow".to_string())?;
                let slot = match path {
                    [] => Some("$result"),
                    [StreamValuePathStep::RecordField(index)] => {
                        match graph
                            .resolve_ref(&root)
                            .map_err(|error| error.to_string())?
                        {
                            SchemaType::Record { fields, .. } => {
                                fields.get(*index as usize).map(|field| field.name.as_str())
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                pending.push(PendingOutput {
                    path: path.to_vec(),
                    endpoint,
                    forwarded,
                    registered_transport_id,
                    registered_role,
                    element_type: element.cloned().unwrap_or_else(SchemaType::u8),
                    element_schema_fingerprint,
                    cancelled: first_result
                        && (cancel_session
                            || slot.is_some_and(|slot| deleted_outputs.contains(slot))),
                });
                Ok(canonical_handle_index)
            })?;

        let session = self.clone();
        let pending = admission
            .submit(move |_, context| async move {
                let mut prepared = Vec::with_capacity(pending.len());
                for (handle_index, output) in pending.into_iter().enumerate() {
                    let forwarded = match output.forwarded {
                        Some(forwarded) => Some(
                            forwarded
                                .prepare_mapping_owned(
                                    &context,
                                    &session,
                                    SessionStreamRole::Output,
                                    StreamReaderForwardPublication::InvocationResult {
                                        handle_index: handle_index as u64,
                                    },
                                )
                                .await?,
                        ),
                        None => None,
                    };
                    prepared.push(PendingOutput {
                        path: output.path,
                        endpoint: output.endpoint,
                        forwarded,
                        registered_transport_id: output.registered_transport_id,
                        registered_role: output.registered_role,
                        element_type: output.element_type,
                        element_schema_fingerprint: output.element_schema_fingerprint,
                        cancelled: output.cancelled,
                    });
                }
                Ok::<_, SessionError>(prepared)
            })
            .await?;
        let session_mapping = StreamSessionMapping {
            session_key: self.session_key.clone(),
            attachment_id: golem_common::model::durable_stream::AttachmentId::primary(
                self.session_key.callee_environment_id,
                &self.session_key.callee,
                &self.session_key.idempotency_key,
            )
            .map_err(|error| error.to_string())?,
            role: golem_common::model::durable_stream::SessionStreamRole::Output,
        };
        let requests = pending
            .iter()
            .filter(|pending| {
                pending.forwarded.is_none() && pending.registered_transport_id.is_none()
            })
            .map(|pending| ProducerRegistrationRequest {
                entity_parent_start_index: self.entity_parent_start_index,
                coordinate: StreamRegistrationCoordinate::Root {
                    invocation_id: self.session_key.clone(),
                    root_kind: StreamRootKind::MethodResult,
                    recursive_value_path: pending.path.clone(),
                },
                source_kind: StreamSourceKind::InvocationOutput,
                source_invocation: self.session_reference.clone(),
                component_revision,
                element_schema_fingerprint: pending.element_schema_fingerprint,
                session_mapping: Some(session_mapping.clone()),
            })
            .collect::<Vec<_>>();
        self.recover_session_mappings().await?;
        let mut new_stream_count = pending
            .iter()
            .filter(|pending| {
                pending.forwarded.as_ref().is_some_and(|mapping| {
                    self.mapping_for_reference(
                        &StreamRecordReference::Foreign(mapping.handle.clone()),
                        SessionStreamRole::Output,
                    )
                    .is_none()
                })
            })
            .count();
        for request in &requests {
            if self
                .producer
                .handle_for_coordinate(&request.coordinate)
                .await
                .map_err(SessionError::from)?
                .is_none()
            {
                new_stream_count += 1;
            }
        }
        self.producer
            .validate_new_session_stream_count(&self.session_key, new_stream_count)
            .await
            .map_err(SessionError::from)?;
        let mut transport_stream_ids = Vec::with_capacity(pending.len());
        let mut request_index = 0usize;
        for pending in &pending {
            let transport_stream_id = if let Some(mapping) = &pending.forwarded {
                let source = StreamRecordReference::Foreign(mapping.handle.clone());
                if first_result && !pending.cancelled && !existing_intents.contains(&source) {
                    self.activate_nested_mapping_under_lock(admission, mapping.clone())
                        .await?;
                }
                mapping.transport_stream_id
            } else {
                if let Some(id) = pending.registered_transport_id {
                    let mapping = self.mapping(id).ok_or_else(|| {
                        "registered result output has no local session mapping".to_string()
                    })?;
                    if Some(mapping.role) != pending.registered_role
                        || mapping.role.direction() != SessionStreamRole::Output
                        || mapping.handle.element_schema_fingerprint
                            != pending.element_schema_fingerprint
                    {
                        return Err(
                            "registered result output does not match its local session binding"
                                .into(),
                        );
                    }
                    id
                } else {
                    let request = &requests[request_index];
                    request_index += 1;
                    let existing_handle = self
                        .producer
                        .handle_for_coordinate(&request.coordinate)
                        .await
                        .map_err(SessionError::from)?;
                    let existing_mapping = if let Some(handle) = existing_handle {
                        let binding = self
                            .producer
                            .local_binding(0, &handle, SessionStreamRole::Output)
                            .await
                            .map_err(SessionError::from)?;
                        self.mapping_for_reference(&binding.source, SessionStreamRole::Output)
                    } else {
                        None
                    };
                    existing_mapping
                        .map(|mapping| mapping.transport_stream_id)
                        .map(Ok)
                        .unwrap_or_else(|| self.allocate_transport_stream_id())?
                }
            };
            transport_stream_ids.push(transport_stream_id);
        }
        let result_bytes = encoded.encode_to_vec();
        let mut requests = requests.into_iter();
        let outputs = pending
            .iter()
            .zip(&transport_stream_ids)
            .map(
                |(pending, &transport_stream_id)| ProducerOutputRegistration {
                    transport_stream_id,
                    cancellation_epoch: cancellation_epoch.filter(|_| {
                        pending.cancelled
                            && !pending.forwarded.as_ref().is_some_and(|mapping| {
                                existing_intents.contains(&StreamRecordReference::Foreign(
                                    mapping.handle.clone(),
                                ))
                            })
                    }),
                    source: match (&pending.forwarded, pending.registered_transport_id) {
                        (Some(mapping), _) => {
                            ProducerOutputSource::Existing(mapping.handle.clone())
                        }
                        (None, Some(transport_stream_id)) => {
                            let mapping = self
                                .mapping(transport_stream_id)
                                .expect("registered output mapping is available");
                            let binding = self
                                .binding(transport_stream_id)
                                .expect("registered output binding is available");
                            ProducerOutputSource::Registered(mapping.handle, binding.source)
                        }
                        (None, None) => ProducerOutputSource::New(
                            requests
                                .next()
                                .expect("each owned output has a registration request"),
                        ),
                    },
                    role: pending
                        .registered_role
                        .or_else(|| pending.forwarded.as_ref().map(|mapping| mapping.role))
                        .unwrap_or(SessionStreamRole::Output),
                },
            )
            .collect::<Vec<_>>();
        let session_key = self.session_key.clone();
        let attribution = self.entity_parent_start_index;
        let ResultStreamRegistration {
            handles: owned_handles,
            ..
        } = admission
            .submit(move |owner, context| async move {
                let result = owner
                    .register_result_streams(
                        Some(&context),
                        session_key,
                        result_bytes,
                        outputs,
                        attribution,
                    )
                    .await?;
                owner.notify_session_records_changed(Some(&context));
                Ok::<_, StreamStoreError>(result)
            })
            .await
            .map_err(SessionError::from)?;

        let mut drains = Vec::with_capacity(pending.len());
        let mut owned_handles = owned_handles.into_iter();
        for (pending, transport_stream_id) in pending.into_iter().zip(transport_stream_ids) {
            let existing_mapping = pending
                .registered_transport_id
                .and_then(|id| self.mapping(id));
            let (handle, local) = match &pending.forwarded {
                Some(mapping) => (mapping.handle.clone(), false),
                None if existing_mapping.is_some() => (
                    existing_mapping
                        .as_ref()
                        .expect("checked above")
                        .handle
                        .clone(),
                    true,
                ),
                None => (
                    owned_handles
                        .next()
                        .expect("result registration returned too few durable handles"),
                    true,
                ),
            };
            let binding = if let Some(mapping) = &existing_mapping {
                self.producer
                    .local_binding(transport_stream_id, &handle, mapping.role)
                    .await
                    .map_err(|error| error.to_string())?
            } else if local {
                self.producer
                    .local_binding(
                        transport_stream_id,
                        &handle,
                        pending.registered_role.unwrap_or(SessionStreamRole::Output),
                    )
                    .await
                    .map_err(SessionError::from)?
            } else {
                StreamBindingRecord {
                    transport_stream_id,
                    source: StreamRecordReference::Foreign(handle.clone()),
                    role: pending
                        .forwarded
                        .as_ref()
                        .map_or(SessionStreamRole::Output, |mapping| mapping.role),
                }
            };
            let mapping = existing_mapping.unwrap_or(StreamSessionMappingRecord {
                transport_stream_id,
                handle: handle.clone(),
                role: binding.role,
            });
            let role = binding.role;
            self.insert_binding_mapping(binding, mapping)?;
            if let Some(endpoint) = pending.endpoint
                && !pending.cancelled
            {
                drains.push(PendingOwnedStreamDrain {
                    activity: self.drain_activity(&endpoint),
                    handle,
                    endpoint,
                    element_type: pending.element_type,
                    role,
                });
            }
        }
        admission.wait_published().await?;
        drop(session_guard);

        Ok(MaterializedResult {
            value: strip_streams(value),
            drains,
        })
    }

    pub(super) async fn drain_materialized_result(
        &self,
        drains: Vec<PendingOwnedStreamDrain>,
        graph: Arc<SchemaGraph>,
        coordinator_source: Option<RuntimeSource>,
        mut coordinator_activity: Option<ExternalActivity>,
    ) -> Result<(), SessionError> {
        if !drains.is_empty() {
            let (nested_tx, mut nested_rx) = mpsc::unbounded_channel();
            let mut tasks = self.producer.tasks().children();
            for drain in drains {
                let streams = self.clone();
                let graph = graph.clone();
                let nested_tx = nested_tx.clone();
                tasks.spawn(async move {
                    if streams.require_root_attachment_before_production {
                        streams.wait_for_active_attachment(&drain.handle).await?;
                    }
                    streams.drain_output(drain, graph, nested_tx).await
                });
            }
            loop {
                while let Ok(drain) = nested_rx.try_recv() {
                    let streams = self.clone();
                    let graph = graph.clone();
                    let nested_tx = nested_tx.clone();
                    tasks.spawn(async move { streams.drain_output(drain, graph, nested_tx).await });
                }
                if tasks.is_empty() {
                    break;
                }
                let wait = async {
                    tokio::select! {
                        Some(drain) = nested_rx.recv() => (Some(drain), None),
                        result = tasks.join_next() => (None, Some(result)),
                    }
                };
                let (drain, result) = match (&mut coordinator_activity, &coordinator_source) {
                    (Some(activity), Some(source)) => activity.coordinate(source, wait).await,
                    _ => wait.await,
                };
                match (drain, result) {
                    (Some(drain), _) => {
                        let streams = self.clone();
                        let graph = graph.clone();
                        let nested_tx = nested_tx.clone();
                        tasks.spawn(
                            async move { streams.drain_output(drain, graph, nested_tx).await },
                        );
                    }
                    (_, Some(result)) => {
                        let task_result = result
                            .expect("durable output drain task set unexpectedly became empty")
                            .map_err(|error| {
                                format!("durable output drain task failed: {error}")
                            })?;
                        task_result?;
                    }
                    _ => unreachable!(),
                }
            }
        }
        Ok(())
    }

    /// Materializes remote result mappings after validating pinned producer identities.
    pub async fn materialize_remote_result(
        &self,
        value: ProtoSchemaValue,
        remote_mappings: Vec<StreamSessionMappingRecord>,
        graph: &SchemaGraph,
        root: &SchemaType,
    ) -> Result<SchemaValue, SessionError> {
        let transport_ids = preflight_proto_recursive_stream_value(&value)?;
        let mut by_transport = HashMap::with_capacity(remote_mappings.len());
        let mut by_handle = HashSet::with_capacity(remote_mappings.len());
        for mapping in remote_mappings {
            if by_transport
                .insert(mapping.transport_stream_id, mapping.clone())
                .is_some()
            {
                return Err(SessionError::from(
                    "durable RPC result contains duplicate transport stream IDs".to_string(),
                ));
            }
            if !by_handle.insert((mapping.handle.clone(), mapping.role)) {
                return Err(SessionError::from(
                    "durable RPC result contains duplicate durable stream handles".to_string(),
                ));
            }
        }
        let referenced_transport_ids = transport_ids.iter().copied().collect::<HashSet<_>>();
        if transport_ids.len() > MAX_NEW_STREAM_HANDLES_PER_VALUE {
            return Err(SessionError::from(
                "ResourceExhausted: durable RPC result materializes more than 256 streams"
                    .to_string(),
            ));
        }
        if referenced_transport_ids.len() != transport_ids.len() {
            return Err(SessionError::from(
                "durable RPC result references a stream more than once".to_string(),
            ));
        }
        if referenced_transport_ids != by_transport.keys().copied().collect::<HashSet<_>>() {
            return Err(SessionError::from(
                "durable RPC result mappings do not exactly match its value".to_string(),
            ));
        }
        if by_transport
            .values()
            .any(|mapping| mapping.role.direction() != SessionStreamRole::Output)
        {
            return Err(SessionError::from(
                "durable RPC result stream has a non-output role".to_string(),
            ));
        }
        let mut schema_transport_ids = Vec::with_capacity(transport_ids.len());
        decode_recursive_stream_value_with_schema(
            value.clone(),
            graph,
            root,
            |transport_id, path| {
                let mapping = by_transport.get(&transport_id).ok_or_else(|| {
                    format!(
                        "durable RPC result references unmapped transport stream {transport_id}"
                    )
                })?;
                let expected_fingerprint =
                    schema_fingerprint_v1(graph, stream_element_schema(graph, root, path)?)
                        .map_err(|error| error.to_string())?;
                if mapping.handle.element_schema_fingerprint != expected_fingerprint {
                    return Err(format!(
                        "durable RPC result stream {transport_id} has the wrong schema fingerprint"
                    ));
                }
                schema_transport_ids.push(transport_id);
                Ok(SchemaValueStream::from_host_endpoint(()))
            },
        )?;
        if schema_transport_ids != transport_ids {
            return Err(SessionError::from(
                "durable RPC result schema traversal changed stream ordering".to_string(),
            ));
        }
        {
            let _session_guard = self.session_lock.lock().await;
            self.recover_session_mappings().await?;
        }
        let new_mapping_count = by_transport
            .values()
            .filter(|mapping| {
                self.mapping_for_reference(
                    &StreamRecordReference::Foreign(mapping.handle.clone()),
                    SessionStreamRole::Output,
                )
                .is_none()
            })
            .count();
        self.producer
            .validate_new_session_stream_count(&self.session_key, new_mapping_count)
            .await
            .map_err(|error| format!("ResourceExhausted: {error}"))?;
        let mut mappings = Vec::with_capacity(transport_ids.len());
        for transport_id in transport_ids {
            let remote = by_transport
                .get(&transport_id)
                .ok_or_else(|| {
                    format!(
                        "durable RPC result references unmapped transport stream {transport_id}"
                    )
                })?
                .clone();
            let mapping = if self.producer.owns_handle_identity(&remote.handle) {
                self.producer
                    .validate_handle(&remote.handle)
                    .await
                    .map_err(SessionError::from)?;
                if let Some(mapping) = self.mapping_for_reference(
                    &StreamRecordReference::Foreign(remote.handle.clone()),
                    SessionStreamRole::Output,
                ) {
                    mapping
                } else {
                    let mapping = StreamSessionMappingRecord {
                        transport_stream_id: self.allocate_transport_stream_id()?,
                        handle: remote.handle,
                        role: SessionStreamRole::Output,
                    };
                    self.insert_mapping(mapping.clone())?;
                    mapping
                }
            } else {
                self.attach_foreign_handle(
                    remote.handle,
                    SessionStreamRole::Output,
                    self.attachment_epoch,
                )
                .await?
            };
            mappings.push(mapping);
        }
        let mut next_handle_index = 0u64;
        let canonical = remap_recursive_stream_references(value, |_, _| {
            let result = next_handle_index;
            next_handle_index = next_handle_index
                .checked_add(1)
                .ok_or_else(|| "durable result handle index overflow".to_string())?;
            Ok(result)
        })?;
        let record = StreamSessionInvocationResultRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: self.session_reference.clone(),
            result: canonical.encode_to_vec(),
            stream_mappings: mappings.iter().map(StreamBindingRecord::foreign).collect(),
        };
        if let Some(existing) = self.remote_result_record().await? {
            if existing != record {
                return Err(SessionError::from(
                    "durable RPC result conflicts with its caller journal".to_string(),
                ));
            }
        } else {
            self.append_record(None, StreamSessionRecord::InvocationResult(record))
                .await?;
            self.commit_consumer_journal().await?;
        }
        self.decode_initial(canonical, &mappings, SessionStreamRole::Output)
            .await
    }

    /// Drains an output whose normal result-leaf mapping was committed during preparation.
    pub(crate) async fn drain_registered_output(
        &self,
        handle: DurableStreamHandle,
        endpoint: LiveStreamEndpoint,
        graph: Arc<SchemaGraph>,
        element_type: SchemaType,
    ) -> Result<(), SessionError> {
        let coordinator_source = self
            .runtime_source
            .clone()
            .or_else(|| endpoint.runtime_source());
        let coordinator_activity = coordinator_source
            .as_ref()
            .map(RuntimeSource::external_activity);
        self.drain_materialized_result(
            vec![PendingOwnedStreamDrain {
                activity: self.drain_activity(&endpoint),
                handle,
                endpoint,
                element_type,
                role: SessionStreamRole::Output,
            }],
            graph,
            coordinator_source,
            coordinator_activity,
        )
        .await
    }

    /// Reconstructs a previously persisted remote result without reissuing the RPC.
    pub async fn replay_remote_result(&self) -> Result<Option<SchemaValue>, SessionError> {
        self.recover_session_mappings().await?;
        let Some(record) = self.remote_result_record().await? else {
            return Ok(None);
        };
        let value = ProtoSchemaValue::decode(record.result.as_slice())
            .map_err(|error| format!("invalid durable caller result: {error}"))?;
        let mappings = self
            .producer
            .materialize_bindings(&record.stream_mappings)
            .await
            .map_err(SessionError::from)?;
        self.decode_initial(value, &mappings, SessionStreamRole::Output)
            .await
            .map(Some)
    }

    pub(super) async fn remote_result_record(
        &self,
    ) -> Result<Option<StreamSessionInvocationResultRecord>, SessionError> {
        let index = self.current_control_metadata().await?.result_position();
        let Some(index) = index else {
            return Ok(None);
        };
        match self.session_record_at(index).await? {
            StreamSessionRecord::InvocationResult(record)
                if record.session_key == self.session_reference =>
            {
                Ok(Some(record))
            }
            _ => Err("durable result metadata points at a different record".into()),
        }
    }

    /// Validates committed bytes and terminals without republishing the historical prefix.
    async fn drain_byte_output(
        &self,
        handle: DurableStreamHandle,
        endpoint: LiveStreamEndpoint,
        mut activity: Option<ExternalActivity>,
    ) -> Result<(), SessionError> {
        let lifecycle = endpoint.lifecycle();
        let runtime_source = endpoint.runtime_source();
        let mut source = endpoint.activate();
        let cancelled = tokio_util::sync::CancellationToken::new();
        let registration_id = self
            .producer
            .register_source_cancellation(handle.stream_id, cancelled.clone());
        let _registration = OutputDrainRegistration {
            producer: self.producer.clone(),
            stream_id: handle.stream_id,
            registration_id,
            lifecycle: lifecycle.clone(),
        };
        let high_water = self
            .producer
            .input_high_water(handle.stream_id)
            .await
            .map_err(SessionError::from)?;
        let mut history = if high_water.is_some() {
            Some(
                self.producer
                    .catch_up(handle.clone(), None)
                    .await
                    .map_err(SessionError::from)?,
            )
        } else {
            None
        };
        let result: Result<(), SessionError> = async {
        let mut pending = None;
        let mut sequence = 0;
        loop {
            let recorded = if let Some(reader) = history.as_mut() {
                let recorded = reader
                    .next()
                    .await
                    .map_err(SessionError::from)?
                    .ok_or_else(|| "byte output history ended before its high water".to_string())?;
                if recorded.producer_sequence != sequence {
                    return Err("byte output history has a non-contiguous sequence".into());
                }
                if matches!(
                    recorded.payload,
                    CommittedProducerStreamEventPayload::Cancel {
                        role: StreamCancelRole::InputConsumer | StreamCancelRole::OutputConsumer,
                        ..
                    }
                ) || (sequence == 0 && matches!(
                    recorded.payload,
                    CommittedProducerStreamEventPayload::Cancel {
                        role: StreamCancelRole::InputProducer,
                        ..
                    }
                ))
                {
                    return Ok(());
                }
                Some(recorded)
            } else {
                None
            };
            let event = match pending.take() {
                Some(event) => event,
                None => tokio::select! {
                    biased;
                    _ = cancelled.cancelled() => return Ok(()),
                    _ = lifecycle.cancelled() => return Ok(()),
                    received = async {
                        match (&mut activity, &runtime_source, recorded.is_none()) {
                            (Some(activity), Some(runtime_source), true) => activity.receive(runtime_source, source.recv()).await,
                            _ => source.recv().await,
                        }
                    } => match received {
                        Ok(event) => event,
                        Err(LiveStreamReceiveError::Closed) if lifecycle.is_aborted() => return Ok(()),
                        Err(error) => {
                            return Err(format!("byte output closed without a terminal: {error:?}").into());
                        }
                    },
                },
            };
            if event.offset != sequence {
                return Err("byte output producer sequence diverged".into());
            }
            let payload = match event.payload {
                LiveStreamEventPayload::Item(SchemaValue::U8(byte)) => {
                    CommittedProducerStreamEventPayload::PackedU8(byte)
                }
                LiveStreamEventPayload::Item(_) => {
                    return Err("byte output contains a non-byte value".into());
                }
                LiveStreamEventPayload::End => {
                    CommittedProducerStreamEventPayload::End(StreamEndResult::Ok)
                }
                LiveStreamEventPayload::Error(error) => CommittedProducerStreamEventPayload::End(
                    StreamEndResult::ErrorContext(error.into_bytes()),
                ),
                LiveStreamEventPayload::ClassifiedError { kind, message } => match kind {
                    HostFailureKind::Permanent => CommittedProducerStreamEventPayload::Cancel {
                        role: StreamCancelRole::System,
                        reason: StreamCancelReason::SourceUnavailable,
                        details: Some(message),
                    },
                    HostFailureKind::Transient => CommittedProducerStreamEventPayload::End(
                        StreamEndResult::ErrorContext(message.into_bytes()),
                    ),
                },
            };
            if let Some(recorded) = recorded {
                if recorded.payload != payload {
                    return Err("byte output differs from its recorded bytes or terminal".into());
                }
                if recorded.is_terminal() {
                    return Ok(());
                }
                if high_water
                    .as_ref()
                    .is_some_and(|water| recorded.offset == water.resulting_offset)
                {
                    history = None;
                }
                sequence += 1;
                continue;
            }
            let terminal = matches!(
                payload,
                CommittedProducerStreamEventPayload::End(_)
                    | CommittedProducerStreamEventPayload::Cancel { .. }
            );
            let written = match payload {
                CommittedProducerStreamEventPayload::PackedU8(byte) => {
                    let first_sequence = sequence;
                    let mut bytes = vec![byte];
                    sequence += 1;
                    let deadline = tokio::time::Instant::now() + PACKED_U8_OUTPUT_FLUSH_DELAY;
                    while bytes.len() < MAX_PACKED_U8_STREAM_ITEM_SIZE {
                        let next = match tokio::time::timeout_at(deadline, source.recv()).await {
                            Ok(Ok(event)) => event,
                            Ok(Err(LiveStreamReceiveError::Closed)) | Err(_) => break,
                            Ok(Err(error)) => return Err(format!("{error:?}").into()),
                        };
                        if next.offset != sequence {
                            return Err("byte output producer sequence diverged".into());
                        }
                        match next.payload {
                            LiveStreamEventPayload::Item(SchemaValue::U8(byte)) => {
                                bytes.push(byte);
                                sequence += 1;
                            }
                            payload => {
                                pending = Some(crate::durable_host::stream_bus::LiveStreamEvent {
                                    offset: next.offset,
                                    payload,
                                });
                                break;
                            }
                        }
                    }
                    self.producer
                        .write_items_with_nested_sources(
                            None,
                            handle.stream_id,
                            first_sequence,
                            StreamItemsPayload::PackedU8(bytes),
                            Vec::new(),
                        )
                        .await
                        .map(|_| ())
                }
                CommittedProducerStreamEventPayload::End(result) => self
                    .producer
                    .end(None, handle.stream_id, sequence, result)
                    .await
                    .map(|_| ()),
                CommittedProducerStreamEventPayload::Cancel { role, reason, details } => self
                    .producer
                    .cancel(handle.stream_id, sequence, role, reason, details)
                    .await
                    .map(|_| ()),
                _ => unreachable!("byte output produces only bytes and terminals"),
            };
            match written {
                Err(StreamStoreError::FencedByTerminal(
                    CommittedProducerStreamEventPayload::Cancel {
                        role: StreamCancelRole::InputConsumer | StreamCancelRole::OutputConsumer,
                        ..
                    },
                )) => return Ok(()),
                Err(error) => return Err(error.into()),
                Ok(()) if terminal => return Ok(()),
                Ok(()) => {}
            }
        }
        }.await;
        if let Err(error) = &result
            && history.is_none()
        {
            let _ = self
                .producer
                .end_open(
                    None,
                    handle.stream_id,
                    StreamEndResult::ErrorContext(error.to_string().into_bytes()),
                )
                .await;
        }
        result
    }

    pub(super) async fn drain_output(
        &self,
        drain: PendingOwnedStreamDrain,
        graph: Arc<SchemaGraph>,
        nested_tx: mpsc::UnboundedSender<PendingOwnedStreamDrain>,
    ) -> Result<(), SessionError> {
        let source = self
            .runtime_source
            .clone()
            .or_else(|| drain.endpoint.runtime_source());
        StreamWriteAdmission::account_output_mutations(
            source,
            self.drain_output_accounted(drain, graph, nested_tx),
        )
        .await
    }

    async fn drain_output_accounted(
        &self,
        drain: PendingOwnedStreamDrain,
        graph: Arc<SchemaGraph>,
        nested_tx: mpsc::UnboundedSender<PendingOwnedStreamDrain>,
    ) -> Result<(), SessionError> {
        let PendingOwnedStreamDrain {
            handle,
            endpoint,
            element_type,
            role,
            mut activity,
        } = drain;
        if matches!(graph.resolve_ref(&element_type), Ok(SchemaType::U8 { .. })) {
            return self.drain_byte_output(handle, endpoint, activity).await;
        }
        // Root drains require admission; children are admitted by their committed parent item.
        // A child returned unread to its producer is consumed locally, without an attachment.
        let lifecycle = endpoint.lifecycle();
        let runtime_source = endpoint.runtime_source();
        let mut source = endpoint.activate();
        let source_cancelled = tokio_util::sync::CancellationToken::new();
        let registration_id = self
            .producer
            .register_source_cancellation(handle.stream_id, source_cancelled.clone());
        let _drain_registration = OutputDrainRegistration {
            producer: self.producer.clone(),
            stream_id: handle.stream_id,
            registration_id,
            lifecycle: lifecycle.clone(),
        };
        if self
            .producer
            .stream_head(&handle)
            .await
            .map_err(SessionError::from)?
            .cancelled
            && self
                .producer
                .input_high_water(handle.stream_id)
                .await
                .map_err(SessionError::from)?
                .is_some_and(|water| water.highest_contiguous_sequence == 0)
        {
            return Ok(());
        }
        let mut next_sequence = 0;
        loop {
            let received = tokio::select! {
                    biased;
                    _ = source_cancelled.cancelled() => {
                        break;
                    },
                    _ = lifecycle.cancelled() => {
                        break;
                    },
                    received = async {
                        match (&mut activity, &runtime_source) {
                            (Some(activity), Some(runtime_source)) => activity.receive(runtime_source, source.recv()).await,
                            _ => source.recv().await,
                        }
                    } => received,
            };
            let event = match received {
                Ok(event) => event,
                Err(LiveStreamReceiveError::Closed) if lifecycle.is_aborted() => {
                    tracing::debug!(
                        stream_id = %handle.stream_id,
                        role = ?role,
                        "Durable stream output drain closed after runtime teardown"
                    );
                    break;
                }
                Err(error) => {
                    let _ = self
                        .producer
                        .end(
                            None,
                            handle.stream_id,
                            next_sequence,
                            StreamEndResult::ErrorContext(format!("{error:?}").into_bytes()),
                        )
                        .await;
                    break;
                }
            };
            let is_item = matches!(&event.payload, LiveStreamEventPayload::Item(_));
            next_sequence = event.offset.saturating_add(1);
            let result = match event.payload {
                LiveStreamEventPayload::Item(value) => {
                    struct NestedOutput<F> {
                        endpoint: Option<LiveStreamEndpoint>,
                        forwarded: Option<F>,
                        element_type: SchemaType,
                        registration: ProducerRegistrationRequest,
                    }
                    enum PublicationError {
                        Forwarding(String),
                        Preparation(SessionError),
                        Store(StreamStoreError),
                    }
                    impl From<StreamStoreError> for PublicationError {
                        fn from(error: StreamStoreError) -> Self {
                            Self::Store(error)
                        }
                    }

                    if let Err(error) = preflight_recursive_stream_value(&value).and_then(|_| {
                        validate_forwarded_durable_input_schemas(
                            &value,
                            &graph,
                            &element_type,
                            "forwarded nested durable handle does not match the stream item schema",
                        )
                    }) {
                        let _ = self
                            .producer
                            .end(
                                None,
                                handle.stream_id,
                                event.offset,
                                StreamEndResult::ErrorContext(error.into_bytes()),
                            )
                            .await;
                        break;
                    }
                    let mut nested_outputs = Vec::new();
                    let value = match encode_recursive_stream_value_with_schema(
                        &value,
                        &graph,
                        &element_type,
                        |stream, path| {
                            let nested_element =
                                stream_element_schema(&graph, &element_type, path)?.cloned();
                            let element_schema_fingerprint =
                                schema_fingerprint_v1(&graph, nested_element.as_ref())
                                    .map_err(|error| error.to_string())?;
                            let forwarded = forwarded_durable_input_reference(stream)?;
                            let (endpoint, forwarded) = match forwarded {
                                Some(forwarded) => (None, Some(forwarded.take(stream)?)),
                                None => (
                                    Some(stream.take_host_endpoint::<LiveStreamEndpoint>()?),
                                    None,
                                ),
                            };
                            let canonical_handle_index = u64::try_from(nested_outputs.len())
                                .map_err(|_| "durable nested handle index overflow".to_string())?;
                            nested_outputs.push(NestedOutput {
                                endpoint,
                                forwarded,
                                element_type: nested_element.unwrap_or_else(SchemaType::u8),
                                registration: ProducerRegistrationRequest {
                                    entity_parent_start_index: self.entity_parent_start_index,
                                    coordinate: StreamRegistrationCoordinate::Nested {
                                        parent_stream_id: handle.stream_id,
                                        parent_producer_sequence: event.offset,
                                        recursive_value_path: path.to_vec(),
                                    },
                                    source_invocation: self.session_reference.clone(),
                                    component_revision: handle.component_revision,
                                    element_schema_fingerprint,
                                    source_kind: StreamSourceKind::Nested,
                                    session_mapping: None,
                                },
                            });
                            Ok(canonical_handle_index)
                        },
                    ) {
                        Ok(value) => value,
                        Err(error) => {
                            let _ = self
                                .producer
                                .end(
                                    None,
                                    handle.stream_id,
                                    event.offset,
                                    StreamEndResult::ErrorContext(error.into_bytes()),
                                )
                                .await;
                            break;
                        }
                    };
                    let payload = StreamItemsPayload::Values(vec![value.encode_to_vec()]);
                    let memory = DurableStreamStore::retained_payload_bytes(&payload)?;
                    let session = self.clone();
                    let stream_id = handle.stream_id;
                    let parent_handle = handle.clone();
                    match self
                        .producer
                        .run_admitted(None, memory, false, move |_, admission| async move {
                            let _session_guard = session.session_lock.lock().await;
                            let prepare_session = session.clone();
                            let (nested_outputs, may_append) = admission
                                .submit(move |owner, context| async move {
                                    let may_append = owner
                                        .input_high_water(stream_id)
                                        .await?
                                        .map_or(event.offset == 0, |water| {
                                            !water.terminal
                                                && water.highest_contiguous_sequence.checked_add(1)
                                                    == Some(event.offset)
                                        });
                                    let mut prepared = Vec::with_capacity(nested_outputs.len());
                                    for (handle_index, output) in nested_outputs.into_iter().enumerate() {
                                        let forwarded = match output.forwarded {
                                            Some(forwarded) => {
                                                let parent = owner.local_binding(0, &parent_handle, role).await?;
                                                let StreamRecordReference::Local(parent_stream) = parent.source else {
                                                    unreachable!("producer item parent is owner-local")
                                                };
                                                Some(forwarded
                                                    .prepare_mapping_owned(
                                                        &context,
                                                        &prepare_session,
                                                        role,
                                                        StreamReaderForwardPublication::ProducerItem {
                                                            parent_stream,
                                                            sequence: event.offset,
                                                            handle_index: handle_index as u64,
                                                        },
                                                    )
                                                    .await
                                                    .map_err(PublicationError::Preparation)?)
                                            }
                                            None => None,
                                        };
                                        prepared.push(NestedOutput {
                                            endpoint: output.endpoint,
                                            forwarded,
                                            element_type: output.element_type,
                                            registration: output.registration,
                                        });
                                    }
                                    Ok::<_, PublicationError>((prepared, may_append))
                                })
                                .await?;
                            if may_append {
                                for output in &nested_outputs {
                                    if let Some(mapping) = &output.forwarded {
                                        session
                                            .activate_nested_mapping_under_lock(
                                                &admission,
                                                mapping.clone(),
                                            )
                                            .await
                                            .map_err(|error| match error {
                                                StreamStoreError::InvalidHandle => {
                                                    PublicationError::Forwarding(error.to_string())
                                                }
                                                error => PublicationError::Store(error),
                                            })?;
                                    }
                                }
                            }
                            let nested_sources = nested_outputs
                                .iter()
                                .map(|output| match &output.forwarded {
                                    Some(mapping) => {
                                        NestedStreamWrite::Forward(mapping.handle.clone())
                                    }
                                    None => {
                                        NestedStreamWrite::Register(output.registration.clone())
                                    }
                                })
                                .collect();
                            let outcome = admission
                                .submit(move |owner, context| async move {
                                    owner
                                        .write_items_with_nested_sources(
                                            Some(&context),
                                            stream_id,
                                            event.offset,
                                            payload,
                                            nested_sources,
                                        )
                                        .await
                                })
                                .await;
                            let outcome = match outcome {
                                Ok(outcome) => Some(outcome),
                                Err(
                                    StreamStoreError::FencedByTerminal(_)
                                    | StreamStoreError::SessionFinished(_),
                                ) => None,
                                Err(
                                    StreamStoreError::TraversalDepthLimit
                                    | StreamStoreError::ValueStreamLimit
                                    | StreamStoreError::StreamLimit
                                    | StreamStoreError::CounterOverflow,
                                ) if may_append => None,
                                Err(error @ StreamStoreError::ItemTooLarge) if may_append => {
                                    return Err(PublicationError::Forwarding(error.to_string()));
                                }
                                Err(error) => return Err(PublicationError::Store(error)),
                            };
                            admission.wait_published().await?;
                            Ok::<_, PublicationError>((outcome, nested_outputs))
                        })
                        .await
                    {
                        Ok((None, _)) => break,
                        Ok((Some(outcome), nested_outputs)) => {
                            tracing::debug!(
                                stream_id = %handle.stream_id,
                                role = ?role,
                                first_sequence = event.offset,
                                replayed = outcome.replayed,
                                "Durable stream output items committed"
                            );
                            if !nested_outputs.is_empty() {
                                let nested_handles = self
                                    .producer
                                    .nested_handles(handle.stream_id, event.offset)
                                    .await
                                    .map_err(SessionError::from)?;
                                if nested_handles.len() != nested_outputs.len() {
                                    return Err(SessionError::from("nested output stream mapping count does not match durable item metadata".to_string()));
                                }
                                let memory =
                                    golem_common::serialization::serialize(&nested_handles)?.len();
                                let session = self.clone();
                                let nested_tx = nested_tx.clone();
                                self.producer
                                    .run_admitted(
                                        None,
                                        memory,
                                        false,
                                        move |_, admission| async move {
                                            let _session_guard = session.session_lock.lock().await;
                                            session.recover_session_mappings().await?;
                                            let session_for_write = session.clone();
                                            admission
                                                .submit(move |_, context| async move {
                                                    let session = session_for_write;
                                                    session.recover_session_mappings().await?;
                                                    for (output, nested_handle) in nested_outputs
                                                        .into_iter()
                                                        .zip(nested_handles)
                                                    {
                                                        if output.forwarded.is_some() {
                                                            continue;
                                                        }
                                                        let mut binding = session
                                                            .producer
                                                            .local_binding(0, &nested_handle, role)
                                                            .await
                                                            .map_err(SessionError::from)?;
                                                        let transport_stream_id = session
                                                            .mapping_for_reference(
                                                                &binding.source,
                                                                role,
                                                            )
                                                            .map(|mapping| {
                                                                mapping.transport_stream_id
                                                            })
                                                            .map(Ok)
                                                            .unwrap_or_else(|| {
                                                                session
                                                                    .allocate_transport_stream_id()
                                                            })?;
                                                        let mapping = StreamSessionMappingRecord {
                                                            transport_stream_id,
                                                            handle: nested_handle.clone(),
                                                            role,
                                                        };
                                                        binding.transport_stream_id =
                                                            transport_stream_id;
                                                        session.insert_binding_mapping(
                                                            binding.clone(),
                                                            mapping.clone(),
                                                        )?;
                                                        session
                                                            .append_mapping_once(&context, binding)
                                                            .await?;
                                                        if let Some(endpoint) = output.endpoint {
                                                            nested_tx
                                                    .send(PendingOwnedStreamDrain {
                                                        activity: session.drain_activity(&endpoint),
                                                        handle: nested_handle,
                                                        endpoint,
                                                        element_type: output.element_type,
                                                        role,
                                                    })
                                                    .map_err(|_| {
                                                        "durable output drain coordinator stopped"
                                                            .to_string()
                                                    })?;
                                                        }
                                                    }
                                                    Ok::<_, SessionError>(())
                                                })
                                                .await
                                        },
                                    )
                                    .await?;
                            }
                            Ok(())
                        }
                        Err(PublicationError::Store(error)) => return Err(error.into()),
                        Err(PublicationError::Preparation(error)) => return Err(error),
                        Err(PublicationError::Forwarding(error)) => Err(error.into()),
                    }
                }
                LiveStreamEventPayload::End => self
                    .producer
                    .end(None, handle.stream_id, event.offset, StreamEndResult::Ok)
                    .await
                    .map(|_| ())
                    .map_err(SessionError::from),
                LiveStreamEventPayload::Error(error) => self
                    .producer
                    .end(
                        None,
                        handle.stream_id,
                        event.offset,
                        StreamEndResult::ErrorContext(error.into_bytes()),
                    )
                    .await
                    .map(|_| ())
                    .map_err(SessionError::from),
                LiveStreamEventPayload::ClassifiedError { kind, message } => match kind {
                    HostFailureKind::Permanent => self
                        .producer
                        .cancel(
                            handle.stream_id,
                            event.offset,
                            StreamCancelRole::System,
                            StreamCancelReason::SourceUnavailable,
                            Some(message),
                        )
                        .await
                        .map(|_| ())
                        .map_err(SessionError::from),
                    HostFailureKind::Transient => self
                        .producer
                        .end(
                            None,
                            handle.stream_id,
                            event.offset,
                            StreamEndResult::ErrorContext(message.into_bytes()),
                        )
                        .await
                        .map(|_| ())
                        .map_err(SessionError::from),
                },
            };
            if let Err(error) = result {
                self.producer
                    .end_open(
                        None,
                        handle.stream_id,
                        StreamEndResult::ErrorContext(error.to_string().into_bytes()),
                    )
                    .await?;
                break;
            }
            if !is_item {
                break;
            }
        }
        Ok(())
    }

    pub(super) async fn finish(
        &self,
        result: Result<(), Vec<u8>>,
        input_cancel_reason: StreamCancelReason,
    ) -> Result<(), SessionError> {
        if self.has_committed_finished().await? {
            return Ok(());
        }
        let session = self.clone();
        let retained_bytes = DurableStreamStore::finish_session_retained_bytes(&result);
        let outcome = self
            .producer
            .run_admitted_accounted(
                None,
                retained_bytes,
                true,
                self.runtime_source
                    .as_ref()
                    .map(RuntimeSource::external_activity),
                move |_, admission| async move {
                    let _session_guard = session.session_lock.lock().await;
                    session.validate_topology_complete().await?;
                    let session_for_write = session.clone();
                    let outcome = admission
                        .submit(move |owner, context| async move {
                            owner
                                .finish_session(
                                    Some(&context),
                                    session_for_write.session_key.clone(),
                                    session_for_write.entity_parent_start_index,
                                    result,
                                    input_cancel_reason,
                                )
                                .await
                                .map_err(SessionError::from)
                        })
                        .await;
                    admission.wait_published().await?;
                    outcome
                },
            )
            .await;
        match outcome {
            Ok(()) => Ok(()),
            Err(error) => match self.has_committed_finished().await {
                Ok(true) => Ok(()),
                Ok(false) => Err(error),
                Err(_) if matches!(error, SessionError::Fenced(_)) => Err(error),
                Err(lookup_error) => Err(SessionError::from(format!(
                    "{error}; committed finish lookup failed: {lookup_error}"
                ))),
            },
        }
    }

    async fn has_committed_finished(&self) -> Result<bool, SessionError> {
        let Some(journal) = &self.consumer_journal else {
            return Ok(false);
        };
        let Some(index) = journal.committed_finished_index(&self.session_key).await? else {
            return Ok(false);
        };
        match self.session_record_at(index).await? {
            StreamSessionRecord::Finished(record)
                if record.session_key == self.session_reference =>
            {
                Ok(true)
            }
            _ => Err("committed finished metadata points at a different session record".into()),
        }
    }

    pub(super) async fn validate_topology_complete(&self) -> Result<(), SessionError> {
        let metadata = self.current_control_metadata().await?;
        metadata
            .validate_topology_complete()
            .map_err(SessionError::from)
    }

    /// Records a failed session terminal after cancelling remaining stream work.
    pub async fn fail(&self, details: String) -> Result<(), SessionError> {
        self.finish(
            Err(details.into_bytes()),
            StreamCancelReason::InvocationFailed,
        )
        .await
    }

    /// Finalizes streams with invocation-failed semantics and records session failure.
    pub async fn fail_invocation(&self, details: String) -> Result<(), SessionError> {
        self.finish(
            Err(details.into_bytes()),
            StreamCancelReason::InvocationFailed,
        )
        .await
    }

    /// Finalizes streams with protocol-error semantics and records session failure.
    pub async fn fail_protocol(&self, details: String) -> Result<(), SessionError> {
        self.finish(Err(details.into_bytes()), StreamCancelReason::Protocol)
            .await
    }

    /// Records successful session completion once all protocol terminals are durable.
    pub async fn complete(&self) -> Result<(), SessionError> {
        self.finish(Ok(()), StreamCancelReason::GuestDrop).await
    }

    /// Completes now unless forwarded inputs still require later terminal processing.
    pub async fn complete_or_defer_for_forwarded_inputs(&self) -> Result<(), SessionError> {
        if self
            .producer
            .has_open_forwarded_session_input(&self.session_key)
            .await
            .map_err(SessionError::from)?
        {
            Ok(())
        } else {
            self.complete().await
        }
    }

    /// Reads the committed invocation result, independently of stream drain progress.
    pub async fn persisted_result(&self) -> Result<Option<SessionValue>, SessionError> {
        if let Some(result) = self.remote_result_record().await? {
            let bindings = result.stream_mappings.clone();
            let value = SessionValue::from_persisted(&self.producer, result).await?;
            for (binding, mapping) in bindings.into_iter().zip(&value.mappings) {
                self.insert_binding_mapping(binding, mapping.clone())?;
            }
            return Ok(Some(value));
        }
        Ok(None)
    }

    /// Waits for a committed invocation result or session failure.
    pub async fn wait_persisted_result(&self) -> Result<SessionValue, SessionError> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(result) = self.persisted_result().await? {
                return Ok(result);
            }
            changed.await;
        }
    }

    /// Reads the committed session terminal without waiting.
    pub async fn persisted_finished(&self) -> Result<Option<Result<(), Vec<u8>>>, SessionError> {
        let index = self.current_control_metadata().await?.finished_position();
        let Some(index) = index else {
            return Ok(None);
        };
        match self.session_record_at(index).await? {
            StreamSessionRecord::Finished(record)
                if record.session_key == self.session_reference =>
            {
                Ok(Some(record.result))
            }
            _ => Err("durable finished metadata points at a different record".into()),
        }
    }

    /// Waits until successful or failed session completion is durable.
    pub async fn wait_persisted_finished(&self) -> Result<Result<(), Vec<u8>>, SessionError> {
        loop {
            let changed = self.producer.session_records_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            if let Some(result) = self.persisted_finished().await? {
                return Ok(result);
            }
            changed.await;
        }
    }
}

impl Drop for OutputDrainRegistration {
    fn drop(&mut self) {
        self.producer
            .unregister_source_cancellation(self.stream_id, self.registration_id);
        self.lifecycle.finish();
    }
}

impl SessionValue {
    pub(super) async fn from_persisted(
        producer: &DurableStreamStore,
        result: StreamSessionInvocationResultRecord,
    ) -> Result<Self, SessionError> {
        let value = ProtoSchemaValue::decode(result.result.as_slice())
            .map_err(|error| format!("invalid persisted durable invocation result: {error}"))?;
        let value = remap_recursive_stream_references(value, |handle_index, _| {
            let index = usize::try_from(handle_index)
                .map_err(|_| format!("durable result handle index {handle_index} is too large"))?;
            result
                .stream_mappings
                .get(index)
                .map(|mapping| mapping.transport_stream_id)
                .ok_or_else(|| format!("unknown durable result handle index {handle_index}"))
        })?;
        let mappings = producer
            .materialize_bindings(&result.stream_mappings)
            .await
            .map_err(SessionError::from)?;
        Ok(Self { value, mappings })
    }

    /// Encodes this value's mappings when sending it over the invocation transport.
    pub fn proto_mappings(&self) -> Vec<DurableStreamMapping> {
        self.mappings
            .iter()
            .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
            .collect()
    }
}
