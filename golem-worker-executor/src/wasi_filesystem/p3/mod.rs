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

use std::future::Future;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::Duration;

use crate::durable_host::authorization::targets::CanonicalGuestPath;
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_p3_view, durable_worker_ctx, observe_function_call,
    observe_function_call_store, run_read_access,
};
use crate::durable_host::tail_work::TailActivity;
use crate::durable_host::{
    CallReplayOutcome, DurableCallSession, LiveAuthorizationPermit, NotCancellable,
    authorize_live_permissions_at_serialized_access,
};
use crate::services::agent_filesystem::{
    self as agent_filesystem, AttributeChanges, Attributes as AgentAttributes,
    Error as AgentFilesystemError, File as AgentFile, FilesystemGenerationHandle,
    FilesystemStorageError, NamespaceEdit, NewObject, ObjectKind, OpenNode, PathTarget,
    SymlinkTarget, Target as AgentTarget, TimeChange, TimeChanges, WritePlacement, WriteResult,
};
use crate::wasi_filesystem::{
    AgentDescriptor, AgentOpenRequest, AgentOpenRouteError, advance_write_placement,
    agent_descriptor_guest_path, calculate_metadata_hash_parts, delete_agent_descriptor,
    filesystem_permission_targets, flush_level, get_agent_descriptor, push_agent_descriptor,
    resize_attribute_changes, route_agent_flush, route_agent_namespace_edit, route_agent_open,
    route_agent_set_attributes, route_agent_write, run_agent_filesystem_call,
};
use crate::workerctx::WorkerCtx;
use bytes::{Bytes, BytesMut};
use golem_common::model::card::FilesystemVerb;
use golem_common::model::entity::FilesystemCapability;
use golem_common::model::oplog::host_functions::{
    P3FilesystemTypesDescriptorAppendViaStream, P3FilesystemTypesDescriptorStat,
    P3FilesystemTypesDescriptorStatAt, P3FilesystemTypesDescriptorWriteViaStream,
};
use golem_common::model::oplog::types::{SerializableFileTimes, SerializableP3FileSystemError};
use golem_common::model::oplog::{
    DurableFunctionType, HostPayloadPair, HostRequestFileSystemPath, HostRequestNoInput,
    HostResponseP3FileSystemStat, HostResponseP3FileSystemWriteAdmission,
};
use wasmtime::component::{
    Access, Accessor, AccessorTask, Destination, FutureReader, Linker, Resource, Source,
    StreamConsumer, StreamProducer, StreamReader, StreamResult,
};
use wasmtime::{AsContextMut, StoreContextMut};
use wasmtime_wasi::filesystem::{Descriptor, WasiFilesystemView};
use wasmtime_wasi::p3::bindings::filesystem::{preopens, types};
use wasmtime_wasi::p3::filesystem::{FilesystemError, FilesystemResult};

fn p3_descriptor_guest_path(
    descriptor: &AgentDescriptor,
    relative: &str,
) -> FilesystemResult<CanonicalGuestPath> {
    agent_descriptor_guest_path(descriptor, relative)
        .map_err(|_| types::ErrorCode::NotPermitted.into())
}

async fn authorize_paths<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    requests: &[(FilesystemVerb, CanonicalGuestPath)],
) -> FilesystemResult<Option<LiveAuthorizationPermit>> {
    let targets = accessor.with(|mut access| {
        let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
        ctx.is_live()
            .then(|| filesystem_permission_targets(ctx, requests))
    });
    let Some(targets) = targets else {
        return Ok(None);
    };
    match authorize_live_permissions_at_serialized_access(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        &targets,
    )
    .await
    .map_err(|error| FilesystemError::trap(wasmtime::Error::from(error)))?
    {
        Ok(permit) => Ok(Some(permit)),
        Err(_) => Err(types::ErrorCode::NotPermitted.into()),
    }
}

fn p3_agent_storage_error(error: FilesystemStorageError) -> FilesystemError {
    match error.io_error() {
        Some(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            types::ErrorCode::CrossDevice.into()
        }
        Some(error) => types::ErrorCode::from(error).into(),
        None => FilesystemError::trap(wasmtime::Error::msg(error.to_string())),
    }
}

fn p3_agent_error(error: AgentFilesystemError) -> FilesystemError {
    match error {
        AgentFilesystemError::Access(agent_filesystem::AccessError::NotPermitted) => {
            types::ErrorCode::NotPermitted.into()
        }
        AgentFilesystemError::Sandbox(error) => p3_agent_storage_error(error),
        AgentFilesystemError::AgentQuota(_) => types::ErrorCode::Quota.into(),
        AgentFilesystemError::PhysicalCapacity(_) => types::ErrorCode::InsufficientSpace.into(),
        error @ (AgentFilesystemError::Access(_) | AgentFilesystemError::RuntimeInvalidated) => {
            FilesystemError::trap(wasmtime::Error::msg(error.to_string()))
        }
    }
}

fn p3_agent_open_error(error: AgentOpenRouteError) -> FilesystemError {
    match error {
        AgentOpenRouteError::Invalid => types::ErrorCode::Invalid.into(),
        AgentOpenRouteError::Unsupported => types::ErrorCode::Unsupported.into(),
        AgentOpenRouteError::SymlinkLoop => types::ErrorCode::Loop.into(),
        AgentOpenRouteError::Filesystem(error) => p3_agent_error(error),
    }
}

fn p3_link_access_error(error: agent_filesystem::AccessError) -> FilesystemError {
    match error {
        agent_filesystem::AccessError::WrongGeneration => types::ErrorCode::CrossDevice.into(),
        error => p3_agent_error(AgentFilesystemError::Access(error)),
    }
}

fn p3_agent_datetime(
    time: std::time::SystemTime,
) -> FilesystemResult<wasmtime_wasi::p3::bindings::clocks::system_clock::Instant> {
    let (seconds, nanoseconds) = match time.duration_since(std::time::SystemTime::UNIX_EPOCH) {
        Ok(duration) => (
            i64::try_from(duration.as_secs()).map_err(|_| types::ErrorCode::Overflow)?,
            duration.subsec_nanos(),
        ),
        Err(error) => {
            let duration = error.duration();
            let nanoseconds = duration.subsec_nanos();
            let seconds = -i128::from(duration.as_secs()) - i128::from(nanoseconds != 0);
            (
                i64::try_from(seconds).map_err(|_| types::ErrorCode::Overflow)?,
                if nanoseconds == 0 {
                    0
                } else {
                    1_000_000_000 - nanoseconds
                },
            )
        }
    };
    Ok(wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
        seconds,
        nanoseconds,
    })
}

fn p3_agent_descriptor_type(kind: ObjectKind) -> types::DescriptorType {
    match kind {
        ObjectKind::File => types::DescriptorType::RegularFile,
        ObjectKind::Directory => types::DescriptorType::Directory,
        ObjectKind::Symlink => types::DescriptorType::SymbolicLink,
    }
}

fn p3_agent_stat(attributes: AgentAttributes) -> FilesystemResult<types::DescriptorStat> {
    Ok(types::DescriptorStat {
        type_: p3_agent_descriptor_type(attributes.kind),
        link_count: attributes.link_count,
        size: attributes.size,
        data_access_timestamp: attributes.accessed.map(p3_agent_datetime).transpose()?,
        data_modification_timestamp: attributes.modified.map(p3_agent_datetime).transpose()?,
        status_change_timestamp: None,
    })
}

fn p3_agent_open_request(
    path_flags: types::PathFlags,
    open_flags: types::OpenFlags,
    descriptor_flags: types::DescriptorFlags,
) -> AgentOpenRequest {
    AgentOpenRequest {
        create: open_flags.contains(types::OpenFlags::CREATE),
        directory: open_flags.contains(types::OpenFlags::DIRECTORY),
        exclusive: open_flags.contains(types::OpenFlags::EXCLUSIVE),
        truncate: open_flags.contains(types::OpenFlags::TRUNCATE),
        follow: path_flags.contains(types::PathFlags::SYMLINK_FOLLOW),
        read: descriptor_flags.contains(types::DescriptorFlags::READ),
        write: descriptor_flags.contains(types::DescriptorFlags::WRITE),
        unsupported_sync: descriptor_flags.intersects(
            types::DescriptorFlags::FILE_INTEGRITY_SYNC
                | types::DescriptorFlags::DATA_INTEGRITY_SYNC
                | types::DescriptorFlags::REQUESTED_WRITE_SYNC,
        ),
    }
}

/// Handles a P3 `open-at` request through the shared agent-filesystem open policy and lifecycle.
/// It returns an opened node for resource-table insertion and maps policy or filesystem failures to P3.
pub(crate) async fn route_open(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
    path_flags: types::PathFlags,
    open_flags: types::OpenFlags,
    descriptor_flags: types::DescriptorFlags,
) -> FilesystemResult<agent_filesystem::Opened> {
    route_agent_open(
        generation_handle,
        target,
        p3_agent_open_request(path_flags, open_flags, descriptor_flags),
    )
    .await
    .map_err(p3_agent_open_error)
}

#[cfg(test)]
pub(crate) async fn route_read_file(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    offset: u64,
    length: usize,
) -> FilesystemResult<Bytes> {
    run_agent_filesystem_call(agent_filesystem::read_file(
        generation_handle,
        file,
        agent_filesystem::ReadRange { offset, length },
    ))
    .await
    .map_err(p3_agent_error)
}

#[cfg(test)]
pub(crate) async fn route_attributes(
    generation_handle: &FilesystemGenerationHandle,
    target: AgentTarget<'_>,
) -> FilesystemResult<types::DescriptorStat> {
    let attributes =
        run_agent_filesystem_call(agent_filesystem::attributes(generation_handle, target))
            .await
            .map_err(p3_agent_error)?;
    p3_agent_stat(attributes)
}

#[cfg(test)]
pub(crate) async fn route_list_directory(
    generation_handle: &FilesystemGenerationHandle,
    directory: &agent_filesystem::Directory,
) -> FilesystemResult<Vec<types::DirectoryEntry>> {
    run_agent_filesystem_call(agent_filesystem::list_directory(
        generation_handle,
        directory,
    ))
    .await
    .map_err(p3_agent_error)?
    .into_iter()
    .map(|entry| {
        Ok(types::DirectoryEntry {
            type_: p3_agent_descriptor_type(entry.kind),
            name: entry
                .name
                .into_string()
                .map_err(|_| FilesystemError::from(types::ErrorCode::IllegalByteSequence))?,
        })
    })
    .collect()
}

/// Resolves a P3 `readlink-at` target through `agent_filesystem`.
/// A target that is not valid Unicode returns `IllegalByteSequence`.
pub(crate) async fn route_symlink_target(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
) -> FilesystemResult<String> {
    let target =
        run_agent_filesystem_call(agent_filesystem::symlink_target(generation_handle, target))
            .await
            .map_err(p3_agent_error)?;
    target
        .0
        .into_os_string()
        .into_string()
        .map_err(|_| types::ErrorCode::IllegalByteSequence.into())
}

async fn route_namespace_edit(
    generation_handle: &FilesystemGenerationHandle,
    edit: NamespaceEdit,
) -> FilesystemResult<()> {
    let call = route_agent_namespace_edit(generation_handle, edit).map_err(p3_agent_error)?;
    call.await.map_err(p3_agent_error)
}

/// Creates a directory for P3 by submitting an agent-filesystem namespace insertion.
pub(crate) async fn route_create_directory(
    generation_handle: &FilesystemGenerationHandle,
    destination: PathTarget,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Insert {
            destination,
            object: NewObject::Directory,
        },
    )
    .await
}

/// Creates a symlink for P3 by submitting its destination and raw target to `agent_filesystem`.
pub(crate) async fn route_create_symlink(
    generation_handle: &FilesystemGenerationHandle,
    destination: PathTarget,
    target: impl Into<PathBuf>,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Insert {
            destination,
            object: NewObject::Symlink(SymlinkTarget(target.into())),
        },
    )
    .await
}

/// Creates a P3 hard link through the agent-filesystem namespace editor.
/// Following the source symlink is invalid, and cross-generation links report `CrossDevice`.
pub(crate) async fn route_hard_link(
    generation_handle: &FilesystemGenerationHandle,
    source: PathTarget,
    source_flags: types::PathFlags,
    destination: PathTarget,
) -> FilesystemResult<()> {
    if source_flags.contains(types::PathFlags::SYMLINK_FOLLOW) {
        return Err(types::ErrorCode::Invalid.into());
    }
    let call = match route_agent_namespace_edit(
        generation_handle,
        NamespaceEdit::Link {
            source,
            destination,
        },
    ) {
        Err(AgentFilesystemError::Access(error)) => return Err(p3_link_access_error(error)),
        result => result.map_err(p3_agent_error)?,
    };
    call.await.map_err(p3_agent_error)
}

/// Renames a P3 path through an agent-filesystem namespace move.
pub(crate) async fn route_rename(
    generation_handle: &FilesystemGenerationHandle,
    source: PathTarget,
    destination: PathTarget,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Move {
            source,
            destination,
        },
    )
    .await
}

/// Removes a P3 directory through `agent_filesystem`, requiring the target to be a directory.
pub(crate) async fn route_remove_directory(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Remove {
            target,
            expected: ObjectKind::Directory,
        },
    )
    .await
}

/// Unlinks a P3 path through `agent_filesystem` while enforcing the caller's expected object kind.
pub(crate) async fn route_unlink(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
    expected: ObjectKind,
) -> FilesystemResult<()> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Remove { target, expected },
    )
    .await
}

type P3WriteFutureResult = wasmtime::Result<Result<(usize, WritePlacement), types::ErrorCode>>;

#[cfg(test)]
pub(crate) fn route_append_via_stream_chunk(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    bytes: Bytes,
) -> FilesystemResult<impl Future<Output = P3WriteFutureResult> + Send + 'static + use<>> {
    route_stream_chunk(generation_handle, file, WritePlacement::Append, bytes)
}

fn route_stream_chunk(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    placement: WritePlacement,
    bytes: Bytes,
) -> FilesystemResult<impl Future<Output = P3WriteFutureResult> + Send + 'static + use<>> {
    let call =
        route_agent_write(generation_handle, file, placement, bytes).map_err(p3_agent_error)?;
    Ok(async move { p3_agent_write_result(call.await, placement) })
}

/// Starts a P3 file resize through the agent-filesystem attribute route.
/// The returned future owns the admitted call and reports its final P3 result.
pub(crate) fn route_set_size(
    generation_handle: &FilesystemGenerationHandle,
    node: &OpenNode,
    size: types::Filesize,
) -> FilesystemResult<impl Future<Output = FilesystemResult<()>> + Send + 'static + use<>> {
    let call = route_agent_set_attributes(
        generation_handle,
        AgentTarget::Open(node),
        resize_attribute_changes(size),
    )
    .map_err(p3_agent_error)?;
    Ok(async move { call.await.map_err(p3_agent_error) })
}

/// Starts a P3 timestamp update for an open descriptor or path through `agent_filesystem`.
/// Invalid timestamps fail before submission; the returned future reports operation failures.
pub(crate) fn route_set_times(
    generation_handle: &FilesystemGenerationHandle,
    target: AgentTarget<'_>,
    accessed: types::NewTimestamp,
    modified: types::NewTimestamp,
) -> FilesystemResult<impl Future<Output = FilesystemResult<()>> + Send + 'static + use<>> {
    let changes = p3_time_changes(accessed, modified)?;
    let call =
        route_agent_set_attributes(generation_handle, target, AttributeChanges::Times(changes))
            .map_err(p3_agent_error)?;
    Ok(async move { call.await.map_err(p3_agent_error) })
}

/// Starts a P3 descriptor flush through `agent_filesystem`.
/// `data_only` selects a data-only flush rather than a data-and-metadata flush.
pub(crate) fn route_flush(
    generation_handle: &FilesystemGenerationHandle,
    node: &OpenNode,
    data_only: bool,
) -> FilesystemResult<impl Future<Output = FilesystemResult<()>> + Send + 'static + use<>> {
    let call = route_agent_flush(generation_handle, node, flush_level(data_only))
        .map_err(p3_agent_error)?;
    Ok(async move { call.await.map_err(p3_agent_error) })
}

#[cfg(test)]
pub(crate) fn route_replay_times(
    generation_handle: &FilesystemGenerationHandle,
    target: AgentTarget<'_>,
    accessed: Option<std::time::SystemTime>,
    modified: Option<std::time::SystemTime>,
) -> FilesystemResult<impl Future<Output = FilesystemResult<()>> + Send + 'static + use<>> {
    let call = crate::wasi_filesystem::route_replay_timestamp_restoration(
        generation_handle,
        target,
        accessed,
        modified,
    )
    .map_err(p3_agent_error)?;
    Ok(async move { call.await.map_err(p3_agent_error) })
}

fn p3_agent_write_result(
    result: Result<WriteResult, AgentFilesystemError>,
    placement: WritePlacement,
) -> P3WriteFutureResult {
    match result {
        Ok(result) => {
            let written = usize::try_from(result.written)
                .map_err(|_| wasmtime::Error::msg("filesystem write progress overflowed"))?;
            let next = advance_write_placement(placement, result.written)
                .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
            Ok(Ok((written, next)))
        }
        Err(AgentFilesystemError::Sandbox(error)) => match error.io_error() {
            Some(error) => Ok(Err(types::ErrorCode::from(error))),
            None => Err(wasmtime::Error::msg(error.to_string())),
        },
        Err(AgentFilesystemError::AgentQuota(_)) => Ok(Err(types::ErrorCode::Quota)),
        Err(AgentFilesystemError::PhysicalCapacity(_)) => {
            Ok(Err(types::ErrorCode::InsufficientSpace))
        }
        Err(
            error @ (AgentFilesystemError::Access(_) | AgentFilesystemError::RuntimeInvalidated),
        ) => Err(wasmtime::Error::msg(error.to_string())),
    }
}

/// Registers the WASI P3 filesystem types and preopens host interfaces.
/// P3 accessors route descriptor ownership through `DurableP3` and the shared agent-filesystem table.
pub(crate) fn add_to_linker<Ctx: WorkerCtx>(linker: &mut Linker<Ctx>) -> wasmtime::Result<()> {
    types::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    preopens::add_to_linker::<_, DurableP3<Ctx>>(linker, durable_p3_view::<Ctx>)?;
    Ok(())
}

const FILESYSTEM_READ_CHUNK_SIZE: usize = 64 * 1024;

type FilesystemReadResult = wasmtime::Result<Result<(), types::ErrorCode>>;

struct FilesystemReadProducer {
    generation_handle: FilesystemGenerationHandle,
    descriptor: AgentDescriptor,
    offset: u64,
    buffered: Bytes,
    pending: Option<Pin<Box<agent_filesystem::FilesystemCall<Bytes>>>>,
    result_tx: Option<tokio::sync::oneshot::Sender<FilesystemReadResult>>,
    #[cfg(test)]
    read_calls: Arc<std::sync::atomic::AtomicUsize>,
}

impl FilesystemReadProducer {
    async fn new(
        generation_handle: FilesystemGenerationHandle,
        descriptor: AgentDescriptor,
        offset: u64,
        result_tx: tokio::sync::oneshot::Sender<FilesystemReadResult>,
    ) -> Self {
        #[cfg(test)]
        let read_calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut producer = Self {
            generation_handle,
            descriptor,
            offset,
            buffered: Bytes::new(),
            pending: None,
            result_tx: Some(result_tx),
            #[cfg(test)]
            read_calls,
        };
        let prefetched = producer.read_chunk(FILESYSTEM_READ_CHUNK_SIZE).await;
        match prefetched {
            Ok(Some(bytes)) => producer.buffered = bytes,
            Ok(None) => producer.close(Ok(Ok(()))),
            Err(error) => producer.close_error(error),
        }
        producer
    }

    fn close(&mut self, result: FilesystemReadResult) {
        if let Some(result_tx) = self.result_tx.take() {
            let _ = result_tx.send(result);
        }
    }

    fn close_error(&mut self, error: FilesystemError) {
        match error.downcast() {
            Ok(error) => self.close(Ok(Err(error))),
            Err(error) => self.close(Err(error)),
        }
    }

    fn start_read_chunk(
        &self,
        length: usize,
    ) -> FilesystemResult<agent_filesystem::FilesystemCall<Bytes>> {
        #[cfg(test)]
        self.read_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.descriptor.with_node(|node| match node {
            OpenNode::File(file) => agent_filesystem::read_file(
                &self.generation_handle,
                file,
                agent_filesystem::ReadRange {
                    offset: self.offset,
                    length,
                },
            )
            .map_err(|error| p3_agent_error(AgentFilesystemError::Access(error))),
            OpenNode::Directory(_) => Err(types::ErrorCode::BadDescriptor.into()),
        })
    }

    async fn read_chunk(&mut self, length: usize) -> FilesystemResult<Option<Bytes>> {
        let bytes = self
            .start_read_chunk(length)?
            .await
            .map_err(p3_agent_error)?;
        if bytes.is_empty() {
            return Ok(None);
        }
        self.offset = self
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or(types::ErrorCode::Overflow)?;
        Ok(Some(bytes))
    }

    fn take_buffered(&mut self, length: usize) -> Bytes {
        let length = self.buffered.len().min(length);
        self.buffered.split_to(length)
    }

    #[cfg(test)]
    fn read_call_count(&self) -> usize {
        self.read_calls.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn poll_read_chunk(
        &mut self,
        cx: &mut Context<'_>,
        length: usize,
    ) -> Poll<FilesystemResult<Option<Bytes>>> {
        if self.pending.is_none() {
            let call = match self.start_read_chunk(length) {
                Ok(call) => call,
                Err(error) => return Poll::Ready(Err(error)),
            };
            self.pending = Some(Box::pin(call));
        }

        let result = match self
            .pending
            .as_mut()
            .expect("read call missing")
            .as_mut()
            .poll(cx)
        {
            Poll::Pending => return Poll::Pending,
            Poll::Ready(result) => result,
        };
        self.pending = None;
        let bytes = result.map_err(p3_agent_error)?;
        if bytes.is_empty() {
            return Poll::Ready(Ok(None));
        }
        self.offset = self
            .offset
            .checked_add(bytes.len() as u64)
            .ok_or(types::ErrorCode::Overflow)?;
        Poll::Ready(Ok(Some(bytes)))
    }
}

impl<D> StreamProducer<D> for FilesystemReadProducer {
    type Item = u8;
    type Buffer = BytesMut;

    fn poll_produce<'a>(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        mut store: StoreContextMut<'a, D>,
        dst: Destination<'a, Self::Item, Self::Buffer>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        if finish {
            self.pending.take();
            return Poll::Ready(Ok(StreamResult::Cancelled));
        }
        if dst.remaining(store.as_context_mut()) == Some(0) {
            return Poll::Ready(Ok(StreamResult::Completed));
        }

        let mut dst = dst.as_direct(store, FILESYSTEM_READ_CHUNK_SIZE);
        let length = dst.remaining().len().min(FILESYSTEM_READ_CHUNK_SIZE);
        if !self.buffered.is_empty() {
            let bytes = self.take_buffered(length);
            let length = bytes.len();
            dst.remaining()[..length].copy_from_slice(&bytes);
            dst.mark_written(length);
            return Poll::Ready(Ok(StreamResult::Completed));
        }
        if self.result_tx.is_none() {
            return Poll::Ready(Ok(StreamResult::Dropped));
        }
        match self.poll_read_chunk(cx, length) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(Some(bytes))) => {
                let length = bytes.len();
                dst.remaining()[..length].copy_from_slice(&bytes);
                dst.mark_written(length);
                Poll::Ready(Ok(StreamResult::Completed))
            }
            Poll::Ready(Ok(None)) => {
                self.close(Ok(Ok(())));
                Poll::Ready(Ok(StreamResult::Dropped))
            }
            Poll::Ready(Err(error)) => {
                self.close_error(error);
                Poll::Ready(Ok(StreamResult::Dropped))
            }
        }
    }
}

impl Drop for FilesystemReadProducer {
    fn drop(&mut self) {
        self.close(Ok(Ok(())));
    }
}

struct FilesystemWriteChunk {
    result_tx: tokio::sync::oneshot::Sender<(usize, FilesystemWriteResult)>,
    bytes: Bytes,
}

#[derive(Clone, Debug)]
enum FilesystemWriteFailure {
    Guest(types::ErrorCode),
}

type FilesystemWriteResult = Result<(), FilesystemWriteFailure>;

struct FilesystemWriteConsumer {
    chunks_tx: Option<tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>>,
    pending_chunk: Option<PendingFilesystemWriteChunk>,
}

struct PendingFilesystemWriteChunk {
    result_rx: tokio::sync::oneshot::Receiver<(usize, FilesystemWriteResult)>,
}

impl FilesystemWriteConsumer {
    fn new(chunks_tx: tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>) -> Self {
        Self {
            chunks_tx: Some(chunks_tx),
            pending_chunk: None,
        }
    }

    fn cancel(&mut self) {
        self.chunks_tx.take();
    }

    fn poll_pending_result(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<wasmtime::Result<Option<(usize, FilesystemWriteResult)>>> {
        let Some(pending) = &mut self.pending_chunk else {
            return Poll::Ready(Ok(None));
        };
        match Pin::new(&mut pending.result_rx).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(result)) => {
                self.pending_chunk = None;
                Poll::Ready(Ok(Some(result)))
            }
            Poll::Ready(Err(_)) => {
                self.pending_chunk = None;
                self.chunks_tx.take();
                Poll::Ready(Err(wasmtime::Error::msg(
                    "filesystem write task dropped before reporting its effect",
                )))
            }
        }
    }
}

impl<D> StreamConsumer<D> for FilesystemWriteConsumer {
    type Item = u8;

    fn poll_consume(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        store: StoreContextMut<D>,
        src: Source<Self::Item>,
        finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);

        if finish {
            self.cancel();
            if self.pending_chunk.is_none() {
                return Poll::Ready(Ok(StreamResult::Cancelled));
            }
        }

        loop {
            // Wait for the in-flight chunk to be persisted before reading more.
            // The receiver must be polled here (not just stored) so its waker is
            // registered; otherwise the write task's completion notification
            // could be missed, hanging the stream.
            if self.pending_chunk.is_some() {
                match self.poll_pending_result(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(Some((written, Ok(()))))) => {
                        src.mark_read(written);
                        return Poll::Ready(Ok(if finish {
                            StreamResult::Cancelled
                        } else {
                            StreamResult::Completed
                        }));
                    }
                    Poll::Ready(Ok(Some((written, Err(FilesystemWriteFailure::Guest(_)))))) => {
                        self.chunks_tx.take();
                        src.mark_read(written);
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }
                    Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                    Poll::Ready(Ok(None)) => unreachable!("pending result disappeared"),
                }
            }

            let bytes = src.remaining();
            if bytes.is_empty() {
                return Poll::Ready(Ok(StreamResult::Completed));
            }

            let Some(chunks_tx) = &self.chunks_tx else {
                return Poll::Ready(Ok(StreamResult::Dropped));
            };

            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            chunks_tx
                .send(FilesystemWriteChunk {
                    result_tx,
                    bytes: Bytes::copy_from_slice(bytes),
                })
                .map_err(|_| wasmtime::Error::msg("filesystem write task dropped"))?;
            self.pending_chunk = Some(PendingFilesystemWriteChunk { result_rx });
            // Loop back to poll the freshly created receiver and register its waker.
        }
    }
}

impl Drop for FilesystemWriteConsumer {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct FilesystemWriteTask<Ctx> {
    chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    generation_handle: FilesystemGenerationHandle,
    descriptor: AgentDescriptor,
    placement: WritePlacement,
    activity: TailActivity,
    _authorization_permit: Option<LiveAuthorizationPermit>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> FilesystemWriteTask<Ctx> {
    fn new(
        chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
        generation_handle: FilesystemGenerationHandle,
        descriptor: AgentDescriptor,
        placement: WritePlacement,
        activity: TailActivity,
        authorization_permit: Option<LiveAuthorizationPermit>,
    ) -> Self {
        Self {
            chunks_rx,
            result_tx,
            generation_handle,
            descriptor,
            placement,
            activity,
            _authorization_permit: authorization_permit,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for FilesystemWriteTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, _accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let FilesystemWriteTask {
            mut chunks_rx,
            result_tx,
            generation_handle,
            descriptor,
            placement,
            activity,
            _authorization_permit,
            _phantom,
        } = self;
        complete_filesystem_write_task(
            result_tx,
            run_streaming_filesystem_write(
                &mut chunks_rx,
                &generation_handle,
                &descriptor,
                placement,
                &activity,
            ),
        )
        .await;
        Ok(())
    }
}

async fn complete_filesystem_write_task(
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    operation: impl Future<Output = wasmtime::Result<Result<(), types::ErrorCode>>>,
) {
    let result = operation.await;
    if !result_tx.is_closed() {
        let _ = result_tx.send(result);
    }
}

fn descriptor_guest_path_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
    relative: &str,
) -> FilesystemResult<CanonicalGuestPath>
where
    U: 'static,
{
    let descriptor = store
        .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, fd))
        .map_err(FilesystemError::trap)?;
    p3_descriptor_guest_path(&descriptor, relative)
}

fn descriptor_path_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<PathBuf>
where
    U: 'static,
{
    store.with(|mut access| {
        Ok(agent_descriptor_from_access::<Ctx, U>(&mut access, fd)?
            .path()
            .to_path_buf())
    })
}

fn descriptor_path_at_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
    path: &str,
) -> wasmtime::Result<PathBuf>
where
    U: 'static,
{
    store.with(|mut access| {
        Ok(agent_descriptor_from_access::<Ctx, U>(&mut access, fd)?
            .path()
            .join(path))
    })
}

fn agent_descriptor_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<AgentDescriptor>
where
    U: 'static,
{
    Ok(get_agent_descriptor(
        durable_worker_ctx::<Ctx, U>(store.data_mut()),
        fd,
    )?)
}

fn agent_path_target(
    descriptor: &AgentDescriptor,
    path: impl Into<PathBuf>,
) -> FilesystemResult<PathTarget> {
    descriptor.with_node(|node| match node {
        OpenNode::Directory(directory) => Ok(PathTarget::at(directory, path)),
        OpenNode::File(_) => Err(types::ErrorCode::NotDirectory.into()),
    })
}

fn agent_descriptor_flags(
    generation_handle: &FilesystemGenerationHandle,
    descriptor: &AgentDescriptor,
) -> FilesystemResult<types::DescriptorFlags> {
    let (kind, mode) = descriptor.with_node(|node| (node.kind(), node.access()));
    let mut flags = types::DescriptorFlags::empty();
    if matches!(
        mode,
        agent_filesystem::AccessMode::Read | agent_filesystem::AccessMode::ReadWrite
    ) {
        flags |= types::DescriptorFlags::READ;
    }
    if matches!(
        mode,
        agent_filesystem::AccessMode::Write | agent_filesystem::AccessMode::ReadWrite
    ) {
        flags |= if kind == ObjectKind::Directory {
            types::DescriptorFlags::MUTATE_DIRECTORY
        } else {
            types::DescriptorFlags::WRITE
        };
    }
    if kind == ObjectKind::File
        && agent_filesystem::path_permissions(generation_handle, descriptor.path())
            .map_err(|error| p3_agent_error(AgentFilesystemError::Access(error)))?
            == golem_common::model::component::AgentFilePermissions::ReadOnly
    {
        flags &= !types::DescriptorFlags::WRITE;
    }
    Ok(flags)
}

fn push_agent_descriptor_from_accessor<Ctx: WorkerCtx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    descriptor: AgentDescriptor,
) -> FilesystemResult<Resource<Descriptor>>
where
    U: 'static,
{
    accessor
        .with(|mut access| {
            push_agent_descriptor(durable_worker_ctx::<Ctx, U>(access.data_mut()), descriptor)
        })
        .map_err(FilesystemError::trap)
}

fn p3_native_time(
    requested: types::NewTimestamp,
) -> Result<Option<std::time::SystemTime>, FilesystemError> {
    match requested {
        types::NewTimestamp::NoChange => Ok(None),
        types::NewTimestamp::Now => Ok(Some(std::time::SystemTime::now())),
        types::NewTimestamp::Timestamp(timestamp) => p3_system_time(timestamp)
            .map(Some)
            .ok_or_else(|| types::ErrorCode::Overflow.into()),
    }
}

fn p3_system_time(
    timestamp: wasmtime_wasi::p3::bindings::clocks::system_clock::Instant,
) -> Option<std::time::SystemTime> {
    if timestamp.nanoseconds >= 1_000_000_000 {
        return None;
    }
    if timestamp.seconds >= 0 {
        std::time::SystemTime::UNIX_EPOCH.checked_add(Duration::new(
            timestamp.seconds as u64,
            timestamp.nanoseconds,
        ))
    } else if timestamp.nanoseconds == 0 {
        std::time::SystemTime::UNIX_EPOCH
            .checked_sub(Duration::from_secs(timestamp.seconds.unsigned_abs()))
    } else {
        std::time::SystemTime::UNIX_EPOCH.checked_sub(Duration::new(
            timestamp.seconds.unsigned_abs() - 1,
            1_000_000_000 - timestamp.nanoseconds,
        ))
    }
}

fn p3_validate_time(requested: types::NewTimestamp) -> Result<(), FilesystemError> {
    match requested {
        types::NewTimestamp::Timestamp(timestamp) => p3_system_time(timestamp)
            .map(|_| ())
            .ok_or_else(|| types::ErrorCode::Overflow.into()),
        types::NewTimestamp::NoChange | types::NewTimestamp::Now => Ok(()),
    }
}

fn p3_time_changes(
    accessed: types::NewTimestamp,
    modified: types::NewTimestamp,
) -> Result<TimeChanges, FilesystemError> {
    p3_validate_time(accessed)?;
    p3_validate_time(modified)?;
    Ok(TimeChanges {
        accessed: p3_time_change(accessed)?,
        modified: p3_time_change(modified)?,
    })
}

fn p3_time_change(requested: types::NewTimestamp) -> Result<TimeChange, FilesystemError> {
    match requested {
        types::NewTimestamp::NoChange => Ok(TimeChange::Keep),
        types::NewTimestamp::Now => Ok(TimeChange::Now),
        types::NewTimestamp::Timestamp(_) => p3_native_time(requested)?
            .map_or_else(|| Ok(TimeChange::Keep), |time| Ok(TimeChange::Set(time))),
    }
}

#[cfg(test)]
fn p3_visible_descriptor_flags(
    mut flags: types::DescriptorFlags,
    read_only: bool,
) -> types::DescriptorFlags {
    if read_only {
        flags &= !types::DescriptorFlags::WRITE;
    }
    flags
}

fn serialize_stat_error(error: &FilesystemError) -> SerializableP3FileSystemError {
    SerializableP3FileSystemError::from_result(
        error
            .downcast_ref()
            .cloned()
            .ok_or_else(|| error.to_string()),
    )
}

fn deserialize_stat_error(error: SerializableP3FileSystemError) -> FilesystemError {
    match error {
        SerializableP3FileSystemError::ErrorCode(error_code) => {
            types::ErrorCode::from(error_code).into()
        }
        SerializableP3FileSystemError::Generic(error) => {
            FilesystemError::trap(wasmtime::Error::msg(error))
        }
    }
}

fn serialize_write_admission_error(error: types::ErrorCode) -> SerializableP3FileSystemError {
    SerializableP3FileSystemError::from_result(Ok(error))
}

fn deserialize_write_admission(
    result: Result<(), SerializableP3FileSystemError>,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    match result {
        Ok(()) => Ok(Ok(())),
        Err(SerializableP3FileSystemError::ErrorCode(error)) => {
            Ok(Err(types::ErrorCode::from(error)))
        }
        Err(SerializableP3FileSystemError::Generic(error)) => Err(wasmtime::Error::msg(error)),
    }
}

async fn admit_stream_write<Pair, Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<Result<Option<LiveAuthorizationPermit>, types::ErrorCode>>
where
    Pair: HostPayloadPair<Req = HostRequestNoInput, Resp = HostResponseP3FileSystemWriteAdmission>,
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let authorization_result = Arc::new(Mutex::new(None));
    let authorization_for_start = Arc::clone(&authorization_result);
    let fd_rep = fd.rep();
    let mut handle = DurableCallSession::<Pair, NotCancellable>::start_access_with(
        accessor,
        durable_worker_ctx::<Ctx, U>,
        DurableFunctionType::ReadLocal,
        async move |start| {
            if start.is_live {
                let result = match descriptor_guest_path_from_accessor::<Ctx, U>(
                    accessor,
                    &Resource::new_borrow(fd_rep),
                    "",
                ) {
                    Ok(path) => authorize_paths(accessor, &[(FilesystemVerb::Write, path)])
                        .await
                        .map_err(|_| ()),
                    Err(_) => Err(()),
                };
                *authorization_for_start.lock().unwrap() = Some(result);
            }
            Ok(HostRequestNoInput {})
        },
    )
    .await
    .map_err(wasmtime::Error::from)?;

    let mut authorization_permit = None;
    let response = if handle.is_live() {
        let result = authorization_result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Err(()));
        let response = match result {
            Ok(permit) => {
                authorization_permit = permit;
                HostResponseP3FileSystemWriteAdmission { result: Ok(()) }
            }
            Err(()) => HostResponseP3FileSystemWriteAdmission {
                result: Err(serialize_write_admission_error(
                    types::ErrorCode::NotPermitted,
                )),
            },
        };
        handle
            .complete_access(accessor, durable_worker_ctx::<Ctx, U>, response)
            .await
            .map_err(wasmtime::Error::from)?
    } else {
        match handle
            .replay_access(accessor, durable_worker_ctx::<Ctx, U>)
            .await
            .map_err(wasmtime::Error::from)?
        {
            CallReplayOutcome::Replayed(response) => response,
            CallReplayOutcome::Incomplete(live) => {
                handle = live;
                handle
                    .complete_access(
                        accessor,
                        durable_worker_ctx::<Ctx, U>,
                        HostResponseP3FileSystemWriteAdmission { result: Ok(()) },
                    )
                    .await
                    .map_err(wasmtime::Error::from)?
            }
        }
    };

    Ok(deserialize_write_admission(response.result)?.map(|()| authorization_permit))
}

fn serialize_stat_result(
    stat: &Result<types::DescriptorStat, SerializableP3FileSystemError>,
) -> Result<SerializableFileTimes, SerializableP3FileSystemError> {
    stat.clone().map(|stat| SerializableFileTimes {
        data_access_timestamp: stat.data_access_timestamp.map(Into::into),
        data_modification_timestamp: stat.data_modification_timestamp.map(Into::into),
    })
}

async fn run_local_stat<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: Resource<Descriptor>,
) -> Result<types::DescriptorStat, SerializableP3FileSystemError>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let generation_handle = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
    });
    let descriptor =
        match store.with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd)) {
            Ok(descriptor) => descriptor,
            Err(error) => return Err(SerializableP3FileSystemError::Generic(error.to_string())),
        };
    let call = match descriptor.with_node(|node| {
        agent_filesystem::attributes(&generation_handle, AgentTarget::Open(node))
            .map_err(|error| p3_agent_error(AgentFilesystemError::Access(error)))
    }) {
        Ok(call) => call,
        Err(error) => return Err(serialize_stat_error(&error)),
    };
    call.await
        .map_err(p3_agent_error)
        .and_then(p3_agent_stat)
        .map_err(|error| serialize_stat_error(&error))
}

async fn run_local_stat_at<Ctx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: Resource<Descriptor>,
    path_flags: types::PathFlags,
    path: String,
) -> Result<types::DescriptorStat, SerializableP3FileSystemError>
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let generation_handle = store.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
    });
    let descriptor =
        match store.with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd)) {
            Ok(descriptor) => descriptor,
            Err(error) => return Err(SerializableP3FileSystemError::Generic(error.to_string())),
        };
    let target = match agent_path_target(&descriptor, path) {
        Ok(target) => target,
        Err(error) => return Err(serialize_stat_error(&error)),
    };
    let follow = if path_flags.contains(types::PathFlags::SYMLINK_FOLLOW) {
        agent_filesystem::Follow::Yes
    } else {
        agent_filesystem::Follow::No
    };
    let call = match agent_filesystem::attributes(
        &generation_handle,
        AgentTarget::Path(&target, follow),
    ) {
        Ok(call) => call,
        Err(error) => {
            return Err(serialize_stat_error(&p3_agent_error(
                AgentFilesystemError::Access(error),
            )));
        }
    };
    call.await
        .map_err(p3_agent_error)
        .and_then(p3_agent_stat)
        .map_err(|error| serialize_stat_error(&error))
}

/// Computes the metadata hash from a durable stat result, using the same hash
/// inputs and function as the WASI P2 implementation so P2 and P3 report
/// identical hashes for the same file state. P3 datetimes use signed seconds
/// while P2 uses unsigned; pre-epoch timestamps fail with `overflow` just like
/// the P2 stat conversion does.
fn metadata_hash_from_stat(
    stat: &types::DescriptorStat,
) -> FilesystemResult<types::MetadataHashValue> {
    let modified = stat
        .data_modification_timestamp
        .map(|timestamp| {
            let seconds =
                u64::try_from(timestamp.seconds).map_err(|_| types::ErrorCode::Overflow)?;
            Ok::<_, types::ErrorCode>((seconds, timestamp.nanoseconds))
        })
        .transpose()?;

    let (lower, upper) = calculate_metadata_hash_parts(modified, stat.size);
    Ok(types::MetadataHashValue { lower, upper })
}

async fn apply_stat_response(
    stat: Result<types::DescriptorStat, SerializableP3FileSystemError>,
    response: HostResponseP3FileSystemStat,
) -> FilesystemResult<types::DescriptorStat> {
    match response.result {
        Ok(times) => {
            let mut stat = stat.unwrap();
            stat.data_access_timestamp = times.data_access_timestamp.map(Into::into);
            stat.data_modification_timestamp = times.data_modification_timestamp.map(Into::into);
            Ok(stat)
        }
        Err(error) => Err(deserialize_stat_error(error)),
    }
}

async fn wait_filesystem_task_result(
    result_rx: tokio::sync::oneshot::Receiver<wasmtime::Result<Result<(), types::ErrorCode>>>,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    result_rx
        .await
        .unwrap_or_else(|_| Err(wasmtime::Error::msg("filesystem stream task dropped")))
}

// Drains and writes the guest's data stream to the worker filesystem chunk by
// chunk, mirroring the WASI P2 behavior: the file effect is driven entirely by
// the input stream finishing or erroring, never by the liveness of the returned
// result future. The bytes themselves are not recorded in the oplog; on replay
// the guest re-issues the same writes which deterministically rebuild the
// transient worker filesystem.
async fn run_streaming_filesystem_write(
    chunks_rx: &mut tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    generation_handle: &FilesystemGenerationHandle,
    descriptor: &AgentDescriptor,
    mut placement: WritePlacement,
    activity: &TailActivity,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    let mut result: FilesystemWriteResult = Ok(());
    // Safe park: each chunk is guest-produced stream data.
    while let Some(chunk) = activity.park(chunks_rx.recv()).await {
        let written_len = if result.is_ok() {
            let operation = descriptor.with_node(|node| match node {
                OpenNode::File(file) => {
                    route_stream_chunk(generation_handle, file, placement, chunk.bytes)
                }
                OpenNode::Directory(_) => Err(types::ErrorCode::BadDescriptor.into()),
            });
            match operation {
                Ok(operation) => match operation.await? {
                    Ok((written, next)) => {
                        placement = next;
                        written
                    }
                    Err(error) => {
                        result = Err(FilesystemWriteFailure::Guest(error));
                        0
                    }
                },
                Err(error) => match error.downcast() {
                    Ok(error) => {
                        result = Err(FilesystemWriteFailure::Guest(error));
                        0
                    }
                    Err(error) => return Err(error),
                },
            }
        } else {
            0
        };

        if chunk.result_tx.send((written_len, result.clone())).is_err() {
            return Err(wasmtime::Error::msg(
                "filesystem write progress could not be delivered",
            ));
        }
    }

    filesystem_write_result_to_wasi(result)
}

fn filesystem_write_result_to_wasi(
    result: FilesystemWriteResult,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    match result {
        Ok(()) => Ok(Ok(())),
        Err(FilesystemWriteFailure::Guest(error)) => Ok(Err(error)),
    }
}

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: FilesystemError) -> wasmtime::Result<types::ErrorCode> {
        observe_function_call(&*self.0, "filesystem::types", "convert-error-code");
        types::Host::convert_error_code(&mut WasiFilesystemView::filesystem(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostDescriptor for DurableP3View<'_, Ctx> {
    fn drop(&mut self, fd: Resource<Descriptor>) -> wasmtime::Result<()> {
        observe_function_call(&*self.0, "filesystem::types::descriptor", "drop");
        delete_agent_descriptor(self.0.durable_ctx_mut(), fd)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> preopens::Host for DurableP3View<'_, Ctx> {
    fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        observe_function_call(&*self.0, "filesystem::preopens", "get-directories");
        let ctx = self.0.durable_ctx_mut();
        if ctx.filesystem_capability() == FilesystemCapability::Incapable {
            return Ok(Vec::new());
        }
        let preopen = ctx.filesystem_preopen();
        Ok(vec![
            (
                crate::wasi_filesystem::push_agent_descriptor(ctx, preopen.clone())?,
                "/".to_string(),
            ),
            (
                crate::wasi_filesystem::push_agent_descriptor(ctx, preopen)?,
                ".".to_string(),
            ),
        ])
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostDescriptorWithStore<U> for DurableP3<Ctx> {
    async fn read_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, "")?;
        if let Err(error) = authorize_paths(accessor, &[(FilesystemVerb::Read, path)]).await {
            let code = error
                .downcast_ref()
                .cloned()
                .unwrap_or(types::ErrorCode::NotPermitted);
            return accessor.with(|mut store| {
                Ok((
                    StreamReader::new(&mut store, Vec::<u8>::new())?,
                    FutureReader::new(&mut store, async move {
                        Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(code))
                    })?,
                ))
            });
        }
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "read-via-stream",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor =
            accessor.with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        if descriptor.with_node(|node| !matches!(node, OpenNode::File(_))) {
            return Err(types::ErrorCode::BadDescriptor.into());
        }

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let producer =
            FilesystemReadProducer::new(generation_handle, descriptor, offset, result_tx).await;
        accessor.with(|mut store| {
            let mut stream = StreamReader::new(&mut store, producer)?;
            let future = match FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))
            {
                Ok(future) => future,
                Err(error) => {
                    let _ = stream.close(&mut store);
                    return Err(error);
                }
            };
            Ok((stream, future))
        })
    }

    async fn write_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
        offset: types::Filesize,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let authorization_permit =
            match admit_stream_write::<P3FilesystemTypesDescriptorWriteViaStream, Ctx, U>(
                accessor, &fd,
            )
            .await?
            {
                Ok(permit) => permit,
                Err(error) => {
                    let mut data = data;
                    accessor.with(|mut store| data.close(&mut store))?;
                    return accessor.with(|mut store| {
                        FutureReader::new(&mut store, async move {
                            Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(error))
                        })
                    });
                }
            };
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "write-via-stream",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor =
            accessor.with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        if !agent_descriptor_flags(&generation_handle, &descriptor)
            .map_err(|error| match error.downcast() {
                Ok(error) => wasmtime::Error::msg(format!("{error:?}")),
                Err(error) => error,
            })?
            .contains(types::DescriptorFlags::WRITE)
        {
            let mut data = data;
            accessor.with(|mut store| data.close(&mut store))?;
            return accessor.with(|mut store| {
                FutureReader::new(&mut store, async move {
                    Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(
                        types::ErrorCode::NotPermitted,
                    ))
                })
            });
        }
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        accessor
            .with(|mut store| data.pipe(&mut store, FilesystemWriteConsumer::new(chunks_tx)))?;

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let future = accessor.with(|mut store| {
            let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
                .tail_work_tracker()
                .activity();
            store.spawn(FilesystemWriteTask::<Ctx>::new(
                chunks_rx,
                result_tx,
                generation_handle,
                descriptor,
                WritePlacement::At(offset),
                activity,
                authorization_permit,
            ));

            FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))
        })?;
        Ok(future)
    }

    async fn append_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let authorization_permit =
            match admit_stream_write::<P3FilesystemTypesDescriptorAppendViaStream, Ctx, U>(
                accessor, &fd,
            )
            .await?
            {
                Ok(permit) => permit,
                Err(error) => {
                    let mut data = data;
                    accessor.with(|mut store| data.close(&mut store))?;
                    return accessor.with(|mut store| {
                        FutureReader::new(&mut store, async move {
                            Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(error))
                        })
                    });
                }
            };
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "append-via-stream",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor =
            accessor.with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        if !agent_descriptor_flags(&generation_handle, &descriptor)
            .map_err(|error| match error.downcast() {
                Ok(error) => wasmtime::Error::msg(format!("{error:?}")),
                Err(error) => error,
            })?
            .contains(types::DescriptorFlags::WRITE)
        {
            let mut data = data;
            accessor.with(|mut store| data.close(&mut store))?;
            return accessor.with(|mut store| {
                FutureReader::new(&mut store, async move {
                    Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(
                        types::ErrorCode::NotPermitted,
                    ))
                })
            });
        }
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        accessor
            .with(|mut store| data.pipe(&mut store, FilesystemWriteConsumer::new(chunks_tx)))?;

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let future = accessor.with(|mut store| {
            let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
                .tail_work_tracker()
                .activity();
            store.spawn(FilesystemWriteTask::<Ctx>::new(
                chunks_rx,
                result_tx,
                generation_handle,
                descriptor,
                WritePlacement::Append,
                activity,
                authorization_permit,
            ));

            FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))
        })?;
        Ok(future)
    }

    async fn advise(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        _offset: types::Filesize,
        _length: types::Filesize,
        _advice: types::Advice,
    ) -> FilesystemResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "advise",
            )
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        if descriptor.with_node(|node| matches!(node, OpenNode::Directory(_))) {
            return Err(types::ErrorCode::BadDescriptor.into());
        }
        Ok(())
    }

    async fn sync_data(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<()> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, "")?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Write, path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "sync-data",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let operation = descriptor.with_node(|node| route_flush(&generation_handle, node, true))?;
        operation.await
    }

    async fn get_flags(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorFlags> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "get-flags",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))
            .map_err(FilesystemError::trap)?;
        agent_descriptor_flags(&generation_handle, &descriptor)
    }

    async fn get_type(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorType> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "get-type",
            )
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        Ok(p3_agent_descriptor_type(
            descriptor.with_node(OpenNode::kind),
        ))
    }

    async fn set_size(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        size: types::Filesize,
    ) -> FilesystemResult<()> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, "")?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Write, path)]).await?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-size",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        if descriptor.with_node(|node| matches!(node, OpenNode::Directory(_))) {
            return Err(types::ErrorCode::BadDescriptor.into());
        }
        let operation =
            descriptor.with_node(|node| route_set_size(&generation_handle, node, size))?;
        operation.await
    }

    async fn set_times(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, "")?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Write, path)]).await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-times",
            )
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let operation = descriptor.with_node(|node| {
            route_set_times(
                &generation_handle,
                AgentTarget::Open(node),
                data_access_timestamp,
                data_modification_timestamp,
            )
        })?;
        operation.await
    }

    async fn read_directory(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> wasmtime::Result<(
        StreamReader<types::DirectoryEntry>,
        FutureReader<Result<(), types::ErrorCode>>,
    )> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, "")?;
        let denied = authorize_paths(accessor, &[(FilesystemVerb::List, path)])
            .await
            .is_err();
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "read-directory",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor =
            accessor.with(|mut store| agent_descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        let call = if denied {
            Err(types::ErrorCode::NotPermitted.into())
        } else {
            descriptor.with_node(|node| match node {
                OpenNode::Directory(directory) => {
                    agent_filesystem::list_directory(&generation_handle, directory)
                        .map_err(|error| p3_agent_error(AgentFilesystemError::Access(error)))
                }
                OpenNode::File(_) => Err(types::ErrorCode::NotDirectory.into()),
            })
        };
        let (mut entries, result) = match call {
            Ok(call) => match call.await {
                Ok(entries) => match entries
                    .into_iter()
                    .map(|entry| {
                        Ok(types::DirectoryEntry {
                            type_: p3_agent_descriptor_type(entry.kind),
                            name: entry
                                .name
                                .into_string()
                                .map_err(|_| types::ErrorCode::IllegalByteSequence)?,
                        })
                    })
                    .collect::<Result<Vec<_>, types::ErrorCode>>()
                {
                    Ok(entries) => (entries, Ok(())),
                    Err(error) => (Vec::new(), Err(error)),
                },
                Err(error) => (Vec::new(), Err(p3_agent_error(error).downcast()?)),
            },
            Err(error) => (Vec::new(), Err(error.downcast()?)),
        };
        entries.sort_by_key(|entry| entry.name.clone());

        accessor.with(|mut store| {
            let stream = StreamReader::new(&mut store, entries)?;
            let future = FutureReader::new(&mut store, async move {
                Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(result)
            })?;
            Ok::<_, wasmtime::Error>((stream, future))
        })
    }

    async fn sync(store: &Accessor<U, Self>, fd: Resource<Descriptor>) -> FilesystemResult<()> {
        let path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, "")?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Write, path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "sync",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let operation =
            descriptor.with_node(|node| route_flush(&generation_handle, node, false))?;
        operation.await
    }

    async fn create_directory_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Write, guest_path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "create-directory-at",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let target = agent_path_target(&descriptor, path)?;
        route_create_directory(&generation_handle, target).await
    }

    async fn stat(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorStat> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, "")?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Stat, guest_path)]).await?;
        let path =
            descriptor_path_from_accessor::<Ctx, U>(store, &fd).map_err(FilesystemError::trap)?;
        let fd_rep = fd.rep();
        let live_stat = Arc::new(Mutex::new(None));
        let live_stat_for_call = Arc::clone(&live_stat);

        let response = run_read_access::<_, _, Ctx, P3FilesystemTypesDescriptorStat, _>(
            store,
            HostRequestFileSystemPath {
                path: path.to_string_lossy().to_string(),
            },
            DurableFunctionType::ReadLocal,
            || async {
                let stat = run_local_stat::<Ctx, U>(store, Resource::new_borrow(fd_rep)).await;
                *live_stat_for_call.lock().unwrap() = Some(stat.clone());
                Ok(HostResponseP3FileSystemStat {
                    result: serialize_stat_result(&stat),
                })
            },
        )
        .await
        .map_err(FilesystemError::trap)?;
        let live_stat = live_stat.lock().unwrap().take();
        let stat = match live_stat {
            Some(stat) => stat,
            None => run_local_stat::<Ctx, U>(store, Resource::new_borrow(fd_rep)).await,
        };

        apply_stat_response(stat, response).await
    }

    async fn stat_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::DescriptorStat> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Stat, guest_path)]).await?;
        let full_path = descriptor_path_at_from_accessor::<Ctx, U>(store, &fd, &path)
            .map_err(FilesystemError::trap)?;
        let fd_rep = fd.rep();
        let live_stat = Arc::new(Mutex::new(None));
        let live_stat_for_call = Arc::clone(&live_stat);
        let live_path = path.clone();
        let response = run_read_access::<_, _, Ctx, P3FilesystemTypesDescriptorStatAt, _>(
            store,
            HostRequestFileSystemPath {
                path: full_path.to_string_lossy().to_string(),
            },
            DurableFunctionType::ReadLocal,
            || async {
                let stat = run_local_stat_at::<Ctx, U>(
                    store,
                    Resource::new_borrow(fd_rep),
                    path_flags,
                    live_path,
                )
                .await;
                *live_stat_for_call.lock().unwrap() = Some(stat.clone());
                Ok(HostResponseP3FileSystemStat {
                    result: serialize_stat_result(&stat),
                })
            },
        )
        .await
        .map_err(FilesystemError::trap)?;
        let live_stat = live_stat.lock().unwrap().take();
        let stat = match live_stat {
            Some(stat) => stat,
            None => {
                run_local_stat_at::<Ctx, U>(store, Resource::new_borrow(fd_rep), path_flags, path)
                    .await
            }
        };

        apply_stat_response(stat, response).await
    }

    async fn set_times_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Write, guest_path)]).await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-times-at",
            )
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let target = agent_path_target(&descriptor, path)?;
        let follow = if path_flags.contains(types::PathFlags::SYMLINK_FOLLOW) {
            agent_filesystem::Follow::Yes
        } else {
            agent_filesystem::Follow::No
        };
        let operation = route_set_times(
            &generation_handle,
            AgentTarget::Path(&target, follow),
            data_access_timestamp,
            data_modification_timestamp,
        )?;
        operation.await
    }

    async fn link_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path_flags: types::PathFlags,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let source_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &old_path)?;
        let destination_path =
            descriptor_guest_path_from_accessor::<Ctx, U>(store, &new_fd, &new_path)?;
        let _authorization_permit = authorize_paths(
            store,
            &[
                (FilesystemVerb::Read, source_path),
                (FilesystemVerb::Write, destination_path),
            ],
        )
        .await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "link-at",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let source_descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let destination_descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &new_fd))
            .map_err(FilesystemError::trap)?;
        let source = agent_path_target(&source_descriptor, old_path)?;
        let destination = agent_path_target(&destination_descriptor, new_path)?;
        route_hard_link(&generation_handle, source, old_path_flags, destination).await
    }

    async fn open_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        open_flags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FilesystemResult<Resource<Descriptor>> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &path)?;
        let mut permissions = Vec::new();
        if flags.contains(types::DescriptorFlags::READ) {
            permissions.push((FilesystemVerb::Read, guest_path.clone()));
        }
        if flags.contains(types::DescriptorFlags::WRITE)
            || open_flags.intersects(types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE)
        {
            permissions.push((FilesystemVerb::Write, guest_path.clone()));
        }
        if open_flags.contains(types::OpenFlags::DIRECTORY) {
            permissions.push((FilesystemVerb::List, guest_path));
        }
        let _authorization_permit = authorize_paths(accessor, &permissions).await?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "open-at",
            )
        });
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let descriptor_path = descriptor.path().join(&path);
        let target = agent_path_target(&descriptor, path)?;
        let opened = route_open(&generation_handle, target, path_flags, open_flags, flags).await?;
        push_agent_descriptor_from_accessor(
            accessor,
            AgentDescriptor::new(opened.node, descriptor_path),
        )
    }

    async fn readlink_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<String> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Stat, guest_path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "readlink-at",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let target = agent_path_target(&descriptor, path)?;
        route_symlink_target(&generation_handle, target).await
    }

    async fn remove_directory_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(store, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(store, &[(FilesystemVerb::Delete, guest_path)]).await?;
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "remove-directory-at",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = store
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let target = agent_path_target(&descriptor, path)?;
        route_remove_directory(&generation_handle, target).await
    }

    async fn rename_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let source_path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &old_path)?;
        let destination_path =
            descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &new_fd, &new_path)?;
        let _authorization_permit = authorize_paths(
            accessor,
            &[
                (FilesystemVerb::Delete, source_path),
                (FilesystemVerb::Write, destination_path),
            ],
        )
        .await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let source_descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let destination_descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &new_fd))
            .map_err(FilesystemError::trap)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "rename-at",
            )
        });
        let source = agent_path_target(&source_descriptor, old_path)?;
        let destination = agent_path_target(&destination_descriptor, new_path)?;
        route_rename(&generation_handle, source, destination).await
    }

    async fn symlink_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> FilesystemResult<()> {
        let destination_path =
            descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &new_path)?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Write, destination_path)]).await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "symlink-at",
            )
        });
        let destination = agent_path_target(&descriptor, new_path)?;
        route_create_symlink(&generation_handle, destination, old_path).await
    }

    async fn unlink_file_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let guest_path = descriptor_guest_path_from_accessor::<Ctx, U>(accessor, &fd, &path)?;
        let _authorization_permit =
            authorize_paths(accessor, &[(FilesystemVerb::Delete, guest_path)]).await?;
        let generation_handle = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let descriptor = accessor
            .with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "unlink-file-at",
            )
        });
        let target = agent_path_target(&descriptor, path)?;
        route_unlink(&generation_handle, target, ObjectKind::File).await
    }

    async fn is_same_object(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "is-same-object",
            )
        });
        let generation_handle = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_generation_handle()
        });
        let left =
            store.with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &fd))?;
        let right =
            store.with(|mut access| agent_descriptor_from_access::<Ctx, U>(&mut access, &other))?;
        let call = AgentDescriptor::with_nodes(&left, &right, |left, right| {
            agent_filesystem::is_same_object(&generation_handle, left, right)
        })?;
        call.await
            .map_err(|error| wasmtime::Error::msg(error.to_string()))
    }

    async fn metadata_hash(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::MetadataHashValue> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "metadata-hash",
            )
        });
        // Computed from the durable stat result so the hash only depends on
        // replay-stable inputs (durable file times + size), matching WASI P2.
        let stat = Self::stat(store, fd).await?;
        metadata_hash_from_stat(&stat)
    }

    async fn metadata_hash_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::MetadataHashValue> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "metadata-hash-at",
            )
        });
        // Computed from the durable stat result so the hash only depends on
        // replay-stable inputs (durable file times + size), matching WASI P2.
        let stat = Self::stat_at(store, fd, path_flags, path).await?;
        metadata_hash_from_stat(&stat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::agent_filesystem::{AccessMode, FileDisposition, Follow, OpenOptions};
    use crate::wasi_filesystem::p2::types::{
        p2_agent_error, p2_agent_open_error, p2_agent_open_request, p2_agent_stat,
        p2_agent_write_result, p2_link_access_error, p2_time_changes, p2_visible_descriptor_flags,
    };
    use crate::wasi_filesystem::{
        AgentOpenDecision, AgentOpenPolicyError, decide_agent_existing_open, decide_agent_open,
        flush_level, replay_time_changes, resize_attribute_changes,
    };
    use fs_set_times::{SystemTimeSpec, set_symlink_times, set_times};
    use golem_common::model::oplog::types::SerializableDateTime;
    use std::future::poll_fn;
    use std::time::{Duration, SystemTime};
    use test_r::test;

    struct ReadProducerFixture {
        _root: tempfile::TempDir,
        filesystem: agent_filesystem::ResidentFilesystem,
        window: crate::services::resource_usage_metering::ResourceUsageMeteringWindow,
        generation_handle: FilesystemGenerationHandle,
        descriptor: AgentDescriptor,
    }

    impl ReadProducerFixture {
        async fn new(contents: Bytes) -> Self {
            use crate::sandbox_filesystem::SandboxFilesystemProvisioning;
            use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
            use crate::services::agent_filesystem::{
                PreparedInitialFiles, ResolvedStorageLimits, bind_resource_usage_metering,
                create_fresh, finish_reconstruction, finish_replay, materialize_initial_files,
                open_resource_usage_window, reconstruction_generation_handle,
                resident_generation_handle,
            };
            use crate::services::golem_config::FilesystemStorageConfig;
            use crate::services::linear_memory::LinearMemoryTracker;
            use crate::services::resource_limits::AtomicResourceEntry;
            use crate::services::resource_usage_metering::ResourceUsageAccount;
            use golem_common::model::account::AccountId;
            use golem_common::model::agent::AgentMode;
            use golem_common::model::component::ComponentId;
            use golem_common::model::environment::EnvironmentId;
            use golem_common::model::{AgentId, OwnedAgentId};
            use std::sync::Arc;
            use std::time::Instant;
            use uuid::Uuid;

            let root = tempfile::tempdir().unwrap();
            let profile = FilesystemStorageConfig {
                deterministic_root_dir: Some(root.path().to_path_buf()),
                ..FilesystemStorageConfig::default()
            };
            let agent = OwnedAgentId::new(
                EnvironmentId::new(),
                &AgentId::from_agent_name_string(ComponentId::new(), "p3-pull-read").unwrap(),
            );
            let provisioning = SandboxFilesystemProvisioning::new(
                profile.deterministic_root_dir.clone(),
                profile.managed_xfs_root_dir.clone(),
                profile.cleanup_retry.clone(),
            )
            .unwrap();
            let created = create_fresh(
                provisioning,
                agent.clone(),
                ResolvedStorageLimits::Unlimited,
            )
            .await
            .unwrap();
            let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
            let memory = LinearMemoryTracker::new(
                0,
                0,
                AgentMode::Durable,
                false,
                entry.clone(),
                Arc::new(std::sync::Mutex::new(MemoryGrant::inert(0))),
                Instant::now(),
            );
            let reconstructing = bind_resource_usage_metering(
                created,
                ResourceUsageAccount::new(AgentMode::Durable, memory, entry.clone()),
            )
            .unwrap();
            let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
            let account_id = AccountId(Uuid::new_v4());
            scheduler.register_account(account_id, entry).await;
            let permit = scheduler.acquire(account_id, agent.agent_id).await;
            let window = open_resource_usage_window(&reconstructing, permit)
                .await
                .unwrap();
            let reconstructing =
                materialize_initial_files(reconstructing, PreparedInitialFiles::empty())
                    .await
                    .unwrap();
            let generation_handle = reconstruction_generation_handle(&reconstructing).unwrap();
            let opened = route_open(
                &generation_handle,
                PathTarget::at_root(&generation_handle, "large-file").unwrap(),
                types::PathFlags::SYMLINK_FOLLOW,
                types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE,
                types::DescriptorFlags::READ | types::DescriptorFlags::WRITE,
            )
            .await
            .unwrap();
            let OpenNode::File(file) = &opened.node else {
                panic!("sandbox file open returned a directory")
            };
            let (written, _) = route_stream_chunk(
                &generation_handle,
                file,
                WritePlacement::At(0),
                contents.clone(),
            )
            .unwrap()
            .await
            .unwrap()
            .unwrap();
            assert_eq!(written, contents.len());
            let descriptor = AgentDescriptor::new(opened.node, PathBuf::from("large-file"));
            let reconstructing = finish_replay(reconstructing).await.unwrap();
            let filesystem = finish_reconstruction(reconstructing).await.unwrap();
            let generation_handle = resident_generation_handle(&filesystem);

            Self {
                _root: root,
                filesystem,
                window,
                generation_handle,
                descriptor,
            }
        }

        async fn close(self) {
            use crate::services::agent_filesystem::{delete, seal};
            use crate::services::resource_usage_metering::close_window;
            use std::time::Instant;

            drop(self.descriptor);
            close_window(self.window, Instant::now() + Duration::from_secs(1))
                .await
                .unwrap();
            delete(seal(self.filesystem)).await.unwrap();
        }
    }

    #[test]
    async fn p3_read_stream_prefetch_is_bounded_and_exhausted_before_the_next_read() {
        let contents = Bytes::from(
            (0..FILESYSTEM_READ_CHUNK_SIZE * 32)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>(),
        );
        let fixture = ReadProducerFixture::new(contents.clone()).await;
        let (result_tx, mut result_rx) = tokio::sync::oneshot::channel();
        let mut producer = FilesystemReadProducer::new(
            fixture.generation_handle.clone(),
            fixture.descriptor.clone(),
            0,
            result_tx,
        )
        .await;

        assert_eq!(producer.read_call_count(), 1);
        assert!(producer.pending.is_none());
        assert_eq!(producer.buffered.len(), FILESYSTEM_READ_CHUNK_SIZE);
        assert_eq!(producer.offset, FILESYSTEM_READ_CHUNK_SIZE as u64);
        assert_eq!(producer.take_buffered(4096), contents.slice(..4096));
        assert_eq!(producer.read_call_count(), 1);
        assert_eq!(
            producer.take_buffered(usize::MAX),
            contents.slice(4096..FILESYSTEM_READ_CHUNK_SIZE)
        );
        assert_eq!(producer.read_call_count(), 1);

        let chunk = poll_fn(|cx| producer.poll_read_chunk(cx, FILESYSTEM_READ_CHUNK_SIZE))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            chunk,
            contents.slice(FILESYSTEM_READ_CHUNK_SIZE..FILESYSTEM_READ_CHUNK_SIZE * 2)
        );
        assert_eq!(producer.read_call_count(), 2);
        assert_eq!(producer.offset, (FILESYSTEM_READ_CHUNK_SIZE * 2) as u64);
        assert!(producer.pending.is_none());
        assert!(matches!(
            result_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));

        drop(producer);
        result_rx.await.unwrap().unwrap().unwrap();
        fixture.close().await;
    }

    #[test]
    async fn dropping_p3_read_stream_before_eof_stops_further_reads() {
        let contents = Bytes::from(vec![0x5a; FILESYSTEM_READ_CHUNK_SIZE * 8]);
        let fixture = ReadProducerFixture::new(contents.clone()).await;
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let producer = FilesystemReadProducer::new(
            fixture.generation_handle.clone(),
            fixture.descriptor.clone(),
            17,
            result_tx,
        )
        .await;

        assert_eq!(producer.read_call_count(), 1);
        assert_eq!(
            producer.buffered,
            contents.slice(17..17 + FILESYSTEM_READ_CHUNK_SIZE)
        );
        assert_eq!(producer.offset, 17 + FILESYSTEM_READ_CHUNK_SIZE as u64);
        let read_calls = producer.read_calls.clone();

        drop(producer);
        result_rx.await.unwrap().unwrap().unwrap();
        assert_eq!(read_calls.load(std::sync::atomic::Ordering::Relaxed), 1);
        fixture.close().await;
    }

    #[test]
    async fn p3_read_stream_preserves_offset_and_detects_eof_after_prefetch() {
        let contents = Bytes::from(
            (0..FILESYSTEM_READ_CHUNK_SIZE + 123)
                .map(|index| (index % 251) as u8)
                .collect::<Vec<_>>(),
        );
        let fixture = ReadProducerFixture::new(contents.clone()).await;
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let mut producer = FilesystemReadProducer::new(
            fixture.generation_handle.clone(),
            fixture.descriptor.clone(),
            17,
            result_tx,
        )
        .await;

        assert_eq!(
            producer.take_buffered(usize::MAX),
            contents.slice(17..17 + FILESYSTEM_READ_CHUNK_SIZE)
        );
        assert_eq!(producer.read_call_count(), 1);
        let chunk = poll_fn(|cx| producer.poll_read_chunk(cx, FILESYSTEM_READ_CHUNK_SIZE))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(chunk, contents.slice(17 + FILESYSTEM_READ_CHUNK_SIZE..));
        assert_eq!(producer.offset, contents.len() as u64);
        assert_eq!(producer.read_call_count(), 2);
        assert!(
            poll_fn(|cx| producer.poll_read_chunk(cx, FILESYSTEM_READ_CHUNK_SIZE))
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(producer.offset, contents.len() as u64);
        assert_eq!(producer.read_call_count(), 3);

        drop(producer);
        result_rx.await.unwrap().unwrap().unwrap();
        fixture.close().await;
    }

    fn p3_datetime_parts(time: SystemTime) -> (i64, u32) {
        let datetime = p3_agent_datetime(time).unwrap();
        (datetime.seconds, datetime.nanoseconds)
    }

    fn p3_timestamp(seconds: i64, nanoseconds: u32) -> types::NewTimestamp {
        types::NewTimestamp::Timestamp(wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
            seconds,
            nanoseconds,
        })
    }

    fn p3_decoded_time(seconds: i64, nanoseconds: u32) -> SystemTime {
        p3_native_time(p3_timestamp(seconds, nanoseconds))
            .unwrap()
            .unwrap()
    }

    fn open_request() -> AgentOpenRequest {
        AgentOpenRequest {
            create: false,
            directory: false,
            exclusive: false,
            truncate: false,
            follow: false,
            read: false,
            write: false,
            unsupported_sync: false,
        }
    }

    fn decided_open(request: AgentOpenRequest) -> OpenOptions {
        match decide_agent_open(request).unwrap() {
            AgentOpenDecision::Open(options) => options,
            AgentOpenDecision::ObserveAttributes { .. } => {
                panic!("open policy unexpectedly requested attributes")
            }
        }
    }

    #[test]
    fn agent_open_policy_rejects_unsupported_sync() {
        assert_eq!(
            decide_agent_open(AgentOpenRequest {
                unsupported_sync: true,
                ..open_request()
            }),
            Err(AgentOpenPolicyError::Unsupported)
        );
    }

    #[test]
    fn agent_open_policy_rejects_each_directory_mutation_flag() {
        for request in [
            AgentOpenRequest {
                directory: true,
                create: true,
                ..open_request()
            },
            AgentOpenRequest {
                directory: true,
                exclusive: true,
                ..open_request()
            },
            AgentOpenRequest {
                directory: true,
                truncate: true,
                ..open_request()
            },
        ] {
            assert_eq!(
                decide_agent_open(request),
                Err(AgentOpenPolicyError::Invalid)
            );
        }
    }

    #[test]
    fn agent_open_policy_selects_access_modes() {
        for (read, write, expected) in [
            (false, false, AccessMode::Read),
            (true, false, AccessMode::Read),
            (false, true, AccessMode::Write),
            (true, true, AccessMode::ReadWrite),
        ] {
            assert_eq!(
                decided_open(AgentOpenRequest {
                    directory: true,
                    read,
                    write,
                    ..open_request()
                }),
                OpenOptions::Existing {
                    expected: ObjectKind::Directory,
                    access: expected,
                    follow: Follow::No,
                }
            );
        }
    }

    #[test]
    fn agent_open_policy_selects_follow_mode() {
        assert_eq!(
            decided_open(AgentOpenRequest {
                directory: true,
                follow: false,
                ..open_request()
            }),
            OpenOptions::Existing {
                expected: ObjectKind::Directory,
                access: AccessMode::Read,
                follow: Follow::No,
            }
        );
        assert_eq!(
            decided_open(AgentOpenRequest {
                directory: true,
                follow: true,
                ..open_request()
            }),
            OpenOptions::Existing {
                expected: ObjectKind::Directory,
                access: AccessMode::Read,
                follow: Follow::Yes,
            }
        );
    }

    #[test]
    fn agent_open_policy_selects_each_file_disposition() {
        for (request, disposition) in [
            (
                AgentOpenRequest {
                    create: true,
                    ..open_request()
                },
                FileDisposition::CreateIfMissing,
            ),
            (
                AgentOpenRequest {
                    create: true,
                    exclusive: true,
                    ..open_request()
                },
                FileDisposition::CreateExclusive,
            ),
            (
                AgentOpenRequest {
                    truncate: true,
                    ..open_request()
                },
                FileDisposition::TruncateExisting,
            ),
            (
                AgentOpenRequest {
                    create: true,
                    truncate: true,
                    ..open_request()
                },
                FileDisposition::CreateOrTruncate,
            ),
        ] {
            assert_eq!(
                decided_open(request),
                OpenOptions::File {
                    access: AccessMode::Read,
                    disposition,
                    follow: Follow::No,
                }
            );
        }
    }

    #[test]
    fn agent_open_policy_uses_observed_existing_kind_and_rejects_symlinks() {
        assert_eq!(
            decide_agent_open(AgentOpenRequest {
                read: true,
                follow: true,
                ..open_request()
            }),
            Ok(AgentOpenDecision::ObserveAttributes {
                access: AccessMode::Read,
                follow: Follow::Yes,
            })
        );
        assert_eq!(
            decide_agent_existing_open(AccessMode::Read, Follow::Yes, ObjectKind::File),
            Ok(OpenOptions::Existing {
                expected: ObjectKind::File,
                access: AccessMode::Read,
                follow: Follow::Yes,
            })
        );
        assert_eq!(
            decide_agent_existing_open(AccessMode::Read, Follow::No, ObjectKind::Directory),
            Ok(OpenOptions::Existing {
                expected: ObjectKind::Directory,
                access: AccessMode::Read,
                follow: Follow::No,
            })
        );
        assert_eq!(
            decide_agent_existing_open(AccessMode::Read, Follow::No, ObjectKind::Symlink),
            Err(AgentOpenPolicyError::SymlinkLoop)
        );
    }

    #[test]
    fn p3_agent_datetime_normalizes_exact_epoch() {
        assert_eq!(p3_datetime_parts(SystemTime::UNIX_EPOCH), (0, 0));
    }

    #[test]
    fn p3_agent_datetime_normalizes_positive_fractional_time() {
        assert_eq!(
            p3_datetime_parts(SystemTime::UNIX_EPOCH + Duration::from_millis(500)),
            (0, 500_000_000)
        );
    }

    #[test]
    fn p3_agent_datetime_normalizes_negative_whole_seconds() {
        assert_eq!(
            p3_datetime_parts(SystemTime::UNIX_EPOCH - Duration::from_secs(2)),
            (-2, 0)
        );
    }

    #[test]
    fn p3_agent_datetime_normalizes_negative_fractional_time() {
        assert_eq!(
            p3_datetime_parts(SystemTime::UNIX_EPOCH - Duration::from_millis(500)),
            (-1, 500_000_000)
        );
    }

    #[test]
    fn p3_native_time_decodes_normalized_signed_fractions() {
        assert_eq!(p3_decoded_time(0, 0), SystemTime::UNIX_EPOCH);
        assert_eq!(
            p3_decoded_time(0, 500_000_000),
            SystemTime::UNIX_EPOCH + Duration::from_millis(500)
        );
        assert_eq!(
            p3_decoded_time(-2, 0),
            SystemTime::UNIX_EPOCH - Duration::from_secs(2)
        );
        assert_eq!(
            p3_decoded_time(-1, 500_000_000),
            SystemTime::UNIX_EPOCH - Duration::from_millis(500)
        );
        assert_eq!(
            p3_decoded_time(-2, 500_000_000),
            SystemTime::UNIX_EPOCH - Duration::from_millis(1_500)
        );
    }

    #[test]
    fn p3_timestamp_encoding_and_decoding_round_trip() {
        for time in [
            SystemTime::UNIX_EPOCH,
            SystemTime::UNIX_EPOCH + Duration::from_millis(500),
            SystemTime::UNIX_EPOCH + Duration::new(12, 34),
            SystemTime::UNIX_EPOCH - Duration::from_secs(2),
            SystemTime::UNIX_EPOCH - Duration::from_millis(500),
            SystemTime::UNIX_EPOCH - Duration::from_millis(1_500),
        ] {
            let (seconds, nanoseconds) = p3_datetime_parts(time);
            assert_eq!(p3_decoded_time(seconds, nanoseconds), time);
        }
    }

    #[test]
    fn p3_native_time_handles_signed_boundaries_and_rejects_invalid_fractions() {
        for (seconds, nanoseconds) in [
            (i64::MIN, 0),
            (i64::MIN, 500_000_000),
            (i64::MAX, 0),
            (i64::MAX, 999_999_999),
        ] {
            match p3_native_time(p3_timestamp(seconds, nanoseconds)) {
                Ok(Some(time)) => assert_eq!(p3_datetime_parts(time), (seconds, nanoseconds)),
                Err(error) => assert!(matches!(
                    error.downcast().unwrap(),
                    types::ErrorCode::Overflow
                )),
                Ok(None) => panic!("explicit timestamp decoded as no change"),
            }
        }

        let error = p3_native_time(p3_timestamp(0, 1_000_000_000)).unwrap_err();
        assert!(matches!(
            error.downcast().unwrap(),
            types::ErrorCode::Overflow
        ));
    }

    #[test]
    fn p3_time_changes_decode_negative_accessed_and_modified_fractions_identically() {
        assert_eq!(
            p3_time_changes(p3_timestamp(-1, 500_000_000), p3_timestamp(-2, 500_000_000),).unwrap(),
            TimeChanges {
                accessed: TimeChange::Set(SystemTime::UNIX_EPOCH - Duration::from_millis(500)),
                modified: TimeChange::Set(SystemTime::UNIX_EPOCH - Duration::from_millis(1_500),),
            }
        );
    }

    #[test]
    fn p2_agent_stat_keeps_pre_epoch_timestamps_as_overflow() {
        let error = p2_agent_stat(AgentAttributes {
            kind: ObjectKind::File,
            link_count: 1,
            size: 0,
            accessed: Some(SystemTime::UNIX_EPOCH - Duration::from_millis(1)),
            modified: None,
        })
        .unwrap_err();

        assert_eq!(
            error.downcast().unwrap(),
            wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::Overflow
        );
    }

    #[test]
    fn p2_p3_agent_open_request_translation_is_identical() {
        use wasmtime_wasi::p2::bindings::filesystem::types::{
            DescriptorFlags as P2DescriptorFlags, OpenFlags as P2OpenFlags,
            PathFlags as P2PathFlags,
        };

        let p2 = p2_agent_open_request(
            P2PathFlags::SYMLINK_FOLLOW,
            P2OpenFlags::CREATE | P2OpenFlags::TRUNCATE,
            P2DescriptorFlags::READ
                | P2DescriptorFlags::WRITE
                | P2DescriptorFlags::DATA_INTEGRITY_SYNC,
        );
        let p3 = p3_agent_open_request(
            types::PathFlags::SYMLINK_FOLLOW,
            types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE,
            types::DescriptorFlags::READ
                | types::DescriptorFlags::WRITE
                | types::DescriptorFlags::DATA_INTEGRITY_SYNC,
        );

        assert_eq!(p2, p3);
        assert_eq!(decide_agent_open(p2), decide_agent_open(p3));
    }

    #[test]
    fn p2_p3_agent_stat_result_mapping_is_identical() {
        let accessed = SystemTime::UNIX_EPOCH + Duration::new(12, 34);
        let modified = SystemTime::UNIX_EPOCH + Duration::new(56, 78);
        let attributes = AgentAttributes {
            kind: ObjectKind::File,
            link_count: 3,
            size: 7,
            accessed: Some(accessed),
            modified: Some(modified),
        };

        let p2 = p2_agent_stat(attributes.clone()).unwrap();
        let p3 = p3_agent_stat(attributes).unwrap();

        assert_eq!(
            p2.type_,
            wasmtime_wasi::p2::bindings::filesystem::types::DescriptorType::RegularFile
        );
        assert!(matches!(p3.type_, types::DescriptorType::RegularFile));
        assert_eq!(p2.link_count, p3.link_count);
        assert_eq!(p2.size, p3.size);
        assert_eq!(
            i64::try_from(p2.data_access_timestamp.unwrap().seconds).unwrap(),
            p3.data_access_timestamp.unwrap().seconds
        );
        assert_eq!(
            i64::try_from(p2.data_modification_timestamp.unwrap().seconds).unwrap(),
            p3.data_modification_timestamp.unwrap().seconds
        );
    }

    #[test]
    fn p2_p3_agent_open_error_mapping_is_identical() {
        use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

        assert_eq!(
            p2_agent_open_error(AgentOpenRouteError::Invalid)
                .downcast()
                .unwrap(),
            P2ErrorCode::Invalid
        );
        assert!(matches!(
            p3_agent_open_error(AgentOpenRouteError::Invalid)
                .downcast()
                .unwrap(),
            types::ErrorCode::Invalid
        ));
        assert_eq!(
            p2_agent_open_error(AgentOpenRouteError::Unsupported)
                .downcast()
                .unwrap(),
            P2ErrorCode::Unsupported
        );
        assert!(matches!(
            p3_agent_open_error(AgentOpenRouteError::Unsupported)
                .downcast()
                .unwrap(),
            types::ErrorCode::Unsupported
        ));
        assert_eq!(
            p2_agent_open_error(AgentOpenRouteError::SymlinkLoop)
                .downcast()
                .unwrap(),
            P2ErrorCode::Loop
        );
        assert!(matches!(
            p3_agent_open_error(AgentOpenRouteError::SymlinkLoop)
                .downcast()
                .unwrap(),
            types::ErrorCode::Loop
        ));
    }

    #[test]
    fn p2_p3_attribute_and_flush_translation_is_identical() {
        use wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime as P2Datetime;
        use wasmtime_wasi::p2::bindings::filesystem::types::NewTimestamp as P2NewTimestamp;

        let p2 = p2_time_changes(
            P2NewTimestamp::Now,
            P2NewTimestamp::Timestamp(P2Datetime {
                seconds: 12,
                nanoseconds: 34,
            }),
        )
        .unwrap();
        let p3 = p3_time_changes(
            types::NewTimestamp::Now,
            types::NewTimestamp::Timestamp(
                wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
                    seconds: 12,
                    nanoseconds: 34,
                },
            ),
        )
        .unwrap();

        assert_eq!(p2, p3);
        assert_eq!(
            resize_attribute_changes(99),
            AttributeChanges::File {
                size: 99,
                times: TimeChanges {
                    accessed: TimeChange::Keep,
                    modified: TimeChange::Keep,
                },
            }
        );
        assert_eq!(flush_level(true), agent_filesystem::FlushLevel::Data);
        assert_eq!(
            flush_level(false),
            agent_filesystem::FlushLevel::DataAndMetadata
        );
    }

    #[test]
    fn replay_timestamp_restoration_uses_the_same_time_changes() {
        let accessed = SystemTime::UNIX_EPOCH + Duration::from_secs(50);
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(60);

        assert_eq!(
            replay_time_changes(Some(accessed), Some(modified)),
            TimeChanges {
                accessed: TimeChange::Set(accessed),
                modified: TimeChange::Set(modified),
            }
        );
        assert_eq!(
            replay_time_changes(None, Some(modified)),
            TimeChanges {
                accessed: TimeChange::Keep,
                modified: TimeChange::Set(modified),
            }
        );
    }

    #[test]
    fn p2_p3_read_only_attribute_error_mapping_is_identical() {
        use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

        let p2: P2ErrorCode = p2_agent_error(AgentFilesystemError::Access(
            agent_filesystem::AccessError::NotPermitted,
        ))
        .downcast()
        .unwrap();
        let p3 = p3_agent_error(AgentFilesystemError::Access(
            agent_filesystem::AccessError::NotPermitted,
        ))
        .downcast()
        .unwrap();

        assert_eq!(p2, P2ErrorCode::NotPermitted);
        assert!(matches!(p3, types::ErrorCode::NotPermitted));
    }

    #[test]
    fn p2_p3_cross_project_link_mapping_is_identical() {
        use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

        let p2: P2ErrorCode = p2_link_access_error(agent_filesystem::AccessError::WrongGeneration)
            .downcast()
            .unwrap();
        let p3 = p3_link_access_error(agent_filesystem::AccessError::WrongGeneration)
            .downcast()
            .unwrap();
        assert_eq!(p2, P2ErrorCode::CrossDevice);
        assert!(matches!(p3, types::ErrorCode::CrossDevice));

        #[cfg(target_os = "linux")]
        {
            let storage_error = || {
                FilesystemStorageError::io(
                    "hard link",
                    std::path::Path::new("<namespace-parity-test>"),
                    std::io::Error::from_raw_os_error(libc::EXDEV),
                )
            };
            let p2: P2ErrorCode = p2_agent_error(AgentFilesystemError::Sandbox(storage_error()))
                .downcast()
                .unwrap();
            let p3 = p3_agent_error(AgentFilesystemError::Sandbox(storage_error()))
                .downcast()
                .unwrap();
            assert_eq!(p2, P2ErrorCode::CrossDevice);
            assert!(matches!(p3, types::ErrorCode::CrossDevice));
        }
    }

    #[test]
    async fn staged_p2_p3_attribute_namespace_and_replay_routes_execute_through_agent_filesystem() {
        use crate::sandbox_filesystem::SandboxFilesystemProvisioning;
        use crate::services::active_agents::{ConcurrentAgentsScheduler, MemoryGrant};
        use crate::services::agent_filesystem::{
            PreparedInitialFiles, ResolvedStorageLimits, bind_resource_usage_metering, close,
            create_fresh, delete, finish_reconstruction, finish_replay, materialize_initial_files,
            open, open_resource_usage_window, reconstruction_generation_handle,
            resident_generation_handle, seal,
        };
        use crate::services::golem_config::FilesystemStorageConfig;
        use crate::services::linear_memory::LinearMemoryTracker;
        use crate::services::resource_limits::AtomicResourceEntry;
        use crate::services::resource_usage_metering::{ResourceUsageAccount, close_window};
        use golem_common::model::account::AccountId;
        use golem_common::model::agent::AgentMode;
        use golem_common::model::component::ComponentId;
        use golem_common::model::environment::EnvironmentId;
        use golem_common::model::{AgentId, OwnedAgentId};
        use std::sync::Arc;
        use std::time::Instant;
        use uuid::Uuid;

        let root = tempfile::tempdir().unwrap();
        let profile = FilesystemStorageConfig {
            deterministic_root_dir: Some(root.path().to_path_buf()),
            ..FilesystemStorageConfig::default()
        };
        let agent = OwnedAgentId::new(
            EnvironmentId::new(),
            &AgentId::from_agent_name_string(ComponentId::new(), "staged-attributes").unwrap(),
        );
        let provisioning = SandboxFilesystemProvisioning::new(
            profile.deterministic_root_dir.clone(),
            profile.managed_xfs_root_dir.clone(),
            profile.cleanup_retry.clone(),
        )
        .unwrap();
        let created = create_fresh(
            provisioning,
            agent.clone(),
            ResolvedStorageLimits::Unlimited,
        )
        .await
        .unwrap();
        let entry = Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 1));
        let memory = LinearMemoryTracker::new(
            0,
            0,
            AgentMode::Durable,
            false,
            entry.clone(),
            Arc::new(std::sync::Mutex::new(MemoryGrant::inert(0))),
            Instant::now(),
        );
        let reconstructing = bind_resource_usage_metering(
            created,
            ResourceUsageAccount::new(AgentMode::Durable, memory, entry.clone()),
        )
        .unwrap();
        let scheduler = Arc::new(ConcurrentAgentsScheduler::new());
        let account_id = AccountId(Uuid::new_v4());
        scheduler.register_account(account_id, entry).await;
        let permit = scheduler.acquire(account_id, agent.agent_id).await;
        let window = open_resource_usage_window(&reconstructing, permit)
            .await
            .unwrap();
        let reconstructing =
            materialize_initial_files(reconstructing, PreparedInitialFiles::empty())
                .await
                .unwrap();
        let generation_handle = reconstruction_generation_handle(&reconstructing).unwrap();
        let opened = route_open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "file").unwrap(),
            types::PathFlags::SYMLINK_FOLLOW,
            types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE,
            types::DescriptorFlags::READ | types::DescriptorFlags::WRITE,
        )
        .await
        .unwrap();
        let node = opened.node;
        let OpenNode::File(file) = &node else {
            panic!("sandbox file open returned a directory")
        };

        assert_eq!(
            crate::wasi_filesystem::p2::types::route_write(
                &generation_handle,
                file,
                0,
                Bytes::from_static(b"p2-replay"),
            )
            .unwrap()
            .await
            .unwrap(),
            9
        );
        assert_eq!(
            route_append_via_stream_chunk(&generation_handle, file, Bytes::from_static(b"-p3"))
                .unwrap()
                .await
                .unwrap()
                .unwrap(),
            (3, WritePlacement::Append)
        );

        crate::wasi_filesystem::p2::types::route_set_size(&generation_handle, &node, 12)
            .unwrap()
            .await
            .unwrap();
        route_set_size(&generation_handle, &node, 12)
            .unwrap()
            .await
            .unwrap();
        let p2_time = wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime {
            seconds: 100,
            nanoseconds: 0,
        };
        crate::wasi_filesystem::p2::types::route_set_times(
            &generation_handle,
            AgentTarget::Open(&node),
            wasmtime_wasi::p2::bindings::filesystem::types::NewTimestamp::NoChange,
            wasmtime_wasi::p2::bindings::filesystem::types::NewTimestamp::Timestamp(p2_time),
        )
        .unwrap()
        .await
        .unwrap();
        route_set_times(
            &generation_handle,
            AgentTarget::Open(&node),
            types::NewTimestamp::Timestamp(
                wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
                    seconds: 200,
                    nanoseconds: 0,
                },
            ),
            types::NewTimestamp::NoChange,
        )
        .unwrap()
        .await
        .unwrap();
        crate::wasi_filesystem::p2::types::route_flush(&generation_handle, &node, true)
            .unwrap()
            .await
            .unwrap();
        route_flush(&generation_handle, &node, false)
            .unwrap()
            .await
            .unwrap();
        route_replay_times(
            &generation_handle,
            AgentTarget::Open(&node),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(300)),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(400)),
        )
        .unwrap()
        .await
        .unwrap();

        let root_node = open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, ".").unwrap(),
            OpenOptions::Existing {
                expected: ObjectKind::Directory,
                access: AccessMode::ReadWrite,
                follow: Follow::Yes,
            },
        )
        .unwrap()
        .await
        .unwrap()
        .node;
        let OpenNode::Directory(root_directory) = &root_node else {
            panic!("sandbox root open returned a file")
        };

        crate::wasi_filesystem::p2::types::route_create_directory(
            &generation_handle,
            PathTarget::at(root_directory, "p2-directory"),
        )
        .await
        .unwrap();
        route_create_directory(
            &generation_handle,
            PathTarget::at(root_directory, "p3-directory"),
        )
        .await
        .unwrap();
        let p2_existing_directory = crate::wasi_filesystem::p2::types::route_create_directory(
            &generation_handle,
            PathTarget::at(root_directory, "p2-directory"),
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert_eq!(
            p2_existing_directory,
            wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::Exist
        );
        let p3_existing_directory = route_create_directory(
            &generation_handle,
            PathTarget::at(root_directory, "p3-directory"),
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert!(matches!(p3_existing_directory, types::ErrorCode::Exist));
        crate::wasi_filesystem::p2::types::route_create_symlink(
            &generation_handle,
            PathTarget::at(root_directory, "p2-symlink"),
            "p2-directory",
        )
        .await
        .unwrap();
        route_create_symlink(
            &generation_handle,
            PathTarget::at(root_directory, "p3-symlink"),
            "p3-directory",
        )
        .await
        .unwrap();
        let p2_existing_symlink = crate::wasi_filesystem::p2::types::route_create_symlink(
            &generation_handle,
            PathTarget::at(root_directory, "p2-symlink"),
            "p2-directory",
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert_eq!(
            p2_existing_symlink,
            wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::Exist
        );
        let p3_existing_symlink = route_create_symlink(
            &generation_handle,
            PathTarget::at(root_directory, "p3-symlink"),
            "p3-directory",
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert!(matches!(p3_existing_symlink, types::ErrorCode::Exist));
        let p2_missing_file = crate::wasi_filesystem::p2::types::route_unlink(
            &generation_handle,
            PathTarget::at(root_directory, "p2-missing-file"),
            ObjectKind::File,
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert_eq!(
            p2_missing_file,
            wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::NoEntry
        );
        let p3_missing_file = route_unlink(
            &generation_handle,
            PathTarget::at(root_directory, "p3-missing-file"),
            ObjectKind::File,
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert!(matches!(p3_missing_file, types::ErrorCode::NoEntry));
        let p2_missing_directory = crate::wasi_filesystem::p2::types::route_remove_directory(
            &generation_handle,
            PathTarget::at(root_directory, "p2-missing-directory"),
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert_eq!(
            p2_missing_directory,
            wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::NoEntry
        );
        let p3_missing_directory = route_remove_directory(
            &generation_handle,
            PathTarget::at(root_directory, "p3-missing-directory"),
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert!(matches!(p3_missing_directory, types::ErrorCode::NoEntry));
        let p2_follow_error = crate::wasi_filesystem::p2::types::route_hard_link(
            &generation_handle,
            PathTarget::at(root_directory, "file"),
            wasmtime_wasi::p2::bindings::filesystem::types::PathFlags::SYMLINK_FOLLOW,
            PathTarget::at(root_directory, "p2-invalid-link"),
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert_eq!(
            p2_follow_error,
            wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::Invalid
        );
        let p3_follow_error = route_hard_link(
            &generation_handle,
            PathTarget::at(root_directory, "file"),
            types::PathFlags::SYMLINK_FOLLOW,
            PathTarget::at(root_directory, "p3-invalid-link"),
        )
        .await
        .unwrap_err()
        .downcast()
        .unwrap();
        assert!(matches!(p3_follow_error, types::ErrorCode::Invalid));
        crate::wasi_filesystem::p2::types::route_hard_link(
            &generation_handle,
            PathTarget::at(root_directory, "file"),
            wasmtime_wasi::p2::bindings::filesystem::types::PathFlags::empty(),
            PathTarget::at(root_directory, "p2-hard-link"),
        )
        .await
        .unwrap();
        route_hard_link(
            &generation_handle,
            PathTarget::at(root_directory, "file"),
            types::PathFlags::empty(),
            PathTarget::at(root_directory, "p3-hard-link"),
        )
        .await
        .unwrap();
        crate::wasi_filesystem::p2::types::route_rename(
            &generation_handle,
            PathTarget::at(root_directory, "p2-hard-link"),
            PathTarget::at(root_directory, "p2-moved"),
        )
        .await
        .unwrap();
        route_rename(
            &generation_handle,
            PathTarget::at(root_directory, "p3-hard-link"),
            PathTarget::at(root_directory, "p3-moved"),
        )
        .await
        .unwrap();
        crate::wasi_filesystem::p2::types::route_unlink(
            &generation_handle,
            PathTarget::at(root_directory, "p2-moved"),
            ObjectKind::File,
        )
        .await
        .unwrap();
        route_unlink(
            &generation_handle,
            PathTarget::at(root_directory, "p3-moved"),
            ObjectKind::File,
        )
        .await
        .unwrap();
        crate::wasi_filesystem::p2::types::route_unlink(
            &generation_handle,
            PathTarget::at(root_directory, "p2-symlink"),
            ObjectKind::Symlink,
        )
        .await
        .unwrap();
        route_unlink(
            &generation_handle,
            PathTarget::at(root_directory, "p3-symlink"),
            ObjectKind::Symlink,
        )
        .await
        .unwrap();
        crate::wasi_filesystem::p2::types::route_remove_directory(
            &generation_handle,
            PathTarget::at(root_directory, "p2-directory"),
        )
        .await
        .unwrap();
        route_remove_directory(
            &generation_handle,
            PathTarget::at(root_directory, "p3-directory"),
        )
        .await
        .unwrap();

        close(node).await.unwrap();
        close(root_node).await.unwrap();

        let reconstructing = finish_replay(reconstructing).await.unwrap();
        let filesystem = finish_reconstruction(reconstructing).await.unwrap();
        let generation_handle = resident_generation_handle(&filesystem);
        let reopened = route_open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, "file").unwrap(),
            types::PathFlags::SYMLINK_FOLLOW,
            types::OpenFlags::empty(),
            types::DescriptorFlags::READ,
        )
        .await
        .unwrap();
        let OpenNode::File(file) = &reopened.node else {
            panic!("sandbox file reopen returned a directory")
        };
        let observed = route_attributes(&generation_handle, AgentTarget::Open(&reopened.node))
            .await
            .unwrap();
        assert_eq!(observed.size, 12);
        assert_eq!(observed.data_access_timestamp.unwrap().seconds, 300);
        assert_eq!(observed.data_modification_timestamp.unwrap().seconds, 400);
        assert_eq!(
            route_read_file(&generation_handle, file, 0, 64)
                .await
                .unwrap(),
            Bytes::from_static(b"p2-replay-p3")
        );
        close(reopened.node).await.unwrap();

        let root_node = route_open(
            &generation_handle,
            PathTarget::at_root(&generation_handle, ".").unwrap(),
            types::PathFlags::SYMLINK_FOLLOW,
            types::OpenFlags::DIRECTORY,
            types::DescriptorFlags::READ,
        )
        .await
        .unwrap()
        .node;
        let OpenNode::Directory(root_directory) = &root_node else {
            panic!("sandbox root reopen returned a file")
        };
        assert_eq!(
            route_list_directory(&generation_handle, root_directory)
                .await
                .unwrap()
                .into_iter()
                .map(|entry| entry.name)
                .collect::<Vec<_>>(),
            vec!["file".to_string()]
        );
        close(root_node).await.unwrap();
        close_window(window, Instant::now() + Duration::from_secs(1))
            .await
            .unwrap();
        delete(seal(filesystem)).await.unwrap();
    }

    #[test]
    fn p2_p3_write_progress_and_placement_translation_is_identical() {
        for placement in [WritePlacement::At(11), WritePlacement::Append] {
            let p2 = p2_agent_write_result(Ok(WriteResult { written: 7 }), placement).unwrap();
            let p3 = p3_agent_write_result(Ok(WriteResult { written: 7 }), placement)
                .unwrap()
                .unwrap();

            assert_eq!(p2.0 as usize, p3.0);
            assert_eq!(p2.1, p3.1);
            assert_eq!(
                p2.1,
                match placement {
                    WritePlacement::At(_) => WritePlacement::At(18),
                    WritePlacement::Append => WritePlacement::Append,
                }
            );
        }
    }

    #[test]
    fn p2_p3_write_quota_and_capacity_errors_are_identical() {
        fn storage_error() -> FilesystemStorageError {
            FilesystemStorageError::io(
                "write",
                std::path::Path::new("<wasi-write-test>"),
                std::io::ErrorKind::StorageFull.into(),
            )
        }

        use wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode as P2ErrorCode;

        for (p2_error, p3_error, p2_expected, p3_expected) in [
            (
                AgentFilesystemError::AgentQuota(storage_error()),
                AgentFilesystemError::AgentQuota(storage_error()),
                P2ErrorCode::Quota,
                "ErrorCode::Quota",
            ),
            (
                AgentFilesystemError::PhysicalCapacity(storage_error()),
                AgentFilesystemError::PhysicalCapacity(storage_error()),
                P2ErrorCode::InsufficientSpace,
                "ErrorCode::InsufficientSpace",
            ),
        ] {
            let p2: P2ErrorCode = p2_agent_write_result(Err(p2_error), WritePlacement::At(0))
                .unwrap_err()
                .downcast()
                .unwrap();
            let p3 = p3_agent_write_result(Err(p3_error), WritePlacement::At(0))
                .unwrap()
                .unwrap_err();

            assert_eq!(p2, p2_expected);
            assert_eq!(format!("{p3:?}"), p3_expected);
        }

        let p3_stream = filesystem_write_result_to_wasi(Err(FilesystemWriteFailure::Guest(
            types::ErrorCode::Quota,
        )))
        .unwrap()
        .unwrap_err();
        assert_eq!(format!("{p3_stream:?}"), "ErrorCode::Quota");
    }

    #[test]
    fn p3_write_runtime_invalidation_is_a_future_trap() {
        assert!(
            p3_agent_write_result(
                Err(AgentFilesystemError::RuntimeInvalidated),
                WritePlacement::Append,
            )
            .is_err()
        );
    }

    #[test]
    fn p2_p3_read_only_flag_mapping_is_identical() {
        use wasmtime_wasi::p2::bindings::filesystem::types::DescriptorFlags as P2DescriptorFlags;

        let p2_flags = P2DescriptorFlags::READ
            | P2DescriptorFlags::WRITE
            | P2DescriptorFlags::FILE_INTEGRITY_SYNC;
        let p3_flags = types::DescriptorFlags::READ
            | types::DescriptorFlags::WRITE
            | types::DescriptorFlags::FILE_INTEGRITY_SYNC;

        let p2_visible = p2_visible_descriptor_flags(p2_flags, true);
        let p3_visible = p3_visible_descriptor_flags(p3_flags, true);

        assert_eq!(
            p2_visible.contains(P2DescriptorFlags::READ),
            p3_visible.contains(types::DescriptorFlags::READ)
        );
        assert_eq!(
            p2_visible.contains(P2DescriptorFlags::WRITE),
            p3_visible.contains(types::DescriptorFlags::WRITE)
        );
        assert_eq!(
            p2_visible.contains(P2DescriptorFlags::FILE_INTEGRITY_SYNC),
            p3_visible.contains(types::DescriptorFlags::FILE_INTEGRITY_SYNC)
        );
    }

    #[test]
    fn metadata_hash_from_stat_matches_p2_hash() {
        let stat = types::DescriptorStat {
            type_: types::DescriptorType::RegularFile,
            link_count: 1,
            size: 42,
            data_access_timestamp: None,
            data_modification_timestamp: Some(
                wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
                    seconds: 123,
                    nanoseconds: 456,
                },
            ),
            status_change_timestamp: None,
        };
        let hash = metadata_hash_from_stat(&stat).unwrap();
        let (lower, upper) = calculate_metadata_hash_parts(Some((123, 456)), 42);
        assert_eq!(hash.lower, lower);
        assert_eq!(hash.upper, upper);
    }

    #[test]
    fn metadata_hash_from_stat_fails_with_overflow_for_pre_epoch_timestamps() {
        let stat = types::DescriptorStat {
            type_: types::DescriptorType::RegularFile,
            link_count: 1,
            size: 42,
            data_access_timestamp: None,
            data_modification_timestamp: Some(
                wasmtime_wasi::p3::bindings::clocks::system_clock::Instant {
                    seconds: -1,
                    nanoseconds: 0,
                },
            ),
            status_change_timestamp: None,
        };
        let error = metadata_hash_from_stat(&stat).unwrap_err();
        assert!(matches!(
            error.downcast().unwrap(),
            types::ErrorCode::Overflow
        ));
    }

    #[test]
    fn dropped_p3_write_progress_sender_reports_error() {
        let (chunks_tx, _chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let mut consumer = FilesystemWriteConsumer {
            chunks_tx: Some(chunks_tx),
            pending_chunk: Some(PendingFilesystemWriteChunk { result_rx }),
        };
        drop(result_tx);

        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            consumer.poll_pending_result(&mut cx),
            Poll::Ready(Err(_))
        ));
    }

    #[test]
    async fn dropped_p3_result_future_does_not_cancel_the_stream_write() {
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        drop(result_rx);
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let completed_by_operation = completed.clone();

        complete_filesystem_write_task(result_tx, async move {
            completed_by_operation.store(true, std::sync::atomic::Ordering::Release);
            Ok(Ok(()))
        })
        .await;

        assert!(completed.load(std::sync::atomic::Ordering::Acquire));
    }

    #[cfg(unix)]
    #[test]
    async fn p3_fs_stat_at_follow_symlink_does_not_mutate_symlink_timestamps() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let target = tempdir.path().join("target");
        let link = tempdir.path().join("link");
        std::fs::write(&target, b"target").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let new = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        set_symlink_times(
            &link,
            Some(SystemTimeSpec::from(old)),
            Some(SystemTimeSpec::from(old)),
        )
        .unwrap();
        let before = std::fs::symlink_metadata(&link)
            .unwrap()
            .modified()
            .unwrap();

        let new_timestamp = SerializableDateTime::from(new);
        apply_stat_response(
            Ok(types::DescriptorStat {
                type_: types::DescriptorType::RegularFile,
                link_count: 1,
                size: 6,
                data_access_timestamp: Some(new_timestamp.clone().into()),
                data_modification_timestamp: Some(new_timestamp.clone().into()),
                status_change_timestamp: None,
            }),
            HostResponseP3FileSystemStat {
                result: Ok(SerializableFileTimes {
                    data_access_timestamp: Some(new_timestamp.clone()),
                    data_modification_timestamp: Some(new_timestamp),
                }),
            },
        )
        .await
        .unwrap();

        let after = std::fs::symlink_metadata(&link)
            .unwrap()
            .modified()
            .unwrap();
        assert_eq!(
            after, before,
            "stat-at with symlink-follow is a read-only operation and must not rewrite the symlink itself"
        );
    }

    #[cfg(unix)]
    #[test]
    async fn p3_fs_stat_at_follow_symlink_does_not_mutate_target_timestamps() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let target = tempdir.path().join("target");
        let link = tempdir.path().join("link");
        std::fs::write(&target, b"target").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let old = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let new = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        set_times(
            &target,
            Some(SystemTimeSpec::from(old)),
            Some(SystemTimeSpec::from(old)),
        )
        .unwrap();
        let before = std::fs::metadata(&target).unwrap().modified().unwrap();

        let new_timestamp = SerializableDateTime::from(new);
        apply_stat_response(
            Ok(types::DescriptorStat {
                type_: types::DescriptorType::RegularFile,
                link_count: 1,
                size: 6,
                data_access_timestamp: Some(new_timestamp.clone().into()),
                data_modification_timestamp: Some(new_timestamp.clone().into()),
                status_change_timestamp: None,
            }),
            HostResponseP3FileSystemStat {
                result: Ok(SerializableFileTimes {
                    data_access_timestamp: Some(new_timestamp.clone()),
                    data_modification_timestamp: Some(new_timestamp),
                }),
            },
        )
        .await
        .unwrap();

        let after = std::fs::metadata(&target).unwrap().modified().unwrap();
        assert_eq!(
            after, before,
            "stat-at with symlink-follow is a read-only operation and must not rewrite the target"
        );
    }
}
