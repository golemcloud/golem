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

use std::collections::{HashMap, HashSet};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use bytes::Bytes;
use cap_std::fs::FileExt;
use futures::FutureExt;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogEntry;
use wasmtime_wasi::p2::{DynOutputStream, OutputStream, Pollable};
use wasmtime_wasi::{StreamError, StreamResult};

use crate::metrics::storage::{
    STORAGE_TYPE_FILESYSTEM, record_storage_bytes_deleted, record_storage_bytes_written,
};
use crate::services::agent_storage_meter::AgentStorageMeter;
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;

const FILE_WRITE_CAPACITY: usize = 1024 * 1024;

#[derive(Clone)]
pub(crate) struct DurableFilesystem {
    state: Arc<Mutex<DurableFilesystemState>>,
    // Namespace mutations and file-size accounting form one transaction domain. Path-keyed
    // locks are not sufficient because hard links and open descriptors alias filesystem objects.
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
    storage_meter: AgentStorageMeter,
}

#[derive(Default)]
struct DurableFilesystemState {
    tracked_objects: HashMap<(u64, u64), TrackedFilesystemObject>,
    output_streams: HashMap<u32, FilesystemOutputStreamState>,
    p2_descriptors: HashSet<u32>,
    p3_descriptors: HashSet<u32>,
}

#[derive(Default)]
struct TrackedFilesystemObject {
    handles: Vec<Weak<cap_std::fs::File>>,
    unlinked: bool,
}

#[derive(Clone, Copy)]
pub(crate) enum FilesystemPreview {
    P2,
    P3,
}

#[derive(Debug)]
pub(crate) struct PendingFilesystemReservation {
    pub base_size: u64,
    pub reservation: Option<FilesystemStorageReservation>,
}

#[derive(Debug)]
pub(crate) struct FilesystemOutputStreamState {
    pub mutation_path: PathBuf,
    pub file: Arc<cap_std::fs::File>,
    pub position: Option<u64>,
    pub pending_write: bool,
    pub pending_reservation: Option<PendingFilesystemReservation>,
    completion: Option<FilesystemWriteCompletion>,
}

pub(crate) struct FilesystemOutputStreamSnapshot {
    pub mutation_path: PathBuf,
    pub file: Arc<cap_std::fs::File>,
    pub position: Option<u64>,
    pub has_pending_write: bool,
}

impl FilesystemOutputStreamState {
    pub(crate) fn new(
        mutation_path: PathBuf,
        file: Arc<cap_std::fs::File>,
        position: Option<u64>,
    ) -> Self {
        Self {
            mutation_path,
            file,
            position,
            pending_write: false,
            pending_reservation: None,
            completion: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum FilesystemWriteMode {
    At(u64),
    Append,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FilesystemSizeChange {
    Grow(u64),
    Shrink(u64),
    Unchanged,
}

impl FilesystemSizeChange {
    pub(crate) fn between(current_size: u64, new_size: u64) -> Self {
        match new_size.cmp(&current_size) {
            std::cmp::Ordering::Greater => Self::Grow(new_size - current_size),
            std::cmp::Ordering::Less => Self::Shrink(current_size - new_size),
            std::cmp::Ordering::Equal => Self::Unchanged,
        }
    }
}

pub(crate) struct FilesystemEntryStorage {
    pub identity: Option<(u64, u64)>,
    pub size: u64,
    pub is_regular_file: bool,
    pub link_count: u64,
}

impl FilesystemEntryStorage {
    pub(crate) fn released_by_unlink(self) -> (u64, Option<(u64, u64)>) {
        if self.identity.is_some() && self.is_regular_file && self.link_count == 1 {
            (self.size, self.identity)
        } else {
            (0, None)
        }
    }
}

pub(crate) fn replaced_file_storage(
    source_identity: Option<(u64, u64)>,
    destination: FilesystemEntryStorage,
) -> (u64, Option<(u64, u64)>) {
    match (source_identity, destination.identity) {
        (Some(source), Some(destination_identity))
            if source != destination_identity
                && destination.is_regular_file
                && destination.link_count == 1 =>
        {
            (destination.size, Some(destination_identity))
        }
        _ => (0, None),
    }
}

pub(crate) async fn directory_entry_identity(
    directory: wasmtime_wasi::filesystem::Dir,
    path: PathBuf,
) -> std::io::Result<Option<(u64, u64)>> {
    let directory = directory.dir;
    tokio::task::spawn_blocking(move || {
        directory
            .symlink_metadata(path)
            .map(|metadata| metadata_object_identity(&metadata))
    })
    .await
    .map_err(std::io::Error::other)?
}

#[cfg(unix)]
fn metadata_object_identity(metadata: &cap_std::fs::Metadata) -> Option<(u64, u64)> {
    use cap_std::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(windows)]
fn metadata_object_identity(metadata: &cap_std::fs::Metadata) -> Option<(u64, u64)> {
    use cap_std::fs::MetadataExt;
    metadata
        .volume_serial_number()
        .zip(metadata.file_index())
        .map(|(volume, index)| (u64::from(volume), index))
}

#[cfg(not(any(unix, windows)))]
fn metadata_object_identity(_metadata: &cap_std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

#[derive(Debug)]
pub(crate) struct FilesystemStorageReservation {
    pub(crate) logical_bytes: u64,
    pub(crate) storage_meter: AgentStorageMeter,
}

impl FilesystemStorageReservation {
    pub(crate) fn logical_bytes(&self) -> u64 {
        self.logical_bytes
    }

    pub(crate) async fn acquire_capacity<Ctx: WorkerCtx>(
        &self,
        worker: &Arc<Worker<Ctx>>,
    ) -> anyhow::Result<()> {
        let storage_meter = &self.storage_meter;
        let _capacity_guard = storage_meter.lock_capacity_acquisition().await;
        let mut requested = storage_meter.capacity_shortfall()?;
        loop {
            if requested == 0 {
                return Ok(());
            }
            match worker.acquire_filesystem_storage_space(requested).await {
                Ok(Some(permit)) => {
                    if storage_meter.merge_capacity(Some(permit)) {
                        return Ok(());
                    }
                    return Err(anyhow::anyhow!(
                        "filesystem storage capacity registration changed during acquisition"
                    ));
                }
                Ok(None) => return Ok(()),
                Err(error) => {
                    let latest = storage_meter.capacity_shortfall()?;
                    if latest < requested {
                        requested = latest;
                    } else {
                        worker.record_desired_extra_filesystem_storage(requested);
                        return Err(error);
                    }
                }
            }
        }
    }

    pub(crate) fn shrink(&mut self, bytes: u64) {
        let bytes = bytes.min(self.logical_bytes);
        if bytes == 0 {
            return;
        }
        self.storage_meter.shrink_reservation(bytes);
        self.logical_bytes = self.logical_bytes.saturating_sub(bytes);
    }

    pub(crate) fn shrink_to(&mut self, bytes: u64) {
        self.shrink(self.logical_bytes.saturating_sub(bytes));
    }
}

impl Drop for FilesystemStorageReservation {
    fn drop(&mut self) {
        if self.logical_bytes > 0 {
            self.storage_meter.rollback_reservation(self.logical_bytes);
        }
    }
}

#[derive(Clone)]
pub(crate) struct FilesystemStorageAccounting<Ctx: WorkerCtx> {
    worker: Arc<Worker<Ctx>>,
    storage_meter: AgentStorageMeter,
    account_id: AccountId,
    environment_id: EnvironmentId,
}

impl<Ctx: WorkerCtx> FilesystemStorageAccounting<Ctx> {
    pub(crate) fn new(
        worker: Arc<Worker<Ctx>>,
        storage_meter: AgentStorageMeter,
        account_id: AccountId,
        environment_id: EnvironmentId,
    ) -> Self {
        Self {
            worker,
            storage_meter,
            account_id,
            environment_id,
        }
    }

    pub(crate) async fn reserve(
        &self,
        bytes: u64,
        limit: u64,
    ) -> anyhow::Result<FilesystemStorageReservation> {
        let guard = self.storage_meter.lock_reservation().await;
        if self.storage_meter.reserve(bytes, limit).is_none() {
            return Err(anyhow::anyhow!(
                golem_service_base::error::worker_executor::GolemSpecificWasmTrap::WorkerAgentExceededFilesystemStorageLimit
            ));
        }
        let reservation = FilesystemStorageReservation {
            logical_bytes: bytes,
            storage_meter: self.storage_meter.clone(),
        };
        drop(guard);
        reservation.acquire_capacity(&self.worker).await?;
        Ok(reservation)
    }

    pub(crate) async fn commit(
        &self,
        mut reservation: FilesystemStorageReservation,
        committed_bytes: u64,
    ) {
        let _guard = self.storage_meter.lock_reservation().await;
        let committed_bytes = committed_bytes.min(reservation.logical_bytes);
        if committed_bytes == 0 {
            reservation.storage_meter.commit_reservation(
                reservation.logical_bytes,
                0,
                std::time::Instant::now(),
            );
            reservation.logical_bytes = 0;
            return;
        }
        self.persist_bytes(committed_bytes, true).await;
        reservation.storage_meter.commit_reservation(
            reservation.logical_bytes,
            committed_bytes,
            std::time::Instant::now(),
        );
        reservation.logical_bytes = 0;
    }

    pub(crate) async fn release(&self, requested_bytes: u64) {
        let _guard = self.storage_meter.lock_reservation().await;
        let freed_bytes = requested_bytes.min(self.storage_meter.current_bytes());
        if freed_bytes == 0 {
            return;
        }
        self.persist_bytes(freed_bytes, false).await;
        self.storage_meter
            .on_release(freed_bytes, std::time::Instant::now());
    }

    async fn persist_bytes(&self, bytes: u64, acquire: bool) {
        let mut remaining = bytes;
        while remaining > 0 {
            let chunk = remaining.min(i64::MAX as u64);
            let delta = if acquire {
                chunk as i64
            } else {
                -(chunk as i64)
            };
            self.worker
                .add_to_oplog(OplogEntry::filesystem_storage_usage_update(delta))
                .await;
            remaining -= chunk;
        }
        if acquire {
            record_storage_bytes_written(
                STORAGE_TYPE_FILESYSTEM,
                &self.account_id.to_string(),
                &self.environment_id.to_string(),
                bytes,
            );
        } else {
            record_storage_bytes_deleted(
                STORAGE_TYPE_FILESYSTEM,
                &self.account_id.to_string(),
                &self.environment_id.to_string(),
                bytes,
            );
        }
    }
}

#[derive(Clone)]
pub(crate) struct FilesystemWriteCompletion {
    outcome: futures::future::Shared<futures::future::BoxFuture<'static, FilesystemWriteTerminal>>,
    writer_abort: tokio::task::AbortHandle,
}

#[derive(Clone, Debug)]
enum FilesystemWriteTerminal {
    Completed(FilesystemWriteOutcome),
    Cancelled,
}

#[derive(Clone, Debug)]
struct FilesystemWriteOutcome {
    written: u64,
    file_size: Option<u64>,
    error: Option<FilesystemWriteError>,
}

#[derive(Clone, Debug)]
struct FilesystemWriteError {
    kind: std::io::ErrorKind,
    raw_os_error: Option<i32>,
    message: Arc<str>,
}

impl FilesystemWriteOutcome {
    fn from_result(written: u64, result: std::io::Result<()>) -> Self {
        match result {
            Ok(()) => Self {
                written,
                file_size: None,
                error: None,
            },
            Err(error) => Self {
                written,
                file_size: None,
                error: Some(FilesystemWriteError {
                    kind: error.kind(),
                    raw_os_error: error.raw_os_error(),
                    message: Arc::from(error.to_string()),
                }),
            },
        }
    }

    fn result(&self) -> std::io::Result<u64> {
        match &self.error {
            Some(error) => Err(match error.raw_os_error {
                Some(raw_os_error) => std::io::Error::from_raw_os_error(raw_os_error),
                None => std::io::Error::new(error.kind, error.message.to_string()),
            }),
            None => Ok(self.written),
        }
    }

    fn with_file_size(mut self, file: &cap_std::fs::File) -> Self {
        self.file_size = file.metadata().ok().map(|metadata| metadata.len());
        self
    }
}

impl std::fmt::Debug for FilesystemWriteCompletion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FilesystemWriteCompletion")
            .finish_non_exhaustive()
    }
}

impl FilesystemWriteCompletion {
    fn from_writer_task(writer_task: tokio::task::JoinHandle<FilesystemWriteOutcome>) -> Self {
        let writer_abort = writer_task.abort_handle();
        let outcome = async move {
            match writer_task.await {
                Ok(outcome) => FilesystemWriteTerminal::Completed(outcome),
                Err(error) if error.is_cancelled() => FilesystemWriteTerminal::Cancelled,
                Err(error) => {
                    FilesystemWriteTerminal::Completed(FilesystemWriteOutcome::from_result(
                        0,
                        Err(std::io::Error::other(format!(
                            "filesystem write task ended without a result: {error}"
                        ))),
                    ))
                }
            }
        }
        .boxed()
        .shared();
        Self {
            outcome,
            writer_abort,
        }
    }

    async fn wait(self) -> std::io::Result<u64> {
        match self.resolve().await {
            FilesystemWriteTerminal::Completed(outcome) => outcome.result(),
            FilesystemWriteTerminal::Cancelled => Ok(0),
        }
    }

    async fn resolve(self) -> FilesystemWriteTerminal {
        self.outcome.await
    }

    fn request_cancel(&self) {
        self.writer_abort.abort();
    }

    fn is_ready(&self) -> bool {
        self.outcome.peek().is_some()
    }
}

pub(crate) struct SettledFilesystemWrite {
    pub stream_rep: u32,
    pub actual_growth: Option<u64>,
    pub result: std::io::Result<()>,
}

pub(crate) struct PreparedFilesystemWriteSettlement {
    pub reservation: Option<(FilesystemStorageReservation, u64)>,
    pub error: Option<std::io::Error>,
}

pub(crate) fn rewrite_descendant_path(path: &mut PathBuf, old_path: &Path, new_path: &Path) {
    if let Ok(remainder) = path.strip_prefix(old_path) {
        *path = new_path.join(remainder);
    }
}

impl DurableFilesystem {
    pub(crate) fn new(storage_meter: AgentStorageMeter) -> Self {
        Self {
            state: Arc::new(Mutex::new(DurableFilesystemState::default())),
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            storage_meter,
        }
    }

    pub(crate) fn storage_meter(&self) -> AgentStorageMeter {
        self.storage_meter.clone()
    }

    pub(crate) fn mark_object_unlinked(&self, identity: Option<(u64, u64)>) {
        if let Some(identity) = identity {
            let mut state = self.state.lock().unwrap();
            prune_tracked_objects(&mut state);
            if let Some(object) = state.tracked_objects.get_mut(&identity) {
                object.unlinked = true;
            }
        }
    }

    pub(crate) async fn mark_file_linked(&self, file: Arc<cap_std::fs::File>) {
        if let Some(identity) = file_object_identity(file.clone()).await {
            let mut state = self.state.lock().unwrap();
            prune_tracked_objects(&mut state);
            let object = state.tracked_objects.entry(identity).or_default();
            object.unlinked = false;
            let handle = Arc::downgrade(&file);
            if !object
                .handles
                .iter()
                .any(|existing| existing.ptr_eq(&handle))
            {
                object.handles.push(handle);
            }
        }
    }

    pub(crate) async fn is_file_unlinked(&self, file: Arc<cap_std::fs::File>) -> bool {
        self.file_accounting_snapshot(file)
            .await
            .is_ok_and(|snapshot| snapshot.0)
    }

    pub(crate) async fn file_accounting_snapshot(
        &self,
        file: Arc<cap_std::fs::File>,
    ) -> std::io::Result<(bool, u64)> {
        let handle = Arc::downgrade(&file);
        let snapshot = tokio::task::spawn_blocking(move || {
            file.metadata()
                .map(|metadata| (metadata_object_identity(&metadata), metadata.len()))
        })
        .await
        .map_err(std::io::Error::other)??;
        let (identity, size) = snapshot;
        let unlinked = identity.is_some_and(|identity| {
            let mut state = self.state.lock().unwrap();
            prune_tracked_objects(&mut state);
            state.tracked_objects.get(&identity).is_some_and(|object| {
                object.unlinked
                    && object
                        .handles
                        .iter()
                        .any(|existing| existing.ptr_eq(&handle))
            })
        });
        Ok((unlinked, size))
    }

    pub(crate) fn normalized_path(path: &Path) -> PathBuf {
        let mut normalized = PathBuf::new();
        let rooted = path.has_root();
        for component in path.components() {
            match component {
                std::path::Component::CurDir => {}
                std::path::Component::ParentDir => {
                    let can_pop = normalized
                        .file_name()
                        .is_some_and(|name| name != std::ffi::OsStr::new(".."));
                    if can_pop {
                        normalized.pop();
                    } else if !rooted {
                        normalized.push(component.as_os_str());
                    }
                }
                _ => normalized.push(component.as_os_str()),
            }
        }
        normalized
    }

    pub(crate) fn mutation_lock(&self) -> Arc<tokio::sync::Mutex<()>> {
        self.mutation_lock.clone()
    }

    #[cfg(test)]
    pub(crate) fn is_mutation_pending(&self, path: &Path) -> bool {
        let path = Self::normalized_path(path);
        self.state
            .lock()
            .unwrap()
            .output_streams
            .values()
            .any(|stream| stream.pending_write && stream.mutation_path == path)
    }

    pub(crate) fn register_output_stream(
        &self,
        stream_rep: u32,
        mut stream: FilesystemOutputStreamState,
    ) {
        stream.mutation_path = Self::normalized_path(&stream.mutation_path);
        self.state
            .lock()
            .unwrap()
            .output_streams
            .insert(stream_rep, stream);
    }

    pub(crate) fn contains_output_stream(&self, stream_rep: u32) -> bool {
        self.state
            .lock()
            .unwrap()
            .output_streams
            .contains_key(&stream_rep)
    }

    pub(crate) fn register_descriptor(&self, preview: FilesystemPreview, descriptor_rep: u32) {
        let mut state = self.state.lock().unwrap();
        match preview {
            FilesystemPreview::P2 => &mut state.p2_descriptors,
            FilesystemPreview::P3 => &mut state.p3_descriptors,
        }
        .insert(descriptor_rep);
    }

    pub(crate) fn unregister_descriptor(&self, preview: FilesystemPreview, descriptor_rep: u32) {
        let mut state = self.state.lock().unwrap();
        match preview {
            FilesystemPreview::P2 => &mut state.p2_descriptors,
            FilesystemPreview::P3 => &mut state.p3_descriptors,
        }
        .remove(&descriptor_rep);
    }

    pub(crate) fn descriptor_reps(&self, preview: FilesystemPreview) -> Vec<u32> {
        let state = self.state.lock().unwrap();
        match preview {
            FilesystemPreview::P2 => &state.p2_descriptors,
            FilesystemPreview::P3 => &state.p3_descriptors,
        }
        .iter()
        .copied()
        .collect()
    }

    pub(crate) fn output_stream_snapshot(
        &self,
        stream_rep: u32,
    ) -> Option<FilesystemOutputStreamSnapshot> {
        self.state
            .lock()
            .unwrap()
            .output_streams
            .get(&stream_rep)
            .map(|stream| FilesystemOutputStreamSnapshot {
                mutation_path: stream.mutation_path.clone(),
                file: stream.file.clone(),
                position: stream.position,
                has_pending_write: stream.pending_write || stream.pending_reservation.is_some(),
            })
    }

    pub(crate) fn pending_write_is_ready(&self, stream_rep: u32) -> bool {
        self.state
            .lock()
            .unwrap()
            .output_streams
            .get(&stream_rep)
            .and_then(|stream| stream.completion.as_ref())
            .is_some_and(FilesystemWriteCompletion::is_ready)
    }

    pub(crate) fn set_pending_reservation(
        &self,
        stream_rep: u32,
        reservation: PendingFilesystemReservation,
    ) {
        if let Some(stream) = self
            .state
            .lock()
            .unwrap()
            .output_streams
            .get_mut(&stream_rep)
        {
            stream.pending_reservation = Some(reservation);
        }
    }

    pub(crate) fn pending_reservation_base_size(&self, stream_rep: u32) -> Option<u64> {
        self.state
            .lock()
            .unwrap()
            .output_streams
            .get(&stream_rep)
            .and_then(|stream| {
                stream
                    .pending_reservation
                    .as_ref()
                    .map(|pending| pending.base_size)
            })
    }

    pub(crate) fn shrink_pending_reservation(&self, stream_rep: u32, actual_write_len: u64) {
        let mut state = self.state.lock().unwrap();
        let Some(stream) = state.output_streams.get_mut(&stream_rep) else {
            return;
        };
        let requested_end = match stream.position {
            Some(position) => position.saturating_add(actual_write_len),
            None => stream
                .pending_reservation
                .as_ref()
                .map(|pending| pending.base_size)
                .unwrap_or(0)
                .saturating_add(actual_write_len),
        };
        let Some(pending) = stream.pending_reservation.as_mut() else {
            return;
        };
        let actual_growth = requested_end.saturating_sub(pending.base_size);
        if let Some(reservation) = &mut pending.reservation {
            reservation.shrink_to(actual_growth);
        }
    }

    pub(crate) fn mark_write_enqueued(
        &self,
        stream_rep: u32,
        write_len: u64,
        completion: FilesystemWriteCompletion,
    ) {
        let mut state = self.state.lock().unwrap();
        if let Some(stream) = state.output_streams.get_mut(&stream_rep) {
            stream.pending_write = true;
            stream.completion = Some(completion);
            if let Some(position) = &mut stream.position {
                *position = position.saturating_add(write_len);
            }
        }
    }

    pub(crate) fn advance_stream_position(&self, stream_rep: u32, write_len: u64) {
        if let Some(position) = self
            .state
            .lock()
            .unwrap()
            .output_streams
            .get_mut(&stream_rep)
            .and_then(|stream| stream.position.as_mut())
        {
            *position = position.saturating_add(write_len);
        }
    }

    pub(crate) fn rollback_pending_reservation(&self, stream_rep: u32) {
        let pending = self
            .state
            .lock()
            .unwrap()
            .output_streams
            .get_mut(&stream_rep)
            .and_then(|stream| stream.pending_reservation.take());
        drop(pending);
    }

    pub(crate) fn finish_pending_write(
        &self,
        stream_rep: u32,
        actual_growth: Option<u64>,
    ) -> Option<(FilesystemStorageReservation, u64)> {
        let pending = self
            .state
            .lock()
            .unwrap()
            .output_streams
            .get_mut(&stream_rep)
            .map(|stream| {
                stream.pending_write = false;
                stream.completion = None;
                stream.pending_reservation.take()
            })??;

        pending.reservation.map(|reservation| {
            let committed_growth = actual_growth.unwrap_or(0).min(reservation.logical_bytes());
            (reservation, committed_growth)
        })
    }

    pub(crate) fn prepare_write_settlements(
        &self,
        settled: Vec<SettledFilesystemWrite>,
    ) -> Vec<PreparedFilesystemWriteSettlement> {
        settled
            .into_iter()
            .map(|settlement| PreparedFilesystemWriteSettlement {
                reservation: self
                    .finish_pending_write(settlement.stream_rep, settlement.actual_growth),
                error: settlement.result.err(),
            })
            .collect()
    }

    pub(crate) async fn settle_pending_writes(
        &self,
        mutation_path: &Path,
    ) -> Vec<SettledFilesystemWrite> {
        let mutation_path = Self::normalized_path(mutation_path);
        self.settle_pending_writes_matching(|stream_path| stream_path == mutation_path)
            .await
    }

    pub(crate) async fn settle_all_pending_writes(&self) -> Vec<SettledFilesystemWrite> {
        self.settle_pending_writes_matching(|_| true).await
    }

    async fn settle_pending_writes_matching(
        &self,
        matches: impl Fn(&Path) -> bool,
    ) -> Vec<SettledFilesystemWrite> {
        let pending = {
            let state = self.state.lock().unwrap();
            let mut pending = state
                .output_streams
                .iter()
                .filter(|(_, stream)| {
                    (stream.pending_write || stream.pending_reservation.is_some())
                        && matches(&stream.mutation_path)
                })
                .map(|(stream_rep, stream)| {
                    (
                        *stream_rep,
                        stream.file.clone(),
                        stream
                            .pending_reservation
                            .as_ref()
                            .map(|pending| pending.base_size),
                        stream.pending_write,
                        stream.completion.clone(),
                    )
                })
                .collect::<Vec<_>>();
            pending.sort_by_key(|(stream_rep, ..)| *stream_rep);
            pending
        };

        let mut settled = Vec::with_capacity(pending.len());
        for (stream_rep, file, base_size, pending_write, completion) in pending {
            let (write_result, completed_size) = match completion {
                Some(completion) => match completion.resolve().await {
                    FilesystemWriteTerminal::Completed(outcome) => {
                        (outcome.result().map(|_| ()), outcome.file_size)
                    }
                    FilesystemWriteTerminal::Cancelled => (Ok(()), None),
                },
                None if pending_write => (
                    Err(std::io::Error::other(
                        "pending filesystem write has no completion",
                    )),
                    None,
                ),
                None => (Ok(()), None),
            };
            let measurement = match base_size {
                Some(base_size) => match completed_size {
                    Some(size) => Ok(Some(size.saturating_sub(base_size))),
                    None => filesystem_file_size(file)
                        .await
                        .map(|size| Some(size.saturating_sub(base_size))),
                },
                None => Ok(None),
            };
            let actual_growth = measurement.as_ref().ok().copied().flatten();
            let result = write_result.and(measurement.map(|_| ()));
            settled.push(SettledFilesystemWrite {
                stream_rep,
                actual_growth,
                result,
            });
        }
        settled
    }

    pub(crate) fn pending_stream_reps(&self) -> Vec<u32> {
        let mut pending = self
            .state
            .lock()
            .unwrap()
            .output_streams
            .iter()
            .filter_map(|(stream_rep, stream)| stream.pending_write.then_some(*stream_rep))
            .collect::<Vec<_>>();
        pending.sort_unstable();
        pending
    }

    pub(crate) fn remove_output_stream(
        &self,
        stream_rep: u32,
    ) -> Option<FilesystemOutputStreamState> {
        let mut state = self.state.lock().unwrap();
        let stream = state.output_streams.remove(&stream_rep)?;
        Some(stream)
    }

    pub(crate) fn rename_stream_paths(&self, old_path: &Path, new_path: &Path) {
        let old_path = Self::normalized_path(old_path);
        let new_path = Self::normalized_path(new_path);
        let mut state = self.state.lock().unwrap();

        for stream in state.output_streams.values_mut() {
            rewrite_descendant_path(&mut stream.mutation_path, &old_path, &new_path);
        }
    }
}

fn prune_tracked_objects(state: &mut DurableFilesystemState) {
    state.tracked_objects.retain(|_, object| {
        object.handles.retain(|handle| handle.strong_count() > 0);
        !object.handles.is_empty()
    });
}

async fn file_object_identity(file: Arc<cap_std::fs::File>) -> Option<(u64, u64)> {
    tokio::task::spawn_blocking(move || {
        file.metadata()
            .ok()
            .and_then(|metadata| metadata_object_identity(&metadata))
    })
    .await
    .ok()
    .flatten()
}

// Wasmtime's P2 file stream does not expose write completion or the resulting file size. Durable
// accounting needs both before committing reserved growth, so this wrapper preserves the upstream
// nonblocking stream contract while making the actual write outcome observable.
pub(crate) struct DurableFileOutputStream {
    file: Arc<cap_std::fs::File>,
    mode: FilesystemWriteMode,
    state: FileOutputState,
}

enum FileOutputState {
    Ready,
    Waiting(FilesystemWriteCompletion),
    Error(std::io::Error),
    Closed,
}

impl DurableFileOutputStream {
    pub(crate) fn new(file: Arc<cap_std::fs::File>, mode: FilesystemWriteMode) -> Self {
        Self {
            file,
            mode,
            state: FileOutputState::Ready,
        }
    }

    #[cfg(test)]
    fn waiting_for_test(
        file: Arc<cap_std::fs::File>,
        completion: FilesystemWriteCompletion,
    ) -> Self {
        Self {
            file,
            mode: FilesystemWriteMode::At(0),
            state: FileOutputState::Waiting(completion),
        }
    }

    pub(crate) fn into_dyn(self) -> DynOutputStream {
        Box::new(self)
    }

    pub(crate) fn current_completion(&self) -> Option<FilesystemWriteCompletion> {
        match &self.state {
            FileOutputState::Waiting(completion) => Some(completion.clone()),
            _ => None,
        }
    }

    async fn wait_ready(&mut self) {
        let FileOutputState::Waiting(completion) =
            std::mem::replace(&mut self.state, FileOutputState::Closed)
        else {
            return;
        };
        let result = completion.wait().await;
        match result {
            Ok(written) => {
                if let FilesystemWriteMode::At(position) = &mut self.mode {
                    *position = position.saturating_add(written);
                }
                self.state = FileOutputState::Ready;
            }
            Err(error) => self.state = FileOutputState::Error(error),
        }
    }
}

#[async_trait::async_trait]
impl OutputStream for DurableFileOutputStream {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        match self.state {
            FileOutputState::Ready => {}
            FileOutputState::Closed => return Err(StreamError::Closed),
            FileOutputState::Waiting(_) | FileOutputState::Error(_) => {
                return Err(StreamError::Trap(wasmtime::Error::msg(
                    "write not permitted: check_write not called first",
                )));
            }
        }

        let file = self.file.clone();
        let mode = self.mode;
        let writer_task = tokio::task::spawn_blocking(move || {
            let (written, result) = write_file_blocking(&file, mode, &bytes);
            let written = u64::try_from(written).unwrap_or(u64::MAX);
            FilesystemWriteOutcome::from_result(written, result).with_file_size(&file)
        });
        let completion = FilesystemWriteCompletion::from_writer_task(writer_task);
        self.state = FileOutputState::Waiting(completion);
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        match self.state {
            FileOutputState::Ready | FileOutputState::Waiting(_) => Ok(()),
            FileOutputState::Closed => Err(StreamError::Closed),
            FileOutputState::Error(_) => {
                match std::mem::replace(&mut self.state, FileOutputState::Closed) {
                    FileOutputState::Error(error) => {
                        Err(StreamError::LastOperationFailed(error.into()))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        match self.state {
            FileOutputState::Ready => Ok(FILE_WRITE_CAPACITY),
            FileOutputState::Waiting(_) => Ok(0),
            FileOutputState::Closed => Err(StreamError::Closed),
            FileOutputState::Error(_) => {
                match std::mem::replace(&mut self.state, FileOutputState::Closed) {
                    FileOutputState::Error(error) => {
                        Err(StreamError::LastOperationFailed(error.into()))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }

    async fn cancel(&mut self) {
        if let FileOutputState::Waiting(completion) =
            std::mem::replace(&mut self.state, FileOutputState::Closed)
        {
            completion.request_cancel();
            let _ = completion.wait().await;
        }
        self.state = FileOutputState::Closed;
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[async_trait::async_trait]
impl Pollable for DurableFileOutputStream {
    async fn ready(&mut self) {
        if matches!(self.state, FileOutputState::Waiting(_)) {
            self.wait_ready().await;
        }
    }
}

pub(crate) async fn write_file(
    file: Arc<cap_std::fs::File>,
    mode: FilesystemWriteMode,
    contents: Vec<u8>,
) -> (u64, std::io::Result<()>) {
    let contents_len = contents.len();
    let (written, result) =
        tokio::task::spawn_blocking(move || write_file_blocking(&file, mode, &contents))
            .await
            .unwrap_or_else(|error| (0, Err(std::io::Error::other(error))));
    (
        u64::try_from(written.min(contents_len)).unwrap_or(u64::MAX),
        result,
    )
}

fn write_file_blocking(
    file: &cap_std::fs::File,
    mode: FilesystemWriteMode,
    contents: &[u8],
) -> (usize, std::io::Result<()>) {
    match mode {
        FilesystemWriteMode::At(mut offset) => {
            let mut written = 0;
            while written < contents.len() {
                match file.write_at(&contents[written..], offset) {
                    Ok(0) => {
                        return (
                            written,
                            Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                        );
                    }
                    Ok(count) => {
                        written += count;
                        let Ok(count) = u64::try_from(count) else {
                            return (
                                written,
                                Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
                            );
                        };
                        let Some(next_offset) = offset.checked_add(count) else {
                            return (
                                written,
                                Err(std::io::Error::from(std::io::ErrorKind::InvalidData)),
                            );
                        };
                        offset = next_offset;
                    }
                    Err(error) => return (written, Err(error)),
                }
            }
            (written, Ok(()))
        }
        FilesystemWriteMode::Append => {
            let mut file = file;
            if let Err(error) = file.seek(SeekFrom::End(0)) {
                return (0, Err(error));
            }
            let mut written = 0;
            while written < contents.len() {
                match file.write(&contents[written..]) {
                    Ok(0) => {
                        return (
                            written,
                            Err(std::io::Error::from(std::io::ErrorKind::WriteZero)),
                        );
                    }
                    Ok(count) => written += count,
                    Err(error) => return (written, Err(error)),
                }
            }
            (written, Ok(()))
        }
    }
}

pub(crate) async fn filesystem_file_size(file: Arc<cap_std::fs::File>) -> std::io::Result<u64> {
    tokio::task::spawn_blocking(move || file.metadata().map(|metadata| metadata.len()))
        .await
        .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::resource_limits::AtomicResourceEntry;
    use golem_common::model::agent::AgentMode;
    use test_r::{test, timeout};

    fn test_filesystem() -> DurableFilesystem {
        DurableFilesystem::new(AgentStorageMeter::new(
            AgentMode::Durable,
            0,
            Arc::new(AtomicResourceEntry::new(0, 0, 0, 0, 0)),
            std::time::Instant::now(),
        ))
    }

    fn test_file(path: &Path) -> Arc<cap_std::fs::File> {
        Arc::new(cap_std::fs::File::from_std(
            std::fs::File::create(path).unwrap(),
        ))
    }

    fn completed_write(written: u64) -> FilesystemWriteCompletion {
        FilesystemWriteCompletion::from_writer_task(tokio::spawn(async move {
            FilesystemWriteOutcome::from_result(written, Ok(()))
        }))
    }

    #[test]
    fn size_change_preserves_exact_delta() {
        assert_eq!(
            FilesystemSizeChange::between(4, 10),
            FilesystemSizeChange::Grow(6)
        );
        assert_eq!(
            FilesystemSizeChange::between(10, 4),
            FilesystemSizeChange::Shrink(6)
        );
        assert_eq!(
            FilesystemSizeChange::between(4, 4),
            FilesystemSizeChange::Unchanged
        );
    }

    #[test]
    fn replacing_same_object_does_not_release_storage() {
        assert_eq!(
            replaced_file_storage(
                Some((1, 2)),
                FilesystemEntryStorage {
                    identity: Some((1, 2)),
                    size: 8,
                    is_regular_file: true,
                    link_count: 1,
                }
            ),
            (0, None)
        );
        assert_eq!(
            replaced_file_storage(
                Some((1, 2)),
                FilesystemEntryStorage {
                    identity: Some((3, 4)),
                    size: 8,
                    is_regular_file: true,
                    link_count: 1,
                }
            ),
            (8, Some((3, 4)))
        );
        assert_eq!(
            replaced_file_storage(
                Some((1, 2)),
                FilesystemEntryStorage {
                    identity: Some((3, 4)),
                    size: 0,
                    is_regular_file: true,
                    link_count: 1,
                }
            ),
            (0, Some((3, 4)))
        );
        assert_eq!(
            replaced_file_storage(
                None,
                FilesystemEntryStorage {
                    identity: Some((3, 4)),
                    size: 8,
                    is_regular_file: true,
                    link_count: 1,
                }
            ),
            (0, None)
        );
    }

    #[test]
    async fn positioned_and_append_writes_share_low_level_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("shared-writer.bin");
        let file = Arc::new(cap_std::fs::File::from_std(
            std::fs::File::create(&path).unwrap(),
        ));

        let (written, result) =
            write_file(file.clone(), FilesystemWriteMode::At(2), b"ab".to_vec()).await;
        result.unwrap();
        assert_eq!(written, 2);

        let (written, result) = write_file(file, FilesystemWriteMode::Append, b"cd".to_vec()).await;
        result.unwrap();
        assert_eq!(written, 2);
        assert_eq!(std::fs::read(path).unwrap(), b"\0\0abcd");
    }

    #[test]
    #[cfg(any(unix, windows))]
    fn object_identity_distinguishes_files_and_matches_hardlinks() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first.bin");
        let second = directory.path().join("second.bin");
        let alias = directory.path().join("alias.bin");
        std::fs::write(&first, b"same metadata inputs").unwrap();
        std::fs::write(&second, b"same metadata inputs").unwrap();
        std::fs::hard_link(&first, &alias).unwrap();

        let first_identity = metadata_object_identity(
            &cap_std::fs::File::from_std(std::fs::File::open(first).unwrap())
                .metadata()
                .unwrap(),
        );
        let second_identity = metadata_object_identity(
            &cap_std::fs::File::from_std(std::fs::File::open(second).unwrap())
                .metadata()
                .unwrap(),
        );
        let alias_identity = metadata_object_identity(
            &cap_std::fs::File::from_std(std::fs::File::open(alias).unwrap())
                .metadata()
                .unwrap(),
        );

        assert_ne!(first_identity, second_identity);
        assert_eq!(first_identity, alias_identity);
    }

    #[test]
    async fn externally_settled_write_remains_ready_to_the_p2_adapter() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settled-stream.bin");
        let file = Arc::new(cap_std::fs::File::from_std(
            std::fs::File::create(&path).unwrap(),
        ));
        let mut stream = DurableFileOutputStream::new(file, FilesystemWriteMode::At(0));

        stream.write(Bytes::from_static(b"ab")).unwrap();
        stream.current_completion().unwrap().wait().await.unwrap();
        stream
            .blocking_write_and_flush(Bytes::from_static(b"cd"))
            .await
            .unwrap();

        assert_eq!(std::fs::read(path).unwrap(), b"abcd");
    }

    #[test]
    #[timeout("30s")]
    async fn write_completion_reports_failed_writer_task() {
        let writer_task = tokio::task::spawn_blocking(move || {
            panic!("simulated filesystem writer failure");
        });
        let completion = FilesystemWriteCompletion::from_writer_task(writer_task);

        let result = completion.wait().await;
        let error = result.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Other);
        assert!(
            error
                .to_string()
                .contains("simulated filesystem writer failure")
        );
    }

    #[test]
    fn write_outcome_preserves_raw_os_error() {
        let outcome =
            FilesystemWriteOutcome::from_result(0, Err(std::io::Error::from_raw_os_error(2)));

        assert_eq!(outcome.result().unwrap_err().raw_os_error(), Some(2));
    }

    #[test]
    #[timeout("10s")]
    async fn cancelling_started_writer_waits_for_its_real_outcome() {
        let (started_sender, started_receiver) = tokio::sync::oneshot::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let writer_task = tokio::task::spawn_blocking(move || {
            let _ = started_sender.send(());
            release_receiver.recv().unwrap();
            FilesystemWriteOutcome::from_result(7, Ok(()))
        });
        let completion = FilesystemWriteCompletion::from_writer_task(writer_task);
        let observed_completion = completion.clone();
        started_receiver.await.unwrap();

        let directory = tempfile::tempdir().unwrap();
        let mut stream = DurableFileOutputStream::waiting_for_test(
            test_file(&directory.path().join("started.bin")),
            completion,
        );
        let cancel = stream.cancel();
        futures::pin_mut!(cancel);
        assert!(futures::poll!(&mut cancel).is_pending());

        release_sender.send(()).unwrap();
        cancel.await;
        assert_eq!(observed_completion.wait().await.unwrap(), 7);
    }

    #[test]
    fn cancelling_queued_writer_prevents_it_from_starting() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (blocker_started_sender, blocker_started_receiver) =
                tokio::sync::oneshot::channel();
            let (release_blocker_sender, release_blocker_receiver) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = blocker_started_sender.send(());
                release_blocker_receiver.recv().unwrap();
            });
            blocker_started_receiver.await.unwrap();

            let writer_started = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let writer_started_in_task = writer_started.clone();
            let writer_task = tokio::task::spawn_blocking(move || {
                writer_started_in_task.store(true, std::sync::atomic::Ordering::Release);
                FilesystemWriteOutcome::from_result(9, Ok(()))
            });
            let completion = FilesystemWriteCompletion::from_writer_task(writer_task);
            let observed_completion = completion.clone();
            let directory = tempfile::tempdir().unwrap();
            let mut stream = DurableFileOutputStream::waiting_for_test(
                test_file(&directory.path().join("queued.bin")),
                completion,
            );

            let cancellation = tokio::spawn(async move { stream.cancel().await });
            // The current-thread runtime polls the spawned cancellation to its first await.
            tokio::task::yield_now().await;
            release_blocker_sender.send(()).unwrap();
            blocker.await.unwrap();
            cancellation.await.unwrap();

            assert_eq!(observed_completion.wait().await.unwrap(), 0);
            assert!(!writer_started.load(std::sync::atomic::Ordering::Acquire));
        });
    }

    #[test]
    #[timeout("10s")]
    async fn completed_write_remains_authoritative_when_cancelled() {
        let (completed_sender, completed_receiver) = tokio::sync::oneshot::channel();
        let writer_task = tokio::task::spawn_blocking(move || {
            let outcome = FilesystemWriteOutcome::from_result(5, Ok(()));
            let _ = completed_sender.send(());
            outcome
        });
        let completion = FilesystemWriteCompletion::from_writer_task(writer_task);
        let observed_completion = completion.clone();
        completed_receiver.await.unwrap();
        observed_completion.clone().wait().await.unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut stream = DurableFileOutputStream::waiting_for_test(
            test_file(&directory.path().join("completed.bin")),
            completion,
        );

        stream.cancel().await;

        assert_eq!(observed_completion.wait().await.unwrap(), 5);
    }

    #[test]
    async fn cancelling_output_stream_leaves_no_detached_write() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cancelled-stream.bin");
        let file = Arc::new(cap_std::fs::File::from_std(
            std::fs::File::create(&path).unwrap(),
        ));
        let mut stream = DurableFileOutputStream::new(file, FilesystemWriteMode::At(0));
        let contents = Bytes::from(vec![1; FILE_WRITE_CAPACITY]);

        stream.write(contents).unwrap();
        let completion = stream.current_completion().unwrap();
        stream.cancel().await;
        let reported_written = completion.wait().await.unwrap();
        let length_after_cancel = std::fs::metadata(&path).unwrap().len();
        tokio::task::yield_now().await;

        assert_eq!(std::fs::metadata(path).unwrap().len(), length_after_cancel);
        assert_eq!(length_after_cancel, reported_written);
    }

    #[test]
    async fn pending_state_is_derived_from_every_stream_on_the_path() {
        let directory = tempfile::tempdir().unwrap();
        let filesystem = test_filesystem();
        let path = PathBuf::from("dir/file.bin");
        filesystem.register_output_stream(
            1,
            FilesystemOutputStreamState::new(
                path.clone(),
                test_file(&directory.path().join("first.bin")),
                Some(0),
            ),
        );
        filesystem.register_output_stream(
            2,
            FilesystemOutputStreamState::new(
                path,
                test_file(&directory.path().join("second.bin")),
                Some(0),
            ),
        );
        filesystem.mark_write_enqueued(1, 1, completed_write(1));
        filesystem.mark_write_enqueued(2, 1, completed_write(1));

        assert!(filesystem.is_mutation_pending(Path::new("dir/./file.bin")));
        filesystem.finish_pending_write(1, None);
        assert!(filesystem.is_mutation_pending(Path::new("dir/file.bin")));
        filesystem.finish_pending_write(2, None);
        assert!(!filesystem.is_mutation_pending(Path::new("dir/file.bin")));
    }

    #[test]
    fn normalized_path_resolves_parent_components_without_escaping_roots() {
        assert_eq!(
            DurableFilesystem::normalized_path(Path::new("dir/sub/../file.bin")),
            PathBuf::from("dir/file.bin")
        );
        assert_eq!(
            DurableFilesystem::normalized_path(Path::new("../../file.bin")),
            PathBuf::from("../../file.bin")
        );
        assert_eq!(
            DurableFilesystem::normalized_path(Path::new("/../../file.bin")),
            PathBuf::from("/file.bin")
        );
    }

    #[test]
    async fn rename_rewrites_real_pending_stream_state() {
        let directory = tempfile::tempdir().unwrap();
        let filesystem = test_filesystem();
        filesystem.register_output_stream(
            1,
            FilesystemOutputStreamState::new(
                PathBuf::from("old/dir/file.bin"),
                test_file(&directory.path().join("file.bin")),
                Some(0),
            ),
        );
        filesystem.mark_write_enqueued(1, 1, completed_write(1));

        filesystem.rename_stream_paths(Path::new("old/./dir"), Path::new("new/dir"));

        assert!(!filesystem.is_mutation_pending(Path::new("old/dir/file.bin")));
        assert!(filesystem.is_mutation_pending(Path::new("new/dir/file.bin")));
        assert_eq!(
            filesystem.output_stream_snapshot(1).unwrap().mutation_path,
            PathBuf::from("new/dir/file.bin")
        );
    }

    #[test]
    async fn settling_one_path_does_not_wait_for_another_path() {
        let directory = tempfile::tempdir().unwrap();
        let filesystem = test_filesystem();
        filesystem.register_output_stream(
            1,
            FilesystemOutputStreamState::new(
                PathBuf::from("ready.bin"),
                test_file(&directory.path().join("ready.bin")),
                Some(0),
            ),
        );
        filesystem.register_output_stream(
            2,
            FilesystemOutputStreamState::new(
                PathBuf::from("blocked.bin"),
                test_file(&directory.path().join("blocked.bin")),
                Some(0),
            ),
        );
        filesystem.mark_write_enqueued(1, 0, completed_write(0));
        let blocked = FilesystemWriteCompletion::from_writer_task(tokio::spawn(async {
            std::future::pending::<FilesystemWriteOutcome>().await
        }));
        filesystem.mark_write_enqueued(2, 0, blocked);

        let settled = filesystem
            .settle_pending_writes(Path::new("ready.bin"))
            .await;

        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].stream_rep, 1);
    }

    #[test]
    fn failed_measurement_finishes_pending_settlement_and_rolls_back_reservation() {
        let directory = tempfile::tempdir().unwrap();
        let filesystem = test_filesystem();
        let meter = filesystem.storage_meter();
        assert_eq!(meter.reserve(8, 8), Some(1024));
        filesystem.register_output_stream(
            1,
            FilesystemOutputStreamState::new(
                PathBuf::from("failed-measurement.bin"),
                test_file(&directory.path().join("failed-measurement.bin")),
                Some(0),
            ),
        );
        filesystem.set_pending_reservation(
            1,
            PendingFilesystemReservation {
                base_size: 0,
                reservation: Some(FilesystemStorageReservation {
                    logical_bytes: 8,
                    storage_meter: meter.clone(),
                }),
            },
        );

        let prepared = filesystem.prepare_write_settlements(vec![SettledFilesystemWrite {
            stream_rep: 1,
            actual_growth: None,
            result: Err(std::io::Error::other("measurement failed")),
        }]);

        assert_eq!(prepared.len(), 1);
        assert_eq!(
            prepared[0].error.as_ref().unwrap().to_string(),
            "measurement failed"
        );
        assert!(
            !filesystem
                .output_stream_snapshot(1)
                .unwrap()
                .has_pending_write
        );
        drop(prepared);
        assert_eq!(meter.reserve(8, 8), Some(1024));
    }

    #[test]
    async fn unlinked_tracking_is_scoped_to_live_registered_handles() {
        let directory = tempfile::tempdir().unwrap();
        let filesystem = test_filesystem();
        let first = test_file(&directory.path().join("first.bin"));
        filesystem.mark_file_linked(first.clone()).await;
        let first_identity = file_object_identity(first.clone()).await;
        filesystem.mark_object_unlinked(first_identity);
        assert!(filesystem.is_file_unlinked(first.clone()).await);

        drop(first);
        let second = test_file(&directory.path().join("second.bin"));
        filesystem.mark_file_linked(second.clone()).await;

        let state = filesystem.state.lock().unwrap();
        assert_eq!(state.tracked_objects.len(), 1);
        assert!(!state.tracked_objects.values().next().unwrap().unlinked);
    }
}
