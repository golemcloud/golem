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

async fn deserialize_oplog_payload<T: BinaryCodec + Send + 'static>(
    bytes: Vec<u8>,
) -> Result<T, anyhow::Error> {
    join_or_park_at_runtime_shutdown(tokio::task::spawn_blocking(move || {
        golem_common::serialization::decode(&bytes).map_err(anyhow::Error::new)
    }))
    .await
    .map_err(anyhow::Error::new)?
}

#[derive(Debug)]
pub enum RawOplogPayloadDownloadError {
    Backend(anyhow::Error),
    Missing(PayloadId),
}

impl std::fmt::Display for RawOplogPayloadDownloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(error) => write!(formatter, "payload backend failure: {error}"),
            Self::Missing(payload_id) => write!(formatter, "payload {payload_id} is missing"),
        }
    }
}

impl std::error::Error for RawOplogPayloadDownloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(error) => Some(error.as_ref()),
            Self::Missing(_) => None,
        }
    }
}

#[derive(Debug)]
pub enum OplogPayloadDownloadError {
    Backend(anyhow::Error),
    Corrupt(anyhow::Error),
}

impl std::fmt::Display for OplogPayloadDownloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Backend(error) => write!(formatter, "payload backend failure: {error}"),
            Self::Corrupt(error) => write!(formatter, "corrupt payload: {error}"),
        }
    }
}

impl std::error::Error for OplogPayloadDownloadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(error) | Self::Corrupt(error) => Some(error.as_ref()),
        }
    }
}

impl From<OplogPayloadDownloadError> for String {
    fn from(error: OplogPayloadDownloadError) -> Self {
        error.to_string()
    }
}

#[async_trait]
pub trait OplogOps: Oplog {
    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload<T: BinaryCodec + Debug + Clone + PartialEq + Sync>(
        &self,
        data: &T,
    ) -> Result<OplogPayload<T>, String> {
        let bytes = serialize(&data)?;
        let raw_payload = self.upload_raw_payload(bytes).await?;
        let cached = Arc::new(data.clone());
        let payload = raw_payload.into_payload_with_cache(cached)?;
        Ok(payload)
    }

    /// Uploads an owned oplog payload and moves it into the in-memory cache.
    async fn upload_payload_owned<T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync>(
        &self,
        data: T,
    ) -> Result<OplogPayload<T>, String> {
        let bytes = serialize(&data)?;
        let raw_payload = self.upload_raw_payload(bytes).await?;
        let payload = raw_payload.into_payload_with_cache(Arc::new(data))?;
        Ok(payload)
    }

    /// Downloads a big oplog payload by its reference
    async fn download_payload<
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    >(
        &self,
        payload: OplogPayload<T>,
    ) -> Result<T, String> {
        match payload {
            OplogPayload::Inline(value) => Ok(*value),
            OplogPayload::SerializedInline {
                cached: Some(v), ..
            } => Ok((*v).clone()),
            OplogPayload::SerializedInline { bytes, .. } => deserialize_oplog_payload(bytes)
                .await
                .map_err(|error| error.to_string()),
            OplogPayload::External {
                cached: Some(v), ..
            } => Ok((*v).clone()),
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => {
                let bytes = self.download_raw_payload(payload_id, md5_hash).await?;
                deserialize_oplog_payload(bytes)
                    .await
                    .map_err(|error| error.to_string())
            }
        }
    }

    async fn download_payload_classified<
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    >(
        &self,
        payload: OplogPayload<T>,
    ) -> Result<T, OplogPayloadDownloadError> {
        match payload {
            OplogPayload::Inline(value) => Ok(*value),
            OplogPayload::SerializedInline {
                cached: Some(value),
                ..
            }
            | OplogPayload::External {
                cached: Some(value),
                ..
            } => Ok((*value).clone()),
            OplogPayload::SerializedInline { bytes, .. } => deserialize_oplog_payload(bytes)
                .await
                .map_err(OplogPayloadDownloadError::Corrupt),
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => {
                let bytes = self
                    .download_raw_payload_classified(payload_id, md5_hash)
                    .await
                    .map_err(|error| match error {
                        RawOplogPayloadDownloadError::Backend(error) => {
                            OplogPayloadDownloadError::Backend(error)
                        }
                        RawOplogPayloadDownloadError::Missing(payload_id) => {
                            OplogPayloadDownloadError::Corrupt(anyhow::anyhow!(
                                "referenced oplog payload {payload_id} is missing"
                            ))
                        }
                    })?;
                deserialize_oplog_payload(bytes)
                    .await
                    .map_err(OplogPayloadDownloadError::Corrupt)
            }
        }
    }

    /// Typed convenience wrapper over [`Oplog::add_start_with_reserved_raw_payload`]: serializes
    /// `request`, then reserves its payload and appends the `Start` (built by `build_start` from the
    /// payload reference), returning the `Start`'s index and the [`PendingUpload`] tracking the
    /// request blob's durable write. It inherits the underlying raw method's ordering contract
    /// (initiation order on replayable oplogs; none on ephemeral).
    ///
    /// The caller must `wait` on the returned [`PendingUpload`] before appending the call's
    /// `End`/`Cancelled` (so an upload failure surfaces at the call), with the leaf oplog's `append`
    /// commit barrier as the backstop. The ordering and durability contract lives entirely in
    /// [`Oplog::add_start_with_reserved_raw_payload`]; this wrapper only adds typed
    /// (de)serialization.
    async fn add_start_with_reserved_payload<T>(
        &self,
        request: T,
        build_start: impl FnOnce(OplogPayload<T>) -> OplogEntry + Send + 'static,
    ) -> Result<(OplogIndex, PendingUpload), OplogError>
    where
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    {
        let bytes = serialize(&request)?;
        let cached = Arc::new(request);
        let ordered = self
            .add_start_with_reserved_raw_payload(
                bytes,
                Box::new(move |raw| {
                    let payload = raw.into_payload_with_cache(cached)?;
                    Ok(build_start(payload))
                }),
            )
            .await?;
        Ok((ordered.index, ordered.pending_upload))
    }

    /// Typed convenience wrapper over
    /// [`Oplog::add_start_with_indexed_reserved_raw_payload`]. The request builder receives the
    /// exact index that will identify the durable host call.
    async fn add_start_with_indexed_reserved_payload<T>(
        &self,
        build_request: impl FnOnce(OplogIndex) -> Result<T, String> + Send + 'static,
        build_start: impl FnOnce(OplogPayload<T>) -> OplogEntry + Send + 'static,
    ) -> Result<(OplogIndex, PendingUpload), OplogError>
    where
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    {
        let ordered = self
            .add_start_with_indexed_reserved_raw_payload(Box::new(move |start_index| {
                let request = build_request(start_index)?;
                let bytes = serialize(&request)?;
                let cached = Arc::new(request);
                Ok((
                    bytes,
                    Box::new(move |raw| {
                        let payload = raw.into_payload_with_cache(cached)?;
                        Ok(build_start(payload))
                    }),
                ))
            }))
            .await?;
        Ok((ordered.index, ordered.pending_upload))
    }

    /// Sequential adapter that persists a completed durable host call as a matched
    /// `Start`/`End` pair.
    /// Returns `(start_idx, end_idx)`. A durable call is identified by the
    /// `OplogIndex` of its `Start`, and the `End` references it via
    /// `start_index`. The two entries are appended atomically via
    /// [`Oplog::add_pair`] so no other writer can interleave between them and
    /// the pair is never split across a commit/crash boundary.
    ///
    /// This is the durability primitive of the sequential (p2) host-call path, a permanent
    /// coexistence path (Rust's std imports p2), not a legacy one. Unlike the concurrent (p3)
    /// path — which orders `Start` eagerly via
    /// [`Oplog::add_start_with_reserved_raw_payload`] before the side effect runs — the
    /// sequential path runs at most one host call at a time per worker, so it can record the
    /// whole call after the effect completed, as one atomic pair.
    ///
    /// `parent_start_index` is the `Start` index of the enclosing durable scope (if any). This is
    /// an explicit parameter because the oplog cannot see the worker state's open scopes, and
    /// because the parent must be the call's own enclosing scope, not whichever sibling scope
    /// happens to be temporally open. The caller derives it explicitly from the call.
    async fn add_completed_host_call(
        &self,
        function_name: HostFunctionName,
        request: &HostRequest,
        response: &HostResponse,
        function_type: DurableFunctionType,
        parent_start_index: Option<OplogIndex>,
        invocation_id: Option<DurableInvocationId>,
    ) -> Result<(OplogIndex, OplogIndex), OplogError> {
        let request_payload: OplogPayload<HostRequest> = self.upload_payload(request).await?;
        let response_payload: OplogPayload<HostResponse> = self.upload_payload(response).await?;
        let now = Timestamp::now_utc();
        let start = OplogEntry::Start {
            timestamp: now,
            parent_start_index,
            function_name,
            invocation_id,
            observational_owner: None,
            request: Some(request_payload),
            durable_function_type: function_type,
            span_started: None,
            denial: None,
        };
        let (start_idx, end_idx) = self
            .add_pair(
                start,
                Box::new(move |start_index| OplogEntry::End {
                    timestamp: now,
                    start_index,
                    response: Some(response_payload),
                    forced_commit: false,
                    span_finished: None,
                    span_attributes: None,
                }),
            )
            .await?;
        Ok((start_idx, end_idx))
    }

    async fn add_agent_invocation_started(
        &self,
        invocation: AgentInvocation,
        invocation_context: InvocationContextStack,
        wallet_pin: InvocationWalletPin,
    ) -> Result<OplogEntry, OplogError> {
        let entry = self
            .agent_invocation_started_entry(invocation, invocation_context, wallet_pin)
            .await?;
        self.add(entry.clone()).await?;
        Ok(entry)
    }

    async fn add_agent_invocation_started_with_index(
        &self,
        invocation: AgentInvocation,
        invocation_context: InvocationContextStack,
        wallet_pin: InvocationWalletPin,
    ) -> Result<OplogIndex, OplogError> {
        let entry = self
            .agent_invocation_started_entry(invocation, invocation_context, wallet_pin)
            .await?;
        self.add(entry).await
    }

    async fn agent_invocation_started_entry(
        &self,
        invocation: AgentInvocation,
        ctx: InvocationContextStack,
        wallet_pin: InvocationWalletPin,
    ) -> Result<OplogEntry, String> {
        let (idempotency_key, invocation_payload, _) = invocation.into_parts();
        let payload = self.upload_payload_owned(invocation_payload).await?;
        Ok(self.agent_invocation_started_entry_from_payload(
            idempotency_key,
            payload,
            ctx,
            wallet_pin,
        ))
    }

    async fn add_agent_invocation_started_from_pending(
        &self,
        idempotency_key: IdempotencyKey,
        payload: OplogPayload<golem_common::model::AgentInvocationPayload>,
        ctx: InvocationContextStack,
        wallet_pin: InvocationWalletPin,
    ) -> Result<OplogIndex, OplogError> {
        let entry = self.agent_invocation_started_entry_from_payload(
            idempotency_key,
            payload,
            ctx,
            wallet_pin,
        );
        self.add(entry).await
    }

    fn agent_invocation_started_entry_from_payload(
        &self,
        idempotency_key: IdempotencyKey,
        payload: OplogPayload<golem_common::model::AgentInvocationPayload>,
        ctx: InvocationContextStack,
        wallet_pin: InvocationWalletPin,
    ) -> OplogEntry {
        let invocation_context = ctx.to_oplog_data();
        OplogEntry::AgentInvocationStarted {
            timestamp: Timestamp::now_utc(),
            idempotency_key,
            payload,
            trace_id: ctx.trace_id,
            trace_states: ctx.trace_states,
            invocation_context,
            wallet_pin: Box::new(wallet_pin),
        }
    }

    async fn add_agent_invocation_finished(
        &self,
        result: &AgentInvocationResult,
        method_name: Option<String>,
        consumed_fuel: u64,
        component_revision: ComponentRevision,
    ) -> Result<OplogIndex, OplogError> {
        let consumed_fuel = if consumed_fuel > i64::MAX as u64 {
            i64::MAX
        } else {
            consumed_fuel as i64
        };

        let payload = self.upload_payload(result).await?;
        let entry = OplogEntry::AgentInvocationFinished {
            timestamp: Timestamp::now_utc(),
            result: payload,
            method_name,
            consumed_fuel,
            component_revision,
        };
        self.add(entry).await
    }

    async fn create_snapshot_based_update_description(
        &self,
        target_revision: ComponentRevision,
        payload: Vec<u8>,
        mime_type: String,
        filesystem_snapshot: Option<FilesystemSnapshotName>,
    ) -> Result<UpdateDescription, String> {
        let payload = self.upload_payload_owned(payload).await?;
        Ok(UpdateDescription::SnapshotBased {
            target_revision,
            payload,
            mime_type,
            filesystem_snapshot,
        })
    }

    async fn get_upload_description_payload(
        &self,
        description: UpdateDescription,
    ) -> Result<Option<(Vec<u8>, String)>, String> {
        match description {
            UpdateDescription::SnapshotBased {
                payload, mime_type, ..
            } => {
                let bytes = self.download_payload(payload).await?;
                Ok(Some((bytes, mime_type)))
            }
            UpdateDescription::Automatic { .. }
            | UpdateDescription::SnapshotAssistedAutomatic { .. } => Ok(None),
        }
    }
}

#[async_trait]
impl<O: Oplog + ?Sized> OplogOps for O {}

#[async_trait]
pub trait OplogServiceOps: OplogService {
    /// Uploads a big oplog payload and returns a reference to it
    async fn upload_payload<T: BinaryCodec + Debug + Clone + PartialEq + Sync>(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        data: &T,
    ) -> Result<OplogPayload<T>, String> {
        let bytes = serialize(&data)?;
        let raw_payload = self
            .upload_raw_payload(owned_agent_id, agent_mode, bytes)
            .await?;
        let cached = Arc::new(data.clone());
        let payload = raw_payload.into_payload_with_cache(cached)?;
        Ok(payload)
    }

    /// Downloads a payload without losing the distinction between storage failure and corruption.
    async fn download_payload<
        T: BinaryCodec + Debug + Clone + PartialEq + Send + Sync + 'static,
    >(
        &self,
        owned_agent_id: &OwnedAgentId,
        agent_mode: AgentMode,
        payload: OplogPayload<T>,
    ) -> Result<T, OplogPayloadDownloadError> {
        match payload {
            OplogPayload::Inline(value) => Ok(*value),
            OplogPayload::SerializedInline {
                cached: Some(value),
                ..
            }
            | OplogPayload::External {
                cached: Some(value),
                ..
            } => Ok((*value).clone()),
            OplogPayload::SerializedInline { bytes, .. } => deserialize_oplog_payload(bytes)
                .await
                .map_err(OplogPayloadDownloadError::Corrupt),
            OplogPayload::External {
                payload_id,
                md5_hash,
                ..
            } => {
                let bytes = self
                    .download_raw_payload_classified(
                        owned_agent_id,
                        agent_mode,
                        payload_id,
                        md5_hash,
                    )
                    .await
                    .map_err(|error| match error {
                        RawOplogPayloadDownloadError::Backend(error) => {
                            OplogPayloadDownloadError::Backend(error)
                        }
                        RawOplogPayloadDownloadError::Missing(payload_id) => {
                            OplogPayloadDownloadError::Corrupt(anyhow::anyhow!(
                                "referenced oplog payload {payload_id} is missing"
                            ))
                        }
                    })?;
                deserialize_oplog_payload(bytes)
                    .await
                    .map_err(OplogPayloadDownloadError::Corrupt)
            }
        }
    }
}

#[async_trait]
impl<O: OplogService + ?Sized> OplogServiceOps for O {}
