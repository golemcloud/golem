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

use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::durable_host::filesystem::types::calculate_metadata_hash_parts;
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call,
    observe_function_call_store, run_read_access, wasi_filesystem_view,
};
use crate::durable_host::tail_work::TailActivity;
#[cfg(test)]
use crate::services::agent_filesystem::state_postcondition;
use crate::services::agent_filesystem::{
    AdmittedFilesystemWrite, AgentFilesystemMutationError, AgentFilesystemRuntime,
    AgentFilesystemWriteMode, AgentFilesystemWriter, FILESYSTEM_MUTATION_MAX_ATTEMPTS,
    FILESYSTEM_MUTATION_RETRY_TIMEOUT, MutationDecision, MutationEffect, MutationFailure,
    MutationOperation, MutationPostcondition, NativeMutationGuestError, NativeOpenOptions,
    NativeOpenResult, PathObjectType, RequestedTime, create_directory as native_create_directory,
    create_directory_postcondition, descriptor_state, descriptor_times,
    hard_link as native_hard_link, link_postcondition, open as native_open, open_postcondition,
    path_state, path_state_with_follow, path_times, remove_directory as native_remove_directory,
    remove_postcondition, rename as native_rename, rename_postcondition,
    resize_file as native_resize_file, resize_postcondition, run_blocking_filesystem_mutation,
    set_descriptor_times as native_set_descriptor_times, set_path_times as native_set_path_times,
    symlink as native_symlink, symlink_postcondition, symlink_state,
    sync_descriptor as native_sync_descriptor, times_postcondition,
    unlink_file as native_unlink_file, validate_descriptor_times, validate_directory_mutation,
    validate_open, validate_resize, validate_two_directory_mutation,
};
use crate::workerctx::WorkerCtx;
use bytes::Bytes;
use golem_common::model::oplog::host_functions::{
    P3FilesystemTypesDescriptorStat, P3FilesystemTypesDescriptorStatAt,
};
use golem_common::model::oplog::types::{SerializableFileTimes, SerializableP3FileSystemError};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestFileSystemPath, HostResponseP3FileSystemStat,
};
use wasmtime::AsContextMut;
use wasmtime::StoreContextMut;
use wasmtime::component::{
    Access, Accessor, AccessorTask, FutureReader, Resource, Source, StreamConsumer, StreamReader,
    StreamResult,
};
use wasmtime_wasi::filesystem::{Descriptor, Dir, File, WasiFilesystem, WasiFilesystemView};
use wasmtime_wasi::p3::bindings::filesystem::{preopens, types};
use wasmtime_wasi::p3::filesystem::{FilesystemError, FilesystemResult};
use wasmtime_wasi::runtime::spawn_blocking;
use wasmtime_wasi::{DirPerms, FilePerms, ResourceTableError};

struct FilesystemWriteChunk {
    result_tx: tokio::sync::oneshot::Sender<(usize, FilesystemWriteResult)>,
    admitted: AdmittedFilesystemWrite,
    cancellation: tokio_util::sync::CancellationToken,
}

#[derive(Clone, Debug)]
enum FilesystemWriteFailure {
    Guest(types::ErrorCode),
    Trap(String),
}

type FilesystemWriteResult = Result<(), FilesystemWriteFailure>;

enum P3MutationAction {
    Retry,
    Success,
    Error(FilesystemError),
    Trap,
}

struct P3MutationAdapter {
    runtime: AgentFilesystemRuntime,
    operation: MutationOperation,
    started: Instant,
    attempts: usize,
}

impl P3MutationAdapter {
    fn new(runtime: AgentFilesystemRuntime) -> Self {
        Self::for_operation(runtime, MutationOperation::Metadata)
    }

    fn for_operation(runtime: AgentFilesystemRuntime, operation: MutationOperation) -> Self {
        Self {
            runtime,
            operation,
            started: Instant::now(),
            attempts: 0,
        }
    }

    fn begin_attempt(&mut self) {
        self.attempts += 1;
    }

    #[cfg(test)]
    async fn failure(
        &self,
        error: FilesystemError,
        postcondition: MutationPostcondition,
        retry_safe: bool,
    ) -> P3MutationAction {
        let Some(guest) = error.downcast_ref().cloned() else {
            let _ = self
                .runtime
                .classify_mutation_failure::<types::ErrorCode>(
                    MutationFailure::Infrastructure(std::io::Error::other(error.to_string())),
                    MutationEffect::Unknown,
                )
                .await;
            return P3MutationAction::Trap;
        };
        let effect = match (postcondition, &guest) {
            (MutationPostcondition::Satisfied, _) => MutationEffect::DesiredPostconditionSatisfied,
            (MutationPostcondition::NoEffect, _) => MutationEffect::ProvenNoEffect,
            (MutationPostcondition::Unknown, _) => MutationEffect::Unknown,
        };
        let failure = match guest.clone() {
            types::ErrorCode::Quota => MutationFailure::StorageExhaustion {
                guest: guest.clone(),
                quota_hint: true,
            },
            types::ErrorCode::InsufficientSpace => MutationFailure::StorageExhaustion {
                guest: guest.clone(),
                quota_hint: false,
            },
            types::ErrorCode::Busy
            | types::ErrorCode::Interrupted
            | types::ErrorCode::InProgress
            | types::ErrorCode::Already => MutationFailure::TransientGuest(guest.clone()),
            types::ErrorCode::Access | types::ErrorCode::NotPermitted => {
                MutationFailure::AccessGuest(guest.clone())
            }
            types::ErrorCode::Io => MutationFailure::UnclassifiedGuest(guest.clone()),
            _ => MutationFailure::Guest(guest.clone()),
        };
        match self
            .runtime
            .classify_mutation_failure_for(self.operation, failure, effect)
            .await
        {
            MutationDecision::PreserveGuest(error) => P3MutationAction::Error(error.into()),
            MutationDecision::Quota => P3MutationAction::Error(types::ErrorCode::Quota.into()),
            MutationDecision::InsufficientSpace => {
                P3MutationAction::Error(types::ErrorCode::InsufficientSpace.into())
            }
            MutationDecision::PhysicalPressure
                if retry_safe
                    && postcondition == MutationPostcondition::NoEffect
                    && self.attempts < FILESYSTEM_MUTATION_MAX_ATTEMPTS
                    && self.started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT =>
            {
                if self
                    .runtime
                    .recover_physical_pressure(
                        self.operation,
                        self.started + FILESYSTEM_MUTATION_RETRY_TIMEOUT,
                    )
                    .await
                    && self.started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT
                {
                    P3MutationAction::Retry
                } else {
                    P3MutationAction::Error(types::ErrorCode::InsufficientSpace.into())
                }
            }
            MutationDecision::PhysicalPressure => {
                P3MutationAction::Error(types::ErrorCode::InsufficientSpace.into())
            }
            MutationDecision::BoundedRetry
                if retry_safe
                    && postcondition == MutationPostcondition::NoEffect
                    && self.attempts < FILESYSTEM_MUTATION_MAX_ATTEMPTS
                    && self.started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT =>
            {
                P3MutationAction::Retry
            }
            MutationDecision::BoundedRetry | MutationDecision::PreserveRaw => {
                P3MutationAction::Error(guest.into())
            }
            MutationDecision::Success => P3MutationAction::Success,
            MutationDecision::Invalidate => P3MutationAction::Trap,
        }
    }

    async fn io_failure(
        &self,
        error: std::io::Error,
        postcondition: MutationPostcondition,
        retry_safe: bool,
    ) -> P3MutationAction {
        let raw_os_error = error.raw_os_error();
        let error_kind = error.kind();
        let error_message = error.to_string();
        let effect = match postcondition {
            MutationPostcondition::Satisfied => MutationEffect::DesiredPostconditionSatisfied,
            MutationPostcondition::NoEffect => MutationEffect::ProvenNoEffect,
            MutationPostcondition::Unknown => MutationEffect::Unknown,
        };
        match self
            .runtime
            .classify_mutation_failure_for::<types::ErrorCode>(
                self.operation,
                MutationFailure::Io(error),
                effect,
            )
            .await
        {
            MutationDecision::PreserveGuest(error) => P3MutationAction::Error(error.into()),
            MutationDecision::Quota => P3MutationAction::Error(types::ErrorCode::Quota.into()),
            MutationDecision::InsufficientSpace => {
                P3MutationAction::Error(types::ErrorCode::InsufficientSpace.into())
            }
            MutationDecision::PhysicalPressure
                if retry_safe
                    && postcondition == MutationPostcondition::NoEffect
                    && self.attempts < FILESYSTEM_MUTATION_MAX_ATTEMPTS
                    && self.started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT =>
            {
                if self
                    .runtime
                    .recover_physical_pressure(
                        self.operation,
                        self.started + FILESYSTEM_MUTATION_RETRY_TIMEOUT,
                    )
                    .await
                    && self.started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT
                {
                    P3MutationAction::Retry
                } else {
                    P3MutationAction::Error(types::ErrorCode::InsufficientSpace.into())
                }
            }
            MutationDecision::PhysicalPressure => {
                P3MutationAction::Error(types::ErrorCode::InsufficientSpace.into())
            }
            MutationDecision::BoundedRetry
                if retry_safe
                    && postcondition == MutationPostcondition::NoEffect
                    && self.attempts < FILESYSTEM_MUTATION_MAX_ATTEMPTS
                    && self.started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT =>
            {
                P3MutationAction::Retry
            }
            MutationDecision::BoundedRetry | MutationDecision::PreserveRaw => {
                let error = raw_os_error.map_or_else(
                    || std::io::Error::new(error_kind, error_message),
                    std::io::Error::from_raw_os_error,
                );
                P3MutationAction::Error(types::ErrorCode::from(&error).into())
            }
            MutationDecision::Success => P3MutationAction::Success,
            MutationDecision::Invalidate => P3MutationAction::Trap,
        }
    }
}

fn p3_mutation_action_result(action: P3MutationAction) -> FilesystemResult<()> {
    match action {
        P3MutationAction::Success => Ok(()),
        P3MutationAction::Error(error) => Err(error),
        P3MutationAction::Trap => Err(FilesystemError::trap(wasmtime::Error::msg(
            "agent filesystem mutation invalidated the runtime",
        ))),
        P3MutationAction::Retry => unreachable!("retry must be handled by the operation loop"),
    }
}

struct FilesystemWriteConsumer {
    chunks_tx: Option<tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>>,
    pending_chunk: Option<PendingFilesystemWriteChunk>,
    pending_invalidation: Option<Pin<Box<dyn Future<Output = ()> + Send>>>,
    filesystem_runtime: crate::services::agent_filesystem::AgentFilesystemRuntime,
    writer: AgentFilesystemWriter,
}

struct PendingFilesystemWriteChunk {
    result_rx: tokio::sync::oneshot::Receiver<(usize, FilesystemWriteResult)>,
    cancellation: tokio_util::sync::CancellationToken,
}

impl FilesystemWriteConsumer {
    fn new(
        chunks_tx: tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>,
        filesystem_runtime: crate::services::agent_filesystem::AgentFilesystemRuntime,
        writer: AgentFilesystemWriter,
    ) -> Self {
        Self {
            chunks_tx: Some(chunks_tx),
            pending_chunk: None,
            pending_invalidation: None,
            filesystem_runtime,
            writer,
        }
    }

    fn cancel(&mut self) {
        if let Some(pending) = &self.pending_chunk {
            pending.cancellation.cancel();
        }
        self.chunks_tx.take();
    }

    fn poll_pending_result(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<wasmtime::Result<Option<(usize, FilesystemWriteResult)>>> {
        if let Some(invalidation) = &mut self.pending_invalidation {
            return match invalidation.as_mut().poll(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(()) => {
                    self.pending_invalidation = None;
                    Poll::Ready(Err(wasmtime::Error::msg(
                        "filesystem write task dropped before reporting its effect",
                    )))
                }
            };
        }
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
                let runtime = self.filesystem_runtime.clone();
                self.pending_invalidation = Some(Box::pin(async move {
                    runtime.invalidate_runtime().await;
                }));
                self.poll_pending_result(cx)
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

        if self.pending_invalidation.is_some() {
            return match self.poll_pending_result(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                Poll::Ready(Ok(_)) => unreachable!("pending invalidation disappeared"),
            };
        }

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
                    Poll::Ready(Ok(Some((written, Err(FilesystemWriteFailure::Trap(error)))))) => {
                        self.chunks_tx.take();
                        src.mark_read(written);
                        return Poll::Ready(Err(wasmtime::Error::msg(error)));
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
            let admitted = self
                .writer
                .admit(Bytes::copy_from_slice(bytes))
                .map_err(|_| {
                    wasmtime::Error::msg("agent filesystem mutation invalidated the runtime")
                })?;
            let cancellation = tokio_util::sync::CancellationToken::new();
            chunks_tx
                .send(FilesystemWriteChunk {
                    result_tx,
                    admitted,
                    cancellation: cancellation.clone(),
                })
                .map_err(|_| wasmtime::Error::msg("filesystem write task dropped"))?;
            self.pending_chunk = Some(PendingFilesystemWriteChunk {
                result_rx,
                cancellation,
            });
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
    filesystem_runtime: crate::services::agent_filesystem::AgentFilesystemRuntime,
    activity: TailActivity,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> FilesystemWriteTask<Ctx> {
    fn new(
        chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
        filesystem_runtime: crate::services::agent_filesystem::AgentFilesystemRuntime,
        activity: TailActivity,
    ) -> Self {
        Self {
            chunks_rx,
            result_tx,
            filesystem_runtime,
            activity,
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
            filesystem_runtime,
            activity,
            _phantom,
        } = self;
        let result =
            run_streaming_filesystem_write(&mut chunks_rx, &filesystem_runtime, &activity).await;
        if !result_tx.is_closed() {
            let _ = result_tx.send(result);
        }
        Ok(())
    }
}

fn descriptor_path_from_accessor<Ctx: WorkerCtx, U>(
    store: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<PathBuf>
where
    U: 'static,
{
    store.with(|mut access| {
        let mut filesystem = Access::<U, WasiFilesystem>::new(
            access.as_context_mut(),
            wasi_filesystem_view::<Ctx, U>,
        );
        let descriptor = filesystem.get().table.get(fd)?;
        Ok(match descriptor {
            Descriptor::File(file) => file.path.clone(),
            Descriptor::Dir(dir) => dir.path.clone(),
        })
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
        let mut filesystem = Access::<U, WasiFilesystem>::new(
            access.as_context_mut(),
            wasi_filesystem_view::<Ctx, U>,
        );
        let descriptor = filesystem.get().table.get(fd)?;
        Ok(match descriptor {
            Descriptor::File(file) => file.path.join(path),
            Descriptor::Dir(dir) => dir.path.join(path),
        })
    })
}

fn file_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<File>
where
    U: 'static,
{
    let mut filesystem =
        Access::<U, WasiFilesystem>::new(store.as_context_mut(), wasi_filesystem_view::<Ctx, U>);
    match filesystem.get().table.get(fd)? {
        Descriptor::File(file) => Ok(file.clone()),
        Descriptor::Dir(_) => Err(FilesystemError::from(types::ErrorCode::BadDescriptor).into()),
    }
}

fn descriptor_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<Descriptor>
where
    U: 'static,
{
    let mut filesystem =
        Access::<U, WasiFilesystem>::new(store.as_context_mut(), wasi_filesystem_view::<Ctx, U>);
    Ok(filesystem.get().table.get(fd)?.clone())
}

fn directory_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> FilesystemResult<Dir>
where
    U: 'static,
{
    match descriptor_from_access::<Ctx, U>(store, fd).map_err(FilesystemError::trap)? {
        Descriptor::Dir(directory) => Ok(directory),
        Descriptor::File(_) => Err(types::ErrorCode::NotDirectory.into()),
    }
}

fn dir_result_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<Result<Dir, types::ErrorCode>>
where
    U: 'static,
{
    let mut filesystem =
        Access::<U, WasiFilesystem>::new(store.as_context_mut(), wasi_filesystem_view::<Ctx, U>);
    Ok(match filesystem.get().table.get(fd)? {
        Descriptor::Dir(dir) => Ok(dir.clone()),
        Descriptor::File(_) => Err(types::ErrorCode::NotDirectory),
    })
}

/// Fails with `not-permitted` when the descriptor refers to a file marked
/// read-only in the worker's initial file system, matching the WASI P2
/// `fail_if_read_only` enforcement. Directories always pass.
fn fail_if_read_only_from_accessor<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> FilesystemResult<()>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let read_only = accessor
        .with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).check_if_file_is_readonly(fd)
        })
        .map_err(|error| FilesystemError::trap(wasmtime::Error::from(error)))?;
    if read_only {
        Err(types::ErrorCode::NotPermitted.into())
    } else {
        Ok(())
    }
}

fn fail_if_read_only_path_from_accessor<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
    path: &str,
    include_descendants: bool,
    follow_final_symlink: bool,
) -> FilesystemResult<()>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let read_only = accessor
        .with(|mut access| {
            let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
            let target = ctx.descriptor_path(fd)?.join(path);
            Ok::<_, ResourceTableError>(if include_descendants {
                ctx.filesystem_runtime
                    .contains_read_only_path(&target, follow_final_symlink)
            } else {
                ctx.filesystem_runtime
                    .is_read_only_path(&target, follow_final_symlink)
            })
        })
        .map_err(|error| FilesystemError::trap(wasmtime::Error::from(error)))?;
    if read_only {
        Err(types::ErrorCode::NotPermitted.into())
    } else {
        Ok(())
    }
}

async fn begin_filesystem_effect<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
) -> FilesystemResult<crate::services::agent_filesystem::AgentFilesystemEffectLease>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let runtime = accessor
        .with(|mut access| durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime());
    runtime.begin_effect().await.map_err(FilesystemError::trap)
}

async fn begin_filesystem_path_effect<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
) -> FilesystemResult<crate::services::agent_filesystem::AgentFilesystemEffectLease>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let runtime = accessor
        .with(|mut access| durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime());
    runtime
        .begin_path_effect()
        .await
        .map_err(FilesystemError::trap)
}

async fn begin_filesystem_update_effect<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
) -> FilesystemResult<crate::services::agent_filesystem::AgentFilesystemUpdateEffectLease>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let runtime = accessor
        .with(|mut access| durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime());
    runtime
        .begin_update_effect()
        .await
        .map_err(FilesystemError::trap)
}

async fn p3_initial_probe<T>(
    runtime: &AgentFilesystemRuntime,
    result: Result<T, std::io::Error>,
) -> FilesystemResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let guest = types::ErrorCode::from(&error);
            match runtime
                .classify_mutation_failure_for(
                    MutationOperation::Metadata,
                    MutationFailure::<types::ErrorCode>::Io(error),
                    MutationEffect::ProvenNoEffect,
                )
                .await
            {
                MutationDecision::Quota => Err(types::ErrorCode::Quota.into()),
                MutationDecision::InsufficientSpace | MutationDecision::PhysicalPressure => {
                    Err(types::ErrorCode::InsufficientSpace.into())
                }
                MutationDecision::BoundedRetry | MutationDecision::PreserveRaw => Err(guest.into()),
                MutationDecision::PreserveGuest(error) => Err(error.into()),
                MutationDecision::Success => unreachable!("failed probe cannot be satisfied"),
                MutationDecision::Invalidate => Err(FilesystemError::trap(wasmtime::Error::msg(
                    "agent filesystem mutation precondition probe invalidated the runtime",
                ))),
            }
        }
    }
}

async fn p3_finish_native_mutation(
    adapter: &P3MutationAdapter,
    error: std::io::Error,
    postcondition: MutationPostcondition,
    retry_safe: bool,
) -> FilesystemResult<bool> {
    match adapter.io_failure(error, postcondition, retry_safe).await {
        P3MutationAction::Retry => Ok(true),
        action => p3_mutation_action_result(action).map(|()| false),
    }
}

fn p3_native_guest(error: NativeMutationGuestError) -> FilesystemError {
    match error {
        NativeMutationGuestError::Invalid => types::ErrorCode::Invalid.into(),
        NativeMutationGuestError::NotDirectory => types::ErrorCode::NotDirectory.into(),
        NativeMutationGuestError::NotPermitted => types::ErrorCode::NotPermitted.into(),
        NativeMutationGuestError::Unsupported => types::ErrorCode::Unsupported.into(),
    }
}

fn p3_requested_time(requested: types::NewTimestamp) -> RequestedTime {
    match requested {
        types::NewTimestamp::NoChange => RequestedTime::NoChange,
        types::NewTimestamp::Now => RequestedTime::Now,
        types::NewTimestamp::Timestamp(timestamp) => RequestedTime::Timestamp {
            seconds: i128::from(timestamp.seconds),
            nanoseconds: timestamp.nanoseconds,
        },
    }
}

fn p3_native_time(
    requested: types::NewTimestamp,
) -> Result<Option<std::time::SystemTime>, FilesystemError> {
    match requested {
        types::NewTimestamp::NoChange => Ok(None),
        types::NewTimestamp::Now => Ok(Some(std::time::SystemTime::now())),
        types::NewTimestamp::Timestamp(timestamp) => {
            let time = if let Ok(seconds) = timestamp.seconds.try_into() {
                std::time::SystemTime::UNIX_EPOCH
                    .checked_add(Duration::new(seconds, timestamp.nanoseconds))
            } else {
                std::time::SystemTime::UNIX_EPOCH.checked_sub(Duration::new(
                    timestamp.seconds.unsigned_abs(),
                    timestamp.nanoseconds,
                ))
            };
            time.map(Some)
                .ok_or_else(|| types::ErrorCode::Overflow.into())
        }
    }
}

fn push_descriptor<Ctx: WorkerCtx, U: 'static>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    descriptor: Descriptor,
) -> FilesystemResult<Resource<Descriptor>> {
    accessor
        .with(|mut access| {
            let mut filesystem = Access::<U, WasiFilesystem>::new(
                access.as_context_mut(),
                wasi_filesystem_view::<Ctx, U>,
            );
            filesystem.get().table.push(descriptor)
        })
        .map_err(FilesystemError::trap)
}

fn write_validation_error_from_access<Ctx: WorkerCtx, U>(
    store: &mut Access<'_, U, DurableP3<Ctx>>,
    fd: &Resource<Descriptor>,
) -> wasmtime::Result<Option<types::ErrorCode>>
where
    U: 'static,
{
    if durable_worker_ctx::<Ctx, U>(store.data_mut()).check_if_file_is_readonly(fd)? {
        return Ok(Some(types::ErrorCode::NotPermitted));
    }

    let mut filesystem =
        Access::<U, WasiFilesystem>::new(store.as_context_mut(), wasi_filesystem_view::<Ctx, U>);
    Ok(match filesystem.get().table.get(fd)? {
        Descriptor::File(file) if !file.perms.contains(FilePerms::WRITE) => {
            Some(types::ErrorCode::NotPermitted)
        }
        Descriptor::File(_) => None,
        Descriptor::Dir(_) => Some(types::ErrorCode::BadDescriptor),
    })
}

fn map_directory_entry(
    entry: std::io::Result<cap_std::fs::DirEntry>,
) -> Result<Option<types::DirectoryEntry>, types::ErrorCode> {
    match entry {
        Ok(entry) => {
            let meta = entry.metadata()?;
            let Ok(name) = entry.file_name().into_string() else {
                return Err(types::ErrorCode::IllegalByteSequence);
            };
            Ok(Some(types::DirectoryEntry {
                type_: meta.file_type().into(),
                name,
            }))
        }
        Err(error) => {
            // On Windows, filter out files like `C:\DumpStack.log.tmp` which we
            // can't get full metadata for, matching the upstream wasmtime-wasi
            // behavior instead of failing the entire directory listing.
            #[cfg(windows)]
            {
                use windows_sys::Win32::Foundation::{
                    ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION,
                };
                if error.raw_os_error() == Some(ERROR_SHARING_VIOLATION as i32)
                    || error.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32)
                {
                    return Ok(None);
                }
            }
            Err(error.into())
        }
    }
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
    let filesystem = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
    match <WasiFilesystem as types::HostDescriptorWithStore<U>>::stat(&filesystem, fd).await {
        Ok(mut stat) => {
            stat.status_change_timestamp = None;
            Ok(stat)
        }
        Err(error) => Err(serialize_stat_error(&error)),
    }
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
    let filesystem = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
    match <WasiFilesystem as types::HostDescriptorWithStore<U>>::stat_at(
        &filesystem,
        fd,
        path_flags,
        path,
    )
    .await
    {
        Ok(mut stat) => {
            stat.status_change_timestamp = None;
            Ok(stat)
        }
        Err(error) => Err(serialize_stat_error(&error)),
    }
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
    filesystem_runtime: &crate::services::agent_filesystem::AgentFilesystemRuntime,
    activity: &TailActivity,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    let mut result: FilesystemWriteResult = Ok(());
    // Safe park: each chunk is guest-produced stream data.
    while let Some(chunk) = activity.park(chunks_rx.recv()).await {
        let cancellation = chunk.cancellation.clone();
        let written_len = if result.is_ok() {
            let (written, write_result) =
                p3_write_result(chunk.admitted.execute(chunk.cancellation).await);
            result = write_result;
            written
        } else {
            0
        };

        deliver_filesystem_write_result(
            filesystem_runtime,
            chunk.result_tx,
            cancellation,
            written_len,
            result.clone(),
        )
        .await?;
    }

    filesystem_write_result_to_wasi(result)
}

fn p3_write_result(
    result: Result<u64, AgentFilesystemMutationError>,
) -> (usize, FilesystemWriteResult) {
    match result {
        Ok(completed) | Err(AgentFilesystemMutationError::Cancelled { completed }) => (
            usize::try_from(completed).expect("completed write length must fit usize"),
            Ok(()),
        ),
        Err(AgentFilesystemMutationError::Native { error, completed }) => (
            usize::try_from(completed).expect("completed write length must fit usize"),
            Err(FilesystemWriteFailure::Guest(types::ErrorCode::from(
                &error.into_io_error(),
            ))),
        ),
        Err(AgentFilesystemMutationError::QuotaExhausted { completed, .. }) => (
            usize::try_from(completed).expect("completed write length must fit usize"),
            Err(FilesystemWriteFailure::Guest(types::ErrorCode::Quota)),
        ),
        Err(AgentFilesystemMutationError::InsufficientSpace { completed, .. }) => (
            usize::try_from(completed).expect("completed write length must fit usize"),
            Err(FilesystemWriteFailure::Guest(
                types::ErrorCode::InsufficientSpace,
            )),
        ),
        Err(AgentFilesystemMutationError::RuntimeInvalidated { completed, .. }) => (
            completed
                .and_then(|completed| usize::try_from(completed).ok())
                .unwrap_or(0),
            Err(FilesystemWriteFailure::Trap(
                "agent filesystem mutation invalidated the runtime".to_string(),
            )),
        ),
    }
}

async fn deliver_filesystem_write_result(
    filesystem_runtime: &AgentFilesystemRuntime,
    result_tx: tokio::sync::oneshot::Sender<(usize, FilesystemWriteResult)>,
    cancellation: tokio_util::sync::CancellationToken,
    written: usize,
    result: FilesystemWriteResult,
) -> wasmtime::Result<()> {
    if result_tx.send((written, result)).is_err() && !cancellation.is_cancelled() {
        filesystem_runtime.invalidate_runtime().await;
        return Err(wasmtime::Error::msg(
            "filesystem write progress could not be delivered",
        ));
    }
    Ok(())
}

fn filesystem_write_result_to_wasi(
    result: FilesystemWriteResult,
) -> wasmtime::Result<Result<(), types::ErrorCode>> {
    match result {
        Ok(()) => Ok(Ok(())),
        Err(FilesystemWriteFailure::Guest(error)) => Ok(Err(error)),
        Err(FilesystemWriteFailure::Trap(error)) => Err(wasmtime::Error::msg(error)),
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
        types::HostDescriptor::drop(&mut WasiFilesystemView::filesystem(self.0), fd)
    }
}

impl<Ctx: WorkerCtx> preopens::Host for DurableP3View<'_, Ctx> {
    fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        observe_function_call(&*self.0, "filesystem::preopens", "get-directories");
        preopens::Host::get_directories(&mut WasiFilesystemView::filesystem(self.0))
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostDescriptorWithStore<U> for DurableP3<Ctx> {
    async fn read_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "read-via-stream",
            )
        });
        // Reads are not recorded in the oplog. On replay the guest re-reads the
        // reconstructed worker filesystem, so we simply delegate to the
        // underlying host stream, matching WASI P2.
        let store = accessor.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::read_via_stream(&store, fd, offset)
            .await
    }

    async fn write_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
        offset: types::Filesize,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "write-via-stream",
            )
        });
        let write_error = accessor
            .with(|mut store| write_validation_error_from_access::<Ctx, U>(&mut store, &fd))?;
        if let Some(error) = write_error {
            let mut data = data;
            accessor.with(|mut store| data.close(&mut store))?;
            return accessor.with(|mut store| {
                FutureReader::new(&mut store, async move {
                    Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(error))
                })
            });
        }

        let file = accessor.with(|mut store| file_from_access::<Ctx, U>(&mut store, &fd))?;
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        let filesystem_runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let writer = filesystem_runtime
            .mutations()
            .writer(file, AgentFilesystemWriteMode::Position(offset));
        accessor.with(|mut store| {
            data.pipe(
                &mut store,
                FilesystemWriteConsumer::new(chunks_tx, filesystem_runtime.clone(), writer),
            )
        })?;

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        accessor.with(|mut store| {
            let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
                .tail_work_tracker()
                .activity();
            store.spawn(FilesystemWriteTask::<Ctx>::new(
                chunks_rx,
                result_tx,
                filesystem_runtime,
                activity,
            ));

            FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))
        })
    }

    async fn append_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "append-via-stream",
            )
        });
        let write_error = accessor
            .with(|mut store| write_validation_error_from_access::<Ctx, U>(&mut store, &fd))?;
        if let Some(error) = write_error {
            let mut data = data;
            accessor.with(|mut store| data.close(&mut store))?;
            return accessor.with(|mut store| {
                FutureReader::new(&mut store, async move {
                    Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(error))
                })
            });
        }

        let file = accessor.with(|mut store| file_from_access::<Ctx, U>(&mut store, &fd))?;
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        let filesystem_runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let writer = filesystem_runtime
            .mutations()
            .writer(file, AgentFilesystemWriteMode::Append);
        accessor.with(|mut store| {
            data.pipe(
                &mut store,
                FilesystemWriteConsumer::new(chunks_tx, filesystem_runtime.clone(), writer),
            )
        })?;

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        accessor.with(|mut store| {
            let activity = durable_worker_ctx::<Ctx, U>(store.data_mut())
                .tail_work_tracker()
                .activity();
            store.spawn(FilesystemWriteTask::<Ctx>::new(
                chunks_rx,
                result_tx,
                filesystem_runtime,
                activity,
            ));

            FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))
        })
    }

    async fn advise(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
        length: types::Filesize,
        advice: types::Advice,
    ) -> FilesystemResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "advise",
            )
        });
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::advise(
            &store, fd, offset, length, advice,
        )
        .await
    }

    async fn sync_data(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "sync-data",
            )
        });
        let effect = Arc::new(begin_filesystem_effect::<Ctx, U>(store).await?);
        let descriptor = store
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let mut adapter = P3MutationAdapter::new(store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        }));
        adapter.begin_attempt();
        match run_blocking_filesystem_mutation(effect, move || {
            native_sync_descriptor(&descriptor, true)
        })
        .await
        {
            Ok(()) => Ok(()),
            Err(error) => p3_mutation_action_result(
                adapter
                    .io_failure(error, MutationPostcondition::Unknown, false)
                    .await,
            ),
        }
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
        // Files marked read-only in the worker's initial file system must not
        // report the write flag, matching the WASI P2 behavior.
        let read_only = accessor
            .with(|mut access| {
                durable_worker_ctx::<Ctx, U>(access.data_mut()).check_if_file_is_readonly(&fd)
            })
            .map_err(|error| FilesystemError::trap(wasmtime::Error::from(error)))?;
        let store = accessor.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        let mut flags =
            <WasiFilesystem as types::HostDescriptorWithStore<U>>::get_flags(&store, fd).await?;

        if read_only {
            flags &= !types::DescriptorFlags::WRITE;
        }

        Ok(flags)
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
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::get_type(&store, fd).await
    }

    async fn set_size(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        size: types::Filesize,
    ) -> FilesystemResult<()> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-size",
            )
        });
        let effect = Arc::new(begin_filesystem_update_effect::<Ctx, U>(accessor).await?);
        fail_if_read_only_from_accessor::<Ctx, U>(accessor, &fd)?;
        let descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let file = match &descriptor {
            Descriptor::File(file) => file.clone(),
            Descriptor::Dir(_) => return Err(types::ErrorCode::BadDescriptor.into()),
        };
        validate_resize(&file).map_err(p3_native_guest)?;
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let before = p3_initial_probe(&runtime, descriptor_state(&descriptor).await).await?;
        let mut adapter = P3MutationAdapter::for_operation(runtime, MutationOperation::Resize);
        loop {
            adapter.begin_attempt();
            let file = file.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || native_resize_file(&file, size))
                .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition =
                        resize_postcondition(before, descriptor_state(&descriptor).await, size);
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn set_times(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let effect = Arc::new(begin_filesystem_update_effect::<Ctx, U>(accessor).await?);
        fail_if_read_only_from_accessor::<Ctx, U>(accessor, &fd)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-times",
            )
        });
        let descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        validate_descriptor_times(&descriptor).map_err(p3_native_guest)?;
        let accessed = p3_native_time(data_access_timestamp)?;
        let modified = p3_native_time(data_modification_timestamp)?;
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let before = p3_initial_probe(&runtime, descriptor_times(&descriptor).await).await?;
        let mut adapter = P3MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let descriptor_for_attempt = descriptor.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                native_set_descriptor_times(&descriptor_for_attempt, accessed, modified)
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = times_postcondition(
                        descriptor_times(&descriptor).await,
                        before,
                        p3_requested_time(data_access_timestamp),
                        p3_requested_time(data_modification_timestamp),
                        false,
                    );
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn read_directory(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> wasmtime::Result<(
        StreamReader<types::DirectoryEntry>,
        FutureReader<Result<(), types::ErrorCode>>,
    )> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "read-directory",
            )
        });
        // The directory listing is snapshotted and sorted by name before the
        // stream is returned. This matches WASI P2 and guarantees deterministic
        // ordering across live execution and replay, regardless of OS iteration
        // order or concurrent directory mutations after this call returns. The
        // entries are not recorded in the oplog: on replay the guest re-lists
        // the reconstructed worker filesystem.
        let dir_result =
            accessor.with(|mut store| dir_result_from_access::<Ctx, U>(&mut store, &fd))?;
        let (entries, result) = match dir_result {
            Ok(dir) => {
                if !dir.perms.contains(DirPerms::READ) {
                    (Vec::new(), Err(types::ErrorCode::NotPermitted))
                } else {
                    let dir = Arc::clone(&dir.dir);
                    let collected = spawn_blocking(move || {
                        let entries = dir.entries()?;
                        let mut sorted = Vec::new();
                        for entry in entries {
                            if let Some(entry) = map_directory_entry(entry)? {
                                sorted.push(entry);
                            }
                        }
                        sorted.sort_by_key(|entry| entry.name.clone());
                        Ok::<Vec<types::DirectoryEntry>, types::ErrorCode>(sorted)
                    })
                    .await;
                    match collected {
                        Ok(entries) => (entries, Ok(())),
                        Err(error) => (Vec::new(), Err(error)),
                    }
                }
            }
            Err(error) => (Vec::new(), Err(error)),
        };

        accessor.with(|mut store| {
            let stream = StreamReader::new(&mut store, entries)?;
            let future = FutureReader::new(&mut store, async move {
                Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(result)
            })?;
            Ok::<_, wasmtime::Error>((stream, future))
        })
    }

    async fn sync(store: &Accessor<U, Self>, fd: Resource<Descriptor>) -> FilesystemResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "sync",
            )
        });
        let effect = Arc::new(begin_filesystem_effect::<Ctx, U>(store).await?);
        let descriptor = store
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let mut adapter = P3MutationAdapter::new(store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        }));
        adapter.begin_attempt();
        match run_blocking_filesystem_mutation(effect, move || {
            native_sync_descriptor(&descriptor, false)
        })
        .await
        {
            Ok(()) => Ok(()),
            Err(error) => p3_mutation_action_result(
                adapter
                    .io_failure(error, MutationPostcondition::Unknown, false)
                    .await,
            ),
        }
    }

    async fn create_directory_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "create-directory-at",
            )
        });
        let effect = Arc::new(begin_filesystem_path_effect::<Ctx, U>(store).await?);
        fail_if_read_only_path_from_accessor::<Ctx, U>(store, &fd, &path, false, false)?;
        let directory =
            store.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &fd))?;
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let runtime = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let before = p3_initial_probe(&runtime, path_state(&directory, &path).await).await?;
        let mut adapter = P3MutationAdapter::for_operation(runtime, MutationOperation::Create);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                native_create_directory(&directory_for_attempt, &path_for_attempt)
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition =
                        create_directory_postcondition(before, path_state(&directory, &path).await);
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn stat(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorStat> {
        let path =
            descriptor_path_from_accessor::<Ctx, U>(store, &fd).map_err(FilesystemError::trap)?;
        let fd_rep = fd.rep();
        let live_stat = Arc::new(Mutex::new(None));
        let live_stat_for_call = Arc::clone(&live_stat);

        let response = run_read_access::<_, _, Ctx, P3FilesystemTypesDescriptorStat, _, _>(
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
        let full_path = descriptor_path_at_from_accessor::<Ctx, U>(store, &fd, &path)
            .map_err(FilesystemError::trap)?;
        let fd_rep = fd.rep();
        let live_stat = Arc::new(Mutex::new(None));
        let live_stat_for_call = Arc::clone(&live_stat);
        let live_path = path.clone();
        let response = run_read_access::<_, _, Ctx, P3FilesystemTypesDescriptorStatAt, _, _>(
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
        let effect = Arc::new(begin_filesystem_update_effect::<Ctx, U>(accessor).await?);
        fail_if_read_only_from_accessor::<Ctx, U>(accessor, &fd)?;
        fail_if_read_only_path_from_accessor::<Ctx, U>(
            accessor,
            &fd,
            &path,
            false,
            path_flags.contains(types::PathFlags::SYMLINK_FOLLOW),
        )?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-times-at",
            )
        });
        let directory =
            accessor.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &fd))?;
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let follow = path_flags.contains(types::PathFlags::SYMLINK_FOLLOW);
        let accessed = p3_native_time(data_access_timestamp)?;
        let modified = p3_native_time(data_modification_timestamp)?;
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let before =
            p3_initial_probe(&runtime, path_times(&directory, &path, follow).await).await?;
        let mut adapter = P3MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                native_set_path_times(
                    &directory_for_attempt,
                    &path_for_attempt,
                    follow,
                    accessed,
                    modified,
                )
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = times_postcondition(
                        path_times(&directory, &path, follow).await,
                        before,
                        p3_requested_time(data_access_timestamp),
                        p3_requested_time(data_modification_timestamp),
                        true,
                    );
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn link_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path_flags: types::PathFlags,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "link-at",
            )
        });
        let effect = Arc::new(begin_filesystem_path_effect::<Ctx, U>(store).await?);
        fail_if_read_only_from_accessor::<Ctx, U>(store, &fd)?;
        fail_if_read_only_from_accessor::<Ctx, U>(store, &new_fd)?;
        fail_if_read_only_path_from_accessor::<Ctx, U>(
            store,
            &fd,
            &old_path,
            false,
            old_path_flags.contains(types::PathFlags::SYMLINK_FOLLOW),
        )?;
        fail_if_read_only_path_from_accessor::<Ctx, U>(store, &new_fd, &new_path, false, false)?;
        let source_directory =
            store.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &fd))?;
        let destination_directory =
            store.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &new_fd))?;
        validate_two_directory_mutation(&source_directory, &destination_directory)
            .map_err(p3_native_guest)?;
        if old_path_flags.contains(types::PathFlags::SYMLINK_FOLLOW) {
            return Err(types::ErrorCode::Invalid.into());
        }
        let runtime = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let source_before =
            p3_initial_probe(&runtime, path_state(&source_directory, &old_path).await).await?;
        let destination_before = p3_initial_probe(
            &runtime,
            path_state(&destination_directory, &new_path).await,
        )
        .await?;
        let mut adapter = P3MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let source_directory_for_attempt = source_directory.clone();
            let destination_directory_for_attempt = destination_directory.clone();
            let old_path_for_attempt = old_path.clone();
            let new_path_for_attempt = new_path.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                native_hard_link(
                    &source_directory_for_attempt,
                    &old_path_for_attempt,
                    &destination_directory_for_attempt,
                    &new_path_for_attempt,
                )
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = link_postcondition(
                        source_before,
                        destination_before,
                        path_state(&source_directory, &old_path).await,
                        path_state(&destination_directory, &new_path).await,
                    );
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn open_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        open_flags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FilesystemResult<Resource<Descriptor>> {
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "open-at",
            )
        });
        let mutating = open_flags.intersects(types::OpenFlags::CREATE | types::OpenFlags::TRUNCATE);
        let effect = if mutating {
            Some(Arc::new(
                begin_filesystem_update_effect::<Ctx, U>(accessor).await?,
            ))
        } else {
            None
        };
        if open_flags.contains(types::OpenFlags::TRUNCATE)
            || flags.contains(types::DescriptorFlags::WRITE)
        {
            fail_if_read_only_path_from_accessor::<Ctx, U>(
                accessor,
                &fd,
                &path,
                false,
                path_flags.contains(types::PathFlags::SYMLINK_FOLLOW),
            )?;
        }
        if !mutating {
            let filesystem = accessor.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
            return <WasiFilesystem as types::HostDescriptorWithStore<U>>::open_at(
                &filesystem,
                fd,
                path_flags,
                path,
                open_flags,
                flags,
            )
            .await;
        }
        let directory =
            accessor.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &fd))?;
        let follow = path_flags.contains(types::PathFlags::SYMLINK_FOLLOW);
        let native_options = NativeOpenOptions {
            create: open_flags.contains(types::OpenFlags::CREATE),
            directory: open_flags.contains(types::OpenFlags::DIRECTORY),
            exclusive: open_flags.contains(types::OpenFlags::EXCLUSIVE),
            truncate: open_flags.contains(types::OpenFlags::TRUNCATE),
            follow,
            read: flags.contains(types::DescriptorFlags::READ),
            write: flags.contains(types::DescriptorFlags::WRITE),
        };
        validate_open(
            &directory,
            native_options,
            flags.intersects(
                types::DescriptorFlags::FILE_INTEGRITY_SYNC
                    | types::DescriptorFlags::DATA_INTEGRITY_SYNC
                    | types::DescriptorFlags::REQUESTED_WRITE_SYNC,
            ),
        )
        .map_err(p3_native_guest)?;
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let before = p3_initial_probe(
            &runtime,
            path_state_with_follow(&directory, &path, follow).await,
        )
        .await?;
        let requested_type = if open_flags.contains(types::OpenFlags::DIRECTORY) {
            PathObjectType::Directory
        } else {
            PathObjectType::RegularFile
        };
        let operation = if open_flags.contains(types::OpenFlags::CREATE) {
            MutationOperation::Create
        } else {
            MutationOperation::Resize
        };
        let mut adapter = P3MutationAdapter::for_operation(runtime, operation);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(effect.as_ref().expect("mutating open has an effect lease"));
            match run_blocking_filesystem_mutation(effect, move || {
                native_open(&directory_for_attempt, &path_for_attempt, native_options)
            })
            .await
            {
                Ok(NativeOpenResult::Descriptor(descriptor)) => {
                    return push_descriptor(accessor, descriptor);
                }
                #[cfg(windows)]
                Ok(NativeOpenResult::IsDirectory) => {
                    return Err(types::ErrorCode::IsDirectory.into());
                }
                Ok(NativeOpenResult::NotDirectory) => {
                    return Err(p3_native_guest(NativeMutationGuestError::NotDirectory));
                }
                Err(error) => {
                    let postcondition = open_postcondition(
                        before,
                        path_state_with_follow(&directory, &path, follow).await,
                        requested_type,
                        open_flags.contains(types::OpenFlags::TRUNCATE),
                        open_flags.contains(types::OpenFlags::EXCLUSIVE),
                    );
                    match adapter.io_failure(error, postcondition, true).await {
                        P3MutationAction::Retry => {}
                        P3MutationAction::Success => {
                            let safe_flags = open_flags & types::OpenFlags::DIRECTORY;
                            let filesystem = accessor
                                .with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
                            return <WasiFilesystem as types::HostDescriptorWithStore<U>>::open_at(
                                &filesystem,
                                Resource::new_borrow(fd.rep()),
                                path_flags,
                                path.clone(),
                                safe_flags,
                                flags,
                            )
                            .await;
                        }
                        P3MutationAction::Error(error) => return Err(error),
                        P3MutationAction::Trap => {
                            return Err(FilesystemError::trap(wasmtime::Error::msg(
                                "agent filesystem mutation invalidated the runtime",
                            )));
                        }
                    }
                }
            }
        }
    }

    async fn readlink_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<String> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "readlink-at",
            )
        });
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::readlink_at(&store, fd, path).await
    }

    async fn remove_directory_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        store.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "remove-directory-at",
            )
        });
        let effect = Arc::new(begin_filesystem_path_effect::<Ctx, U>(store).await?);
        fail_if_read_only_path_from_accessor::<Ctx, U>(store, &fd, &path, true, false)?;
        let directory =
            store.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &fd))?;
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let runtime = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let before = p3_initial_probe(&runtime, path_state(&directory, &path).await).await?;
        let mut adapter = P3MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                native_remove_directory(&directory_for_attempt, &path_for_attempt)
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition =
                        remove_postcondition(before, path_state(&directory, &path).await);
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn rename_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let effect = Arc::new(begin_filesystem_path_effect::<Ctx, U>(accessor).await?);
        fail_if_read_only_from_accessor::<Ctx, U>(accessor, &fd)?;
        fail_if_read_only_from_accessor::<Ctx, U>(accessor, &new_fd)?;
        fail_if_read_only_path_from_accessor::<Ctx, U>(accessor, &fd, &old_path, true, false)?;
        fail_if_read_only_path_from_accessor::<Ctx, U>(accessor, &new_fd, &new_path, true, false)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "rename-at",
            )
        });
        let source_directory =
            accessor.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &fd))?;
        let destination_directory =
            accessor.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &new_fd))?;
        validate_two_directory_mutation(&source_directory, &destination_directory)
            .map_err(p3_native_guest)?;
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let source_before =
            p3_initial_probe(&runtime, path_state(&source_directory, &old_path).await).await?;
        let destination_before = p3_initial_probe(
            &runtime,
            path_state(&destination_directory, &new_path).await,
        )
        .await?;
        let mut adapter = P3MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let source_directory_for_attempt = source_directory.clone();
            let destination_directory_for_attempt = destination_directory.clone();
            let old_path_for_attempt = old_path.clone();
            let new_path_for_attempt = new_path.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                native_rename(
                    &source_directory_for_attempt,
                    &old_path_for_attempt,
                    &destination_directory_for_attempt,
                    &new_path_for_attempt,
                )
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = rename_postcondition(
                        source_before,
                        destination_before,
                        path_state(&source_directory, &old_path).await,
                        path_state(&destination_directory, &new_path).await,
                    );
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn symlink_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> FilesystemResult<()> {
        let effect = Arc::new(begin_filesystem_path_effect::<Ctx, U>(accessor).await?);
        fail_if_read_only_from_accessor::<Ctx, U>(accessor, &fd)?;
        fail_if_read_only_path_from_accessor::<Ctx, U>(accessor, &fd, &new_path, false, false)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "symlink-at",
            )
        });
        let directory =
            accessor.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &fd))?;
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let before = p3_initial_probe(&runtime, symlink_state(&directory, &new_path).await).await?;
        let mut adapter = P3MutationAdapter::for_operation(runtime, MutationOperation::Create);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let old_path_for_attempt = old_path.clone();
            let new_path_for_attempt = new_path.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                native_symlink(
                    &directory_for_attempt,
                    &old_path_for_attempt,
                    &new_path_for_attempt,
                )
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = symlink_postcondition(
                        &before,
                        symlink_state(&directory, &new_path).await,
                        &old_path,
                    );
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn unlink_file_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let effect = Arc::new(begin_filesystem_path_effect::<Ctx, U>(accessor).await?);
        fail_if_read_only_from_accessor::<Ctx, U>(accessor, &fd)?;
        fail_if_read_only_path_from_accessor::<Ctx, U>(accessor, &fd, &path, false, false)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "unlink-file-at",
            )
        });
        let directory =
            accessor.with(|mut access| directory_from_access::<Ctx, U>(&mut access, &fd))?;
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let before = p3_initial_probe(&runtime, path_state(&directory, &path).await).await?;
        let mut adapter = P3MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            match run_blocking_filesystem_mutation(effect, move || {
                native_unlink_file(&directory_for_attempt, &path_for_attempt)
            })
            .await
            {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition =
                        remove_postcondition(before, path_state(&directory, &path).await);
                    if !p3_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
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
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::is_same_object(&store, fd, other)
            .await
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
    use crate::services::agent_filesystem::{FilesystemCapacity, ObjectIdentity, PathState};
    use fs_set_times::{SystemTimeSpec, set_symlink_times, set_times};
    use golem_common::model::oplog::types::SerializableDateTime;
    use std::time::{Duration, SystemTime};
    use test_r::test;

    fn test_file(path: std::path::PathBuf) -> File {
        File::new(
            cap_std::fs::File::from_std(std::fs::File::create(&path).unwrap()),
            FilePerms::WRITE,
            wasmtime_wasi::filesystem::OpenMode::WRITE,
            false,
            path,
        )
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn p3_mutation_adapter_retries_only_proven_no_effect_once() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mut adapter = P3MutationAdapter::new(runtime.clone());

        adapter.begin_attempt();
        assert!(matches!(
            adapter
                .failure(
                    types::ErrorCode::Busy.into(),
                    MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P3MutationAction::Retry
        ));
        adapter.begin_attempt();
        assert!(matches!(
            adapter
                .failure(
                    types::ErrorCode::Busy.into(),
                    MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P3MutationAction::Error(error)
                if matches!(error.downcast_ref(), Some(types::ErrorCode::Busy))
        ));
        assert!(runtime.begin_effect().await.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn p3_mutation_adapter_accepts_satisfied_postcondition() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mut adapter = P3MutationAdapter::new(runtime.clone());
        adapter.begin_attempt();

        assert!(matches!(
            adapter
                .failure(
                    types::ErrorCode::Interrupted.into(),
                    MutationPostcondition::Satisfied,
                    true,
                )
                .await,
            P3MutationAction::Success
        ));
        assert!(runtime.begin_effect().await.is_ok());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn p3_mutation_adapter_seals_unknown_effect() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mut adapter = P3MutationAdapter::new(runtime.clone());
        adapter.begin_attempt();

        assert!(matches!(
            adapter
                .failure(
                    types::ErrorCode::Busy.into(),
                    MutationPostcondition::Unknown,
                    true,
                )
                .await,
            P3MutationAction::Trap
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn p3_native_eio_reaches_terminal_classifier() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mut adapter = P3MutationAdapter::new(runtime.clone());
        adapter.begin_attempt();

        assert!(matches!(
            adapter
                .io_failure(
                    std::io::Error::from_raw_os_error(libc::EIO),
                    MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P3MutationAction::Trap
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn p3_physical_pressure_runs_one_safe_recovery_cycle() {
        let runtime = AgentFilesystemRuntime::new_for_test_with_observations(
            None,
            None,
            FilesystemCapacity {
                total_bytes: 100,
                available_bytes: 0,
                total_filesystem_objects: 100,
                available_filesystem_objects: 100,
            },
        );
        let recovery_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        runtime.set_pressure_recovery_callback(Some({
            let recovery_attempts = Arc::clone(&recovery_attempts);
            Arc::new(move |operation, _deadline| {
                let recovery_attempts = Arc::clone(&recovery_attempts);
                Box::pin(async move {
                    assert_eq!(operation, MutationOperation::Metadata);
                    recovery_attempts.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    true
                })
            })
        }));
        let mut adapter = P3MutationAdapter::new(runtime);
        adapter.begin_attempt();
        assert!(matches!(
            adapter
                .failure(
                    types::ErrorCode::InsufficientSpace.into(),
                    MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P3MutationAction::Retry
        ));
        adapter.begin_attempt();
        assert!(matches!(
            adapter
                .failure(
                    types::ErrorCode::InsufficientSpace.into(),
                    MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P3MutationAction::Error(error)
                if matches!(error.downcast_ref(), Some(types::ErrorCode::InsufficientSpace))
        ));
        assert_eq!(
            recovery_attempts.load(std::sync::atomic::Ordering::Acquire),
            1
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn p3_physical_pressure_respects_effect_and_time_bounds() {
        fn pressure_runtime() -> AgentFilesystemRuntime {
            AgentFilesystemRuntime::new_for_test_with_observations(
                None,
                None,
                FilesystemCapacity {
                    total_bytes: 100,
                    available_bytes: 0,
                    total_filesystem_objects: 100,
                    available_filesystem_objects: 100,
                },
            )
        }

        let completed_recoveries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let completed = pressure_runtime();
        completed.set_pressure_recovery_callback(Some({
            let completed_recoveries = Arc::clone(&completed_recoveries);
            Arc::new(move |_, _deadline| {
                let completed_recoveries = Arc::clone(&completed_recoveries);
                Box::pin(async move {
                    completed_recoveries.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    true
                })
            })
        }));
        let mut completed_adapter = P3MutationAdapter::new(completed);
        completed_adapter.begin_attempt();
        assert!(matches!(
            completed_adapter
                .failure(
                    types::ErrorCode::InsufficientSpace.into(),
                    MutationPostcondition::Satisfied,
                    true,
                )
                .await,
            P3MutationAction::Success
        ));
        assert_eq!(
            completed_recoveries.load(std::sync::atomic::Ordering::Acquire),
            0
        );

        let unknown_recoveries = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let unknown = pressure_runtime();
        unknown.set_pressure_recovery_callback(Some({
            let unknown_recoveries = Arc::clone(&unknown_recoveries);
            Arc::new(move |_, _deadline| {
                let unknown_recoveries = Arc::clone(&unknown_recoveries);
                Box::pin(async move {
                    unknown_recoveries.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                    true
                })
            })
        }));
        let mut unknown_adapter = P3MutationAdapter::new(unknown);
        unknown_adapter.begin_attempt();
        assert!(matches!(
            unknown_adapter
                .failure(
                    types::ErrorCode::InsufficientSpace.into(),
                    MutationPostcondition::Unknown,
                    true,
                )
                .await,
            P3MutationAction::Trap
        ));
        assert_eq!(
            unknown_recoveries.load(std::sync::atomic::Ordering::Acquire),
            0
        );

        let timed_out = pressure_runtime();
        timed_out.set_pressure_recovery_callback(Some(Arc::new(move |_, _deadline| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(260)).await;
                true
            })
        })));
        let mut timed_out_adapter = P3MutationAdapter::new(timed_out);
        timed_out_adapter.begin_attempt();
        assert!(matches!(
            timed_out_adapter
                .failure(
                    types::ErrorCode::InsufficientSpace.into(),
                    MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P3MutationAction::Error(error)
                if matches!(error.downcast_ref(), Some(types::ErrorCode::InsufficientSpace))
        ));
    }

    #[cfg(target_os = "linux")]
    #[test]
    async fn p3_shared_path_probe_distinguishes_unchanged_and_satisfied_state() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let directory = Dir::new(
            cap_std::fs::Dir::open_ambient_dir(tempdir.path(), cap_std::ambient_authority())
                .unwrap(),
            DirPerms::all(),
            FilePerms::all(),
            wasmtime_wasi::filesystem::OpenMode::READ | wasmtime_wasi::filesystem::OpenMode::WRITE,
            false,
            tempdir.path().to_path_buf(),
        );
        let before = path_state(&directory, "entry").await.unwrap();
        assert_eq!(before, None);

        let unchanged = state_postcondition(
            path_state(&directory, "entry").await,
            |state| state.is_some(),
            |state| state == before,
        );
        assert_eq!(unchanged, MutationPostcondition::NoEffect);

        std::fs::create_dir(tempdir.path().join("entry")).unwrap();
        let satisfied = state_postcondition(
            path_state(&directory, "entry").await,
            |state| state.is_some_and(|state| state.type_ == PathObjectType::Directory),
            |_| false,
        );
        assert_eq!(satisfied, MutationPostcondition::Satisfied);
    }

    #[test]
    fn p3_shared_operation_postconditions_cover_non_prefix_effects() {
        let source = PathState {
            identity: Some(ObjectIdentity {
                device: 1,
                inode: 10,
            }),
            type_: PathObjectType::RegularFile,
            size: 17,
        };
        let replacement = PathState {
            identity: Some(ObjectIdentity {
                device: 1,
                inode: 11,
            }),
            type_: PathObjectType::RegularFile,
            size: 8,
        };

        let open_cases = [
            (
                None,
                Ok(None),
                false,
                false,
                MutationPostcondition::NoEffect,
            ),
            (
                Some(source),
                Ok(Some(source)),
                false,
                false,
                MutationPostcondition::Satisfied,
            ),
            (
                Some(source),
                Ok(Some(replacement)),
                false,
                false,
                MutationPostcondition::Satisfied,
            ),
            (
                Some(source),
                Ok(Some(PathState { size: 0, ..source })),
                true,
                false,
                MutationPostcondition::Satisfied,
            ),
        ];
        for (before, current, truncate, exclusive, expected) in open_cases {
            assert_eq!(
                open_postcondition(
                    before,
                    current,
                    PathObjectType::RegularFile,
                    truncate,
                    exclusive,
                ),
                expected
            );
        }

        assert_eq!(
            rename_postcondition(Some(source), Some(replacement), Ok(None), Ok(Some(source))),
            MutationPostcondition::Satisfied
        );
        assert_eq!(
            rename_postcondition(
                Some(source),
                Some(replacement),
                Ok(Some(source)),
                Ok(Some(replacement)),
            ),
            MutationPostcondition::NoEffect
        );
        assert_eq!(
            link_postcondition(Some(source), None, Ok(Some(source)), Ok(Some(source))),
            MutationPostcondition::Satisfied
        );
        assert_eq!(
            create_directory_postcondition(Some(source), Ok(Some(source))),
            MutationPostcondition::NoEffect
        );
        assert_eq!(
            remove_postcondition(None, Ok(None)),
            MutationPostcondition::NoEffect
        );
        assert_eq!(
            link_postcondition(None, Some(replacement), Ok(None), Ok(Some(replacement))),
            MutationPostcondition::NoEffect
        );
        assert_eq!(
            rename_postcondition(None, Some(replacement), Ok(None), Ok(Some(replacement))),
            MutationPostcondition::NoEffect
        );
        let existing_symlink = crate::services::agent_filesystem::SymlinkState {
            object: Some(PathState {
                type_: PathObjectType::SymbolicLink,
                ..source
            }),
            target: Some("existing".into()),
        };
        assert_eq!(
            symlink_postcondition(&existing_symlink, Ok(existing_symlink.clone()), "requested",),
            MutationPostcondition::NoEffect
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
    fn p3_write_result_maps_completion_cancellation_and_invalidation() {
        let (completed, result) = p3_write_result(Ok(4));
        assert_eq!(completed, 4);
        assert!(result.is_ok());

        let (completed, result) = p3_write_result(Err(AgentFilesystemMutationError::Cancelled {
            completed: 2,
        }));
        assert_eq!(completed, 2);
        assert!(result.is_ok());

        let (completed, result) =
            p3_write_result(Err(AgentFilesystemMutationError::RuntimeInvalidated {
                error: None,
                completed: Some(3),
            }));
        assert_eq!(completed, 3);
        assert!(matches!(result, Err(FilesystemWriteFailure::Trap(_))));
    }

    #[test]
    async fn dropped_p3_host_result_sender_invalidates_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let invalidated = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        runtime.set_invalidation_callback(Some(Arc::new({
            let invalidated = Arc::clone(&invalidated);
            let release = Arc::clone(&release);
            move || {
                let invalidated = Arc::clone(&invalidated);
                let release = Arc::clone(&release);
                Box::pin(async move {
                    release.acquire().await.unwrap().forget();
                    invalidated.store(true, std::sync::atomic::Ordering::Release);
                })
            }
        })));
        let tempdir = tempfile::TempDir::new().unwrap();
        let writer = runtime.mutations().writer(
            test_file(tempdir.path().join("out")),
            AgentFilesystemWriteMode::Position(0),
        );
        let (chunks_tx, _chunks_rx) = tokio::sync::mpsc::unbounded_channel();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let mut consumer = FilesystemWriteConsumer {
            chunks_tx: Some(chunks_tx),
            pending_chunk: Some(PendingFilesystemWriteChunk {
                result_rx,
                cancellation: tokio_util::sync::CancellationToken::new(),
            }),
            pending_invalidation: None,
            filesystem_runtime: runtime.clone(),
            writer,
        };
        drop(result_tx);

        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert!(matches!(
            consumer.poll_pending_result(&mut cx),
            Poll::Pending
        ));
        assert!(runtime.begin_effect().await.is_err());
        assert!(!invalidated.load(std::sync::atomic::Ordering::Acquire));

        release.add_permits(1);
        let result = consumer.poll_pending_result(&mut cx);

        assert!(matches!(result, Poll::Ready(Err(_))));
        assert!(invalidated.load(std::sync::atomic::Ordering::Acquire));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[test]
    async fn lost_p3_write_progress_delivery_invalidates_runtime() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        drop(result_rx);

        let result = deliver_filesystem_write_result(
            &runtime,
            result_tx,
            tokio_util::sync::CancellationToken::new(),
            2,
            Ok(()),
        )
        .await;

        assert!(result.is_err());
        assert!(runtime.begin_effect().await.is_err());
    }

    #[test]
    async fn cancelled_p3_write_does_not_require_progress_delivery() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        drop(result_rx);
        let cancellation = tokio_util::sync::CancellationToken::new();
        cancellation.cancel();

        let result =
            deliver_filesystem_write_result(&runtime, result_tx, cancellation, 2, Ok(())).await;

        assert!(result.is_ok());
        assert!(runtime.begin_effect().await.is_ok());
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
