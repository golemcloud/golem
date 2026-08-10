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
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use cap_std::fs::FileExt;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::OplogEntry;
use wasmtime_wasi::p2::{DynOutputStream, OutputStream, Pollable};
use wasmtime_wasi::{StreamError, StreamResult};

use crate::durable_host::{PendingFilesystemReservation, PendingFilesystemReservationState};
use crate::metrics::storage::{
    STORAGE_TYPE_FILESYSTEM, record_storage_bytes_deleted, record_storage_bytes_written,
};
use crate::services::agent_storage_meter::{AgentStorageMeter, StorageAccountingGuard};
use crate::worker::Worker;
use crate::workerctx::WorkerCtx;

const FILE_WRITE_CAPACITY: usize = 1024 * 1024;

#[derive(Clone, Default)]
pub(crate) struct DurableFilesystem {
    state: Arc<Mutex<DurableFilesystemState>>,
    mutation_lock: Arc<tokio::sync::Mutex<()>>,
    storage_meter: Option<AgentStorageMeter>,
}

#[derive(Default)]
struct DurableFilesystemState {
    pending_paths: HashSet<PathBuf>,
    unlinked_objects: HashSet<(u64, u64)>,
    output_streams: HashMap<u32, FilesystemOutputStreamState>,
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

pub(crate) fn replaced_file_storage(
    source_identity: Option<(u64, u64)>,
    destination_identity: Option<(u64, u64)>,
    destination_size: u64,
    destination_is_regular_file: bool,
    destination_link_count: u64,
) -> (u64, Option<(u64, u64)>) {
    match (source_identity, destination_identity) {
        (Some(source), Some(destination))
            if source != destination
                && destination_is_regular_file
                && destination_link_count == 1 =>
        {
            (destination_size, Some(destination))
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
    pub(crate) storage_meter: Option<AgentStorageMeter>,
    pub(crate) finalized: bool,
}

impl FilesystemStorageReservation {
    pub(crate) fn logical_bytes(&self) -> u64 {
        self.logical_bytes
    }

    pub(crate) async fn acquire_capacity<Ctx: WorkerCtx>(
        &self,
        worker: &Arc<Worker<Ctx>>,
    ) -> anyhow::Result<()> {
        let Some(storage_meter) = &self.storage_meter else {
            return Ok(());
        };
        let _capacity_guard = storage_meter.lock_capacity_acquisition().await;
        let mut requested = storage_meter.capacity_shortfall();
        loop {
            if requested == 0 {
                return Ok(());
            }
            match worker.acquire_filesystem_storage_space(requested).await {
                Ok(Some(permit)) => {
                    storage_meter.merge_capacity(Some(permit));
                    return Ok(());
                }
                Ok(None) => return Ok(()),
                Err(error) => {
                    let latest = storage_meter.capacity_shortfall();
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
        if let Some(storage_meter) = &self.storage_meter {
            storage_meter.shrink_reservation(bytes);
        }
        self.logical_bytes = self.logical_bytes.saturating_sub(bytes);
    }

    pub(crate) fn shrink_to(&mut self, bytes: u64) {
        self.shrink(self.logical_bytes.saturating_sub(bytes));
    }

    pub(crate) async fn commit<Ctx: WorkerCtx>(
        mut self,
        worker: Arc<Worker<Ctx>>,
        storage_meter: AgentStorageMeter,
        committed_bytes: u64,
        account_id: AccountId,
        environment_id: EnvironmentId,
    ) {
        let _guard = storage_meter.lock_reservation().await;
        if committed_bytes == 0 {
            if let Some(meter) = self.storage_meter.take() {
                meter.commit_reservation(self.logical_bytes, 0, std::time::Instant::now());
            }
            return;
        }
        worker
            .add_to_oplog(OplogEntry::filesystem_storage_usage_update(
                committed_bytes as i64,
            ))
            .await;
        if let Some(meter) = self.storage_meter.take() {
            meter.commit_reservation(
                self.logical_bytes,
                committed_bytes,
                std::time::Instant::now(),
            );
        } else {
            storage_meter.on_acquire(committed_bytes, std::time::Instant::now());
        }
        record_storage_bytes_written(
            STORAGE_TYPE_FILESYSTEM,
            &account_id.to_string(),
            &environment_id.to_string(),
            committed_bytes,
        );
    }

    pub(crate) async fn release<Ctx: WorkerCtx>(
        self,
        worker: Arc<Worker<Ctx>>,
        storage_meter: AgentStorageMeter,
        guard: Option<StorageAccountingGuard>,
        account_id: AccountId,
        environment_id: EnvironmentId,
    ) {
        let requested_bytes = self.logical_bytes;
        let _guard = match guard {
            Some(guard) => guard,
            None => storage_meter.lock_reservation().await,
        };
        let freed_bytes = requested_bytes.min(storage_meter.current_bytes());
        if freed_bytes == 0 {
            return;
        }
        worker
            .add_to_oplog(OplogEntry::filesystem_storage_usage_update(
                -(freed_bytes as i64),
            ))
            .await;
        storage_meter.on_release(freed_bytes, std::time::Instant::now());
        record_storage_bytes_deleted(
            STORAGE_TYPE_FILESYSTEM,
            &account_id.to_string(),
            &environment_id.to_string(),
            freed_bytes,
        );
    }
}

impl Drop for FilesystemStorageReservation {
    fn drop(&mut self) {
        if !self.finalized
            && let Some(storage_meter) = self.storage_meter.take()
        {
            storage_meter.rollback_reservation(self.logical_bytes);
        }
    }
}

pub(crate) struct FilesystemStorageCommit<Ctx: WorkerCtx> {
    pub(crate) worker: Arc<Worker<Ctx>>,
    pub(crate) storage_meter: AgentStorageMeter,
    pub(crate) committed_bytes: u64,
    pub(crate) account_id: AccountId,
    pub(crate) environment_id: EnvironmentId,
}

impl<Ctx: WorkerCtx> FilesystemStorageCommit<Ctx> {
    pub(crate) async fn apply(self, reservation: FilesystemStorageReservation) {
        reservation
            .commit(
                self.worker,
                self.storage_meter,
                self.committed_bytes,
                self.account_id,
                self.environment_id,
            )
            .await;
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FilesystemWriteCompletion {
    outcome: tokio::sync::watch::Receiver<Option<FilesystemWriteOutcome>>,
    outcome_sender: tokio::sync::watch::Sender<Option<FilesystemWriteOutcome>>,
}

#[derive(Clone, Debug)]
struct FilesystemWriteOutcome {
    written: u64,
    error: Option<(std::io::ErrorKind, Option<i32>, Arc<str>)>,
}

impl FilesystemWriteOutcome {
    fn from_result(written: u64, result: std::io::Result<()>) -> Self {
        match result {
            Ok(()) => Self {
                written,
                error: None,
            },
            Err(error) => Self {
                written,
                error: Some((
                    error.kind(),
                    error.raw_os_error(),
                    Arc::from(error.to_string()),
                )),
            },
        }
    }

    fn result(&self) -> std::io::Result<u64> {
        match &self.error {
            Some((_, Some(raw_os_error), _)) => {
                Err(std::io::Error::from_raw_os_error(*raw_os_error))
            }
            Some((kind, None, message)) => Err(std::io::Error::new(*kind, message.to_string())),
            None => Ok(self.written),
        }
    }
}

impl FilesystemWriteCompletion {
    async fn wait(mut self) -> std::io::Result<u64> {
        loop {
            if let Some(outcome) = self.outcome.borrow_and_update().clone() {
                return outcome.result();
            }
            self.outcome.changed().await.map_err(|_| {
                std::io::Error::other("filesystem write task ended without a result")
            })?;
        }
    }

    fn complete_cancelled(&self) {
        if self.outcome.borrow().is_none() {
            self.outcome_sender
                .send_replace(Some(FilesystemWriteOutcome::from_result(0, Ok(()))));
        }
    }
}

pub(crate) struct SettledFilesystemWrite {
    pub stream_rep: u32,
    pub actual_growth: Option<u64>,
    pub result: std::io::Result<()>,
}

impl DurableFilesystem {
    pub(crate) fn new(storage_meter: AgentStorageMeter) -> Self {
        Self {
            state: Arc::new(Mutex::new(DurableFilesystemState::default())),
            mutation_lock: Arc::new(tokio::sync::Mutex::new(())),
            storage_meter: Some(storage_meter),
        }
    }

    pub(crate) fn storage_meter(&self) -> AgentStorageMeter {
        self.storage_meter
            .clone()
            .expect("worker filesystem must have a storage meter")
    }

    pub(crate) fn mark_object_unlinked(&self, identity: Option<(u64, u64)>) {
        if let Some(identity) = identity {
            self.state.lock().unwrap().unlinked_objects.insert(identity);
        }
    }

    pub(crate) async fn mark_file_linked(&self, file: Arc<cap_std::fs::File>) {
        if let Some(identity) = file_object_identity(file).await {
            self.state
                .lock()
                .unwrap()
                .unlinked_objects
                .remove(&identity);
        }
    }

    pub(crate) async fn is_file_unlinked(&self, file: Arc<cap_std::fs::File>) -> bool {
        let identity = file_object_identity(file).await;
        identity.is_some_and(|identity| {
            self.state
                .lock()
                .unwrap()
                .unlinked_objects
                .contains(&identity)
        })
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

    pub(crate) fn mutation_lock(&self, _path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        self.mutation_lock.clone()
    }

    pub(crate) fn ordered_mutation_locks(
        &self,
        first_path: &Path,
        second_path: &Path,
    ) -> Vec<Arc<tokio::sync::Mutex<()>>> {
        let _ = (first_path, second_path);
        vec![self.mutation_lock.clone()]
    }

    pub(crate) fn is_mutation_pending(&self, _path: &Path) -> bool {
        !self.state.lock().unwrap().pending_paths.is_empty()
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

    pub(crate) fn output_stream_snapshot(
        &self,
        stream_rep: u32,
    ) -> Option<(PathBuf, Arc<cap_std::fs::File>, Option<u64>, bool)> {
        self.state
            .lock()
            .unwrap()
            .output_streams
            .get(&stream_rep)
            .map(|stream| {
                (
                    stream.mutation_path.clone(),
                    stream.file.clone(),
                    stream.position,
                    stream.pending_write,
                )
            })
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
        if let PendingFilesystemReservationState::Reserved(reservation) = &mut pending.state {
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
        let mutation_path = if let Some(stream) = state.output_streams.get_mut(&stream_rep) {
            stream.pending_write = true;
            stream.completion = Some(completion);
            if let Some(position) = &mut stream.position {
                *position = position.saturating_add(write_len);
            }
            Some(stream.mutation_path.clone())
        } else {
            None
        };
        if let Some(path) = mutation_path {
            state.pending_paths.insert(Self::normalized_path(&path));
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

    pub(crate) fn finish_pending_write<Ctx: WorkerCtx>(
        &self,
        stream_rep: u32,
        actual_growth: Option<u64>,
        prepare_commit: impl FnOnce(
            &FilesystemStorageReservation,
            u64,
        ) -> Option<FilesystemStorageCommit<Ctx>>,
    ) -> Option<(FilesystemStorageCommit<Ctx>, FilesystemStorageReservation)> {
        let mut state = self.state.lock().unwrap();
        let (mutation_path, pending) = state.output_streams.get_mut(&stream_rep).map(|stream| {
            stream.pending_write = false;
            stream.completion = None;
            (
                stream.mutation_path.clone(),
                stream.pending_reservation.take(),
            )
        })?;
        state.pending_paths.remove(&mutation_path);
        let pending = pending?;

        match pending.state {
            PendingFilesystemReservationState::Reserved(reservation) => {
                let committed_growth = actual_growth.unwrap_or(0).min(reservation.logical_bytes());
                prepare_commit(&reservation, committed_growth).map(|commit| (commit, reservation))
            }
            PendingFilesystemReservationState::Unchanged => None,
        }
    }

    pub(crate) async fn settle_pending_writes(
        &self,
        mutation_path: &Path,
    ) -> Vec<SettledFilesystemWrite> {
        let _ = mutation_path;
        let pending = {
            let state = self.state.lock().unwrap();
            state
                .output_streams
                .iter()
                .filter(|(_, stream)| stream.pending_write)
                .map(|(stream_rep, stream)| {
                    (
                        *stream_rep,
                        stream.file.clone(),
                        stream
                            .pending_reservation
                            .as_ref()
                            .map(|pending| pending.base_size),
                        stream.completion.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };

        let mut settled = Vec::with_capacity(pending.len());
        for (stream_rep, file, base_size, completion) in pending {
            let write_result = match completion {
                Some(completion) => completion.wait().await.map(|_| ()),
                None => Err(std::io::Error::other(
                    "pending filesystem write has no completion",
                )),
            };
            let measurement = match base_size {
                Some(base_size) => filesystem_file_size(file)
                    .await
                    .map(|size| Some(size.saturating_sub(base_size))),
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
        self.state
            .lock()
            .unwrap()
            .output_streams
            .iter()
            .filter_map(|(stream_rep, stream)| stream.pending_write.then_some(*stream_rep))
            .collect()
    }

    pub(crate) fn remove_output_stream(
        &self,
        stream_rep: u32,
    ) -> Option<FilesystemOutputStreamState> {
        let mut state = self.state.lock().unwrap();
        let stream = state.output_streams.remove(&stream_rep)?;
        if stream.pending_write {
            state.pending_paths.remove(&stream.mutation_path);
        }
        Some(stream)
    }

    pub(crate) fn rename_stream_paths(&self, old_path: &Path, new_path: &Path) {
        let old_path = Self::normalized_path(old_path);
        let new_path = Self::normalized_path(new_path);
        let mut state = self.state.lock().unwrap();

        let aliases = state
            .output_streams
            .values()
            .filter_map(|stream| {
                stream
                    .mutation_path
                    .strip_prefix(&old_path)
                    .ok()
                    .map(|remainder| (stream.mutation_path.clone(), new_path.join(remainder)))
            })
            .collect::<Vec<_>>();
        for (old_stream_path, new_stream_path) in &aliases {
            if state.pending_paths.remove(old_stream_path) {
                state.pending_paths.insert(new_stream_path.clone());
            }
        }
        for stream in state.output_streams.values_mut() {
            if let Ok(remainder) = stream.mutation_path.strip_prefix(&old_path) {
                stream.mutation_path = new_path.join(remainder);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_path_pending(&self, path: &Path) {
        self.state
            .lock()
            .unwrap()
            .pending_paths
            .insert(Self::normalized_path(path));
    }

    #[cfg(test)]
    pub(crate) fn clear_path_pending(&self, path: &Path) {
        self.state
            .lock()
            .unwrap()
            .pending_paths
            .remove(&Self::normalized_path(path));
    }

    #[cfg(test)]
    pub(crate) fn alias_paths(&self, old_path: &Path, new_path: &Path) {
        let old_path = Self::normalized_path(old_path);
        let new_path = Self::normalized_path(new_path);
        let mut state = self.state.lock().unwrap();
        if state.pending_paths.remove(&old_path) {
            state.pending_paths.insert(new_path);
        }
    }
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

pub(crate) struct DurableFileOutputStream {
    file: Arc<cap_std::fs::File>,
    mode: FilesystemWriteMode,
    state: FileOutputState,
}

enum FileOutputState {
    Ready,
    Waiting {
        completion: FilesystemWriteCompletion,
        task: tokio::task::JoinHandle<()>,
    },
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

    pub(crate) fn into_dyn(self) -> DynOutputStream {
        Box::new(self)
    }

    pub(crate) fn current_completion(&self) -> Option<FilesystemWriteCompletion> {
        match &self.state {
            FileOutputState::Waiting { completion, .. } => Some(completion.clone()),
            _ => None,
        }
    }

    async fn wait_ready(&mut self) {
        let FileOutputState::Waiting {
            completion, task, ..
        } = std::mem::replace(&mut self.state, FileOutputState::Closed)
        else {
            return;
        };
        let result = completion.wait().await;
        drop(task);
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
            FileOutputState::Waiting { .. } | FileOutputState::Error(_) => {
                return Err(StreamError::Trap(wasmtime::Error::msg(
                    "write not permitted: check_write not called first",
                )));
            }
        }

        let file = self.file.clone();
        let mode = self.mode;
        let (outcome_sender, outcome_rx) = tokio::sync::watch::channel(None);
        let task_outcome_sender = outcome_sender.clone();
        let task = tokio::task::spawn_blocking(move || {
            let (written, result) = write_file_blocking(&file, mode, &bytes);
            let written = u64::try_from(written).unwrap_or(u64::MAX);
            task_outcome_sender
                .send_replace(Some(FilesystemWriteOutcome::from_result(written, result)));
        });
        self.state = FileOutputState::Waiting {
            completion: FilesystemWriteCompletion {
                outcome: outcome_rx,
                outcome_sender,
            },
            task,
        };
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        match self.state {
            FileOutputState::Ready | FileOutputState::Waiting { .. } => Ok(()),
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
            FileOutputState::Waiting { .. } => Ok(0),
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
        if let FileOutputState::Waiting { completion, task } =
            std::mem::replace(&mut self.state, FileOutputState::Closed)
        {
            task.abort();
            if task.await.is_err() {
                completion.complete_cancelled();
            }
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
        if matches!(self.state, FileOutputState::Waiting { .. }) {
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

async fn filesystem_file_size(file: Arc<cap_std::fs::File>) -> std::io::Result<u64> {
    tokio::task::spawn_blocking(move || file.metadata().map(|metadata| metadata.len()))
        .await
        .map_err(std::io::Error::other)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

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
            replaced_file_storage(Some((1, 2)), Some((1, 2)), 8, true, 1),
            (0, None)
        );
        assert_eq!(
            replaced_file_storage(Some((1, 2)), Some((3, 4)), 8, true, 1),
            (8, Some((3, 4)))
        );
        assert_eq!(
            replaced_file_storage(Some((1, 2)), Some((3, 4)), 0, true, 1),
            (0, Some((3, 4)))
        );
        assert_eq!(
            replaced_file_storage(None, Some((3, 4)), 8, true, 1),
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
}
