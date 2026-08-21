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
use std::time::Duration;

use crate::durable_host::filesystem::types::calculate_metadata_hash_parts;
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, observe_function_call,
    observe_function_call_store, run_read_access, wasi_filesystem_view,
};
use crate::durable_host::tail_work::TailActivity;
use crate::services::agent_filesystem::{
    AdmittedFilesystemWrite, AgentFilesystemMutationError, AgentFilesystemRuntime,
    AgentFilesystemStreamSetupAdmission, AgentFilesystemWriteMode, AgentFilesystemWriter,
    NativeMutationGuestError, NativeOpenOptions, NativeOpenResult, RequestedTime,
    validate_descriptor_times, validate_directory_mutation, validate_open_capabilities,
    validate_open_flags, validate_resize, validate_two_directory_mutation,
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
use wasmtime_wasi::{DirPerms, FilePerms};

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

fn p3_native_guest(error: NativeMutationGuestError) -> FilesystemError {
    match error {
        NativeMutationGuestError::Invalid => types::ErrorCode::Invalid.into(),
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

fn p3_validate_time(requested: types::NewTimestamp) -> Result<(), FilesystemError> {
    match requested {
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
            time.map(|_| ())
                .ok_or_else(|| types::ErrorCode::Overflow.into())
        }
        types::NewTimestamp::NoChange | types::NewTimestamp::Now => Ok(()),
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

struct P3WriteStreamSetup {
    file: File,
    admission: AgentFilesystemStreamSetupAdmission,
}

#[derive(Debug)]
enum P3WriteStreamSetupError {
    Guest(types::ErrorCode),
    Trap,
}

fn prepare_p3_write_stream_setup(
    descriptor: &Descriptor,
    runtime: &AgentFilesystemRuntime,
) -> Result<P3WriteStreamSetup, P3WriteStreamSetupError> {
    let file = match descriptor {
        Descriptor::File(file) if !file.perms.contains(FilePerms::WRITE) => {
            return Err(P3WriteStreamSetupError::Guest(
                types::ErrorCode::NotPermitted,
            ));
        }
        Descriptor::File(file) => file.clone(),
        Descriptor::Dir(_) => {
            return Err(P3WriteStreamSetupError::Guest(
                types::ErrorCode::BadDescriptor,
            ));
        }
    };
    let admission = match runtime
        .mutations()
        .authorize_and_admit_stream_setup(descriptor)
    {
        Ok(admission) => admission,
        Err(AgentFilesystemMutationError::Guest(error)) => {
            let error = match error {
                NativeMutationGuestError::Invalid => types::ErrorCode::Invalid,
                NativeMutationGuestError::NotPermitted => types::ErrorCode::NotPermitted,
                NativeMutationGuestError::Unsupported => types::ErrorCode::Unsupported,
            };
            return Err(P3WriteStreamSetupError::Guest(error));
        }
        Err(_) => return Err(P3WriteStreamSetupError::Trap),
    };
    Ok(P3WriteStreamSetup { file, admission })
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
        Err(AgentFilesystemMutationError::Guest(error)) => (
            0,
            Err(FilesystemWriteFailure::Guest(match error {
                NativeMutationGuestError::Invalid => types::ErrorCode::Invalid,
                NativeMutationGuestError::NotPermitted => types::ErrorCode::NotPermitted,
                NativeMutationGuestError::Unsupported => types::ErrorCode::Unsupported,
            })),
        ),
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

fn p3_mutation_result<T>(result: Result<T, AgentFilesystemMutationError>) -> FilesystemResult<T> {
    match result {
        Ok(value) => Ok(value),
        Err(AgentFilesystemMutationError::Guest(error)) => Err(p3_native_guest(error)),
        Err(AgentFilesystemMutationError::Native { error, .. }) => {
            Err(types::ErrorCode::from(&error.into_io_error()).into())
        }
        Err(AgentFilesystemMutationError::QuotaExhausted { .. }) => {
            Err(types::ErrorCode::Quota.into())
        }
        Err(AgentFilesystemMutationError::InsufficientSpace { .. }) => {
            Err(types::ErrorCode::InsufficientSpace.into())
        }
        Err(AgentFilesystemMutationError::Cancelled { .. }) => {
            unreachable!("non-stream mutation is not cancellable")
        }
        Err(AgentFilesystemMutationError::RuntimeInvalidated { .. }) => Err(FilesystemError::trap(
            wasmtime::Error::msg("agent filesystem mutation invalidated the runtime"),
        )),
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
        let descriptor =
            accessor.with(|mut store| descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        let filesystem_runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let P3WriteStreamSetup { file, admission } =
            match prepare_p3_write_stream_setup(&descriptor, &filesystem_runtime) {
                Ok(setup) => setup,
                Err(P3WriteStreamSetupError::Guest(error)) => {
                    let mut data = data;
                    accessor.with(|mut store| data.close(&mut store))?;
                    return accessor.with(|mut store| {
                        FutureReader::new(&mut store, async move {
                            Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(error))
                        })
                    });
                }
                Err(P3WriteStreamSetupError::Trap) => {
                    let mut data = data;
                    accessor.with(|mut store| data.close(&mut store))?;
                    return accessor.with(|mut store| {
                        FutureReader::new(&mut store, async move {
                            Err(wasmtime::Error::msg(
                                "agent filesystem mutation invalidated the runtime",
                            ))
                        })
                    });
                }
            };
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
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
        let future = accessor.with(|mut store| {
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
        })?;
        drop(admission);
        Ok(future)
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
        let descriptor =
            accessor.with(|mut store| descriptor_from_access::<Ctx, U>(&mut store, &fd))?;
        let filesystem_runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let P3WriteStreamSetup { file, admission } =
            match prepare_p3_write_stream_setup(&descriptor, &filesystem_runtime) {
                Ok(setup) => setup,
                Err(P3WriteStreamSetupError::Guest(error)) => {
                    let mut data = data;
                    accessor.with(|mut store| data.close(&mut store))?;
                    return accessor.with(|mut store| {
                        FutureReader::new(&mut store, async move {
                            Ok::<Result<(), types::ErrorCode>, wasmtime::Error>(Err(error))
                        })
                    });
                }
                Err(P3WriteStreamSetupError::Trap) => {
                    let mut data = data;
                    accessor.with(|mut store| data.close(&mut store))?;
                    return accessor.with(|mut store| {
                        FutureReader::new(&mut store, async move {
                            Err(wasmtime::Error::msg(
                                "agent filesystem mutation invalidated the runtime",
                            ))
                        })
                    });
                }
            };
        let (chunks_tx, chunks_rx) = tokio::sync::mpsc::unbounded_channel();
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
        let future = accessor.with(|mut store| {
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
        })?;
        drop(admission);
        Ok(future)
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
        let runtime = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let descriptor = store
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let admitted = p3_mutation_result(runtime.mutations().admit_sync().await)?;
        p3_mutation_result(runtime.mutations().sync(admitted, descriptor, true).await)
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
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let file = match &descriptor {
            Descriptor::File(file) => file.clone(),
            Descriptor::Dir(_) => return Err(types::ErrorCode::BadDescriptor.into()),
        };
        validate_resize(&file).map_err(p3_native_guest)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_resize(descriptor.clone(), size)
                .await,
        )?;
        let prepared = p3_mutation_result(mutations.prepare_resize(checked, file).await)?;
        p3_mutation_result(mutations.resize(prepared).await.map(|_| ()))
    }

    async fn set_times(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let policy_descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        validate_descriptor_times(&policy_descriptor).map_err(p3_native_guest)?;
        p3_validate_time(data_access_timestamp)?;
        p3_validate_time(data_modification_timestamp)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_descriptor_times(policy_descriptor)
                .await,
        )?;
        let accessed = p3_native_time(data_access_timestamp)?;
        let modified = p3_native_time(data_modification_timestamp)?;
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
        let validated =
            p3_mutation_result(mutations.prepare_descriptor_times(checked, descriptor))?;
        let prepared = mutations.bind_descriptor_times(
            validated,
            accessed,
            modified,
            p3_requested_time(data_access_timestamp),
            p3_requested_time(data_modification_timestamp),
        );
        p3_mutation_result(mutations.set_descriptor_times(prepared).await)
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
        let runtime = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let descriptor = store
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let admitted = p3_mutation_result(runtime.mutations().admit_sync().await)?;
        p3_mutation_result(runtime.mutations().sync(admitted, descriptor, false).await)
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
        let runtime = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let descriptor = store
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let directory = match &descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_create_directory(descriptor, path)
                .await,
        )?;
        let prepared = p3_mutation_result(mutations.prepare_create_directory(checked, directory))?;
        p3_mutation_result(mutations.run_namespace_mutation(prepared).await)
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
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let follow = path_flags.contains(types::PathFlags::SYMLINK_FOLLOW);
        let policy_descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let directory = match &policy_descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        p3_validate_time(data_access_timestamp)?;
        p3_validate_time(data_modification_timestamp)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_path_times(policy_descriptor, path, follow)
                .await,
        )?;
        let accessed = p3_native_time(data_access_timestamp)?;
        let modified = p3_native_time(data_modification_timestamp)?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "set-times-at",
            )
        });
        let validated = p3_mutation_result(mutations.prepare_path_times(checked, directory))?;
        let prepared = mutations.bind_path_times(
            validated,
            accessed,
            modified,
            p3_requested_time(data_access_timestamp),
            p3_requested_time(data_modification_timestamp),
        );
        p3_mutation_result(mutations.set_path_times(prepared).await)
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
        let runtime = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let source_descriptor = store
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let destination_descriptor = store
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &new_fd))
            .map_err(FilesystemError::trap)?;
        let source_follow = old_path_flags.contains(types::PathFlags::SYMLINK_FOLLOW);
        if source_follow {
            return Err(types::ErrorCode::Invalid.into());
        }
        let source_directory = match &source_descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        let destination_directory = match &destination_descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        validate_two_directory_mutation(&source_directory, &destination_directory)
            .map_err(p3_native_guest)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_hard_link(
                    source_descriptor,
                    old_path,
                    source_follow,
                    destination_descriptor,
                    new_path,
                )
                .await,
        )?;
        let prepared = p3_mutation_result(mutations.prepare_hard_link(
            checked,
            source_directory,
            destination_directory,
        ))?;
        p3_mutation_result(mutations.run_namespace_mutation(prepared).await)
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
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let follow = path_flags.contains(types::PathFlags::SYMLINK_FOLLOW);
        let writable = open_flags.contains(types::OpenFlags::TRUNCATE)
            || flags.contains(types::DescriptorFlags::WRITE);
        let descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let directory = match &descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        let native_options = NativeOpenOptions {
            create: open_flags.contains(types::OpenFlags::CREATE),
            directory: open_flags.contains(types::OpenFlags::DIRECTORY),
            exclusive: open_flags.contains(types::OpenFlags::EXCLUSIVE),
            truncate: open_flags.contains(types::OpenFlags::TRUNCATE),
            follow,
            read: flags.contains(types::DescriptorFlags::READ),
            write: flags.contains(types::DescriptorFlags::WRITE),
        };
        let unsupported_sync_flags = flags.intersects(
            types::DescriptorFlags::FILE_INTEGRITY_SYNC
                | types::DescriptorFlags::DATA_INTEGRITY_SYNC
                | types::DescriptorFlags::REQUESTED_WRITE_SYNC,
        );
        validate_open_flags(native_options, unsupported_sync_flags).map_err(p3_native_guest)?;
        validate_open_capabilities(&directory, native_options).map_err(p3_native_guest)?;
        if writable && !mutating {
            p3_mutation_result(mutations.authorize_writable_open(&descriptor, &path, follow))?;
        }
        let checked = if mutating {
            Some(p3_mutation_result(
                mutations
                    .authorize_and_admit_mutating_open(descriptor.clone(), path.clone(), follow)
                    .await,
            )?)
        } else {
            None
        };
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
        let prepared = p3_mutation_result(mutations.prepare_mutating_open(
            checked.expect("mutating open has checked admission"),
            directory,
            native_options,
        ))?;
        match p3_mutation_result(mutations.open_mutating(prepared).await)? {
            NativeOpenResult::Descriptor(descriptor) => push_descriptor(accessor, descriptor),
            #[cfg(windows)]
            NativeOpenResult::IsDirectory => Err(types::ErrorCode::IsDirectory.into()),
            NativeOpenResult::NotDirectory => Err(types::ErrorCode::NotDirectory.into()),
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
        let runtime = store.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let descriptor = store
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let directory = match &descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_remove_directory(descriptor, path)
                .await,
        )?;
        let prepared = p3_mutation_result(mutations.prepare_remove_directory(checked, directory))?;
        p3_mutation_result(mutations.run_namespace_mutation(prepared).await)
    }

    async fn rename_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let source_descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let destination_descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &new_fd))
            .map_err(FilesystemError::trap)?;
        let source_directory = match &source_descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        let destination_directory = match &destination_descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        validate_two_directory_mutation(&source_directory, &destination_directory)
            .map_err(p3_native_guest)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_rename(
                    source_descriptor,
                    old_path,
                    destination_descriptor,
                    new_path,
                )
                .await,
        )?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "rename-at",
            )
        });
        let prepared = p3_mutation_result(mutations.prepare_rename(
            checked,
            source_directory,
            destination_directory,
        ))?;
        p3_mutation_result(mutations.run_namespace_mutation(prepared).await)
    }

    async fn symlink_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> FilesystemResult<()> {
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let directory = match &descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_symlink(descriptor, old_path, new_path)
                .await,
        )?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "symlink-at",
            )
        });
        let prepared = p3_mutation_result(mutations.prepare_symlink(checked, directory))?;
        p3_mutation_result(mutations.run_namespace_mutation(prepared).await)
    }

    async fn unlink_file_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let runtime = accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut()).filesystem_runtime()
        });
        let mutations = runtime.mutations();
        let descriptor = accessor
            .with(|mut access| descriptor_from_access::<Ctx, U>(&mut access, &fd))
            .map_err(FilesystemError::trap)?;
        let directory = match &descriptor {
            Descriptor::Dir(directory) => directory.clone(),
            Descriptor::File(_) => return Err(types::ErrorCode::NotDirectory.into()),
        };
        validate_directory_mutation(&directory).map_err(p3_native_guest)?;
        let checked = p3_mutation_result(
            mutations
                .authorize_and_admit_unlink_file(descriptor, path)
                .await,
        )?;
        accessor.with(|mut access| {
            observe_function_call_store::<Ctx, U>(
                access.data_mut(),
                "filesystem::types::descriptor",
                "unlink-file-at",
            )
        });
        let prepared = p3_mutation_result(mutations.prepare_unlink_file(checked, directory))?;
        p3_mutation_result(mutations.run_namespace_mutation(prepared).await)
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
    use crate::durable_host::filesystem::types::{
        P2WriteStreamSetupError, prepare_p2_write_stream_setup,
    };
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

    #[test]
    fn p2_p3_write_stream_setup_precedence_is_identical() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let read_only_path = tempdir.path().join("read-only");
        std::fs::write(&read_only_path, b"contents").unwrap();
        let p2_runtime = AgentFilesystemRuntime::new_for_test();
        let p3_runtime = AgentFilesystemRuntime::new_for_test();
        p2_runtime.mark_read_only_for_test(read_only_path.clone());
        p3_runtime.mark_read_only_for_test(read_only_path.clone());
        p2_runtime.seal();
        p3_runtime.seal();
        let p2_descriptor = Descriptor::File(test_file(read_only_path.clone()));
        let p3_descriptor = Descriptor::File(test_file(read_only_path));

        assert!(matches!(
            prepare_p2_write_stream_setup(&p2_descriptor, &p2_runtime),
            Err(P2WriteStreamSetupError::Guest(
                wasmtime_wasi::p2::bindings::filesystem::types::ErrorCode::NotPermitted,
            ))
        ));
        assert!(matches!(
            prepare_p3_write_stream_setup(&p3_descriptor, &p3_runtime),
            Err(P3WriteStreamSetupError::Guest(
                types::ErrorCode::NotPermitted
            ))
        ));

        let writable_path = tempdir.path().join("writable");
        std::fs::write(&writable_path, b"").unwrap();
        let p2_runtime = AgentFilesystemRuntime::new_for_test();
        let p3_runtime = AgentFilesystemRuntime::new_for_test();
        let p2_descriptor = Descriptor::File(test_file(writable_path.clone()));
        let p3_descriptor = Descriptor::File(test_file(writable_path));
        let p2_setup = prepare_p2_write_stream_setup(&p2_descriptor, &p2_runtime).unwrap();
        let p3_setup = prepare_p3_write_stream_setup(&p3_descriptor, &p3_runtime).unwrap();
        assert!(p2_runtime.has_active_effects());
        assert!(p3_runtime.has_active_effects());
        let _p2_pause = p2_runtime.pause_effect_admission();
        let _p3_pause = p3_runtime.pause_effect_admission();

        assert!(matches!(
            prepare_p2_write_stream_setup(&p2_descriptor, &p2_runtime),
            Err(P2WriteStreamSetupError::Trap)
        ));
        assert!(matches!(
            prepare_p3_write_stream_setup(&p3_descriptor, &p3_runtime),
            Err(P3WriteStreamSetupError::Trap)
        ));
        drop((p2_setup, p3_setup));
        assert!(!p2_runtime.has_active_effects());
        assert!(!p3_runtime.has_active_effects());
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
