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

use std::io::{Seek, SeekFrom, Write};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, run_read_access, wasi_filesystem_view,
};
use crate::workerctx::WorkerCtx;
use cap_std::fs::FileExt;
use golem_common::model::oplog::host_functions::{
    P3FilesystemTypesDescriptorStat, P3FilesystemTypesDescriptorStatAt,
};
use golem_common::model::oplog::types::{SerializableFileTimes, SerializableP3FileSystemError};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestFileSystemPath, HostResponseP3FileSystemStat, OplogEntry,
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

static FILESYSTEM_APPEND_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();

struct FilesystemWriteChunk {
    contents: Vec<u8>,
    result_tx: tokio::sync::oneshot::Sender<Result<(), types::ErrorCode>>,
}

struct FilesystemWriteConsumer {
    chunks_tx: Option<tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>>,
    pending_chunk: Option<(
        usize,
        tokio::sync::oneshot::Receiver<Result<(), types::ErrorCode>>,
    )>,
}

impl FilesystemWriteConsumer {
    fn new(chunks_tx: tokio::sync::mpsc::UnboundedSender<FilesystemWriteChunk>) -> Self {
        Self {
            chunks_tx: Some(chunks_tx),
            pending_chunk: None,
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
        _finish: bool,
    ) -> Poll<wasmtime::Result<StreamResult>> {
        let mut src = src.as_direct(store);

        loop {
            // Wait for the in-flight chunk to be persisted before reading more.
            // The receiver must be polled here (not just stored) so its waker is
            // registered; otherwise the write task's completion notification
            // could be missed, hanging the stream.
            if let Some((len, result_rx)) = &mut self.pending_chunk {
                match Pin::new(result_rx).poll(cx) {
                    Poll::Pending => return Poll::Pending,
                    Poll::Ready(Ok(Ok(()))) => {
                        let len = *len;
                        self.pending_chunk = None;
                        src.mark_read(len);
                        return Poll::Ready(Ok(StreamResult::Completed));
                    }
                    Poll::Ready(Ok(Err(_))) | Poll::Ready(Err(_)) => {
                        let len = *len;
                        self.pending_chunk = None;
                        self.chunks_tx.take();
                        src.mark_read(len);
                        return Poll::Ready(Ok(StreamResult::Dropped));
                    }
                }
            }

            let bytes = src.remaining();
            if bytes.is_empty() {
                return Poll::Ready(Ok(StreamResult::Completed));
            }

            let len = bytes.len();
            let Some(chunks_tx) = &self.chunks_tx else {
                src.mark_read(len);
                return Poll::Ready(Ok(StreamResult::Dropped));
            };

            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            chunks_tx
                .send(FilesystemWriteChunk {
                    contents: bytes.to_vec(),
                    result_tx,
                })
                .map_err(|_| wasmtime::Error::msg("filesystem write task dropped"))?;
            self.pending_chunk = Some((len, result_rx));
            // Loop back to poll the freshly created receiver and register its waker.
        }
    }
}

impl Drop for FilesystemWriteConsumer {
    fn drop(&mut self) {
        self.chunks_tx.take();
    }
}

#[derive(Clone, Copy)]
enum FilesystemWriteMode {
    At(types::Filesize),
    Append,
}

struct FilesystemWriteTask<Ctx> {
    file: File,
    mode: FilesystemWriteMode,
    chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
    result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    _phantom: PhantomData<fn() -> Ctx>,
}

impl<Ctx> FilesystemWriteTask<Ctx> {
    fn new(
        file: File,
        mode: FilesystemWriteMode,
        chunks_rx: tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
        result_tx: tokio::sync::oneshot::Sender<wasmtime::Result<Result<(), types::ErrorCode>>>,
    ) -> Self {
        Self {
            file,
            mode,
            chunks_rx,
            result_tx,
            _phantom: PhantomData,
        }
    }
}

impl<Ctx, U> AccessorTask<U, DurableP3<Ctx>> for FilesystemWriteTask<Ctx>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    async fn run(self, accessor: &Accessor<U, DurableP3<Ctx>>) -> wasmtime::Result<()> {
        let FilesystemWriteTask {
            file,
            mode,
            mut chunks_rx,
            result_tx,
            _phantom,
        } = self;
        let result =
            run_streaming_filesystem_write::<Ctx, U>(accessor, &file, mode, &mut chunks_rx).await;
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

/// Returns the size of the file referenced by `fd`, or `0` if it cannot be
/// stat-ed. Uses the underlying (non-durable) host stat so it produces no oplog
/// side effects; it is only used to compute storage-quota deltas, mirroring the
/// WASI P2 implementation.
async fn descriptor_size<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    fd: Resource<Descriptor>,
) -> u64
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let filesystem = accessor.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
    match <WasiFilesystem as types::HostDescriptorWithStore<U>>::stat(&filesystem, fd).await {
        Ok(stat) => stat.size,
        Err(_) => 0,
    }
}

/// Returns the size of the file at `path` relative to `fd`, or `0` if it cannot
/// be stat-ed. Uses the underlying (non-durable) host stat so it produces no
/// oplog side effects; it is only used to compute storage-quota deltas,
/// mirroring the WASI P2 implementation.
async fn descriptor_size_at<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    fd: Resource<Descriptor>,
    path_flags: types::PathFlags,
    path: String,
) -> u64
where
    Ctx: WorkerCtx,
    U: Send + 'static,
{
    let filesystem = accessor.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
    match <WasiFilesystem as types::HostDescriptorWithStore<U>>::stat_at(
        &filesystem,
        fd,
        path_flags,
        path,
    )
    .await
    {
        Ok(stat) => stat.size,
        Err(_) => 0,
    }
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

#[derive(Clone, Copy)]
struct FilesystemStorageReservation {
    base_size: Option<u64>,
    reserved_growth: u64,
}

async fn filesystem_file_size(file: &File) -> Option<u64> {
    let file = Arc::clone(&file.file);
    spawn_blocking(move || file.metadata().map(|metadata| metadata.len()).ok()).await
}

async fn reserve_filesystem_write_storage<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    file: &File,
    mode: FilesystemWriteMode,
    write_len: u64,
) -> wasmtime::Result<FilesystemStorageReservation>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let base_size = filesystem_file_size(file).await;
    let reserved_growth = match (base_size, mode) {
        (Some(current_size), FilesystemWriteMode::At(offset)) => offset
            .saturating_add(write_len)
            .saturating_sub(current_size),
        (Some(current_size), FilesystemWriteMode::Append) => current_size
            .saturating_add(write_len)
            .saturating_sub(current_size),
        (None, _) => write_len,
    };

    reserve_filesystem_storage_bytes::<Ctx, U>(accessor, reserved_growth).await?;

    Ok(FilesystemStorageReservation {
        base_size,
        reserved_growth,
    })
}

/// Reserve `bytes` of filesystem storage quota: check the per-agent limit,
/// acquire executor-wide permits, and record the growth in the oplog. No-op for
/// `bytes == 0` and during replay (the helpers short-circuit). On acquisition
/// failure the optimistic reservation is rolled back and the error is returned.
async fn reserve_filesystem_storage_bytes<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    bytes: u64,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    if bytes == 0 {
        return Ok(());
    }

    if let Some(worker) = accessor
        .with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut())
                .prepare_filesystem_storage_reservation(bytes)
        })
        .map_err(wasmtime::Error::from_anyhow)?
    {
        if let Err(error) = worker.acquire_filesystem_storage_space(bytes).await {
            accessor.with(|mut access| {
                durable_worker_ctx::<Ctx, U>(access.data_mut())
                    .rollback_filesystem_storage_reservation(bytes);
            });
            return Err(wasmtime::Error::from_anyhow(error));
        }
        worker
            .add_to_oplog(OplogEntry::filesystem_storage_usage_update(bytes as i64))
            .await;
        accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut())
                .finish_filesystem_storage_reservation(bytes);
        });
    }

    Ok(())
}

async fn release_filesystem_write_storage<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    bytes: u64,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    if bytes == 0 {
        return Ok(());
    }

    if let Some((worker, bytes)) = accessor.with(|mut access| {
        durable_worker_ctx::<Ctx, U>(access.data_mut()).prepare_filesystem_storage_release(bytes)
    }) {
        worker
            .add_to_oplog(OplogEntry::filesystem_storage_usage_update(-(bytes as i64)))
            .await;
        worker.release_filesystem_storage_space(bytes).await;
        accessor.with(|mut access| {
            durable_worker_ctx::<Ctx, U>(access.data_mut())
                .finish_filesystem_storage_release(bytes);
        });
    }

    Ok(())
}

async fn reconcile_filesystem_write_storage<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    file: &File,
    reservation: FilesystemStorageReservation,
    write_result: &Result<(), types::ErrorCode>,
) -> wasmtime::Result<()>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    if reservation.reserved_growth == 0 {
        return Ok(());
    }

    if write_result.is_err() {
        return release_filesystem_write_storage::<Ctx, U>(accessor, reservation.reserved_growth)
            .await;
    }

    let Some(base_size) = reservation.base_size else {
        return Ok(());
    };
    let Some(actual_end) = filesystem_file_size(file).await else {
        return Ok(());
    };

    let actual_growth = actual_end.saturating_sub(base_size);
    let over_reserved = reservation.reserved_growth.saturating_sub(actual_growth);
    release_filesystem_write_storage::<Ctx, U>(accessor, over_reserved).await
}

// Drains and writes the guest's data stream to the worker filesystem chunk by
// chunk, mirroring the WASI P2 behavior: the file effect is driven entirely by
// the input stream finishing or erroring, never by the liveness of the returned
// result future. The bytes themselves are not recorded in the oplog; on replay
// the guest re-issues the same writes which deterministically rebuild the
// transient worker filesystem. Storage-quota deltas are reserved and reconciled
// per chunk (no-ops during replay).
async fn run_streaming_filesystem_write<Ctx, U>(
    accessor: &Accessor<U, DurableP3<Ctx>>,
    file: &File,
    mode: FilesystemWriteMode,
    chunks_rx: &mut tokio::sync::mpsc::UnboundedReceiver<FilesystemWriteChunk>,
) -> wasmtime::Result<Result<(), types::ErrorCode>>
where
    Ctx: WorkerCtx,
    U: 'static,
{
    let mut result = Ok(());
    let mut position = match mode {
        FilesystemWriteMode::At(offset) => Some(offset),
        FilesystemWriteMode::Append => None,
    };

    while let Some(chunk) = chunks_rx.recv().await {
        if result.is_ok() {
            let chunk_mode = match position {
                Some(offset) => FilesystemWriteMode::At(offset),
                None => FilesystemWriteMode::Append,
            };
            let write_len = chunk.contents.len() as u64;
            let reservation =
                reserve_filesystem_write_storage::<Ctx, U>(accessor, file, chunk_mode, write_len)
                    .await?;

            let (written_len, write_result) =
                run_live_filesystem_write_chunk(file.clone(), chunk_mode, chunk.contents).await;
            reconcile_filesystem_write_storage::<Ctx, U>(
                accessor,
                file,
                reservation,
                &write_result,
            )
            .await?;
            if let Some(offset) = &mut position {
                *offset = offset.saturating_add(written_len);
            }
            result = write_result;
        }

        let _ = chunk.result_tx.send(result.clone());
    }

    Ok(result)
}

async fn run_live_filesystem_write_chunk(
    file: File,
    mode: FilesystemWriteMode,
    contents: Vec<u8>,
) -> (u64, Result<(), types::ErrorCode>) {
    let _append_guard = if matches!(mode, FilesystemWriteMode::Append) {
        Some(
            FILESYSTEM_APPEND_LOCK
                .get_or_init(|| tokio::sync::Mutex::new(()))
                .lock()
                .await,
        )
    } else {
        None
    };
    let file = Arc::clone(&file.file);
    let (contents, written, result) = spawn_blocking(move || match mode {
        FilesystemWriteMode::At(mut offset) => {
            let mut written = 0;
            while written < contents.len() {
                match file.write_at(&contents[written..], offset) {
                    Ok(0) => {
                        return (
                            contents,
                            written,
                            Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                        );
                    }
                    Ok(n) => {
                        written += n;
                        let n = match u64::try_from(n) {
                            Ok(n) => n,
                            Err(_) => {
                                return (
                                    contents,
                                    written,
                                    Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
                                );
                            }
                        };
                        offset = match offset.checked_add(n) {
                            Some(offset) => offset,
                            None => {
                                return (
                                    contents,
                                    written,
                                    Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
                                );
                            }
                        };
                    }
                    Err(error) => return (contents, written, Err(error)),
                }
            }
            (contents, written, Ok(()))
        }
        FilesystemWriteMode::Append => {
            let mut file = file.as_ref();
            if let Err(error) = file.seek(SeekFrom::End(0)) {
                return (contents, 0, Err(error));
            }
            let mut written = 0;
            while written < contents.len() {
                match file.write(&contents[written..]) {
                    Ok(0) => {
                        return (
                            contents,
                            written,
                            Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                        );
                    }
                    Ok(n) => {
                        written += n;
                    }
                    Err(error) => return (contents, written, Err(error)),
                }
            }
            (contents, written, Ok(()))
        }
    })
    .await;

    let written = written.min(contents.len());
    (
        written as u64,
        result.map_err(|error: std::io::Error| error.into()),
    )
}

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: FilesystemError) -> wasmtime::Result<types::ErrorCode> {
        types::Host::convert_error_code(&mut WasiFilesystemView::filesystem(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostDescriptor for DurableP3View<'_, Ctx> {
    fn drop(&mut self, fd: Resource<Descriptor>) -> wasmtime::Result<()> {
        types::HostDescriptor::drop(&mut WasiFilesystemView::filesystem(self.0), fd)
    }
}

impl<Ctx: WorkerCtx> preopens::Host for DurableP3View<'_, Ctx> {
    fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        preopens::Host::get_directories(&mut WasiFilesystemView::filesystem(self.0))
    }
}

impl<U: Send + 'static, Ctx: WorkerCtx> types::HostDescriptorWithStore<U> for DurableP3<Ctx> {
    async fn read_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
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
        accessor
            .with(|mut store| data.pipe(&mut store, FilesystemWriteConsumer::new(chunks_tx)))?;

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        accessor.with(|mut store| {
            store.spawn(FilesystemWriteTask::<Ctx>::new(
                file,
                FilesystemWriteMode::At(offset),
                chunks_rx,
                result_tx,
            ));

            FutureReader::new(&mut store, wait_filesystem_task_result(result_rx))
        })
    }

    async fn append_via_stream(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
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
        accessor
            .with(|mut store| data.pipe(&mut store, FilesystemWriteConsumer::new(chunks_tx)))?;

        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        accessor.with(|mut store| {
            store.spawn(FilesystemWriteTask::<Ctx>::new(
                file,
                FilesystemWriteMode::Append,
                chunks_rx,
                result_tx,
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
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::sync_data(&store, fd).await
    }

    async fn get_flags(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorFlags> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::get_flags(&store, fd).await
    }

    async fn get_type(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorType> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::get_type(&store, fd).await
    }

    async fn set_size(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        size: types::Filesize,
    ) -> FilesystemResult<()> {
        // Charge growth before resizing and credit shrink afterwards, matching
        // the WASI P2 storage-quota accounting. The quota helpers are no-ops
        // during replay, so the storage usage is rebuilt purely from the oplog.
        let current_size =
            descriptor_size::<Ctx, U>(accessor, Resource::new_borrow(fd.rep())).await;
        let growth = size.saturating_sub(current_size);
        if growth > 0 {
            reserve_filesystem_storage_bytes::<Ctx, U>(accessor, growth)
                .await
                .map_err(FilesystemError::trap)?;
        }

        let result = {
            let store = accessor.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
            <WasiFilesystem as types::HostDescriptorWithStore<U>>::set_size(&store, fd, size).await
        };

        if growth > 0 {
            if result.is_err() {
                release_filesystem_write_storage::<Ctx, U>(accessor, growth)
                    .await
                    .map_err(FilesystemError::trap)?;
            }
        } else if result.is_ok() && size < current_size {
            release_filesystem_write_storage::<Ctx, U>(accessor, current_size - size)
                .await
                .map_err(FilesystemError::trap)?;
        }

        result
    }

    async fn set_times(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::set_times(
            &store,
            fd,
            data_access_timestamp,
            data_modification_timestamp,
        )
        .await
    }

    async fn read_directory(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> wasmtime::Result<(
        StreamReader<types::DirectoryEntry>,
        FutureReader<Result<(), types::ErrorCode>>,
    )> {
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
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::sync(&store, fd).await
    }

    async fn create_directory_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::create_directory_at(&store, fd, path)
            .await
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
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::set_times_at(
            &store,
            fd,
            path_flags,
            path,
            data_access_timestamp,
            data_modification_timestamp,
        )
        .await
    }

    async fn link_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path_flags: types::PathFlags,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::link_at(
            &store,
            fd,
            old_path_flags,
            old_path,
            new_fd,
            new_path,
        )
        .await
    }

    async fn open_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        open_flags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FilesystemResult<Resource<Descriptor>> {
        // Opening with TRUNCATE discards the existing file contents, so credit
        // the freed bytes back to the storage quota on success, matching WASI
        // P2. The release helper is a no-op during replay.
        let truncated_size = if open_flags.contains(types::OpenFlags::TRUNCATE) {
            descriptor_size_at::<Ctx, U>(
                accessor,
                Resource::new_borrow(fd.rep()),
                path_flags,
                path.clone(),
            )
            .await
        } else {
            0
        };

        let result = {
            let store = accessor.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
            <WasiFilesystem as types::HostDescriptorWithStore<U>>::open_at(
                &store, fd, path_flags, path, open_flags, flags,
            )
            .await
        };

        if result.is_ok() && truncated_size > 0 {
            release_filesystem_write_storage::<Ctx, U>(accessor, truncated_size)
                .await
                .map_err(FilesystemError::trap)?;
        }

        result
    }

    async fn readlink_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<String> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::readlink_at(&store, fd, path).await
    }

    async fn remove_directory_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::remove_directory_at(&store, fd, path)
            .await
    }

    async fn rename_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::rename_at(
            &store, fd, old_path, new_fd, new_path,
        )
        .await
    }

    async fn symlink_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::symlink_at(
            &store, fd, old_path, new_path,
        )
        .await
    }

    async fn unlink_file_at(
        accessor: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        // Stat the file before unlinking so the freed bytes can be credited back
        // to the storage quota on success, matching WASI P2. The release helper
        // is a no-op during replay.
        let file_size = descriptor_size_at::<Ctx, U>(
            accessor,
            Resource::new_borrow(fd.rep()),
            types::PathFlags::empty(),
            path.clone(),
        )
        .await;

        let result = {
            let store = accessor.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
            <WasiFilesystem as types::HostDescriptorWithStore<U>>::unlink_file_at(&store, fd, path)
                .await
        };

        if result.is_ok() && file_size > 0 {
            release_filesystem_write_storage::<Ctx, U>(accessor, file_size)
                .await
                .map_err(FilesystemError::trap)?;
        }

        result
    }

    async fn is_same_object(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::is_same_object(&store, fd, other)
            .await
    }

    async fn metadata_hash(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::MetadataHashValue> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::metadata_hash(&store, fd).await
    }

    async fn metadata_hash_at(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::MetadataHashValue> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore<U>>::metadata_hash_at(
            &store, fd, path_flags, path,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    async fn p3_fs_live_write_chunk_at_offset_writes_bytes() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let path = tempdir.path().join("out");
        let file = test_file(path.clone());

        let (written, result) = run_live_filesystem_write_chunk(
            file.clone(),
            FilesystemWriteMode::At(0),
            b"hello".to_vec(),
        )
        .await;
        assert_eq!(written, 5);
        assert!(result.is_ok());

        let (written, result) =
            run_live_filesystem_write_chunk(file, FilesystemWriteMode::At(5), b" world".to_vec())
                .await;
        assert_eq!(written, 6);
        assert!(result.is_ok());

        assert_eq!(std::fs::read(&path).unwrap(), b"hello world");
    }

    #[test]
    async fn p3_fs_live_write_chunk_append_writes_bytes() {
        let tempdir = tempfile::TempDir::new().unwrap();
        let path = tempdir.path().join("out");
        let file = test_file(path.clone());

        for chunk in [b"foo".to_vec(), b"bar".to_vec(), b"baz".to_vec()] {
            let len = chunk.len() as u64;
            let (written, result) =
                run_live_filesystem_write_chunk(file.clone(), FilesystemWriteMode::Append, chunk)
                    .await;
            assert_eq!(written, len);
            assert!(result.is_ok());
        }

        assert_eq!(std::fs::read(&path).unwrap(), b"foobarbaz");
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
