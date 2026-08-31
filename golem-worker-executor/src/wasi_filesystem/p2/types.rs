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
use std::pin::Pin;
use std::time::Duration;
use std::time::SystemTime;

use bytes::Bytes;
use wasmtime::component::Resource;
use wasmtime_wasi::filesystem::WasiFilesystemView as _;
use wasmtime_wasi::p2::FsError;
use wasmtime_wasi::p2::ReaddirIterator;
use wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime;
use wasmtime_wasi::p2::bindings::filesystem::types::{
    Advice, Descriptor, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry,
    DirectoryEntryStream, Error, ErrorCode, Filesize, Host, HostDescriptor,
    HostDirectoryEntryStream, InputStream, MetadataHashValue, NewTimestamp, OpenFlags,
    OutputStream, PathFlags,
};

use crate::durable_host::authorization::targets::CanonicalGuestPath;
use crate::durable_host::{
    DurabilityHost, DurableCallSession, DurableWorkerCtx, LiveAuthorizationPermit, NotCancellable,
};
use crate::services::agent_filesystem::{
    self as agent_filesystem, AccessMode, AttributeChanges, Attributes as AgentAttributes,
    Error as AgentFilesystemError, File as AgentFile, FilesystemGenerationHandle,
    FilesystemStorageError, NamespaceEdit, NewObject, ObjectKind, OpenNode, PathTarget,
    SymlinkTarget, Target as AgentTarget, TimeChange, TimeChanges, WritePlacement, WriteResult,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::card::FilesystemVerb;
use golem_common::model::oplog::host_functions::{
    FilesystemTypesDescriptorStat, FilesystemTypesDescriptorStatAt,
};
use golem_common::model::oplog::types::{
    FileSystemError, SerializableDateTime, SerializableFileTimes,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestFileSystemPath, HostResponseFileSystemStat,
};

use crate::wasi_filesystem::{
    AgentDescriptor, AgentOpenRequest, AgentOpenRouteError, advance_write_placement,
    agent_descriptor_guest_path, calculate_metadata_hash_parts, delete_agent_descriptor,
    filesystem_permission_targets, flush_level, get_agent_descriptor, push_agent_descriptor,
    resize_attribute_changes, route_agent_flush, route_agent_namespace_edit, route_agent_open,
    route_agent_set_attributes, route_agent_write, route_replay_timestamp_restoration,
    run_agent_filesystem_call,
};

fn p2_descriptor_guest_path(
    descriptor: &AgentDescriptor,
    relative: &str,
) -> Result<CanonicalGuestPath, FsError> {
    agent_descriptor_guest_path(descriptor, relative)
        .map_err(|_| FsError::from(ErrorCode::NotPermitted))
}

async fn authorize_paths<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    requests: &[(FilesystemVerb, CanonicalGuestPath)],
) -> Result<Option<LiveAuthorizationPermit>, FsError> {
    if !ctx.is_live() {
        return Ok(None);
    }
    let targets = filesystem_permission_targets(ctx, requests);
    match ctx.authorize_live_permissions(&targets).await {
        Ok(Ok(permit)) => Ok(Some(permit)),
        Ok(Err(_)) | Err(_) => Err(ErrorCode::NotPermitted.into()),
    }
}

fn p2_agent_storage_error(error: FilesystemStorageError) -> FsError {
    match error.io_error() {
        Some(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {
            ErrorCode::CrossDevice.into()
        }
        Some(error) => ErrorCode::from(error).into(),
        None => FsError::trap(wasmtime::Error::msg(error.to_string())),
    }
}

/// Converts a shared agent-filesystem failure into the P2 filesystem error contract.
/// Expected access, quota, capacity, and sandbox I/O failures become error codes; internal lifecycle failures trap.
pub(in crate::wasi_filesystem) fn p2_agent_error(error: AgentFilesystemError) -> FsError {
    match error {
        AgentFilesystemError::Access(agent_filesystem::AccessError::NotPermitted) => {
            ErrorCode::NotPermitted.into()
        }
        AgentFilesystemError::Sandbox(error) => p2_agent_storage_error(error),
        AgentFilesystemError::AgentQuota(_) => ErrorCode::Quota.into(),
        AgentFilesystemError::PhysicalCapacity(_) => ErrorCode::InsufficientSpace.into(),
        error @ (AgentFilesystemError::Access(_) | AgentFilesystemError::RuntimeInvalidated) => {
            FsError::trap(wasmtime::Error::msg(error.to_string()))
        }
    }
}

/// Converts shared open-policy and agent-filesystem failures into P2 open errors.
pub(in crate::wasi_filesystem) fn p2_agent_open_error(error: AgentOpenRouteError) -> FsError {
    match error {
        AgentOpenRouteError::Invalid => ErrorCode::Invalid.into(),
        AgentOpenRouteError::Unsupported => ErrorCode::Unsupported.into(),
        AgentOpenRouteError::SymlinkLoop => ErrorCode::Loop.into(),
        AgentOpenRouteError::Filesystem(error) => p2_agent_error(error),
    }
}

/// Maps agent-filesystem hard-link admission failures to P2 errors.
/// Linking across filesystem generations reports `CrossDevice`; other failures use the common P2 mapping.
pub(in crate::wasi_filesystem) fn p2_link_access_error(
    error: agent_filesystem::AccessError,
) -> FsError {
    match error {
        agent_filesystem::AccessError::WrongGeneration => ErrorCode::CrossDevice.into(),
        error => p2_agent_error(AgentFilesystemError::Access(error)),
    }
}

fn p2_agent_datetime(time: SystemTime) -> Result<Datetime, FsError> {
    let duration = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| FsError::from(ErrorCode::Overflow))?;
    Ok(Datetime {
        seconds: duration.as_secs(),
        nanoseconds: duration.subsec_nanos(),
    })
}

fn p2_agent_descriptor_type(kind: ObjectKind) -> DescriptorType {
    match kind {
        ObjectKind::File => DescriptorType::RegularFile,
        ObjectKind::Directory => DescriptorType::Directory,
        ObjectKind::Symlink => DescriptorType::SymbolicLink,
    }
}

/// Converts agent-filesystem attributes into a P2 descriptor stat.
/// Unrepresentable pre-epoch timestamps return `Overflow`, and status-change time is unavailable.
pub(in crate::wasi_filesystem) fn p2_agent_stat(
    attributes: AgentAttributes,
) -> Result<DescriptorStat, FsError> {
    Ok(DescriptorStat {
        type_: p2_agent_descriptor_type(attributes.kind),
        link_count: attributes.link_count,
        size: attributes.size,
        data_access_timestamp: attributes.accessed.map(p2_agent_datetime).transpose()?,
        data_modification_timestamp: attributes.modified.map(p2_agent_datetime).transpose()?,
        status_change_timestamp: None,
    })
}

/// Translates P2 path, open, and descriptor flags into the shared agent-filesystem open request.
/// Unsupported P2 integrity-sync flags are retained for common policy rejection.
pub(in crate::wasi_filesystem) fn p2_agent_open_request(
    path_flags: PathFlags,
    open_flags: OpenFlags,
    descriptor_flags: DescriptorFlags,
) -> AgentOpenRequest {
    AgentOpenRequest {
        create: open_flags.contains(OpenFlags::CREATE),
        directory: open_flags.contains(OpenFlags::DIRECTORY),
        exclusive: open_flags.contains(OpenFlags::EXCLUSIVE),
        truncate: open_flags.contains(OpenFlags::TRUNCATE),
        follow: path_flags.contains(PathFlags::SYMLINK_FOLLOW),
        read: descriptor_flags.contains(DescriptorFlags::READ),
        write: descriptor_flags.contains(DescriptorFlags::WRITE),
        unsupported_sync: descriptor_flags.intersects(
            DescriptorFlags::FILE_INTEGRITY_SYNC
                | DescriptorFlags::DATA_INTEGRITY_SYNC
                | DescriptorFlags::REQUESTED_WRITE_SYNC,
        ),
    }
}

/// Handles a P2 `open-at` request through the shared agent-filesystem open policy and lifecycle.
/// It returns an opened node for the host method to place in the resource table, with failures mapped to P2.
pub(crate) async fn route_open(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
    path_flags: PathFlags,
    open_flags: OpenFlags,
    descriptor_flags: DescriptorFlags,
) -> Result<agent_filesystem::Opened, FsError> {
    route_agent_open(
        generation_handle,
        target,
        p2_agent_open_request(path_flags, open_flags, descriptor_flags),
    )
    .await
    .map_err(p2_agent_open_error)
}

/// Resolves a P2 `readlink-at` target through `agent_filesystem`.
/// A target that cannot be represented as a Rust `String` returns `IllegalByteSequence`.
pub(crate) async fn route_symlink_target(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
) -> Result<String, FsError> {
    let target =
        run_agent_filesystem_call(agent_filesystem::symlink_target(generation_handle, target))
            .await
            .map_err(p2_agent_error)?;
    target
        .0
        .into_os_string()
        .into_string()
        .map_err(|_| ErrorCode::IllegalByteSequence.into())
}

async fn route_namespace_edit(
    generation_handle: &FilesystemGenerationHandle,
    edit: NamespaceEdit,
) -> Result<(), FsError> {
    let call = route_agent_namespace_edit(generation_handle, edit).map_err(p2_agent_error)?;
    call.await.map_err(p2_agent_error)
}

/// Creates a directory for P2 by submitting an agent-filesystem namespace insertion.
pub(crate) async fn route_create_directory(
    generation_handle: &FilesystemGenerationHandle,
    destination: PathTarget,
) -> Result<(), FsError> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Insert {
            destination,
            object: NewObject::Directory,
        },
    )
    .await
}

/// Creates a symlink for P2 by submitting its destination and raw target to `agent_filesystem`.
pub(crate) async fn route_create_symlink(
    generation_handle: &FilesystemGenerationHandle,
    destination: PathTarget,
    target: impl Into<std::path::PathBuf>,
) -> Result<(), FsError> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Insert {
            destination,
            object: NewObject::Symlink(SymlinkTarget(target.into())),
        },
    )
    .await
}

/// Creates a P2 hard link through the agent-filesystem namespace editor.
/// Following the source symlink is invalid, and cross-generation links report `CrossDevice`.
pub(crate) async fn route_hard_link(
    generation_handle: &FilesystemGenerationHandle,
    source: PathTarget,
    source_flags: PathFlags,
    destination: PathTarget,
) -> Result<(), FsError> {
    if source_flags.contains(PathFlags::SYMLINK_FOLLOW) {
        return Err(ErrorCode::Invalid.into());
    }
    let call = match route_agent_namespace_edit(
        generation_handle,
        NamespaceEdit::Link {
            source,
            destination,
        },
    ) {
        Err(AgentFilesystemError::Access(error)) => return Err(p2_link_access_error(error)),
        result => result.map_err(p2_agent_error)?,
    };
    call.await.map_err(p2_agent_error)
}

/// Renames a P2 path through an agent-filesystem namespace move.
pub(crate) async fn route_rename(
    generation_handle: &FilesystemGenerationHandle,
    source: PathTarget,
    destination: PathTarget,
) -> Result<(), FsError> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Move {
            source,
            destination,
        },
    )
    .await
}

/// Removes a P2 directory through `agent_filesystem`, requiring the target to be a directory.
pub(crate) async fn route_remove_directory(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
) -> Result<(), FsError> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Remove {
            target,
            expected: ObjectKind::Directory,
        },
    )
    .await
}

/// Unlinks a P2 path through `agent_filesystem` while enforcing the caller's expected object kind.
pub(crate) async fn route_unlink(
    generation_handle: &FilesystemGenerationHandle,
    target: PathTarget,
    expected: ObjectKind,
) -> Result<(), FsError> {
    route_namespace_edit(
        generation_handle,
        NamespaceEdit::Remove { target, expected },
    )
    .await
}

/// Starts a positioned P2 write through `agent_filesystem`.
/// Immediate admission errors are returned now; the future reports bytes written or the final P2 error.
pub(crate) fn route_write(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    offset: Filesize,
    bytes: Bytes,
) -> Result<impl Future<Output = Result<Filesize, FsError>> + Send + 'static + use<>, FsError> {
    let call = route_agent_write(generation_handle, file, WritePlacement::At(offset), bytes)
        .map_err(p2_agent_error)?;
    Ok(async move { p2_agent_write_result(call.await, WritePlacement::At(offset)).map(|v| v.0) })
}

/// Starts a P2 file resize through the agent-filesystem attribute route.
/// The returned future owns the admitted call and reports its final P2 result.
pub(crate) fn route_set_size(
    generation_handle: &FilesystemGenerationHandle,
    node: &OpenNode,
    size: Filesize,
) -> Result<impl Future<Output = Result<(), FsError>> + Send + 'static + use<>, FsError> {
    let call = route_agent_set_attributes(
        generation_handle,
        AgentTarget::Open(node),
        resize_attribute_changes(size),
    )
    .map_err(p2_agent_error)?;
    Ok(async move { call.await.map_err(p2_agent_error) })
}

/// Starts a P2 timestamp update for an open descriptor or path through `agent_filesystem`.
/// Invalid timestamps fail before submission; the returned future reports operation failures.
pub(crate) fn route_set_times(
    generation_handle: &FilesystemGenerationHandle,
    target: AgentTarget<'_>,
    accessed: NewTimestamp,
    modified: NewTimestamp,
) -> Result<impl Future<Output = Result<(), FsError>> + Send + 'static + use<>, FsError> {
    let changes = p2_time_changes(accessed, modified)?;
    let call =
        route_agent_set_attributes(generation_handle, target, AttributeChanges::Times(changes))
            .map_err(p2_agent_error)?;
    Ok(async move { call.await.map_err(p2_agent_error) })
}

/// Starts a P2 descriptor flush through `agent_filesystem`.
/// `data_only` selects a data-only flush rather than a data-and-metadata flush.
pub(crate) fn route_flush(
    generation_handle: &FilesystemGenerationHandle,
    node: &OpenNode,
    data_only: bool,
) -> Result<impl Future<Output = Result<(), FsError>> + Send + 'static + use<>, FsError> {
    let call = route_agent_flush(generation_handle, node, flush_level(data_only))
        .map_err(p2_agent_error)?;
    Ok(async move { call.await.map_err(p2_agent_error) })
}

/// Starts agent-filesystem timestamp restoration for a replayed P2 stat result.
/// Absent durable timestamps leave the corresponding sandbox values unchanged.
pub(crate) fn route_replay_times(
    generation_handle: &FilesystemGenerationHandle,
    target: AgentTarget<'_>,
    accessed: Option<SystemTime>,
    modified: Option<SystemTime>,
) -> Result<impl Future<Output = Result<(), FsError>> + Send + 'static + use<>, FsError> {
    let call = route_replay_timestamp_restoration(generation_handle, target, accessed, modified)
        .map_err(p2_agent_error)?;
    Ok(async move { call.await.map_err(p2_agent_error) })
}

/// Starts one P2 output-stream write chunk through `agent_filesystem`.
/// Completion returns bytes written and the next placement, preserving append mode and detecting offset overflow.
pub(crate) fn route_output_stream_chunk(
    generation_handle: &FilesystemGenerationHandle,
    file: &AgentFile,
    placement: WritePlacement,
    bytes: Bytes,
) -> Result<
    impl Future<Output = Result<(Filesize, WritePlacement), FsError>> + Send + 'static + use<>,
    FsError,
> {
    let call =
        route_agent_write(generation_handle, file, placement, bytes).map_err(p2_agent_error)?;
    Ok(async move { p2_agent_write_result(call.await, placement) })
}

/// Converts an agent-filesystem write completion into P2 progress and the next stream placement.
/// Filesystem failures use P2 codes, while positioned-offset overflow follows the internal-error mapping.
pub(in crate::wasi_filesystem) fn p2_agent_write_result(
    result: Result<WriteResult, AgentFilesystemError>,
    placement: WritePlacement,
) -> Result<(Filesize, WritePlacement), FsError> {
    let result = result.map_err(p2_agent_error)?;
    let next = advance_write_placement(placement, result.written).map_err(p2_agent_error)?;
    Ok((result.written, next))
}

fn p2_agent_descriptor<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    descriptor: &Resource<Descriptor>,
) -> Result<AgentDescriptor, FsError> {
    Ok(get_agent_descriptor(ctx, descriptor)?)
}

fn p2_agent_path_target(
    descriptor: &AgentDescriptor,
    path: impl Into<std::path::PathBuf>,
) -> Result<PathTarget, FsError> {
    descriptor.with_node(|node| match node {
        OpenNode::Directory(directory) => Ok(PathTarget::at(directory, path)),
        OpenNode::File(_) => Err(ErrorCode::NotDirectory.into()),
    })
}

fn p2_agent_flags(
    generation_handle: &FilesystemGenerationHandle,
    descriptor: &AgentDescriptor,
) -> Result<DescriptorFlags, FsError> {
    let (kind, mode) = descriptor.with_node(|node| (node.kind(), node.access()));
    let mut flags = DescriptorFlags::empty();
    if matches!(mode, AccessMode::Read | AccessMode::ReadWrite) {
        flags |= DescriptorFlags::READ;
    }
    if matches!(mode, AccessMode::Write | AccessMode::ReadWrite) {
        flags |= if kind == ObjectKind::Directory {
            DescriptorFlags::MUTATE_DIRECTORY
        } else {
            DescriptorFlags::WRITE
        };
    }
    if kind == ObjectKind::File
        && agent_filesystem::path_permissions(generation_handle, descriptor.path())
            .map_err(|error| p2_agent_error(AgentFilesystemError::Access(error)))?
            == golem_common::model::component::AgentFilePermissions::ReadOnly
    {
        flags &= !DescriptorFlags::WRITE;
    }
    Ok(flags)
}

struct AgentFileInputStream {
    generation_handle: FilesystemGenerationHandle,
    descriptor: AgentDescriptor,
    offset: u64,
    buffered: Bytes,
    eof: bool,
    error: Option<wasmtime::Error>,
}

impl AgentFileInputStream {
    fn new(
        generation_handle: FilesystemGenerationHandle,
        descriptor: AgentDescriptor,
        offset: u64,
    ) -> Self {
        Self {
            generation_handle,
            descriptor,
            offset,
            buffered: Bytes::new(),
            eof: false,
            error: None,
        }
    }
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::InputStream for AgentFileInputStream {
    fn read(&mut self, size: usize) -> wasmtime_wasi::p2::StreamResult<Bytes> {
        if let Some(error) = self.error.take() {
            return Err(wasmtime_wasi::p2::StreamError::LastOperationFailed(error));
        }
        if self.buffered.is_empty() {
            return if self.eof {
                Err(wasmtime_wasi::p2::StreamError::Closed)
            } else {
                Ok(Bytes::new())
            };
        }
        Ok(self.buffered.split_to(size.min(self.buffered.len())))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for AgentFileInputStream {
    async fn ready(&mut self) {
        if !self.buffered.is_empty() || self.eof || self.error.is_some() {
            return;
        }
        let call = self.descriptor.with_node(|node| match node {
            OpenNode::File(file) => agent_filesystem::read_file(
                &self.generation_handle,
                file,
                agent_filesystem::ReadRange {
                    offset: self.offset,
                    length: 64 * 1024,
                },
            )
            .map_err(AgentFilesystemError::Access),
            OpenNode::Directory(_) => Err(AgentFilesystemError::RuntimeInvalidated),
        });
        match call {
            Ok(call) => match call.await {
                Ok(bytes) => {
                    self.eof = bytes.is_empty();
                    self.offset = self.offset.saturating_add(bytes.len() as u64);
                    self.buffered = bytes;
                }
                Err(error) => self.error = Some(wasmtime::Error::msg(error.to_string())),
            },
            Err(error) => self.error = Some(wasmtime::Error::msg(error.to_string())),
        }
    }
}

struct AgentFileOutputStream {
    generation_handle: FilesystemGenerationHandle,
    descriptor: AgentDescriptor,
    placement: WritePlacement,
    state: AgentFileOutputState,
    _authorization_permit: Option<LiveAuthorizationPermit>,
}

type PendingAgentFileWrite =
    Pin<Box<dyn Future<Output = Result<(Filesize, WritePlacement), FsError>> + Send + 'static>>;

enum AgentFileOutputState {
    Ready,
    Waiting(PendingAgentFileWrite),
    Error(Option<FsError>),
    Closed,
}

impl AgentFileOutputStream {
    fn new(
        generation_handle: FilesystemGenerationHandle,
        descriptor: AgentDescriptor,
        placement: WritePlacement,
        authorization_permit: Option<LiveAuthorizationPermit>,
    ) -> Self {
        Self {
            generation_handle,
            descriptor,
            placement,
            state: AgentFileOutputState::Ready,
            _authorization_permit: authorization_permit,
        }
    }

    fn take_error(&mut self) -> wasmtime_wasi::p2::StreamError {
        let AgentFileOutputState::Error(error) = &mut self.state else {
            unreachable!("filesystem output stream error state changed unexpectedly")
        };
        let error = error
            .take()
            .expect("filesystem output stream error missing");
        self.state = AgentFileOutputState::Closed;
        p2_output_stream_error(error)
    }
}

fn p2_output_stream_error(error: FsError) -> wasmtime_wasi::p2::StreamError {
    let error = match error.downcast() {
        Ok(error) => error.into(),
        Err(error) => error,
    };
    wasmtime_wasi::p2::StreamError::LastOperationFailed(error)
}

fn p2_output_stream_error_code(error: &wasmtime::Error) -> Option<ErrorCode> {
    error.downcast_ref::<ErrorCode>().copied()
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::OutputStream for AgentFileOutputStream {
    fn write(&mut self, bytes: Bytes) -> wasmtime_wasi::p2::StreamResult<()> {
        if !matches!(self.state, AgentFileOutputState::Ready) {
            return Err(wasmtime_wasi::p2::StreamError::trap(
                "write not permitted before check-write reports readiness",
            ));
        }
        let operation = self.descriptor.with_node(|node| match node {
            OpenNode::File(file) => {
                route_output_stream_chunk(&self.generation_handle, file, self.placement, bytes)
            }
            OpenNode::Directory(_) => Err(ErrorCode::BadDescriptor.into()),
        });
        self.state = match operation {
            Ok(operation) => AgentFileOutputState::Waiting(Box::pin(operation)),
            Err(error) => AgentFileOutputState::Error(Some(error)),
        };
        Ok(())
    }

    fn flush(&mut self) -> wasmtime_wasi::p2::StreamResult<()> {
        match self.state {
            AgentFileOutputState::Ready | AgentFileOutputState::Waiting(_) => Ok(()),
            AgentFileOutputState::Error(_) => Err(self.take_error()),
            AgentFileOutputState::Closed => Err(wasmtime_wasi::p2::StreamError::Closed),
        }
    }

    fn check_write(&mut self) -> wasmtime_wasi::p2::StreamResult<usize> {
        match self.state {
            AgentFileOutputState::Ready => Ok(1024 * 1024),
            AgentFileOutputState::Waiting(_) => Ok(0),
            AgentFileOutputState::Error(_) => Err(self.take_error()),
            AgentFileOutputState::Closed => Err(wasmtime_wasi::p2::StreamError::Closed),
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for AgentFileOutputStream {
    async fn ready(&mut self) {
        let AgentFileOutputState::Waiting(operation) = &mut self.state else {
            return;
        };
        self.state = match operation.await {
            Ok((_, placement)) => {
                self.placement = placement;
                AgentFileOutputState::Ready
            }
            Err(error) => AgentFileOutputState::Error(Some(error)),
        };
    }
}

fn p2_timestamp(seconds: u64, nanoseconds: u32) -> Result<SystemTime, ErrorCode> {
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::new(seconds, nanoseconds))
        .ok_or(ErrorCode::Overflow)
}

fn p2_validate_time(requested: NewTimestamp) -> Result<(), FsError> {
    match requested {
        NewTimestamp::Timestamp(timestamp) => {
            p2_timestamp(timestamp.seconds, timestamp.nanoseconds)
                .map(|_| ())
                .map_err(Into::into)
        }
        NewTimestamp::NoChange | NewTimestamp::Now => Ok(()),
    }
}

/// Converts P2 timestamp requests into agent-filesystem changes for `set-times` calls.
/// Values outside `SystemTime` return `Overflow` before either timestamp is changed.
pub(in crate::wasi_filesystem) fn p2_time_changes(
    accessed: NewTimestamp,
    modified: NewTimestamp,
) -> Result<TimeChanges, FsError> {
    p2_validate_time(accessed)?;
    p2_validate_time(modified)?;
    Ok(TimeChanges {
        accessed: p2_time_change(accessed)?,
        modified: p2_time_change(modified)?,
    })
}

fn p2_time_change(requested: NewTimestamp) -> Result<TimeChange, FsError> {
    match requested {
        NewTimestamp::NoChange => Ok(TimeChange::Keep),
        NewTimestamp::Now => Ok(TimeChange::Now),
        NewTimestamp::Timestamp(timestamp) => {
            p2_timestamp(timestamp.seconds, timestamp.nanoseconds)
                .map(TimeChange::Set)
                .map_err(Into::into)
        }
    }
}

#[cfg(test)]
pub(in crate::wasi_filesystem) fn p2_visible_descriptor_flags(
    mut flags: DescriptorFlags,
    read_only: bool,
) -> DescriptorFlags {
    if read_only {
        flags &= !DescriptorFlags::WRITE;
    }
    flags
}

impl<Ctx: WorkerCtx> HostDescriptor for DurableWorkerCtx<Ctx> {
    async fn read_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<InputStream>, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit = authorize_paths(self, &[(FilesystemVerb::Read, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "read_via_stream");
        if descriptor.with_node(|node| !matches!(node, OpenNode::File(_))) {
            return Err(ErrorCode::BadDescriptor.into());
        }
        let stream: wasmtime_wasi::p2::DynInputStream = Box::new(AgentFileInputStream::new(
            generation_handle,
            descriptor,
            offset,
        ));
        let stream = self.table().push(stream)?;
        self.register_filesystem_input_stream(stream.rep());
        Ok(stream)
    }

    async fn write_via_stream(
        &mut self,
        fd: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<OutputStream>, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let authorization_permit = authorize_paths(self, &[(FilesystemVerb::Write, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "write_via_stream");
        let flags = p2_agent_flags(&generation_handle, &descriptor)?;
        if !flags.contains(DescriptorFlags::WRITE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        let stream: wasmtime_wasi::p2::DynOutputStream = Box::new(AgentFileOutputStream::new(
            generation_handle,
            descriptor,
            WritePlacement::At(offset),
            authorization_permit,
        ));
        let stream = self.table().push(stream)?;
        self.register_filesystem_output_stream(stream.rep());
        Ok(stream)
    }

    async fn append_via_stream(
        &mut self,
        fd: Resource<Descriptor>,
    ) -> Result<Resource<OutputStream>, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let authorization_permit = authorize_paths(self, &[(FilesystemVerb::Write, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "append_via_stream");
        let flags = p2_agent_flags(&generation_handle, &descriptor)?;
        if !flags.contains(DescriptorFlags::WRITE) {
            return Err(ErrorCode::NotPermitted.into());
        }
        let stream: wasmtime_wasi::p2::DynOutputStream = Box::new(AgentFileOutputStream::new(
            generation_handle,
            descriptor,
            WritePlacement::Append,
            authorization_permit,
        ));
        let stream = self.table().push(stream)?;
        self.register_filesystem_output_stream(stream.rep());
        Ok(stream)
    }

    async fn advise(
        &mut self,
        self_: Resource<Descriptor>,
        _offset: Filesize,
        _length: Filesize,
        _advice: Advice,
    ) -> Result<(), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "advise");
        let descriptor = p2_agent_descriptor(self, &self_)?;
        if descriptor.with_node(|node| matches!(node, OpenNode::Directory(_))) {
            return Err(ErrorCode::BadDescriptor.into());
        }
        Ok(())
    }

    async fn sync_data(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit = authorize_paths(self, &[(FilesystemVerb::Write, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "sync_data");
        let operation = descriptor.with_node(|node| route_flush(&generation_handle, node, true))?;
        operation.await
    }

    async fn get_flags(&mut self, fd: Resource<Descriptor>) -> Result<DescriptorFlags, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "get_flags");

        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        p2_agent_flags(&generation_handle, &descriptor)
    }

    async fn get_type(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorType, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "get_type");
        let descriptor = p2_agent_descriptor(self, &self_)?;
        Ok(p2_agent_descriptor_type(
            descriptor.with_node(OpenNode::kind),
        ))
    }

    async fn set_size(&mut self, fd: Resource<Descriptor>, size: Filesize) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit = authorize_paths(self, &[(FilesystemVerb::Write, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "set_size");
        if descriptor.with_node(|node| matches!(node, OpenNode::Directory(_))) {
            return Err(ErrorCode::BadDescriptor.into());
        }
        let operation =
            descriptor.with_node(|node| route_set_size(&generation_handle, node, size))?;
        operation.await
    }

    async fn set_times(
        &mut self,
        fd: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit = authorize_paths(self, &[(FilesystemVerb::Write, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "set_times");
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

    async fn read(
        &mut self,
        self_: Resource<Descriptor>,
        length: Filesize,
        offset: Filesize,
    ) -> Result<(Vec<u8>, bool), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit = authorize_paths(self, &[(FilesystemVerb::Read, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "read");
        let length = usize::try_from(length).unwrap_or(usize::MAX).min(64 * 1024);
        let call = descriptor.with_node(|node| match node {
            OpenNode::File(file) => agent_filesystem::read_file(
                &generation_handle,
                file,
                agent_filesystem::ReadRange { offset, length },
            )
            .map_err(|error| p2_agent_error(AgentFilesystemError::Access(error))),
            OpenNode::Directory(_) => Err(ErrorCode::BadDescriptor.into()),
        })?;
        let bytes = call.await.map_err(p2_agent_error)?;
        let eof = bytes.is_empty();
        Ok((bytes.to_vec(), eof))
    }

    async fn write(
        &mut self,
        fd: Resource<Descriptor>,
        buffer: Vec<u8>,
        offset: Filesize,
    ) -> Result<Filesize, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit = authorize_paths(self, &[(FilesystemVerb::Write, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "write");
        let operation = descriptor.with_node(|node| match node {
            OpenNode::File(file) => {
                route_write(&generation_handle, file, offset, Bytes::from(buffer))
            }
            OpenNode::Directory(_) => Err(ErrorCode::BadDescriptor.into()),
        })?;
        operation.await
    }

    async fn read_directory(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<DirectoryEntryStream>, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit = authorize_paths(self, &[(FilesystemVerb::List, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "read_directory");
        let call = descriptor.with_node(|node| match node {
            OpenNode::Directory(directory) => {
                agent_filesystem::list_directory(&generation_handle, directory)
                    .map_err(|error| p2_agent_error(AgentFilesystemError::Access(error)))
            }
            OpenNode::File(_) => Err(ErrorCode::NotDirectory.into()),
        })?;
        let mut entries = call
            .await
            .map_err(p2_agent_error)?
            .into_iter()
            .map(|entry| {
                Ok(DirectoryEntry {
                    type_: p2_agent_descriptor_type(entry.kind),
                    name: entry
                        .name
                        .into_string()
                        .map_err(|_| FsError::from(ErrorCode::IllegalByteSequence))?,
                })
            })
            .collect::<Result<Vec<_>, FsError>>()?;
        entries.sort_by_key(|entry| entry.name.clone());

        Ok(self
            .table()
            .push(ReaddirIterator::new(entries.into_iter().map(Ok)))?)
    }

    async fn sync(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit = authorize_paths(self, &[(FilesystemVerb::Write, path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "sync");
        let operation =
            descriptor.with_node(|node| route_flush(&generation_handle, node, false))?;
        operation.await
    }

    async fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let guest_path = p2_descriptor_guest_path(&descriptor, &path)?;
        let _authorization_permit =
            authorize_paths(self, &[(FilesystemVerb::Write, guest_path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "create_directory_at");
        let target = p2_agent_path_target(&descriptor, path)?;
        route_create_directory(&generation_handle, target).await
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let guest_path = p2_descriptor_guest_path(&descriptor, "")?;
        let _authorization_permit =
            authorize_paths(self, &[(FilesystemVerb::Stat, guest_path)]).await?;
        let path = descriptor.path().to_path_buf();

        // `ReadLocal`: the local stat always runs (its timestamps are then overridden by the durable
        // value), so only the file-times are made durable via `DurableCallSession::run`.
        let handle = DurableCallSession::<FilesystemTypesDescriptorStat, NotCancellable>::start(
            self,
            HostRequestFileSystemPath {
                path: path.to_string_lossy().to_string(),
            },
            DurableFunctionType::ReadLocal,
        )
        .await
        .map_err(FsError::trap)?;

        let call = descriptor.with_node(|node| {
            agent_filesystem::attributes(&generation_handle, AgentTarget::Open(node))
                .map_err(|error| p2_agent_error(AgentFilesystemError::Access(error)))
        })?;
        let stat = match call.await.map_err(p2_agent_error).and_then(p2_agent_stat) {
            Ok(stat) => Ok(stat),
            Err(fs_error) => Err(fs_error
                .downcast_ref()
                .cloned()
                .ok_or_else(|| fs_error.to_string())),
        };

        let result = handle
            .run(self, async |_ctx| -> wasmtime::Result<_> {
                let result = stat
                    .clone()
                    .map(|stat| SerializableFileTimes {
                        data_access_timestamp: stat.data_access_timestamp.map(|t| t.into()),
                        data_modification_timestamp: stat
                            .data_modification_timestamp
                            .map(|t| t.into()),
                    })
                    .map_err(FileSystemError::from_result);
                Ok(HostResponseFileSystemStat { result })
            })
            .await
            .map_err(FsError::trap)?;

        match result.result {
            Ok(times) => {
                let accessed = times
                    .data_access_timestamp
                    .as_ref()
                    .map(|t| <SerializableDateTime as Into<SystemTime>>::into(t.clone()));
                let modified = times
                    .data_modification_timestamp
                    .as_ref()
                    .map(|t| <SerializableDateTime as Into<SystemTime>>::into(t.clone()));
                let restoration = descriptor.with_node(|node| {
                    route_replay_times(
                        &generation_handle,
                        AgentTarget::Open(node),
                        accessed,
                        modified,
                    )
                })?;
                restoration.await?;
                let mut stat = stat.map_err(|error| match error {
                    Ok(error) => FsError::from(error),
                    Err(error) => FsError::trap(wasmtime::Error::msg(error)),
                })?;
                stat.data_access_timestamp = times.data_access_timestamp.map(|t| t.into());
                stat.data_modification_timestamp =
                    times.data_modification_timestamp.map(|t| t.into());
                Ok(stat)
            }
            Err(err) => Err(err.into()),
        }
    }

    async fn stat_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let guest_path = p2_descriptor_guest_path(&descriptor, &path)?;
        let _authorization_permit =
            authorize_paths(self, &[(FilesystemVerb::Stat, guest_path)]).await?;
        let full_path = descriptor.path().join(path.clone());
        let target = p2_agent_path_target(&descriptor, path.clone())?;
        let follow = if path_flags.contains(PathFlags::SYMLINK_FOLLOW) {
            agent_filesystem::Follow::Yes
        } else {
            agent_filesystem::Follow::No
        };

        // `ReadLocal`: the local stat always runs (its timestamps are then overridden by the durable
        // value), so only the file-times are made durable via `DurableCallSession::run`.
        let handle = DurableCallSession::<FilesystemTypesDescriptorStatAt, NotCancellable>::start(
            self,
            HostRequestFileSystemPath {
                path: full_path.to_string_lossy().to_string(),
            },
            DurableFunctionType::ReadLocal,
        )
        .await
        .map_err(FsError::trap)?;

        let call =
            agent_filesystem::attributes(&generation_handle, AgentTarget::Path(&target, follow))
                .map_err(|error| p2_agent_error(AgentFilesystemError::Access(error)))?;
        let stat = match call.await.map_err(p2_agent_error).and_then(p2_agent_stat) {
            Ok(stat) => Ok(stat),
            Err(fs_error) => Err(fs_error
                .downcast_ref()
                .cloned()
                .ok_or_else(|| fs_error.to_string())),
        };

        let result = handle
            .run(self, async |_ctx| -> wasmtime::Result<_> {
                let result = stat
                    .clone()
                    .map(|stat| SerializableFileTimes {
                        data_access_timestamp: stat.data_access_timestamp.map(|t| t.into()),
                        data_modification_timestamp: stat
                            .data_modification_timestamp
                            .map(|t| t.into()),
                    })
                    .map_err(FileSystemError::from_result);
                Ok(HostResponseFileSystemStat { result })
            })
            .await
            .map_err(FsError::trap)?;

        match result.result {
            Ok(times) => {
                let accessed = times
                    .data_access_timestamp
                    .as_ref()
                    .map(|t| <SerializableDateTime as Into<SystemTime>>::into(t.clone()));
                let modified = times
                    .data_modification_timestamp
                    .as_ref()
                    .map(|t| <SerializableDateTime as Into<SystemTime>>::into(t.clone()));
                let restoration = route_replay_times(
                    &generation_handle,
                    AgentTarget::Path(&target, follow),
                    accessed,
                    modified,
                )?;
                restoration.await?;
                let mut stat = stat.map_err(|error| match error {
                    Ok(error) => FsError::from(error),
                    Err(error) => FsError::trap(wasmtime::Error::msg(error)),
                })?;
                stat.data_access_timestamp = times.data_access_timestamp.map(|t| t.into());
                stat.data_modification_timestamp =
                    times.data_modification_timestamp.map(|t| t.into());
                Ok(stat)
            }
            Err(err) => Err(err.into()),
        }
    }

    async fn set_times_at(
        &mut self,
        fd: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        let guest_path = p2_descriptor_guest_path(&descriptor, &path)?;
        let _authorization_permit =
            authorize_paths(self, &[(FilesystemVerb::Write, guest_path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "set_times_at");
        let target = p2_agent_path_target(&descriptor, path)?;
        let follow = if path_flags.contains(PathFlags::SYMLINK_FOLLOW) {
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
        &mut self,
        self_: Resource<Descriptor>,
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let source_descriptor = p2_agent_descriptor(self, &self_)?;
        let destination_descriptor = p2_agent_descriptor(self, &new_descriptor)?;
        let source_path = p2_descriptor_guest_path(&source_descriptor, &old_path)?;
        let destination_path = p2_descriptor_guest_path(&destination_descriptor, &new_path)?;
        let _authorization_permit = authorize_paths(
            self,
            &[
                (FilesystemVerb::Read, source_path),
                (FilesystemVerb::Write, destination_path),
            ],
        )
        .await?;
        self.observe_function_call("filesystem::types::descriptor", "link_at");
        let source = p2_agent_path_target(&source_descriptor, old_path)?;
        let destination = p2_agent_path_target(&destination_descriptor, new_path)?;
        route_hard_link(&generation_handle, source, old_path_flags, destination).await
    }

    async fn open_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<Resource<Descriptor>, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let guest_path = p2_descriptor_guest_path(&descriptor, &path)?;
        let mut permissions = Vec::new();
        if flags.contains(DescriptorFlags::READ) {
            permissions.push((FilesystemVerb::Read, guest_path.clone()));
        }
        if flags.contains(DescriptorFlags::WRITE)
            || open_flags.intersects(OpenFlags::CREATE | OpenFlags::TRUNCATE)
        {
            permissions.push((FilesystemVerb::Write, guest_path.clone()));
        }
        if open_flags.contains(OpenFlags::DIRECTORY) {
            permissions.push((FilesystemVerb::List, guest_path));
        }
        let _authorization_permit = authorize_paths(self, &permissions).await?;
        self.observe_function_call("filesystem::types::descriptor", "open_at");
        let descriptor_path = descriptor.path().join(&path);
        let target = p2_agent_path_target(&descriptor, path)?;
        let opened = route_open(&generation_handle, target, path_flags, open_flags, flags).await?;
        Ok(push_agent_descriptor(
            self,
            AgentDescriptor::new(opened.node, descriptor_path),
        )?)
    }

    async fn readlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<String, FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let guest_path = p2_descriptor_guest_path(&descriptor, &path)?;
        let _authorization_permit =
            authorize_paths(self, &[(FilesystemVerb::Stat, guest_path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "readlink_at");
        let target = p2_agent_path_target(&descriptor, path)?;
        route_symlink_target(&generation_handle, target).await
    }

    async fn remove_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &self_)?;
        let guest_path = p2_descriptor_guest_path(&descriptor, &path)?;
        let _authorization_permit =
            authorize_paths(self, &[(FilesystemVerb::Delete, guest_path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "remove_directory_at");
        let target = p2_agent_path_target(&descriptor, path)?;
        route_remove_directory(&generation_handle, target).await
    }

    async fn rename_at(
        &mut self,
        old_fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let source_descriptor = p2_agent_descriptor(self, &old_fd)?;
        let destination_descriptor = p2_agent_descriptor(self, &new_fd)?;
        let source_path = p2_descriptor_guest_path(&source_descriptor, &old_path)?;
        let destination_path = p2_descriptor_guest_path(&destination_descriptor, &new_path)?;
        let _authorization_permit = authorize_paths(
            self,
            &[
                (FilesystemVerb::Delete, source_path),
                (FilesystemVerb::Write, destination_path),
            ],
        )
        .await?;
        self.observe_function_call("filesystem::types::descriptor", "rename_at");
        let source = p2_agent_path_target(&source_descriptor, old_path)?;
        let destination = p2_agent_path_target(&destination_descriptor, new_path)?;
        route_rename(&generation_handle, source, destination).await
    }

    async fn symlink_at(
        &mut self,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        let destination_path = p2_descriptor_guest_path(&descriptor, &new_path)?;
        let _authorization_permit =
            authorize_paths(self, &[(FilesystemVerb::Write, destination_path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "symlink_at");
        let destination = p2_agent_path_target(&descriptor, new_path)?;
        route_create_symlink(&generation_handle, destination, old_path).await
    }

    async fn unlink_file_at(
        &mut self,
        fd: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let generation_handle = self.filesystem_generation_handle();
        let descriptor = p2_agent_descriptor(self, &fd)?;
        let guest_path = p2_descriptor_guest_path(&descriptor, &path)?;
        let _authorization_permit =
            authorize_paths(self, &[(FilesystemVerb::Delete, guest_path)]).await?;
        self.observe_function_call("filesystem::types::descriptor", "unlink_file_at");
        let target = p2_agent_path_target(&descriptor, path)?;
        route_unlink(&generation_handle, target, ObjectKind::File).await
    }

    async fn is_same_object(
        &mut self,
        self_: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        self.observe_function_call("filesystem::types::descriptor", "is_same_object");
        let generation_handle = self.filesystem_generation_handle();
        let left = p2_agent_descriptor(self, &self_)?;
        let right = p2_agent_descriptor(self, &other)?;
        let call = AgentDescriptor::with_nodes(&left, &right, |left, right| {
            agent_filesystem::is_same_object(&generation_handle, left, right)
        })?;
        call.await
            .map_err(|error| wasmtime::Error::msg(error.to_string()))
    }

    async fn metadata_hash(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<MetadataHashValue, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "metadata_hash");

        // Using the WASI stat function as it guarantees the file times are preserved
        let metadata = self.stat(self_).await?;

        Ok(calculate_metadata_hash(&metadata))
    }

    async fn metadata_hash_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "metadata_hash_at");
        // Using the WASI stat_at function as it guarantees the file times are preserved
        let metadata = self.stat_at(self_, path_flags, path).await?;

        Ok(calculate_metadata_hash(&metadata))
    }

    fn drop(&mut self, rep: Resource<Descriptor>) -> wasmtime::Result<()> {
        self.observe_function_call("filesystem::types::descriptor", "drop");
        delete_agent_descriptor(self, rep)?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> HostDirectoryEntryStream for DurableWorkerCtx<Ctx> {
    async fn read_directory_entry(
        &mut self,
        self_: Resource<DirectoryEntryStream>,
    ) -> Result<Option<DirectoryEntry>, FsError> {
        self.observe_function_call(
            "filesystem::types::directory_entry_stream",
            "read_directory_entry",
        );
        let mut view = self.as_wasi_view();
        HostDirectoryEntryStream::read_directory_entry(&mut view.filesystem(), self_).await
    }

    fn drop(&mut self, rep: Resource<DirectoryEntryStream>) -> wasmtime::Result<()> {
        self.observe_function_call("filesystem::types::directory_entry_stream", "drop");
        HostDirectoryEntryStream::drop(&mut self.as_wasi_view().filesystem(), rep)
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn filesystem_error_code(
        &mut self,
        err: Resource<Error>,
    ) -> wasmtime::Result<Option<ErrorCode>> {
        if let Some(error) = p2_output_stream_error_code(self.table().get(&err)?) {
            return Ok(Some(error));
        }
        Host::filesystem_error_code(&mut self.as_wasi_view().filesystem(), err)
    }

    fn convert_error_code(&mut self, err: FsError) -> wasmtime::Result<ErrorCode> {
        Host::convert_error_code(&mut self.as_wasi_view().filesystem(), err)
    }
}

fn calculate_metadata_hash(meta: &DescriptorStat) -> MetadataHashValue {
    let (lower, upper) = calculate_metadata_hash_parts(
        meta.data_modification_timestamp
            .map(|t| (t.seconds, t.nanoseconds)),
        meta.size,
    );
    MetadataHashValue { lower, upper }
}

#[cfg(test)]
mod tests {
    use super::*;

    use test_r::test;

    fn error_code<T: std::fmt::Debug>(result: Result<T, FsError>) -> ErrorCode {
        result.unwrap_err().downcast().unwrap()
    }

    #[test]
    fn read_only_descriptor_flags_remove_only_write() {
        let flags =
            DescriptorFlags::READ | DescriptorFlags::WRITE | DescriptorFlags::FILE_INTEGRITY_SYNC;

        assert_eq!(p2_visible_descriptor_flags(flags, false), flags);
        assert_eq!(
            p2_visible_descriptor_flags(flags, true),
            DescriptorFlags::READ | DescriptorFlags::FILE_INTEGRITY_SYNC
        );
    }

    #[test]
    fn p2_timestamp_validation_accepts_representable_values_and_rejects_overflow() {
        assert_eq!(
            p2_timestamp(12, 345).unwrap(),
            SystemTime::UNIX_EPOCH + Duration::new(12, 345)
        );
        assert!(p2_validate_time(NewTimestamp::NoChange).is_ok());
        assert!(p2_validate_time(NewTimestamp::Now).is_ok());
        assert_eq!(
            error_code(p2_validate_time(NewTimestamp::Timestamp(Datetime {
                seconds: u64::MAX,
                nanoseconds: 0,
            }))),
            ErrorCode::Overflow
        );
    }

    #[test]
    fn p2_output_stream_completion_preserves_guest_error_code() {
        let error = p2_output_stream_error(ErrorCode::Quota.into());
        let wasmtime_wasi::p2::StreamError::LastOperationFailed(error) = error else {
            panic!("quota completion did not become a failed stream operation");
        };

        assert_eq!(p2_output_stream_error_code(&error), Some(ErrorCode::Quota));
    }

    #[test]
    fn p2_link_maps_cross_project_and_native_cross_device_without_trapping() {
        assert_eq!(
            error_code::<()>(Err(p2_link_access_error(
                agent_filesystem::AccessError::WrongGeneration
            ))),
            ErrorCode::CrossDevice
        );
        assert_eq!(
            error_code::<()>(Err(p2_link_access_error(
                agent_filesystem::AccessError::NotPermitted
            ))),
            ErrorCode::NotPermitted
        );
        #[cfg(unix)]
        {
            let sandbox = FilesystemStorageError::io(
                "hard link",
                std::path::Path::new("<p2-namespace-test>"),
                std::io::Error::from_raw_os_error(libc::EXDEV),
            );
            assert_eq!(
                error_code::<()>(Err(p2_agent_error(AgentFilesystemError::Sandbox(sandbox)))),
                ErrorCode::CrossDevice
            );
        }
    }
}
