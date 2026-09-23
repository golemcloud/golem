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
use crate::durable_host::durable_stream::{
    CommittedProducerStreamEventPayload, DurableStreamStore, ExternalAppendOutcome,
    ExternalProducer, StreamHandleReadResult, StreamStoreError, StreamWriteAdmission,
};
use golem_api_grpc::proto::golem::schema::{SchemaValue as ProtoValue, schema_value};
use golem_common::model::DurableStreamPublicBinding;
use golem_common::model::ScheduledAction;
use golem_common::model::durable_stream::{
    DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle, DurableStreamReadRequest,
    ExternalProducerId, PersistedInvocationTarget, StreamHandleReadRequest, StreamItemsPayload,
    StreamOffset, StreamRegistrationInvocation, StreamSessionExpiredRecord,
    StreamSessionExpiryPolicy, StreamSessionExpiryRefreshedRecord, StreamSessionKey,
};
use golem_common::model::invocation_session_public::validate_durable_stream_session_id;
use golem_common::schema::{
    AgentMethodSchema, FieldSource, OutputSchema, SchemaGraph, SchemaType, SchemaValue,
};
use golem_schema::schema::fingerprint::schema_fingerprint_v1;
use golem_schema::schema::validation::validate_value;
use prost::Message;

/// Domain result of creating or replay-attaching a stream session.
pub struct CreateStreamSessionResult {
    pub session: String,
    pub replayed: bool,
    pub component_revision: ComponentRevision,
    pub invocation_key: IdempotencyKey,
    pub expiry_policy: StreamSessionExpiryPolicy,
    pub expiry_deadline_millis: Option<u64>,
}

#[derive(Clone, Copy)]
pub enum StreamSessionCreationIntent {
    ExplicitPut,
    LazyPost,
}

pub struct StreamSessionCreationAdmission {
    _guard: OwnedMutexGuard<()>,
    public_session_id: String,
    invocation_key: IdempotencyKey,
    expiry_policy: StreamSessionExpiryPolicy,
    expiry_deadline_millis: Option<u64>,
}

#[derive(Clone)]
struct PublicStreamSessionBinding {
    invocation_key: IdempotencyKey,
    expiry_policy: StreamSessionExpiryPolicy,
    expiry_deadline_millis: Option<u64>,
}

enum StreamSessionExpiryTransition {
    Stale,
    Early,
    Applied,
    AlreadyApplied,
}

impl StreamSessionCreationAdmission {
    pub fn invocation_key(&self) -> &IdempotencyKey {
        &self.invocation_key
    }
}

/// Domain request for reading one invocation stream slot.
pub struct ReadStreamSlotRequest {
    pub session: String,
    pub slot: String,
    pub from_offset: Option<StreamOffset>,
    pub max_items: u32,
    pub max_bytes: u64,
    pub wait_millis: u64,
    pub expected_method: String,
    pub admission: StreamSlotReadAdmission,
}

pub enum StreamSlotReadAdmission {
    TouchingOriginGet,
    Head,
    Continuation(IdempotencyKey),
}

/// One domain item returned by a stream-slot read.
pub struct StreamSlotItem {
    pub offset: StreamOffset,
    pub content: StreamSlotItemContent,
}

/// Encoding selected by the slot's pinned element schema.
pub enum StreamSlotItemContent {
    Value(Vec<u8>),
    PackedU8(Vec<u8>),
}

/// Domain result of reading one invocation stream slot.
pub struct ReadStreamSlotResult {
    pub items: Vec<StreamSlotItem>,
    pub next_offset: Option<StreamOffset>,
    pub closed: bool,
    pub cancelled: bool,
    pub element_schema: SchemaGraph,
    pub content_type: &'static str,
    pub up_to_date: bool,
    pub head_offset: Option<StreamOffset>,
    pub stream_identity: String,
    pub slots: Vec<String>,
    pub tombstoned: bool,
    pub writable: bool,
    pub fork: Option<golem_api_grpc::proto::golem::workerexecutor::v1::ForkStreamSlotSuccess>,
    pub invocation_key: IdempotencyKey,
    pub expiry_policy: StreamSessionExpiryPolicy,
    pub expiry_deadline_millis: Option<u64>,
}

/// Domain target for cancelling a session or tombstoning one export slot.
pub struct ExportStreamControlRequest {
    pub session: String,
    pub slot: Option<String>,
    pub expected_method: String,
}

/// Stable outcome of an export stream control operation.
pub enum ExportStreamControlResult {
    Applied,
    NotFound,
    Gone,
}

/// Domain payload accepted by an input stream slot.
pub enum AppendStreamSlotPayload {
    Values(Vec<Vec<u8>>),
    PackedU8(Vec<u8>),
}

/// Client producer coordinates for idempotent external appends.
pub struct StreamSlotProducer {
    pub id: String,
    pub epoch: u64,
    pub sequence: u64,
}

/// Domain request for appending to one input stream slot.
pub struct AppendToStreamSlotRequest {
    pub session: String,
    pub slot: String,
    pub payload: Option<AppendStreamSlotPayload>,
    pub close: bool,
    pub producer: Option<StreamSlotProducer>,
    pub expected_method: String,
}

/// Domain outcome of an input stream-slot append.
pub enum AppendStreamSlotOutcome {
    Accepted(StreamOffset),
    Duplicate {
        offset: StreamOffset,
        highest_sequence: Option<u64>,
    },
    EpochFenced(u64),
    SequenceGap {
        expected: u64,
        received: u64,
    },
    Closed,
    NotFound,
    Gone,
    ReadOnly,
}

pub struct AppendToStreamSlotResult {
    pub outcome: AppendStreamSlotOutcome,
    pub invocation_key: Option<IdempotencyKey>,
    pub expiry_policy: Option<StreamSessionExpiryPolicy>,
    pub expiry_deadline_millis: Option<u64>,
    pub stream_head_offset: Option<StreamOffset>,
    pub stream_closed: Option<bool>,
}

impl AppendToStreamSlotResult {
    fn not_found() -> Self {
        Self {
            outcome: AppendStreamSlotOutcome::NotFound,
            invocation_key: None,
            expiry_policy: None,
            expiry_deadline_millis: None,
            stream_head_offset: None,
            stream_closed: None,
        }
    }
}

struct Slot {
    session: StreamSessionKey,
    name: String,
    slots: Vec<String>,
    graph: SchemaGraph,
    writable: bool,
    bytes: bool,
    source: SlotSource,
}

pub(crate) struct ExportForkSlot {
    pub(crate) handle: Option<DurableStreamHandle>,
    pub(crate) expiry_policy: StreamSessionExpiryPolicy,
    pub(crate) writable: bool,
    pub(crate) bytes: bool,
    pub(crate) tombstoned: bool,
    pub(crate) graph: SchemaGraph,
    pub(crate) snapshot: Option<crate::durable_host::durable_stream::ExportForkStreamSnapshot>,
}

enum SlotSource {
    Stream(DurableStreamHandle),
    Value {
        encoded: Vec<u8>,
        offset: StreamOffset,
    },
    Pending {
        finished: bool,
    },
    Tombstoned,
}

#[derive(Debug, PartialEq)]
struct SlotSchema {
    element: SchemaType,
    field_index: Option<usize>,
    writable: bool,
    is_stream: bool,
}

fn append_error(error: StreamStoreError) -> WorkerExecutorError {
    match error {
        StreamStoreError::InvalidValueBatch
        | StreamStoreError::ItemTooLarge
        | StreamStoreError::InvalidPackedU8Batch
        | StreamStoreError::InvalidHandle
        | StreamStoreError::UnknownStream(_) => {
            WorkerExecutorError::invalid_request(error.to_string())
        }
        // A refused write keeps its type, so the caller is sent to the shard's new owner.
        error => error.into_worker_executor_error(WorkerExecutorError::runtime),
    }
}

impl SlotSchema {
    /// Looks up a canonical input or output slot in the method's pinned schema.
    fn lookup(
        graph: &SchemaGraph,
        method: &AgentMethodSchema,
        name: &str,
    ) -> Result<Option<Self>, WorkerExecutorError> {
        let resolve = |ty| {
            graph
                .resolve_ref(ty)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))
        };
        for (index, field) in method
            .input_schema
            .fields()
            .iter()
            .filter(|field| matches!(field.source, FieldSource::UserSupplied))
            .enumerate()
        {
            if field.name == name
                && let SchemaType::Stream {
                    inner: Some(element),
                    ..
                } = resolve(&field.schema)?
            {
                return Ok(Some(Self {
                    element: (**element).clone(),
                    field_index: Some(index),
                    writable: true,
                    is_stream: true,
                }));
            }
        }
        let OutputSchema::Single(output) = &method.output_schema else {
            return Ok(None);
        };
        match resolve(output)? {
            SchemaType::Stream {
                inner: Some(element),
                ..
            } if name == "$result" => Ok(Some(Self {
                element: (**element).clone(),
                field_index: None,
                writable: false,
                is_stream: true,
            })),
            SchemaType::Record { fields, .. } => {
                if let Some((index, field)) = fields
                    .iter()
                    .enumerate()
                    .find(|(_, field)| field.name == name)
                    && let SchemaType::Stream {
                        inner: Some(element),
                        ..
                    } = resolve(&field.body)?
                {
                    return Ok(Some(Self {
                        element: (**element).clone(),
                        field_index: Some(index),
                        writable: false,
                        is_stream: true,
                    }));
                }
                if name == "$result"
                    && !golem_common::schema::agent::contains_stream_in_graph(graph, output)
                {
                    Ok(Some(Self {
                        element: (**output).clone(),
                        field_index: None,
                        writable: false,
                        is_stream: false,
                    }))
                } else {
                    Ok(None)
                }
            }
            _ if name == "$result"
                && !golem_common::schema::agent::contains_stream_in_graph(graph, output) =>
            {
                Ok(Some(Self {
                    element: (**output).clone(),
                    field_index: None,
                    writable: false,
                    is_stream: false,
                }))
            }
            _ => Ok(None),
        }
    }

    /// Extracts this slot's canonical durable handle from a persisted value.
    fn extract_handle(
        &self,
        encoded: &[u8],
        handles: &[DurableStreamHandle],
    ) -> Result<DurableStreamHandle, WorkerExecutorError> {
        let mut value = ProtoValue::decode(encoded)
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        if let Some(index) = self.field_index {
            let Some(schema_value::Value::RecordValue(record)) = value.value else {
                return Err(WorkerExecutorError::runtime(
                    "persisted slot value is not a record",
                ));
            };
            value =
                record.fields.get(index).cloned().ok_or_else(|| {
                    WorkerExecutorError::runtime("persisted slot field is missing")
                })?;
        }
        let Some(schema_value::Value::StreamReference(reference)) = value.value else {
            return Err(WorkerExecutorError::runtime(
                "persisted slot is not a stream reference",
            ));
        };
        usize::try_from(reference.stream_id)
            .ok()
            .and_then(|index| handles.get(index))
            .cloned()
            .ok_or_else(|| WorkerExecutorError::runtime("persisted slot has no durable handle"))
    }
}

impl<Ctx: WorkerCtx> Worker<Ctx> {
    async fn stream_session_content_key(
        &self,
        invocation_key: &IdempotencyKey,
    ) -> Result<IdempotencyKey, WorkerExecutorError> {
        Ok(self
            .durable_stream_session_status(invocation_key)
            .await?
            .and_then(|status| status.export_source_invocation)
            .map_or_else(|| invocation_key.clone(), |source| source.idempotency_key))
    }

    async fn commit_stream_session_expiry_owned(
        self: &Arc<Self>,
        owner: &Arc<DurableStreamStore>,
        admission: &Arc<StreamWriteAdmission>,
        public_session_id: String,
        invocation_key: IdempotencyKey,
        expected_deadline_millis: u64,
        expired_at_millis: u64,
    ) -> Result<(), AdmittedTouchError> {
        let content_key = self.stream_session_content_key(&invocation_key).await?;
        let Some(prepared) = self.prepared_stream_session(&content_key).await? else {
            return Err(WorkerExecutorError::runtime(
                "expiring durable stream session has no Prepared record",
            )
            .into());
        };
        let session_reference = StreamRegistrationInvocation::Local(content_key);
        let streams = StreamSession::open(
            owner.clone(),
            self.oplog.clone(),
            session_reference.clone(),
            prepared.stream_mappings.iter().cloned(),
        )
        .await
        .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
        let metadata = streams
            .current_control_metadata()
            .await
            .map_err(WorkerExecutorError::runtime)?;
        let epoch = streams
            .authoritative_attachment_state()
            .await
            .map_err(WorkerExecutorError::runtime)?
            .epoch;
        let mut records = metadata
            .cancellation_records(epoch, &session_reference)
            .map_err(WorkerExecutorError::runtime)?
            .unwrap_or_default();
        records.insert(
            0,
            StreamSessionRecord::Expired(StreamSessionExpiredRecord {
                format_version: DURABLE_STREAM_FORMAT_VERSION,
                session_key: invocation_key,
                public_session_id,
                expected_deadline_millis,
                expired_at_millis,
            }),
        );
        admission
            .submit(move |owner, context| async move {
                owner
                    .append_session_records_owned(&context, None, records)
                    .await?;
                Ok::<_, AdmittedTouchError>(())
            })
            .await
    }

    pub(crate) async fn schedule_stream_session_expiry(
        &self,
        public_session_id: String,
        session_key: IdempotencyKey,
        deadline_millis: Option<u64>,
    ) -> Result<(), WorkerExecutorError> {
        let Some(deadline_millis) = deadline_millis else {
            return Ok(());
        };
        let deadline = i64::try_from(deadline_millis)
            .ok()
            .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
            .ok_or_else(|| WorkerExecutorError::invalid_request("stream expiry is out of range"))?;
        self.deps
            .scheduler_service()
            .schedule(
                deadline,
                ScheduledAction::ExpireDurableStreamSession {
                    owned_agent_id: self.owned_agent_id.clone(),
                    target_agent_fingerprint: self.initial_worker_metadata.fingerprint,
                    public_session_id,
                    session_key,
                    expected_deadline_millis: deadline_millis,
                },
            )
            .await;
        Ok(())
    }

    async fn public_stream_session_binding(
        &self,
        public_session_id: &str,
    ) -> Result<Option<DurableStreamPublicBinding>, WorkerExecutorError> {
        self.worker_service()
            .lookup_durable_stream_public_binding(
                &self.owned_agent_id,
                self.agent_mode(),
                public_session_id,
            )
            .await
            .map_err(WorkerExecutorError::runtime)
    }

    async fn expire_stream_session_transition(
        self: &Arc<Self>,
        public_session_id: String,
        invocation_key: IdempotencyKey,
        expected_deadline_millis: u64,
    ) -> Result<StreamSessionExpiryTransition, WorkerExecutorError> {
        let producer = self.durable_stream_producer().await?;
        let content_key = self.stream_session_content_key(&invocation_key).await?;
        let session_key =
            producer.qualify_session(&StreamRegistrationInvocation::Local(content_key));
        let worker = self.clone();
        let transition = producer
            .run_admitted(None, 0, true, move |owner, admission| async move {
                let lock = owner.session_lock(&session_key);
                let _guard = lock.lock_owned().await;
                let binding = worker
                    .public_stream_session_binding(&public_session_id)
                    .await?;
                match binding {
                    Some(DurableStreamPublicBinding::Retired { session_key })
                        if session_key == invocation_key =>
                    {
                        return Ok::<_, AdmittedTouchError>(
                            StreamSessionExpiryTransition::AlreadyApplied,
                        );
                    }
                    Some(DurableStreamPublicBinding::Live {
                        session_key,
                        expiry_deadline_millis: Some(deadline),
                        ..
                    }) if session_key == invocation_key && deadline == expected_deadline_millis => {
                    }
                    _ => return Ok(StreamSessionExpiryTransition::Stale),
                }
                let now_millis = Timestamp::now_utc().to_millis();
                if now_millis < expected_deadline_millis {
                    return Ok(StreamSessionExpiryTransition::Early);
                }
                worker
                    .commit_stream_session_expiry_owned(
                        &owner,
                        &admission,
                        public_session_id,
                        invocation_key,
                        expected_deadline_millis,
                        now_millis,
                    )
                    .await?;
                Ok(StreamSessionExpiryTransition::Applied)
            })
            .await
            .map_err(|AdmittedTouchError(error)| error)?;
        Ok(transition)
    }

    pub async fn deliver_stream_session_expiry(
        self: &Arc<Self>,
        public_session_id: String,
        invocation_key: IdempotencyKey,
        expected_deadline_millis: u64,
    ) -> Result<(), WorkerExecutorError> {
        let transition = self
            .expire_stream_session_transition(
                public_session_id.clone(),
                invocation_key.clone(),
                expected_deadline_millis,
            )
            .await?;
        if matches!(transition, StreamSessionExpiryTransition::Early) {
            self.schedule_stream_session_expiry(
                public_session_id,
                invocation_key,
                Some(expected_deadline_millis),
            )
            .await?;
            return Ok(());
        }
        let recover = if matches!(transition, StreamSessionExpiryTransition::Stale) {
            self.durable_stream_session_status(&invocation_key)
                .await?
                .is_some_and(|status| status.expired)
        } else {
            true
        };
        if recover {
            let producer = self.durable_stream_producer().await?;
            let content_key = self.stream_session_content_key(&invocation_key).await?;
            let session_key =
                producer.qualify_session(&StreamRegistrationInvocation::Local(content_key));
            self.recover_durable_stream_topologies(true, Some(&session_key))
                .await?;
        }
        Ok(())
    }

    async fn expire_public_stream_session_if_due(
        self: &Arc<Self>,
        public_session_id: &str,
    ) -> Result<(), WorkerExecutorError> {
        let Some(DurableStreamPublicBinding::Live {
            session_key,
            expiry_deadline_millis: Some(deadline),
            ..
        }) = self
            .public_stream_session_binding(public_session_id)
            .await?
        else {
            return Ok(());
        };
        if Timestamp::now_utc().to_millis() < deadline {
            return Ok(());
        }
        match self
            .deliver_stream_session_expiry(
                public_session_id.to_owned(),
                session_key.clone(),
                deadline,
            )
            .await
        {
            Ok(()) => Ok(()),
            Err(error) => match self
                .public_stream_session_binding(public_session_id)
                .await?
            {
                Some(DurableStreamPublicBinding::Retired {
                    session_key: retired,
                }) if retired == session_key => Ok(()),
                _ => Err(error),
            },
        }
    }

    async fn resolve_public_stream_session_binding(
        &self,
        public_session_id: &str,
    ) -> Result<Option<PublicStreamSessionBinding>, WorkerExecutorError> {
        validate_durable_stream_session_id(public_session_id)
            .map_err(WorkerExecutorError::invalid_request)?;
        Ok(
            match self
                .public_stream_session_binding(public_session_id)
                .await?
            {
                Some(DurableStreamPublicBinding::Live {
                    session_key,
                    expiry_policy,
                    expiry_deadline_millis,
                }) => Some(PublicStreamSessionBinding {
                    invocation_key: session_key,
                    expiry_policy,
                    expiry_deadline_millis,
                }),
                Some(DurableStreamPublicBinding::Retired { .. }) | None => None,
            },
        )
    }

    async fn resolve_public_stream_session_key(
        &self,
        public_session_id: &str,
    ) -> Result<Option<IdempotencyKey>, WorkerExecutorError> {
        Ok(self
            .resolve_public_stream_session_binding(public_session_id)
            .await?
            .map(|binding| binding.invocation_key))
    }

    pub async fn begin_stream_session_creation(
        self: &Arc<Self>,
        public_session_id: String,
        requested_expiry_policy: StreamSessionExpiryPolicy,
        intent: StreamSessionCreationIntent,
    ) -> Result<StreamSessionCreationAdmission, WorkerExecutorError> {
        validate_durable_stream_session_id(&public_session_id)
            .map_err(WorkerExecutorError::invalid_request)?;
        let guard = self.stream_session_creation_lock.clone().lock_owned().await;
        self.expire_public_stream_session_if_due(&public_session_id)
            .await?;
        let binding = self
            .public_stream_session_binding(&public_session_id)
            .await?;
        let now_millis = Timestamp::now_utc().to_millis();
        let new_deadline = || match requested_expiry_policy {
            StreamSessionExpiryPolicy::None => Ok(None),
            StreamSessionExpiryPolicy::Sliding { ttl_seconds } => ttl_seconds
                .checked_mul(1_000)
                .and_then(|ttl| now_millis.checked_add(ttl))
                .map(Some)
                .ok_or_else(|| WorkerExecutorError::invalid_request("stream TTL overflows")),
            StreamSessionExpiryPolicy::Absolute { expires_at_millis }
                if expires_at_millis > now_millis =>
            {
                Ok(Some(expires_at_millis))
            }
            StreamSessionExpiryPolicy::Absolute { .. } => Err(
                WorkerExecutorError::invalid_request("stream expiry must be in the future"),
            ),
        };
        let (invocation_key, expiry_policy, expiry_deadline_millis) = match binding {
            Some(DurableStreamPublicBinding::Live {
                session_key,
                expiry_policy,
                expiry_deadline_millis,
            }) => {
                if expiry_policy != requested_expiry_policy {
                    return Err(WorkerExecutorError::invalid_request(
                        "IdempotencyConflict: the public stream session is already bound with a different expiry policy",
                    ));
                }
                (session_key, expiry_policy, expiry_deadline_millis)
            }
            Some(DurableStreamPublicBinding::Retired { .. }) => match intent {
                StreamSessionCreationIntent::LazyPost => {
                    return Err(WorkerExecutorError::invalid_request(
                        "NotFound: the public stream session has expired",
                    ));
                }
                StreamSessionCreationIntent::ExplicitPut
                    if self.agent_mode() == AgentMode::Ephemeral =>
                {
                    return Err(WorkerExecutorError::invalid_request(
                        "IdempotencyConflict: an expired ephemeral stream session cannot be recreated",
                    ));
                }
                StreamSessionCreationIntent::ExplicitPut => (
                    IdempotencyKey::new(uuid::Uuid::new_v4().to_string()),
                    requested_expiry_policy,
                    new_deadline()?,
                ),
            },
            None => (
                if self.agent_mode() == AgentMode::Ephemeral {
                    IdempotencyKey::new(public_session_id.clone())
                } else {
                    IdempotencyKey::new(uuid::Uuid::new_v4().to_string())
                },
                requested_expiry_policy,
                new_deadline()?,
            ),
        };
        Ok(StreamSessionCreationAdmission {
            _guard: guard,
            public_session_id,
            invocation_key,
            expiry_policy,
            expiry_deadline_millis,
        })
    }

    pub(crate) async fn resolve_export_fork_slot(
        self: &Arc<Self>,
        session: &str,
        name: &str,
        expected_method: &str,
    ) -> Result<Option<ExportForkSlot>, WorkerExecutorError> {
        self.expire_public_stream_session_if_due(session).await?;
        let Some(DurableStreamPublicBinding::Live {
            session_key,
            expiry_policy,
            expiry_deadline_millis,
        }) = self.public_stream_session_binding(session).await?
        else {
            return Ok(None);
        };
        let producer = self.durable_stream_producer().await?;
        let Some(slot) = producer
            .with_metadata_activity(self.resolve_stream_slot(session, name, Some(expected_method)))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))??
        else {
            return Ok(None);
        };
        let Some(admitted) = self
            .admit_public_stream_read(
                &producer,
                session.to_owned(),
                PublicStreamSessionBinding {
                    invocation_key: session_key,
                    expiry_policy,
                    expiry_deadline_millis,
                },
                false,
            )
            .await?
        else {
            return Ok(None);
        };
        let tombstoned = matches!(slot.source, SlotSource::Tombstoned);
        let handle = match slot.source {
            SlotSource::Stream(handle) => handle,
            SlotSource::Tombstoned | SlotSource::Value { .. } | SlotSource::Pending { .. } => {
                return Ok(Some(ExportForkSlot {
                    handle: None,
                    expiry_policy: admitted.expiry_policy,
                    writable: slot.writable,
                    bytes: slot.bytes,
                    tombstoned,
                    graph: slot.graph,
                    snapshot: None,
                }));
            }
        };
        let snapshot = producer
            .export_fork_snapshot(&handle)
            .await
            .map_err(|error| WorkerExecutorError::invalid_request(error.to_string()))?;
        Ok(Some(ExportForkSlot {
            handle: Some(handle),
            expiry_policy: admitted.expiry_policy,
            writable: slot.writable,
            bytes: slot.bytes,
            tombstoned,
            graph: slot.graph,
            snapshot: Some(snapshot),
        }))
    }

    /// Resolves the pinned revision for a stream-slot session before transport decoding.
    pub async fn stream_session_revision(
        &self,
        key: &IdempotencyKey,
    ) -> Result<ComponentRevision, WorkerExecutorError> {
        let producer = self.durable_stream_producer().await?;
        let pinned = producer
            .with_metadata_activity(self.prepared_stream_session(key))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))??;
        if pinned.is_none() && self.agent_mode() != AgentMode::Ephemeral {
            Self::ensure_not_failed(
                &self.deps,
                &self.owned_agent_id,
                self.agent_mode(),
                self.get_last_known_status().await.as_ref(),
            )
            .await?;
        }
        Ok(match pinned {
            Some(prepared) => prepared.attempt.invocation.target_component_revision,
            None => self.get_last_known_status().await.component_revision,
        })
    }

    /// Accepts a fully domain-built streaming invocation and returns its stable session identity.
    pub async fn create_stream_session(
        self: &Arc<Self>,
        mut request: DurableStreamingInvocationRequest,
        admission: StreamSessionCreationAdmission,
    ) -> Result<CreateStreamSessionResult, WorkerExecutorError> {
        if request.attempt.session_key.idempotency_key != admission.invocation_key {
            return Err(WorkerExecutorError::runtime(
                "stream session admission key does not match the invocation",
            ));
        }
        request.public_session_id = admission.public_session_id.clone();
        request.expiry_policy = admission.expiry_policy;
        request.expiry_deadline_millis = admission.expiry_deadline_millis;
        let component_revision = request.attempt.invocation.target_component_revision;
        let public_session_id = admission.public_session_id.clone();
        let invocation_key = admission.invocation_key.clone();
        let expiry_policy = admission.expiry_policy;
        let expiry_deadline_millis = admission.expiry_deadline_millis;
        let acceptance = self
            .accept_durable_stream_slot_invocation(request, admission)
            .await?;
        Ok(CreateStreamSessionResult {
            session: public_session_id,
            replayed: acceptance.replayed,
            component_revision,
            invocation_key,
            expiry_policy,
            expiry_deadline_millis,
        })
    }

    async fn resolve_stream_slot(
        &self,
        session: &str,
        name: &str,
        expected_method: Option<&str>,
    ) -> Result<Option<Slot>, WorkerExecutorError> {
        validate_durable_stream_session_id(session)
            .map_err(WorkerExecutorError::invalid_request)?;
        let Some(session_key) = self.resolve_public_stream_session_key(session).await? else {
            return Ok(None);
        };
        self.resolve_stream_slot_by_key(&session_key, name, expected_method)
            .await
    }

    async fn resolve_stream_slot_by_key(
        &self,
        session_key: &IdempotencyKey,
        name: &str,
        expected_method: Option<&str>,
    ) -> Result<Option<Slot>, WorkerExecutorError> {
        let Some(status) = self.durable_stream_session_status(session_key).await? else {
            return Ok(None);
        };
        let content_status = match status.export_source_invocation.as_ref() {
            Some(source) => self
                .durable_stream_session_status(&source.idempotency_key)
                .await?
                .ok_or_else(|| {
                    WorkerExecutorError::runtime("fork source session status is missing")
                })?,
            None => status.clone(),
        };
        let Some(prepared_index) = content_status.first_prepared else {
            return Ok(None);
        };
        let StreamSessionRecord::Prepared(prepared) =
            self.read_stream_session_record(prepared_index).await?
        else {
            return Err(WorkerExecutorError::runtime(
                "session prepared locator is invalid",
            ));
        };
        let descriptor = &prepared.attempt.invocation;
        let PersistedInvocationTarget::AgentMethod { method_name } = &descriptor.target else {
            return Ok(None);
        };
        if expected_method.is_some_and(|expected| expected != method_name) {
            return Ok(None);
        }
        let component = self
            .component_service()
            .get_metadata(
                self.component_id(),
                Some(descriptor.target_component_revision),
            )
            .await?;
        let parsed = ParsedAgentId::parse(&self.agent_id().agent_id, &component.metadata)
            .map_err(WorkerExecutorError::invalid_request)?;
        let agent = component
            .metadata
            .find_agent_type_by_name_ref(&parsed.agent_type)
            .ok_or_else(|| WorkerExecutorError::runtime("persisted agent type is missing"))?;
        let method = agent
            .methods
            .iter()
            .find(|method| method.name == *method_name)
            .ok_or_else(|| WorkerExecutorError::runtime("persisted agent method is missing"))?;
        let mut candidates = method
            .input_schema
            .fields()
            .iter()
            .map(|field| field.name.clone())
            .collect::<Vec<_>>();
        candidates.push("$result".into());
        if let OutputSchema::Single(output) = &method.output_schema
            && let SchemaType::Record { fields, .. } = agent
                .schema
                .resolve_ref(output)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
        {
            candidates.extend(fields.iter().map(|field| field.name.clone()));
        }
        let mut slots = Vec::new();
        for candidate in candidates {
            if !slots.contains(&candidate)
                && SlotSchema::lookup(&agent.schema, method, &candidate)?.is_some()
            {
                slots.push(candidate);
            }
        }
        let name = if name.is_empty() {
            let Some(name) = slots.first() else {
                return Ok(None);
            };
            name.as_str()
        } else {
            name
        };
        let Some(schema) = SlotSchema::lookup(&agent.schema, method, name)? else {
            return Ok(None);
        };
        let source = if status.tombstoned_slots.contains(name)
            || content_status.tombstoned_slots.contains(name)
        {
            SlotSource::Tombstoned
        } else if schema.writable {
            let mappings = self
                .durable_stream_producer()
                .await?
                .materialize_bindings(&prepared.stream_mappings)
                .await
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            let input = golem_api_grpc::proto::golem::schema::TypedSchemaValue::decode(
                descriptor.invocation_value.as_slice(),
            )
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?
            .value
            .ok_or_else(|| WorkerExecutorError::runtime("persisted invocation input is missing"))?;
            SlotSource::Stream(
                schema.extract_handle(
                    &input.encode_to_vec(),
                    &mappings
                        .into_iter()
                        .map(|mapping| mapping.handle)
                        .collect::<Vec<_>>(),
                )?,
            )
        } else if let Some(result_index) = content_status.invocation_result {
            let StreamSessionRecord::InvocationResult(result) =
                self.read_stream_session_record(result_index).await?
            else {
                return Err(WorkerExecutorError::runtime(
                    "session result locator is invalid",
                ));
            };
            if schema.is_stream {
                let mappings = self
                    .durable_stream_producer()
                    .await?
                    .materialize_bindings(&result.stream_mappings)
                    .await
                    .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
                SlotSource::Stream(
                    schema.extract_handle(
                        &result.result,
                        &mappings
                            .into_iter()
                            .map(|mapping| mapping.handle)
                            .collect::<Vec<_>>(),
                    )?,
                )
            } else {
                SlotSource::Value {
                    encoded: result.result,
                    offset: StreamOffset::new(result_index, 0),
                }
            }
        } else {
            SlotSource::Pending {
                finished: content_status.finished.is_some()
                    || status.finished.is_some()
                    || (schema.is_stream
                        && (content_status.cancellation_requested
                            || status.cancellation_requested)),
            }
        };
        if let SlotSource::Stream(handle) = &source {
            let fingerprint = schema_fingerprint_v1(&agent.schema, Some(&schema.element))
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?;
            if fingerprint != handle.element_schema_fingerprint {
                return Err(WorkerExecutorError::runtime(
                    "slot schema does not match durable handle",
                ));
            }
        }
        let bytes = matches!(
            agent
                .schema
                .resolve_ref(&schema.element)
                .map_err(|error| WorkerExecutorError::runtime(error.to_string()))?,
            SchemaType::U8 { .. }
        ) && schema.is_stream;
        Ok(Some(Slot {
            session: StreamRegistrationInvocation::Local(prepared.session_key).qualify(
                self.owned_agent_id.environment_id,
                &self.owned_agent_id.agent_id,
                self.initial_worker_metadata.fingerprint,
            ),
            name: name.to_owned(),
            slots,
            graph: SchemaGraph {
                defs: agent.schema.defs.clone(),
                root: schema.element,
            },
            writable: schema.writable,
            bytes,
            source,
        }))
    }

    fn expiry_refresh(
        public_session_id: &str,
        binding: &PublicStreamSessionBinding,
        refreshed_at_millis: u64,
    ) -> Result<Option<StreamSessionExpiryRefreshedRecord>, WorkerExecutorError> {
        let StreamSessionExpiryPolicy::Sliding { ttl_seconds } = binding.expiry_policy else {
            return Ok(None);
        };
        let expected_deadline_millis = binding.expiry_deadline_millis.ok_or_else(|| {
            WorkerExecutorError::runtime("sliding stream session has no expiry deadline")
        })?;
        let ttl_millis = ttl_seconds
            .checked_mul(1_000)
            .ok_or_else(|| WorkerExecutorError::runtime("stream TTL deadline overflows"))?;
        let deadline_millis = refreshed_at_millis
            .checked_add(ttl_millis)
            .ok_or_else(|| WorkerExecutorError::runtime("stream TTL deadline overflows"))?;
        let minimum_extension_millis = (ttl_millis / 10).max(1);
        if deadline_millis.saturating_sub(expected_deadline_millis) < minimum_extension_millis {
            return Ok(None);
        }
        Ok(Some(StreamSessionExpiryRefreshedRecord {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            session_key: binding.invocation_key.clone(),
            public_session_id: public_session_id.to_owned(),
            expected_deadline_millis,
            refreshed_at_millis,
            deadline_millis,
        }))
    }

    async fn admit_public_stream_read(
        self: &Arc<Self>,
        producer: &Arc<DurableStreamStore>,
        public_session_id: String,
        expected: PublicStreamSessionBinding,
        refresh_sliding_expiry: bool,
    ) -> Result<Option<PublicStreamSessionBinding>, WorkerExecutorError> {
        let content_key = self
            .stream_session_content_key(&expected.invocation_key)
            .await?;
        let session_key =
            producer.qualify_session(&StreamRegistrationInvocation::Local(content_key));
        let recovery_key = session_key.clone();
        let worker = self.clone();
        let (binding, expired) = producer
            .run_admitted(None, 0, false, move |owner, admission| async move {
                let lock = owner.session_lock(&session_key);
                let _guard = lock.lock_owned().await;
                let Some(current) = worker
                    .resolve_public_stream_session_binding(&public_session_id)
                    .await?
                else {
                    return Ok::<_, AdmittedTouchError>((None, false));
                };
                if current.invocation_key != expected.invocation_key
                    || current.expiry_policy != expected.expiry_policy
                {
                    return Ok((None, false));
                }
                let now_millis = Timestamp::now_utc().to_millis();
                if current
                    .expiry_deadline_millis
                    .is_some_and(|deadline| deadline <= now_millis)
                {
                    let deadline = current
                        .expiry_deadline_millis
                        .expect("checked stream expiry deadline");
                    worker
                        .commit_stream_session_expiry_owned(
                            &owner,
                            &admission,
                            public_session_id,
                            current.invocation_key,
                            deadline,
                            now_millis,
                        )
                        .await?;
                    return Ok((None, true));
                }
                if !refresh_sliding_expiry
                    || !matches!(
                        current.expiry_policy,
                        StreamSessionExpiryPolicy::Sliding { .. }
                    )
                {
                    return Ok((Some(current), false));
                }
                let Some(refresh) = Self::expiry_refresh(&public_session_id, &current, now_millis)?
                else {
                    return Ok((Some(current), false));
                };
                let deadline_millis = refresh.deadline_millis;
                worker
                    .schedule_stream_session_expiry(
                        public_session_id.clone(),
                        current.invocation_key.clone(),
                        Some(deadline_millis),
                    )
                    .await?;
                owner
                    .refresh_session_expiry_admitted(&admission, refresh)
                    .await?;
                Ok((
                    Some(PublicStreamSessionBinding {
                        expiry_deadline_millis: Some(deadline_millis),
                        ..current
                    }),
                    false,
                ))
            })
            .await
            .map_err(|AdmittedTouchError(error)| error)?;
        if expired {
            self.recover_durable_stream_topologies(true, Some(&recovery_key))
                .await?;
        }
        Ok(binding)
    }

    /// Reads data and metadata from a slot resolved against the session's pinned schema.
    pub async fn read_stream_slot(
        self: &Arc<Self>,
        request: ReadStreamSlotRequest,
    ) -> Result<Option<ReadStreamSlotResult>, DurableStreamReadError<WorkerExecutorError>> {
        let requested_wait_millis = request.wait_millis;
        let touching = matches!(
            &request.admission,
            StreamSlotReadAdmission::TouchingOriginGet
        );
        let deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(if touching {
                0
            } else {
                requested_wait_millis.min(30_000)
            });
        let after = request.from_offset;
        let producer = self.load_durable_stream_producer().await.map_err(|error| {
            DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        validate_durable_stream_session_id(&request.session)
            .map_err(WorkerExecutorError::invalid_request)?;
        if !matches!(request.admission, StreamSlotReadAdmission::Continuation(_)) {
            self.expire_public_stream_session_if_due(&request.session)
                .await?;
        }
        let binding = match &request.admission {
            StreamSlotReadAdmission::TouchingOriginGet | StreamSlotReadAdmission::Head => {
                let Some(binding) = self
                    .resolve_public_stream_session_binding(&request.session)
                    .await?
                else {
                    return Ok(None);
                };
                binding
            }
            StreamSlotReadAdmission::Continuation(invocation_key) => {
                let Some(status) = self.durable_stream_session_status(invocation_key).await? else {
                    return Ok(None);
                };
                if status.public_session_id.as_deref() != Some(&request.session) {
                    return Ok(None);
                }
                PublicStreamSessionBinding {
                    invocation_key: invocation_key.clone(),
                    expiry_policy: status.expiry_policy,
                    expiry_deadline_millis: status.expiry_deadline_millis,
                }
            }
        };
        let Some(mut slot) = producer
            .with_metadata_activity(self.resolve_stream_slot_by_key(
                &binding.invocation_key,
                &request.slot,
                Some(&request.expected_method),
            ))
            .await
            .map_err(|error| {
                DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
            })??
        else {
            return Ok(None);
        };
        loop {
            if !matches!(slot.source, SlotSource::Pending { finished: false })
                || request.max_items == 0
                || tokio::time::Instant::now() >= deadline
            {
                break;
            }
            let notified = producer.session_records_changed().notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let Some(next) = producer
                .with_metadata_activity(self.resolve_stream_slot_by_key(
                    &binding.invocation_key,
                    &request.slot,
                    Some(&request.expected_method),
                ))
                .await
                .map_err(|error| {
                    DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
                })??
            else {
                return Ok(None);
            };
            slot = next;
            if matches!(slot.source, SlotSource::Pending { finished: false }) {
                let _ = tokio::time::timeout_at(deadline, notified).await;
            }
        }
        let stream_id = match &slot.source {
            SlotSource::Stream(handle) => Some(handle.stream_id),
            SlotSource::Value { .. } | SlotSource::Pending { .. } | SlotSource::Tombstoned => None,
        };
        let identity = golem_common::serialization::serialize(&(
            slot.session.clone(),
            slot.name.clone(),
            stream_id,
        ))
        .map_err(WorkerExecutorError::runtime)?;
        let mut response = ReadStreamSlotResult {
            items: Vec::new(),
            next_offset: after,
            closed: false,
            cancelled: false,
            element_schema: slot.graph,
            content_type: if slot.bytes {
                "application/octet-stream"
            } else {
                "application/json"
            },
            up_to_date: true,
            head_offset: None,
            stream_identity: blake3::hash(&identity).to_hex().to_string(),
            slots: slot.slots,
            tombstoned: matches!(slot.source, SlotSource::Tombstoned),
            writable: slot.writable,
            fork: self
                .export_fork_receipt
                .get_or_try_init(|| async {
                    use crate::services::worker_fork::export;
                    let receipt = if self.agent_mode() == AgentMode::Durable {
                        export::creation_record(self.oplog_service().as_ref(), &self.owned_agent_id)
                            .await?
                    } else {
                        None
                    };
                    Ok::<_, WorkerExecutorError>(receipt.and_then(|receipt| {
                        receipt.export.map(|export| {
                            export::response(
                                &export,
                                receipt.cut_index,
                                true,
                                &binding.invocation_key,
                            )
                        })
                    }))
                })
                .await?
                .clone(),
            invocation_key: binding.invocation_key.clone(),
            expiry_policy: binding.expiry_policy,
            expiry_deadline_millis: binding.expiry_deadline_millis,
        };
        match slot.source {
            SlotSource::Stream(handle) => {
                let read = StreamHandleReadRequest {
                    handle,
                    after,
                    max_items: request.max_items,
                    max_bytes: request.max_bytes,
                    wait_millis: deadline
                        .saturating_duration_since(tokio::time::Instant::now())
                        .as_millis() as u64,
                };
                let bytes = self
                    .rpc()
                    .read_durable_stream_segment(
                        DurableStreamReadRequest::AuthorizedExport(Box::new(read)),
                        &AuthCtx::System,
                    )
                    .await
                    .map_err(|error| {
                        error.map_other(|error| WorkerExecutorError::runtime(error.to_string()))
                    })?;
                let read: StreamHandleReadResult = golem_common::serialization::deserialize(&bytes)
                    .map_err(WorkerExecutorError::runtime)?;
                response.next_offset = read.next_offset;
                response.head_offset = read.head_offset;
                response.closed = read.closed;
                response.cancelled = read.cancelled;
                response.up_to_date = read.next_offset >= read.head_offset;
                for event in read.events {
                    let content = match event.payload {
                        CommittedProducerStreamEventPayload::Value(value) => {
                            Some(StreamSlotItemContent::Value(value))
                        }
                        CommittedProducerStreamEventPayload::PackedU8(byte) => {
                            Some(StreamSlotItemContent::PackedU8(vec![byte]))
                        }
                        _ => None,
                    };
                    if let Some(content) = content {
                        response.items.push(StreamSlotItem {
                            offset: event.offset,
                            content,
                        });
                    }
                }
            }
            SlotSource::Value {
                encoded: value,
                offset,
            } => {
                response.head_offset = Some(offset);
                response.closed = true;
                if request.max_items > 0 && after.is_none_or(|after| after < offset) {
                    if value.len() as u64 > request.max_bytes {
                        return Err(WorkerExecutorError::invalid_request(
                            "slot value exceeds read byte limit",
                        )
                        .into());
                    }
                    response.next_offset = Some(offset);
                    response.items.push(StreamSlotItem {
                        offset,
                        content: StreamSlotItemContent::Value(value),
                    });
                }
                response.up_to_date = response.next_offset >= response.head_offset;
            }
            SlotSource::Pending { finished } => {
                response.closed = finished;
                response.cancelled = finished;
            }
            SlotSource::Tombstoned => {}
        }
        producer.ensure_healthy().map_err(|error| {
            DurableStreamReadError::from_producer(error, WorkerExecutorError::runtime)
        })?;
        if !matches!(request.admission, StreamSlotReadAdmission::Continuation(_)) {
            let refresh = touching && (!response.tombstoned || request.slot.is_empty());
            let Some(admitted) = self
                .admit_public_stream_read(&producer, request.session.clone(), binding, refresh)
                .await?
            else {
                return Ok(None);
            };
            response.expiry_deadline_millis = admitted.expiry_deadline_millis;
            if touching
                && requested_wait_millis > 0
                && request.max_items > 0
                && response.items.is_empty()
                && !response.closed
            {
                return Box::pin(self.read_stream_slot(ReadStreamSlotRequest {
                    session: request.session,
                    slot: request.slot,
                    from_offset: response.next_offset,
                    max_items: request.max_items,
                    max_bytes: request.max_bytes,
                    wait_millis: requested_wait_millis,
                    expected_method: request.expected_method,
                    admission: StreamSlotReadAdmission::Continuation(admitted.invocation_key),
                }))
                .await;
            }
        }
        Ok(Some(response))
    }

    /// Cancels all session streams or tombstones one canonical export slot.
    pub async fn control_export_stream(
        self: &Arc<Self>,
        request: ExportStreamControlRequest,
    ) -> Result<ExportStreamControlResult, WorkerExecutorError> {
        validate_durable_stream_session_id(&request.session)
            .map_err(WorkerExecutorError::invalid_request)?;
        self.expire_public_stream_session_if_due(&request.session)
            .await?;
        if request.expected_method.is_empty()
            || request
                .slot
                .as_ref()
                .is_some_and(|slot| slot.is_empty() || slot.starts_with("__ds"))
        {
            return Err(WorkerExecutorError::invalid_request(
                "invalid export stream control target",
            ));
        }
        let producer = self.durable_stream_producer().await?;
        let Some(binding) = self
            .resolve_public_stream_session_binding(&request.session)
            .await?
        else {
            return Ok(ExportStreamControlResult::NotFound);
        };
        let Some(admitted) = self
            .admit_public_stream_read(&producer, request.session.clone(), binding, false)
            .await?
        else {
            return Ok(ExportStreamControlResult::NotFound);
        };
        let session_key = admitted.invocation_key;
        let content_key = self.stream_session_content_key(&session_key).await?;
        let Some(prepared) = self.prepared_stream_session(&content_key).await? else {
            return Ok(ExportStreamControlResult::NotFound);
        };
        if !matches!(&prepared.attempt.invocation.target,
            PersistedInvocationTarget::AgentMethod { method_name } if *method_name == request.expected_method)
        {
            return Ok(ExportStreamControlResult::NotFound);
        }
        let streams = StreamSession::open(
            producer.clone(),
            self.oplog.clone(),
            StreamRegistrationInvocation::Local(prepared.session_key.clone()),
            prepared.stream_mappings.iter().cloned(),
        )
        .await
        .map_err(WorkerExecutorError::runtime)?
        .with_rpc(self.rpc())
        .with_consumer_journal(self.durable_stream_consumer_journal())
        .with_auth_ctx(self.durable_stream_consumer_auth_ctx()?);
        if let Some(name) = request.slot {
            let worker = self.clone();
            let session_key = streams.session_key.clone();
            producer
                .run_admitted(None, 0, true, move |owner, admission| async move {
                    let lock = owner.session_lock(&session_key);
                    let guard = lock.lock_owned().await;
                    let Some(slot) = worker
                        .resolve_stream_slot_by_key(
                            &session_key.idempotency_key,
                            &name,
                            Some(&request.expected_method),
                        )
                        .await
                        .map_err(|error| error.to_string())?
                    else {
                        return Ok::<_, String>(ExportStreamControlResult::NotFound);
                    };
                    let stream = match slot.source {
                        SlotSource::Tombstoned => return Ok(ExportStreamControlResult::Gone),
                        SlotSource::Stream(handle) => Some((
                            handle,
                            if slot.writable {
                                SessionStreamRole::Input
                            } else {
                                SessionStreamRole::Output
                            },
                        )),
                        SlotSource::Value { .. } | SlotSource::Pending { .. } => None,
                    };
                    let applied = streams
                        .tombstone_slot_owned(&admission, slot.name, stream, guard)
                        .await?;
                    Ok(if applied {
                        ExportStreamControlResult::Applied
                    } else {
                        ExportStreamControlResult::Gone
                    })
                })
                .await
                .map_err(WorkerExecutorError::runtime)
        } else {
            streams
                .cancel_session_streams()
                .await
                .map(|exists| {
                    if exists {
                        ExportStreamControlResult::Applied
                    } else {
                        ExportStreamControlResult::NotFound
                    }
                })
                .map_err(WorkerExecutorError::runtime)
        }
    }

    /// Validates and durably appends a batch to one writable stream slot.
    ///
    /// The slot is resolved and the batch committed under the session lock, so a concurrent
    /// slot or session deletion cannot tombstone the slot between the check and the commit.
    pub async fn append_to_stream_slot(
        self: &Arc<Self>,
        request: AppendToStreamSlotRequest,
    ) -> Result<AppendToStreamSlotResult, WorkerExecutorError> {
        validate_durable_stream_session_id(&request.session)
            .map_err(WorkerExecutorError::invalid_request)?;
        self.expire_public_stream_session_if_due(&request.session)
            .await?;
        let producer = self.durable_stream_producer().await?;
        let Some(binding) = self
            .resolve_public_stream_session_binding(&request.session)
            .await?
        else {
            return Ok(AppendToStreamSlotResult::not_found());
        };
        let content_key = self
            .stream_session_content_key(&binding.invocation_key)
            .await?;
        let Some(prepared) = producer
            .with_metadata_activity(self.prepared_stream_session(&content_key))
            .await
            .map_err(|error| WorkerExecutorError::runtime(error.to_string()))??
        else {
            return Ok(AppendToStreamSlotResult::not_found());
        };
        let payload = request.payload.map(|payload| match payload {
            AppendStreamSlotPayload::PackedU8(bytes) => StreamItemsPayload::PackedU8(bytes),
            AppendStreamSlotPayload::Values(values) => StreamItemsPayload::Values(values),
        });
        let external_producer = request.producer.map(|producer| ExternalProducer {
            id: ExternalProducerId::Client(producer.id),
            epoch: producer.epoch,
            sequence: producer.sequence,
        });
        let retained_bytes = DurableStreamStore::external_input_reservation(
            payload.as_ref(),
            external_producer.as_ref(),
        );
        let worker = self.clone();
        let session_key =
            producer.qualify_session(&StreamRegistrationInvocation::Local(prepared.session_key));
        let public_session_id = request.session.clone();
        let expiry_policy = binding.expiry_policy;
        let invocation_key = binding.invocation_key.clone();
        let (outcome, expiry_deadline_millis, stream_metadata, expired_session) = producer
            .run_admitted(
                None,
                retained_bytes,
                false,
                move |owner, admission| async move {
                    let lock = owner.session_lock(&session_key);
                    let _guard = lock.lock_owned().await;
                    let Some(current) = worker
                        .resolve_public_stream_session_binding(&public_session_id)
                        .await?
                    else {
                        return Ok::<_, AdmittedAppendError>((
                            AppendStreamSlotOutcome::NotFound,
                            None,
                            None,
                            None,
                        ));
                    };
                    if current.invocation_key != binding.invocation_key
                        || current.expiry_policy != binding.expiry_policy
                    {
                        return Ok((AppendStreamSlotOutcome::NotFound, None, None, None));
                    }
                    let now_millis = Timestamp::now_utc().to_millis();
                    if current
                        .expiry_deadline_millis
                        .is_some_and(|deadline| deadline <= now_millis)
                    {
                        let deadline = current
                            .expiry_deadline_millis
                            .expect("checked stream expiry deadline");
                        worker
                            .commit_stream_session_expiry_owned(
                                &owner,
                                &admission,
                                public_session_id,
                                current.invocation_key,
                                deadline,
                                now_millis,
                            )
                            .await
                            .map_err(|AdmittedTouchError(error)| AdmittedAppendError(error))?;
                        return Ok((
                            AppendStreamSlotOutcome::NotFound,
                            None,
                            None,
                            Some(session_key),
                        ));
                    }
                    let Some(slot) = owner
                        .with_metadata_activity(worker.resolve_stream_slot_by_key(
                            &current.invocation_key,
                            &request.slot,
                            Some(&request.expected_method),
                        ))
                        .await??
                    else {
                        return Ok((AppendStreamSlotOutcome::NotFound, None, None, None));
                    };
                    if matches!(slot.source, SlotSource::Tombstoned) {
                        return Ok((
                            AppendStreamSlotOutcome::Gone,
                            current.expiry_deadline_millis,
                            None,
                            None,
                        ));
                    }
                    if !slot.writable {
                        return Ok((
                            AppendStreamSlotOutcome::ReadOnly,
                            current.expiry_deadline_millis,
                            None,
                            None,
                        ));
                    }
                    let SlotSource::Stream(handle) = slot.source else {
                        return Err(WorkerExecutorError::runtime("input slot has no stream").into());
                    };
                    match &payload {
                        Some(StreamItemsPayload::PackedU8(_)) if slot.bytes => {}
                        Some(StreamItemsPayload::Values(values)) if !slot.bytes => {
                            for encoded in values {
                                let proto =
                                    ProtoValue::decode(encoded.as_slice()).map_err(|error| {
                                        WorkerExecutorError::invalid_request(error.to_string())
                                    })?;
                                let value: SchemaValue = proto
                                    .try_into()
                                    .map_err(WorkerExecutorError::invalid_request)?;
                                validate_value(&slot.graph, &slot.graph.root, &value).map_err(
                                    |errors| {
                                        WorkerExecutorError::invalid_request(format!(
                                            "invalid stream item: {errors:?}"
                                        ))
                                    },
                                )?;
                            }
                        }
                        None => {}
                        _ => {
                            return Err(WorkerExecutorError::invalid_request(
                                "payload does not match stream content type",
                            )
                            .into());
                        }
                    }
                    owner.validate_handle(&handle).await?;
                    DurableStreamStore::validate_external_input(payload.as_ref())?;
                    let expiry_refresh =
                        Self::expiry_refresh(&public_session_id, &current, now_millis)?;
                    let refreshed_deadline = expiry_refresh
                        .as_ref()
                        .map(|refresh| refresh.deadline_millis);
                    if let Some(refresh) = &expiry_refresh {
                        worker
                            .schedule_stream_session_expiry(
                                public_session_id.clone(),
                                current.invocation_key.clone(),
                                Some(refresh.deadline_millis),
                            )
                            .await?;
                    }
                    let result = owner
                        .append_external_input_admitted(
                            &admission,
                            &slot.session,
                            handle.stream_id,
                            payload,
                            request.close,
                            external_producer,
                            expiry_refresh,
                        )
                        .await?;
                    let outcome = AppendStreamSlotOutcome::from(result);
                    let stream_metadata = if matches!(
                        outcome,
                        AppendStreamSlotOutcome::Accepted(_)
                            | AppendStreamSlotOutcome::Duplicate { .. }
                            | AppendStreamSlotOutcome::Closed
                    ) {
                        let head = owner.stream_head(&handle).await?;
                        Some((head.offset, head.closed))
                    } else {
                        None
                    };
                    let deadline = if matches!(
                        outcome,
                        AppendStreamSlotOutcome::Accepted(_)
                            | AppendStreamSlotOutcome::Duplicate { .. }
                    ) {
                        refreshed_deadline.or(current.expiry_deadline_millis)
                    } else {
                        current.expiry_deadline_millis
                    };
                    Ok((outcome, deadline, stream_metadata, None))
                },
            )
            .await
            .map_err(|AdmittedAppendError(error)| error)?;
        if let Some(session_key) = expired_session {
            self.recover_durable_stream_topologies(true, Some(&session_key))
                .await?;
        }
        Ok(AppendToStreamSlotResult {
            outcome,
            invocation_key: Some(invocation_key),
            expiry_policy: Some(expiry_policy),
            expiry_deadline_millis,
            stream_head_offset: stream_metadata.and_then(|(offset, _)| offset),
            stream_closed: stream_metadata.map(|(_, closed)| closed),
        })
    }
}

struct AdmittedTouchError(WorkerExecutorError);

impl From<StreamStoreError> for AdmittedTouchError {
    fn from(error: StreamStoreError) -> Self {
        Self(WorkerExecutorError::runtime(error.to_string()))
    }
}

impl From<WorkerExecutorError> for AdmittedTouchError {
    fn from(error: WorkerExecutorError) -> Self {
        Self(error)
    }
}

/// Error of an admitted slot append: store errors are classified like direct append errors.
struct AdmittedAppendError(WorkerExecutorError);

impl From<StreamStoreError> for AdmittedAppendError {
    fn from(error: StreamStoreError) -> Self {
        Self(append_error(error))
    }
}

impl From<WorkerExecutorError> for AdmittedAppendError {
    fn from(error: WorkerExecutorError) -> Self {
        Self(error)
    }
}

impl From<ExternalAppendOutcome> for AppendStreamSlotOutcome {
    fn from(result: ExternalAppendOutcome) -> Self {
        match result {
            ExternalAppendOutcome::Accepted(offset) => AppendStreamSlotOutcome::Accepted(offset),
            ExternalAppendOutcome::Duplicate {
                offset,
                highest_sequence,
            } => AppendStreamSlotOutcome::Duplicate {
                offset,
                highest_sequence,
            },
            ExternalAppendOutcome::EpochFenced(current_epoch) => {
                AppendStreamSlotOutcome::EpochFenced(current_epoch)
            }
            ExternalAppendOutcome::SeqGap { expected, received } => {
                AppendStreamSlotOutcome::SequenceGap { expected, received }
            }
            ExternalAppendOutcome::Closed => AppendStreamSlotOutcome::Closed,
            ExternalAppendOutcome::NotFound => AppendStreamSlotOutcome::NotFound,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::{InputSchema, NamedField, NamedFieldType};
    use test_r::test;

    #[test]
    fn sliding_expiry_refreshes_at_most_ten_times_per_ttl_window() {
        let binding = PublicStreamSessionBinding {
            invocation_key: IdempotencyKey::new("invocation".into()),
            expiry_policy: StreamSessionExpiryPolicy::Sliding { ttl_seconds: 10 },
            expiry_deadline_millis: Some(20_000),
        };

        assert!(
            Worker::<crate::workerctx::default::Context>::expiry_refresh(
                "session", &binding, 10_999,
            )
            .unwrap()
            .is_none()
        );
        let refresh = Worker::<crate::workerctx::default::Context>::expiry_refresh(
            "session", &binding, 11_000,
        )
        .unwrap()
        .expect("ten percent extension refreshes the deadline");
        assert_eq!(refresh.expected_deadline_millis, 20_000);
        assert_eq!(refresh.refreshed_at_millis, 11_000);
        assert_eq!(refresh.deadline_millis, 21_000);
    }

    #[test]
    fn named_slots_preserve_field_indices_and_scalar_results() {
        let graph = SchemaGraph::empty();
        let mut method = AgentMethodSchema {
            name: "run".into(),
            description: String::new(),
            prompt_hint: None,
            input_schema: InputSchema::parameters(vec![
                NamedField::user_supplied("count", SchemaType::u32()),
                NamedField::user_supplied("input", SchemaType::stream(Some(SchemaType::string()))),
            ]),
            output_schema: OutputSchema::Single(Box::new(SchemaType::record(vec![
                NamedFieldType {
                    name: "numbers".into(),
                    body: SchemaType::stream(Some(SchemaType::u64())),
                    metadata: Default::default(),
                },
                NamedFieldType {
                    name: "bytes".into(),
                    body: SchemaType::stream(Some(SchemaType::u8())),
                    metadata: Default::default(),
                },
            ]))),
            http_endpoint: vec![],
            read_only: None,
        };
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "input").unwrap(),
            Some(SlotSchema {
                element: SchemaType::string(),
                field_index: Some(1),
                writable: true,
                is_stream: true
            })
        );
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "bytes").unwrap(),
            Some(SlotSchema {
                element: SchemaType::u8(),
                field_index: Some(1),
                writable: false,
                is_stream: true
            })
        );
        method.input_schema = InputSchema::parameters(vec![NamedField::user_supplied(
            "bytes",
            SchemaType::string(),
        )]);
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "bytes").unwrap(),
            Some(SlotSchema {
                element: SchemaType::u8(),
                field_index: Some(1),
                writable: false,
                is_stream: true
            })
        );
        assert_eq!(SlotSchema::lookup(&graph, &method, "count").unwrap(), None);
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "$result").unwrap(),
            None
        );
        method.output_schema = OutputSchema::Single(Box::new(SchemaType::u64()));
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "$result").unwrap(),
            Some(SlotSchema {
                element: SchemaType::u64(),
                field_index: None,
                writable: false,
                is_stream: false
            })
        );
        method.output_schema =
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u8()))));
        assert_eq!(
            SlotSchema::lookup(&graph, &method, "$result").unwrap(),
            Some(SlotSchema {
                element: SchemaType::u8(),
                field_index: None,
                writable: false,
                is_stream: true
            })
        );
    }

    #[test]
    fn canonical_slot_reference_is_not_a_transport_id_or_field_index() {
        use golem_common::model::durable_stream::{StreamId, StreamInvocationId};
        let id = AgentId {
            component_id: golem_common::model::component::ComponentId(uuid::Uuid::from_u128(2)),
            agent_id: "source".into(),
        };
        let environment =
            golem_common::base_model::environment::EnvironmentId(uuid::Uuid::from_u128(3));
        let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(4));
        let source = StreamInvocationId {
            callee_environment_id: environment,
            callee: id.clone(),
            callee_fingerprint: fingerprint,
            idempotency_key: IdempotencyKey::new("source-session".into()),
        };
        let first = DurableStreamHandle {
            format_version: 1,
            stream_id: StreamId(uuid::Uuid::from_u128(101)),
            producer_environment_id: environment,
            producer: id,
            expected_producer_fingerprint: fingerprint,
            producer_generation: OplogIndex::NONE,
            source_invocation: source,
            component_revision: golem_common::model::component::ComponentRevision::INITIAL,
            element_schema_fingerprint: golem_schema::schema::SchemaFingerprintV1([0; 32]),
        };
        let mut second = first.clone();
        second.stream_id = StreamId(uuid::Uuid::from_u128(202));
        let reference = ProtoValue {
            value: Some(schema_value::Value::StreamReference(
                golem_api_grpc::proto::golem::schema::SchemaValueStreamReference { stream_id: 1 },
            )),
        };
        let record = ProtoValue {
            value: Some(schema_value::Value::RecordValue(
                golem_api_grpc::proto::golem::schema::RecordValue {
                    fields: vec![reference.clone()],
                },
            )),
        };
        assert_eq!(
            SlotSchema {
                element: SchemaType::u8(),
                field_index: Some(0),
                writable: false,
                is_stream: true
            }
            .extract_handle(&record.encode_to_vec(), &[first.clone(), second.clone()])
            .unwrap(),
            second
        );
        assert_eq!(
            SlotSchema {
                element: SchemaType::u8(),
                field_index: None,
                writable: false,
                is_stream: true
            }
            .extract_handle(&reference.encode_to_vec(), &[first.clone(), second.clone()])
            .unwrap(),
            second
        );
        assert!(
            SlotSchema {
                element: SchemaType::u8(),
                field_index: Some(0),
                writable: false,
                is_stream: true
            }
            .extract_handle(&record.encode_to_vec(), &[first])
            .is_err()
        );
    }
}
