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

use std::hash::Hasher;
use std::sync::Arc;
use std::time::SystemTime;
use std::time::{Duration, Instant};

use bytes::Bytes;
use cap_std::fs::FileExt;
use fs_set_times::{SystemTimeSpec, set_symlink_times};
use metrohash::MetroHash128;
use wasmtime::component::Resource;
use wasmtime_wasi::FilePerms;
use wasmtime_wasi::filesystem::WasiFilesystemView as _;
use wasmtime_wasi::p2::FsError;
use wasmtime_wasi::p2::ReaddirIterator;
use wasmtime_wasi::p2::bindings::filesystem::types::{
    Advice, Descriptor, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry,
    DirectoryEntryStream, Error, ErrorCode, Filesize, Host, HostDescriptor,
    HostDirectoryEntryStream, InputStream, MetadataHashValue, NewTimestamp, OpenFlags,
    OutputStream, PathFlags,
};
use wasmtime_wasi::runtime::spawn_blocking;

use crate::durable_host::concurrent::{CallHandle, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, FilesystemOutputStreamState};
use crate::services::agent_filesystem::{
    AgentFilesystemRuntime, ClassifiedFileOutputStream, FILESYSTEM_MUTATION_MAX_ATTEMPTS,
    FILESYSTEM_MUTATION_RETRY_TIMEOUT, FilesystemStreamMode, MutationDecision, MutationEffect,
    MutationFailure, MutationOperation, MutationPostcondition as P2MutationPostcondition,
    NativeMutationGuestError, NativeOpenOptions, NativeOpenResult,
    PathObjectType as P2PathObjectType, RequestedTime, create_directory as native_create_directory,
    create_directory_postcondition as p2_create_directory_postcondition,
    descriptor_state as p2_descriptor_state, descriptor_times as p2_descriptor_times,
    hard_link as native_hard_link, link_postcondition as p2_link_postcondition,
    native_write_failure_effect, open as native_open, open_postcondition as p2_open_postcondition,
    path_state as p2_path_state, path_state_with_follow as p2_path_state_with_follow,
    path_times as p2_path_times, remove_directory as native_remove_directory,
    remove_postcondition as p2_remove_postcondition, rename as native_rename,
    rename_postcondition as p2_rename_postcondition, resize_file as native_resize_file,
    resize_postcondition as p2_resize_postcondition, run_blocking_filesystem_mutation,
    set_descriptor_times as native_set_descriptor_times, set_path_times as native_set_path_times,
    symlink as native_symlink, symlink_postcondition as p2_symlink_postcondition,
    symlink_state as p2_symlink_state, sync_descriptor as native_sync_descriptor,
    times_postcondition, unlink_file as native_unlink_file, validate_descriptor_times,
    validate_directory_mutation, validate_open, validate_resize, validate_two_directory_mutation,
};
#[cfg(test)]
use crate::services::agent_filesystem::{
    ObjectIdentity as P2ObjectIdentity, PathState as P2PathState, same_object as p2_same_object,
    same_optional_object as p2_same_optional_object,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::host_functions::{
    FilesystemTypesDescriptorStat, FilesystemTypesDescriptorStatAt,
};
use golem_common::model::oplog::types::{
    FileSystemError, SerializableDateTime, SerializableFileTimes,
};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestFileSystemPath, HostResponseFileSystemStat,
};

enum P2MutationAction {
    Retry,
    Success,
    Error(FsError),
    Trap,
}

struct P2MutationAdapter {
    runtime: AgentFilesystemRuntime,
    operation: MutationOperation,
    started: Instant,
    attempts: usize,
}

impl P2MutationAdapter {
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
        error: FsError,
        postcondition: P2MutationPostcondition,
        retry_safe: bool,
    ) -> P2MutationAction {
        let Some(guest) = error.downcast_ref().copied() else {
            let _ = self
                .runtime
                .classify_mutation_failure::<ErrorCode>(
                    MutationFailure::Infrastructure(std::io::Error::other(error.to_string())),
                    MutationEffect::Unknown,
                )
                .await;
            return P2MutationAction::Trap;
        };
        let effect = match (postcondition, guest) {
            (P2MutationPostcondition::Satisfied, _) => {
                MutationEffect::DesiredPostconditionSatisfied
            }
            (P2MutationPostcondition::NoEffect, _) => MutationEffect::ProvenNoEffect,
            (P2MutationPostcondition::Unknown, _) => MutationEffect::Unknown,
        };
        let failure = match guest {
            ErrorCode::Quota => MutationFailure::StorageExhaustion {
                guest,
                quota_hint: true,
            },
            ErrorCode::InsufficientSpace => MutationFailure::StorageExhaustion {
                guest,
                quota_hint: false,
            },
            ErrorCode::Busy
            | ErrorCode::Interrupted
            | ErrorCode::InProgress
            | ErrorCode::Already => MutationFailure::TransientGuest(guest),
            ErrorCode::Access | ErrorCode::NotPermitted => MutationFailure::AccessGuest(guest),
            ErrorCode::Io => MutationFailure::UnclassifiedGuest(guest),
            _ => MutationFailure::Guest(guest),
        };
        match self
            .runtime
            .classify_mutation_failure_for(self.operation, failure, effect)
            .await
        {
            MutationDecision::PreserveGuest(error) => P2MutationAction::Error(error.into()),
            MutationDecision::Quota => P2MutationAction::Error(ErrorCode::Quota.into()),
            MutationDecision::InsufficientSpace => {
                P2MutationAction::Error(ErrorCode::InsufficientSpace.into())
            }
            MutationDecision::PhysicalPressure
                if retry_safe
                    && postcondition == P2MutationPostcondition::NoEffect
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
                    P2MutationAction::Retry
                } else {
                    P2MutationAction::Error(ErrorCode::InsufficientSpace.into())
                }
            }
            MutationDecision::PhysicalPressure => {
                P2MutationAction::Error(ErrorCode::InsufficientSpace.into())
            }
            MutationDecision::BoundedRetry
                if retry_safe
                    && postcondition == P2MutationPostcondition::NoEffect
                    && self.attempts < FILESYSTEM_MUTATION_MAX_ATTEMPTS
                    && self.started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT =>
            {
                P2MutationAction::Retry
            }
            MutationDecision::BoundedRetry => P2MutationAction::Error(guest.into()),
            MutationDecision::PreserveRaw => P2MutationAction::Error(guest.into()),
            MutationDecision::Success => P2MutationAction::Success,
            MutationDecision::Invalidate => P2MutationAction::Trap,
        }
    }

    async fn io_failure(
        &self,
        error: std::io::Error,
        postcondition: P2MutationPostcondition,
        retry_safe: bool,
    ) -> P2MutationAction {
        let raw_os_error = error.raw_os_error();
        let error_kind = error.kind();
        let error_message = error.to_string();
        let effect = match postcondition {
            P2MutationPostcondition::Satisfied => MutationEffect::DesiredPostconditionSatisfied,
            P2MutationPostcondition::NoEffect => MutationEffect::ProvenNoEffect,
            P2MutationPostcondition::Unknown => MutationEffect::Unknown,
        };
        match self
            .runtime
            .classify_mutation_failure_for::<ErrorCode>(
                self.operation,
                MutationFailure::Io(error),
                effect,
            )
            .await
        {
            MutationDecision::PreserveGuest(error) => P2MutationAction::Error(error.into()),
            MutationDecision::Quota => P2MutationAction::Error(ErrorCode::Quota.into()),
            MutationDecision::InsufficientSpace => {
                P2MutationAction::Error(ErrorCode::InsufficientSpace.into())
            }
            MutationDecision::PhysicalPressure
                if retry_safe
                    && postcondition == P2MutationPostcondition::NoEffect
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
                    P2MutationAction::Retry
                } else {
                    P2MutationAction::Error(ErrorCode::InsufficientSpace.into())
                }
            }
            MutationDecision::PhysicalPressure => {
                P2MutationAction::Error(ErrorCode::InsufficientSpace.into())
            }
            MutationDecision::BoundedRetry
                if retry_safe
                    && postcondition == P2MutationPostcondition::NoEffect
                    && self.attempts < FILESYSTEM_MUTATION_MAX_ATTEMPTS
                    && self.started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT =>
            {
                P2MutationAction::Retry
            }
            MutationDecision::BoundedRetry | MutationDecision::PreserveRaw => {
                let error = raw_os_error.map_or_else(
                    || std::io::Error::new(error_kind, error_message),
                    std::io::Error::from_raw_os_error,
                );
                P2MutationAction::Error(error.into())
            }
            MutationDecision::Success => P2MutationAction::Success,
            MutationDecision::Invalidate => P2MutationAction::Trap,
        }
    }
}

fn p2_mutation_action_result(action: P2MutationAction) -> Result<(), FsError> {
    match action {
        P2MutationAction::Success => Ok(()),
        P2MutationAction::Error(error) => Err(error),
        P2MutationAction::Trap => Err(FsError::trap(wasmtime::Error::msg(
            "agent filesystem mutation invalidated the runtime",
        ))),
        P2MutationAction::Retry => unreachable!("retry must be handled by the operation loop"),
    }
}

async fn p2_initial_probe<T>(
    runtime: &AgentFilesystemRuntime,
    result: Result<T, std::io::Error>,
) -> Result<T, FsError> {
    match result {
        Ok(value) => Ok(value),
        Err(error) => {
            let raw_os_error = error.raw_os_error();
            let error_kind = error.kind();
            let message = error.to_string();
            match runtime
                .classify_mutation_failure_for::<ErrorCode>(
                    MutationOperation::Metadata,
                    MutationFailure::Io(error),
                    MutationEffect::ProvenNoEffect,
                )
                .await
            {
                MutationDecision::Quota => Err(ErrorCode::Quota.into()),
                MutationDecision::InsufficientSpace | MutationDecision::PhysicalPressure => {
                    Err(ErrorCode::InsufficientSpace.into())
                }
                MutationDecision::BoundedRetry | MutationDecision::PreserveRaw => {
                    let error = raw_os_error.map_or_else(
                        || std::io::Error::new(error_kind, message),
                        std::io::Error::from_raw_os_error,
                    );
                    Err(error.into())
                }
                MutationDecision::PreserveGuest(error) => Err(error.into()),
                MutationDecision::Success => unreachable!("failed probe cannot be satisfied"),
                MutationDecision::Invalidate => Err(FsError::trap(wasmtime::Error::msg(
                    "agent filesystem mutation precondition probe invalidated the runtime",
                ))),
            }
        }
    }
}

fn p2_directory(descriptor: &Descriptor) -> Result<wasmtime_wasi::filesystem::Dir, FsError> {
    match descriptor {
        Descriptor::Dir(directory) => Ok(directory.clone()),
        Descriptor::File(_) => Err(ErrorCode::NotDirectory.into()),
    }
}

async fn p2_finish_native_mutation(
    adapter: &P2MutationAdapter,
    error: std::io::Error,
    postcondition: P2MutationPostcondition,
    retry_safe: bool,
) -> Result<bool, FsError> {
    match adapter.io_failure(error, postcondition, retry_safe).await {
        P2MutationAction::Retry => Ok(true),
        action => p2_mutation_action_result(action).map(|()| false),
    }
}

fn p2_native_guest(error: NativeMutationGuestError) -> FsError {
    match error {
        NativeMutationGuestError::Invalid => ErrorCode::Invalid.into(),
        NativeMutationGuestError::NotDirectory => ErrorCode::NotDirectory.into(),
        NativeMutationGuestError::NotPermitted => ErrorCode::NotPermitted.into(),
        NativeMutationGuestError::Unsupported => ErrorCode::Unsupported.into(),
    }
}

fn p2_requested_time(requested: NewTimestamp) -> RequestedTime {
    match requested {
        NewTimestamp::NoChange => RequestedTime::NoChange,
        NewTimestamp::Now => RequestedTime::Now,
        NewTimestamp::Timestamp(timestamp) => RequestedTime::Timestamp {
            seconds: i128::from(timestamp.seconds),
            nanoseconds: timestamp.nanoseconds,
        },
    }
}

fn p2_native_time(requested: NewTimestamp) -> Result<Option<SystemTime>, FsError> {
    match requested {
        NewTimestamp::NoChange => Ok(None),
        NewTimestamp::Now => Ok(Some(SystemTime::now())),
        NewTimestamp::Timestamp(timestamp) => SystemTime::UNIX_EPOCH
            .checked_add(Duration::new(timestamp.seconds, timestamp.nanoseconds))
            .map(Some)
            .ok_or_else(|| ErrorCode::Overflow.into()),
    }
}

fn p2_times_postcondition(
    current: Result<crate::services::agent_filesystem::TimesState, std::io::Error>,
    before: crate::services::agent_filesystem::TimesState,
    accessed: NewTimestamp,
    modified: NewTimestamp,
    identity_required: bool,
) -> P2MutationPostcondition {
    times_postcondition(
        current,
        before,
        p2_requested_time(accessed),
        p2_requested_time(modified),
        identity_required,
    )
}

async fn classified_positioned_write(
    filesystem_runtime: crate::services::agent_filesystem::AgentFilesystemRuntime,
    file: wasmtime_wasi::filesystem::File,
    buffer: Vec<u8>,
    offset: Filesize,
    effect: crate::services::agent_filesystem::AgentFilesystemEffectLease,
) -> Result<Filesize, FsError> {
    let file = Arc::clone(&file.file);
    let buffer = Bytes::from(buffer);
    let effect = Arc::new(effect);
    let started = Instant::now();

    for attempt in 0..FILESYSTEM_MUTATION_MAX_ATTEMPTS {
        let file = Arc::clone(&file);
        let buffer = buffer.clone();
        let effect = Arc::clone(&effect);
        match spawn_blocking(move || {
            let _effect = effect;
            file.write_at(&buffer, offset)
        })
        .await
        {
            Ok(written) => {
                return Ok(Filesize::try_from(written).expect("usize fits in Filesize"));
            }
            Err(error) => {
                let raw_os_error = error.raw_os_error();
                let error_kind = error.kind();
                let error_message = error.to_string();
                let effect = native_write_failure_effect(&error, 0);
                let decision = filesystem_runtime
                    .classify_mutation_failure_for::<ErrorCode>(
                        MutationOperation::Write,
                        MutationFailure::Io(error),
                        effect,
                    )
                    .await;
                match decision {
                    MutationDecision::BoundedRetry
                        if attempt + 1 < FILESYSTEM_MUTATION_MAX_ATTEMPTS
                            && started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT => {}
                    MutationDecision::BoundedRetry => {
                        let error = raw_os_error.map_or_else(
                            || std::io::Error::new(error_kind, error_message),
                            std::io::Error::from_raw_os_error,
                        );
                        return Err(error.into());
                    }
                    MutationDecision::PreserveRaw => {
                        let error = raw_os_error.map_or_else(
                            || std::io::Error::new(error_kind, error_message),
                            std::io::Error::from_raw_os_error,
                        );
                        return Err(error.into());
                    }
                    MutationDecision::PreserveGuest(error) => return Err(error.into()),
                    MutationDecision::Quota => return Err(ErrorCode::Quota.into()),
                    MutationDecision::InsufficientSpace => {
                        return Err(ErrorCode::InsufficientSpace.into());
                    }
                    MutationDecision::PhysicalPressure
                        if attempt + 1 < FILESYSTEM_MUTATION_MAX_ATTEMPTS
                            && started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT
                            && filesystem_runtime
                                .recover_physical_pressure(
                                    MutationOperation::Write,
                                    started + FILESYSTEM_MUTATION_RETRY_TIMEOUT,
                                )
                                .await
                            && started.elapsed() <= FILESYSTEM_MUTATION_RETRY_TIMEOUT => {}
                    MutationDecision::PhysicalPressure => {
                        return Err(ErrorCode::InsufficientSpace.into());
                    }
                    MutationDecision::Success => return Ok(0),
                    MutationDecision::Invalidate => {
                        return Err(FsError::trap(wasmtime::Error::msg(
                            "agent filesystem mutation invalidated the runtime",
                        )));
                    }
                }
            }
        }
    }

    unreachable!("positioned write loop always returns")
}

impl<Ctx: WorkerCtx> HostDescriptor for DurableWorkerCtx<Ctx> {
    fn read_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<InputStream>, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "read_via_stream");
        let stream =
            HostDescriptor::read_via_stream(&mut self.as_wasi_view().filesystem(), self_, offset)?;
        self.state
            .open_filesystem_input_streams
            .insert(stream.rep());
        Ok(stream)
    }

    fn write_via_stream(
        &mut self,
        fd: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<OutputStream>, FsError> {
        self.fail_if_read_only(&fd)?;
        self.observe_function_call("filesystem::types::descriptor", "write_via_stream");
        let file = match self.table().get(&fd)? {
            Descriptor::File(file) if file.perms.contains(FilePerms::WRITE) => file.clone(),
            Descriptor::File(_) => return Err(ErrorCode::NotPermitted.into()),
            Descriptor::Dir(_) => return Err(ErrorCode::BadDescriptor.into()),
        };
        let filesystem_runtime = self.filesystem_runtime();
        let stream = self.table().push(
            ClassifiedFileOutputStream::new(
                file,
                filesystem_runtime,
                FilesystemStreamMode::Position(offset),
            )
            .into_dyn(),
        )?;
        self.state.open_filesystem_output_streams.insert(
            stream.rep(),
            FilesystemOutputStreamState {
                position: Some(offset),
            },
        );
        Ok(stream)
    }

    fn append_via_stream(
        &mut self,
        fd: Resource<Descriptor>,
    ) -> Result<Resource<OutputStream>, FsError> {
        self.fail_if_read_only(&fd)?;
        self.observe_function_call("filesystem::types::descriptor", "append_via_stream");
        let file = match self.table().get(&fd)? {
            Descriptor::File(file) if file.perms.contains(FilePerms::WRITE) => file.clone(),
            Descriptor::File(_) => return Err(ErrorCode::NotPermitted.into()),
            Descriptor::Dir(_) => return Err(ErrorCode::BadDescriptor.into()),
        };
        let filesystem_runtime = self.filesystem_runtime();
        let stream = self.table().push(
            ClassifiedFileOutputStream::new(file, filesystem_runtime, FilesystemStreamMode::Append)
                .into_dyn(),
        )?;
        self.state
            .open_filesystem_output_streams
            .insert(stream.rep(), FilesystemOutputStreamState { position: None });
        Ok(stream)
    }

    async fn advise(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
        length: Filesize,
        advice: Advice,
    ) -> Result<(), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "advise");
        let mut view = self.as_wasi_view();
        HostDescriptor::advise(&mut view.filesystem(), self_, offset, length, advice).await
    }

    async fn sync_data(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "sync_data");
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_effect()
                .await
                .map_err(FsError::trap)?,
        );
        let descriptor = self.table().get(&self_)?.clone();
        let mut adapter = P2MutationAdapter::new(self.filesystem_runtime());
        adapter.begin_attempt();
        match run_blocking_filesystem_mutation(effect, move || {
            native_sync_descriptor(&descriptor, true)
        })
        .await
        {
            Ok(()) => Ok(()),
            Err(error) => p2_mutation_action_result(
                adapter
                    .io_failure(error, P2MutationPostcondition::Unknown, false)
                    .await,
            ),
        }
    }

    async fn get_flags(&mut self, fd: Resource<Descriptor>) -> Result<DescriptorFlags, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "get_flags");

        let read_only = self.check_if_file_is_readonly(&fd)?;
        let mut view = self.as_wasi_view();
        let mut descriptor_flags = HostDescriptor::get_flags(&mut view.filesystem(), fd).await?;

        if read_only {
            descriptor_flags &= !DescriptorFlags::WRITE
        };

        Ok(descriptor_flags)
    }

    async fn get_type(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorType, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "get_type");
        let mut view = self.as_wasi_view();
        HostDescriptor::get_type(&mut view.filesystem(), self_).await
    }

    async fn set_size(&mut self, fd: Resource<Descriptor>, size: Filesize) -> Result<(), FsError> {
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_update_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_read_only(&fd)?;

        let descriptor = self.table().get(&fd)?.clone();
        let file = match &descriptor {
            Descriptor::File(file) => file.clone(),
            Descriptor::Dir(_) => return Err(ErrorCode::BadDescriptor.into()),
        };
        validate_resize(&file).map_err(p2_native_guest)?;
        let runtime = self.filesystem_runtime();
        let before = p2_initial_probe(&runtime, p2_descriptor_state(&descriptor).await).await?;

        self.observe_function_call("filesystem::types::descriptor", "set_size");
        let mut adapter = P2MutationAdapter::for_operation(runtime, MutationOperation::Resize);
        loop {
            adapter.begin_attempt();
            let file = file.clone();
            let effect = Arc::clone(&effect);
            let result =
                run_blocking_filesystem_mutation(effect, move || native_resize_file(&file, size))
                    .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = p2_resize_postcondition(
                        before,
                        p2_descriptor_state(&descriptor).await,
                        size,
                    );
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn set_times(
        &mut self,
        fd: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_update_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_read_only(&fd)?;

        self.observe_function_call("filesystem::types::descriptor", "set_times");
        let descriptor = self.table().get(&fd)?.clone();
        validate_descriptor_times(&descriptor).map_err(p2_native_guest)?;
        let accessed = p2_native_time(data_access_timestamp)?;
        let modified = p2_native_time(data_modification_timestamp)?;
        let runtime = self.filesystem_runtime();
        let before = p2_initial_probe(&runtime, p2_descriptor_times(&descriptor).await).await?;
        let mut adapter = P2MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let descriptor_for_attempt = descriptor.clone();
            let effect = Arc::clone(&effect);
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_set_descriptor_times(&descriptor_for_attempt, accessed, modified)
            })
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = p2_times_postcondition(
                        p2_descriptor_times(&descriptor).await,
                        before,
                        data_access_timestamp,
                        data_modification_timestamp,
                        false,
                    );
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn read(
        &mut self,
        self_: Resource<Descriptor>,
        length: Filesize,
        offset: Filesize,
    ) -> Result<(Vec<u8>, bool), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "read");
        let mut view = self.as_wasi_view();
        HostDescriptor::read(&mut view.filesystem(), self_, length, offset).await
    }

    async fn write(
        &mut self,
        fd: Resource<Descriptor>,
        buffer: Vec<u8>,
        offset: Filesize,
    ) -> Result<Filesize, FsError> {
        self.fail_if_read_only(&fd)?;
        let file = match self.table().get(&fd)? {
            Descriptor::File(file) if file.perms.contains(FilePerms::WRITE) => file.clone(),
            Descriptor::File(_) => return Err(ErrorCode::NotPermitted.into()),
            Descriptor::Dir(_) => return Err(ErrorCode::BadDescriptor.into()),
        };
        let effect = self
            .filesystem_runtime()
            .begin_effect()
            .await
            .map_err(FsError::trap)?;

        self.observe_function_call("filesystem::types::descriptor", "write");
        classified_positioned_write(self.filesystem_runtime(), file, buffer, offset, effect).await
    }

    async fn read_directory(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<DirectoryEntryStream>, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "read_directory");
        let mut view = self.as_wasi_view();
        let stream = HostDescriptor::read_directory(&mut view.filesystem(), self_).await?;
        // Iterating through the whole stream to make sure we have a stable order
        let mut entries = Vec::new();
        let iter = self.table().delete(stream)?;
        for entry in iter {
            entries.push(entry?.clone());
        }
        entries.sort_by_key(|entry| entry.name.clone());

        Ok(self
            .table()
            .push(ReaddirIterator::new(entries.into_iter().map(Ok)))?)
    }

    async fn sync(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "sync");
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_effect()
                .await
                .map_err(FsError::trap)?,
        );
        let descriptor = self.table().get(&self_)?.clone();
        let mut adapter = P2MutationAdapter::new(self.filesystem_runtime());
        adapter.begin_attempt();
        match run_blocking_filesystem_mutation(effect, move || {
            native_sync_descriptor(&descriptor, false)
        })
        .await
        {
            Ok(()) => Ok(()),
            Err(error) => p2_mutation_action_result(
                adapter
                    .io_failure(error, P2MutationPostcondition::Unknown, false)
                    .await,
            ),
        }
    }

    async fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "create_directory_at");
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_path_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_read_only_path(&self_, &path, false)?;
        let directory = p2_directory(self.table().get(&self_)?)?;
        validate_directory_mutation(&directory).map_err(p2_native_guest)?;
        let runtime = self.filesystem_runtime();
        let before = p2_initial_probe(&runtime, p2_path_state(&directory, &path).await).await?;
        let mut adapter = P2MutationAdapter::for_operation(runtime, MutationOperation::Create);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_create_directory(&directory_for_attempt, &path_for_attempt)
            })
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = p2_create_directory_postcondition(
                        before,
                        p2_path_state(&directory, &path).await,
                    );
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        let path = match self.table().get(&self_)? {
            Descriptor::File(f) => f.path.clone(),
            Descriptor::Dir(d) => d.path.clone(),
        };
        let _effect = self
            .filesystem_runtime()
            .begin_effect()
            .await
            .map_err(FsError::trap)?;

        // `ReadLocal`: the local stat always runs (its timestamps are then overridden by the durable
        // value), so only the file-times are made durable via `CallHandle::run`.
        let handle = CallHandle::<FilesystemTypesDescriptorStat, NotCancellable>::start(
            self,
            HostRequestFileSystemPath {
                path: path.to_string_lossy().to_string(),
            },
            DurableFunctionType::ReadLocal,
        )
        .await
        .map_err(FsError::trap)?;

        let mut view = self.as_wasi_view();
        let stat = HostDescriptor::stat(&mut view.filesystem(), self_).await;

        let stat = match stat {
            Ok(mut stat) => {
                stat.status_change_timestamp = None; // We cannot guarantee this to be the same during replays, so we rather not support it
                Ok(stat)
            }
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
                let accessed = times.data_access_timestamp.as_ref().map(|t| {
                    SystemTimeSpec::from(<SerializableDateTime as Into<SystemTime>>::into(
                        t.clone(),
                    ))
                });
                let modified = times.data_modification_timestamp.as_ref().map(|t| {
                    SystemTimeSpec::from(<SerializableDateTime as Into<SystemTime>>::into(
                        t.clone(),
                    ))
                });
                let span = tracing::Span::current();
                spawn_blocking(move || {
                    let _enter = span.enter();
                    set_symlink_times(path, accessed, modified)
                })
                .await?;
                let mut stat = stat.unwrap();
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
        let full_path = match self.table().get(&self_)? {
            Descriptor::File(f) => f.path.join(path.clone()),
            Descriptor::Dir(d) => d.path.join(path.clone()),
        };
        let _effect = self
            .filesystem_runtime()
            .begin_effect()
            .await
            .map_err(FsError::trap)?;

        // `ReadLocal`: the local stat always runs (its timestamps are then overridden by the durable
        // value), so only the file-times are made durable via `CallHandle::run`.
        let handle = CallHandle::<FilesystemTypesDescriptorStatAt, NotCancellable>::start(
            self,
            HostRequestFileSystemPath {
                path: full_path.to_string_lossy().to_string(),
            },
            DurableFunctionType::ReadLocal,
        )
        .await
        .map_err(FsError::trap)?;

        let mut view = self.as_wasi_view();
        let stat = HostDescriptor::stat_at(&mut view.filesystem(), self_, path_flags, path).await;

        let stat = match stat {
            Ok(mut stat) => {
                stat.status_change_timestamp = None; // We cannot guarantee this to be the same during replays, so we rather not support it
                Ok(stat)
            }
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
                let accessed = times.data_access_timestamp.as_ref().map(|t| {
                    SystemTimeSpec::from(<SerializableDateTime as Into<SystemTime>>::into(
                        t.clone(),
                    ))
                });
                let modified = times.data_modification_timestamp.as_ref().map(|t| {
                    SystemTimeSpec::from(<SerializableDateTime as Into<SystemTime>>::into(
                        t.clone(),
                    ))
                });
                let span = tracing::Span::current();
                spawn_blocking(move || {
                    let _enter = span.enter();
                    set_symlink_times(full_path, accessed, modified)
                })
                .await?;
                let mut stat = stat.unwrap();
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
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_update_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_read_only(&fd)?;
        self.fail_if_read_only_path(&fd, &path, path_flags.contains(PathFlags::SYMLINK_FOLLOW))?;

        self.observe_function_call("filesystem::types::descriptor", "set_times_at");
        let directory = p2_directory(self.table().get(&fd)?)?;
        validate_directory_mutation(&directory).map_err(p2_native_guest)?;
        let follow = path_flags.contains(PathFlags::SYMLINK_FOLLOW);
        let accessed = p2_native_time(data_access_timestamp)?;
        let modified = p2_native_time(data_modification_timestamp)?;
        let runtime = self.filesystem_runtime();
        let before =
            p2_initial_probe(&runtime, p2_path_times(&directory, &path, follow).await).await?;
        let mut adapter = P2MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_set_path_times(
                    &directory_for_attempt,
                    &path_for_attempt,
                    follow,
                    accessed,
                    modified,
                )
            })
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = p2_times_postcondition(
                        p2_path_times(&directory, &path, follow).await,
                        before,
                        data_access_timestamp,
                        data_modification_timestamp,
                        true,
                    );
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn link_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "link_at");
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_path_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_read_only(&self_)?;
        self.fail_if_read_only(&new_descriptor)?;
        self.fail_if_read_only_path(
            &self_,
            &old_path,
            old_path_flags.contains(PathFlags::SYMLINK_FOLLOW),
        )?;
        self.fail_if_read_only_path(&new_descriptor, &new_path, false)?;
        let source_directory = p2_directory(self.table().get(&self_)?)?;
        let destination_directory = p2_directory(self.table().get(&new_descriptor)?)?;
        validate_two_directory_mutation(&source_directory, &destination_directory)
            .map_err(p2_native_guest)?;
        if old_path_flags.contains(PathFlags::SYMLINK_FOLLOW) {
            return Err(ErrorCode::Invalid.into());
        }
        let runtime = self.filesystem_runtime();
        let source_before =
            p2_initial_probe(&runtime, p2_path_state(&source_directory, &old_path).await).await?;
        let destination_before = p2_initial_probe(
            &runtime,
            p2_path_state(&destination_directory, &new_path).await,
        )
        .await?;
        let mut adapter = P2MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let source_directory_for_attempt = source_directory.clone();
            let destination_directory_for_attempt = destination_directory.clone();
            let old_path_for_attempt = old_path.clone();
            let new_path_for_attempt = new_path.clone();
            let effect = Arc::clone(&effect);
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_hard_link(
                    &source_directory_for_attempt,
                    &old_path_for_attempt,
                    &destination_directory_for_attempt,
                    &new_path_for_attempt,
                )
            })
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = p2_link_postcondition(
                        source_before,
                        destination_before,
                        p2_path_state(&source_directory, &old_path).await,
                        p2_path_state(&destination_directory, &new_path).await,
                    );
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn open_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<Resource<Descriptor>, FsError> {
        let mutating = open_flags.intersects(OpenFlags::CREATE | OpenFlags::TRUNCATE);
        let effect = if mutating {
            Some(Arc::new(
                self.filesystem_runtime()
                    .begin_update_effect()
                    .await
                    .map_err(FsError::trap)?,
            ))
        } else {
            None
        };
        if open_flags.contains(OpenFlags::TRUNCATE) || flags.contains(DescriptorFlags::WRITE) {
            self.fail_if_read_only_path(
                &self_,
                &path,
                path_flags.contains(PathFlags::SYMLINK_FOLLOW),
            )?;
        }
        self.observe_function_call("filesystem::types::descriptor", "open_at");
        if !mutating {
            let mut view = self.as_wasi_view();
            return HostDescriptor::open_at(
                &mut view.filesystem(),
                self_,
                path_flags,
                path,
                open_flags,
                flags,
            )
            .await;
        }

        let directory = p2_directory(self.table().get(&self_)?)?;
        let follow = path_flags.contains(PathFlags::SYMLINK_FOLLOW);
        let native_options = NativeOpenOptions {
            create: open_flags.contains(OpenFlags::CREATE),
            directory: open_flags.contains(OpenFlags::DIRECTORY),
            exclusive: open_flags.contains(OpenFlags::EXCLUSIVE),
            truncate: open_flags.contains(OpenFlags::TRUNCATE),
            follow,
            read: flags.contains(DescriptorFlags::READ),
            write: flags.contains(DescriptorFlags::WRITE),
        };
        validate_open(
            &directory,
            native_options,
            flags.intersects(
                DescriptorFlags::FILE_INTEGRITY_SYNC
                    | DescriptorFlags::DATA_INTEGRITY_SYNC
                    | DescriptorFlags::REQUESTED_WRITE_SYNC,
            ),
        )
        .map_err(p2_native_guest)?;
        let runtime = self.filesystem_runtime();
        let before = p2_initial_probe(
            &runtime,
            p2_path_state_with_follow(&directory, &path, follow).await,
        )
        .await?;
        let requested_type = if open_flags.contains(OpenFlags::DIRECTORY) {
            P2PathObjectType::Directory
        } else {
            P2PathObjectType::RegularFile
        };
        let operation = if open_flags.contains(OpenFlags::CREATE) {
            MutationOperation::Create
        } else {
            MutationOperation::Resize
        };
        let mut adapter = P2MutationAdapter::for_operation(runtime, operation);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(effect.as_ref().expect("mutating open has an effect lease"));
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_open(&directory_for_attempt, &path_for_attempt, native_options)
            })
            .await;
            match result {
                Ok(NativeOpenResult::Descriptor(descriptor)) => {
                    return Ok(self.table().push(descriptor)?);
                }
                #[cfg(windows)]
                Ok(NativeOpenResult::IsDirectory) => {
                    return Err(ErrorCode::IsDirectory.into());
                }
                Ok(NativeOpenResult::NotDirectory) => {
                    return Err(p2_native_guest(NativeMutationGuestError::NotDirectory));
                }
                Err(error) => {
                    let postcondition = p2_open_postcondition(
                        before,
                        p2_path_state_with_follow(&directory, &path, follow).await,
                        requested_type,
                        open_flags.contains(OpenFlags::TRUNCATE),
                        open_flags.contains(OpenFlags::EXCLUSIVE),
                    );
                    match adapter.io_failure(error, postcondition, true).await {
                        P2MutationAction::Retry => {}
                        P2MutationAction::Success => {
                            let safe_flags = open_flags & OpenFlags::DIRECTORY;
                            let mut view = self.as_wasi_view();
                            return HostDescriptor::open_at(
                                &mut view.filesystem(),
                                Resource::new_borrow(self_.rep()),
                                path_flags,
                                path.clone(),
                                safe_flags,
                                flags,
                            )
                            .await;
                        }
                        P2MutationAction::Error(error) => return Err(error),
                        P2MutationAction::Trap => {
                            return Err(FsError::trap(wasmtime::Error::msg(
                                "agent filesystem mutation invalidated the runtime",
                            )));
                        }
                    }
                }
            }
        }
    }

    async fn readlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<String, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "readlink_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::readlink_at(&mut view.filesystem(), self_, path).await
    }

    async fn remove_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "remove_directory_at");
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_path_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_contains_read_only_path(&self_, &path, false)?;
        let directory = p2_directory(self.table().get(&self_)?)?;
        validate_directory_mutation(&directory).map_err(p2_native_guest)?;
        let runtime = self.filesystem_runtime();
        let before = p2_initial_probe(&runtime, p2_path_state(&directory, &path).await).await?;
        let mut adapter = P2MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_remove_directory(&directory_for_attempt, &path_for_attempt)
            })
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition =
                        p2_remove_postcondition(before, p2_path_state(&directory, &path).await);
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn rename_at(
        &mut self,
        old_fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_path_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_read_only(&old_fd)?;
        self.fail_if_read_only(&new_fd)?;
        self.fail_if_contains_read_only_path(&old_fd, &old_path, false)?;
        self.fail_if_contains_read_only_path(&new_fd, &new_path, false)?;

        self.observe_function_call("filesystem::types::descriptor", "rename_at");
        let source_directory = p2_directory(self.table().get(&old_fd)?)?;
        let destination_directory = p2_directory(self.table().get(&new_fd)?)?;
        validate_two_directory_mutation(&source_directory, &destination_directory)
            .map_err(p2_native_guest)?;
        let runtime = self.filesystem_runtime();
        let source_before =
            p2_initial_probe(&runtime, p2_path_state(&source_directory, &old_path).await).await?;
        let destination_before = p2_initial_probe(
            &runtime,
            p2_path_state(&destination_directory, &new_path).await,
        )
        .await?;
        let mut adapter = P2MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let source_directory_for_attempt = source_directory.clone();
            let destination_directory_for_attempt = destination_directory.clone();
            let old_path_for_attempt = old_path.clone();
            let new_path_for_attempt = new_path.clone();
            let effect = Arc::clone(&effect);
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_rename(
                    &source_directory_for_attempt,
                    &old_path_for_attempt,
                    &destination_directory_for_attempt,
                    &new_path_for_attempt,
                )
            })
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = p2_rename_postcondition(
                        source_before,
                        destination_before,
                        p2_path_state(&source_directory, &old_path).await,
                        p2_path_state(&destination_directory, &new_path).await,
                    );
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn symlink_at(
        &mut self,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_path_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_read_only(&fd)?;
        self.fail_if_read_only_path(&fd, &new_path, false)?;

        self.observe_function_call("filesystem::types::descriptor", "symlink_at");
        let directory = p2_directory(self.table().get(&fd)?)?;
        validate_directory_mutation(&directory).map_err(p2_native_guest)?;
        let runtime = self.filesystem_runtime();
        let before =
            p2_initial_probe(&runtime, p2_symlink_state(&directory, &new_path).await).await?;
        let mut adapter = P2MutationAdapter::for_operation(runtime, MutationOperation::Create);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let old_path_for_attempt = old_path.clone();
            let new_path_for_attempt = new_path.clone();
            let effect = Arc::clone(&effect);
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_symlink(
                    &directory_for_attempt,
                    &old_path_for_attempt,
                    &new_path_for_attempt,
                )
            })
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition = p2_symlink_postcondition(
                        &before,
                        p2_symlink_state(&directory, &new_path).await,
                        &old_path,
                    );
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn unlink_file_at(
        &mut self,
        fd: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let effect = Arc::new(
            self.filesystem_runtime()
                .begin_path_effect()
                .await
                .map_err(FsError::trap)?,
        );
        self.fail_if_read_only(&fd)?;
        self.fail_if_read_only_path(&fd, &path, false)?;

        self.observe_function_call("filesystem::types::descriptor", "unlink_file_at");
        let directory = p2_directory(self.table().get(&fd)?)?;
        validate_directory_mutation(&directory).map_err(p2_native_guest)?;
        let runtime = self.filesystem_runtime();
        let before = p2_initial_probe(&runtime, p2_path_state(&directory, &path).await).await?;
        let mut adapter = P2MutationAdapter::new(runtime);
        loop {
            adapter.begin_attempt();
            let directory_for_attempt = directory.clone();
            let path_for_attempt = path.clone();
            let effect = Arc::clone(&effect);
            let result = run_blocking_filesystem_mutation(effect, move || {
                native_unlink_file(&directory_for_attempt, &path_for_attempt)
            })
            .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error) => {
                    let postcondition =
                        p2_remove_postcondition(before, p2_path_state(&directory, &path).await);
                    if !p2_finish_native_mutation(&adapter, error, postcondition, true).await? {
                        return Ok(());
                    }
                }
            }
        }
    }

    async fn is_same_object(
        &mut self,
        self_: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        self.observe_function_call("filesystem::types::descriptor", "is_same_object");
        let mut view = self.as_wasi_view();
        HostDescriptor::is_same_object(&mut view.filesystem(), self_, other).await
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
        HostDescriptor::drop(&mut self.as_wasi_view().filesystem(), rep)
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
        if let Some(error) =
            crate::services::agent_filesystem::classified_filesystem_stream_error_code(
                self.table().get(&err)?,
            )
        {
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

/// Computes the deterministic metadata hash from the durable stat result's
/// modification timestamp and file size. Shared by the P2 and P3 filesystem
/// host implementations so both produce identical hashes for the same durable
/// stat data.
pub(crate) fn calculate_metadata_hash_parts(modified: Option<(u64, u32)>, size: u64) -> (u64, u64) {
    let mut hasher = MetroHash128::new();

    let (seconds, nanoseconds) = modified.unwrap_or((0, 0));
    hasher.write_u64(seconds);
    hasher.write_u32(nanoseconds);
    hasher.write_u64(size);

    hasher.finish128()
}

#[cfg(test)]
mod p2_mutation_tests {
    use super::*;
    use crate::services::agent_filesystem::{
        AgentFilesystemUsage, FilesystemCapacity, ResolvedAgentFilesystemLimits,
    };
    use test_r::test;

    fn healthy_capacity() -> FilesystemCapacity {
        FilesystemCapacity {
            total_bytes: 100,
            available_bytes: 50,
            total_filesystem_objects: 100,
            available_filesystem_objects: 50,
        }
    }

    fn path_state(identity: Option<P2ObjectIdentity>) -> P2PathState {
        P2PathState {
            identity,
            type_: P2PathObjectType::RegularFile,
            size: 17,
        }
    }

    #[test]
    fn p2_object_comparison_requires_authoritative_identity() {
        assert!(!p2_same_object(path_state(None), path_state(None)));
        assert!(!p2_same_optional_object(
            Some(path_state(None)),
            Some(path_state(None)),
        ));
        assert!(p2_same_optional_object(None, None));

        let identity = P2ObjectIdentity {
            device: 3,
            inode: 5,
        };
        assert!(p2_same_object(
            path_state(Some(identity)),
            path_state(Some(identity)),
        ));
    }

    #[test]
    async fn p2_adapter_retries_only_proven_no_effect_and_at_most_once() {
        let runtime = crate::services::agent_filesystem::AgentFilesystemRuntime::new_for_test();
        let mut adapter = P2MutationAdapter::new(runtime);

        adapter.begin_attempt();
        assert!(matches!(
            adapter
                .failure(
                    ErrorCode::Busy.into(),
                    P2MutationPostcondition::NoEffect,
                    true
                )
                .await,
            P2MutationAction::Retry
        ));
        adapter.begin_attempt();
        assert!(matches!(
            adapter
                .failure(ErrorCode::Busy.into(), P2MutationPostcondition::NoEffect, true)
                .await,
            P2MutationAction::Error(error)
                if error.downcast_ref() == Some(&ErrorCode::Busy)
        ));
    }

    #[test_r::test]
    async fn p2_adapter_accepts_satisfied_postcondition_without_retry() {
        let runtime = crate::services::agent_filesystem::AgentFilesystemRuntime::new_for_test();
        let mut adapter = P2MutationAdapter::new(runtime);
        adapter.begin_attempt();

        assert!(matches!(
            adapter
                .failure(
                    ErrorCode::Interrupted.into(),
                    P2MutationPostcondition::Satisfied,
                    true,
                )
                .await,
            P2MutationAction::Success
        ));

        let mut adapter = P2MutationAdapter::new(
            crate::services::agent_filesystem::AgentFilesystemRuntime::new_for_test(),
        );
        adapter.begin_attempt();
        assert!(matches!(
            adapter
                .failure(
                    ErrorCode::NoEntry.into(),
                    P2MutationPostcondition::Satisfied,
                    true,
                )
                .await,
            P2MutationAction::Success
        ));
    }

    #[test_r::test]
    async fn p2_adapter_invalidates_changed_or_unknown_effect() {
        let runtime = crate::services::agent_filesystem::AgentFilesystemRuntime::new_for_test();
        let mut adapter = P2MutationAdapter::new(runtime.clone());
        adapter.begin_attempt();

        assert!(matches!(
            adapter
                .failure(
                    ErrorCode::Busy.into(),
                    P2MutationPostcondition::Unknown,
                    true
                )
                .await,
            P2MutationAction::Trap
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[cfg(target_os = "linux")]
    #[test_r::test]
    async fn p2_native_eio_reaches_terminal_classifier() {
        let runtime = AgentFilesystemRuntime::new_for_test();
        let mut adapter = P2MutationAdapter::new(runtime.clone());
        adapter.begin_attempt();

        assert!(matches!(
            adapter
                .io_failure(
                    std::io::Error::from_raw_os_error(libc::EIO),
                    P2MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P2MutationAction::Trap
        ));
        assert!(runtime.begin_effect().await.is_err());
    }

    #[test_r::test]
    async fn p2_adapter_distinguishes_quota_from_physical_exhaustion() {
        let exhausted = FilesystemCapacity {
            available_bytes: 0,
            available_filesystem_objects: 0,
            ..healthy_capacity()
        };
        let quota_runtime =
            crate::services::agent_filesystem::AgentFilesystemRuntime::new_for_test_with_observations(
                Some(AgentFilesystemUsage {
                    allocated_bytes: 50,
                    filesystem_objects: 10,
                }),
                Some(ResolvedAgentFilesystemLimits {
                    allocated_bytes: 50,
                    filesystem_objects: 10,
                    filesystem_object_limit_policy_version: 2,
                }),
                exhausted,
            );
        let mut quota = P2MutationAdapter::new(quota_runtime);
        quota.begin_attempt();
        assert!(matches!(
            quota
                .failure(
                    ErrorCode::InsufficientSpace.into(),
                    P2MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P2MutationAction::Error(error)
                if error.downcast_ref() == Some(&ErrorCode::Quota)
        ));

        let physical_runtime =
            crate::services::agent_filesystem::AgentFilesystemRuntime::new_for_test_with_observations(
                None, None, exhausted,
            );
        let recovery_attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        physical_runtime.set_pressure_recovery_callback(Some({
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
        let mut physical = P2MutationAdapter::new(physical_runtime);
        physical.begin_attempt();
        assert!(matches!(
            physical
                .failure(
                    ErrorCode::InsufficientSpace.into(),
                    P2MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P2MutationAction::Retry
        ));
        physical.begin_attempt();
        assert!(matches!(
            physical
                .failure(
                    ErrorCode::InsufficientSpace.into(),
                    P2MutationPostcondition::NoEffect,
                    true,
                )
                .await,
            P2MutationAction::Error(error)
                if error.downcast_ref() == Some(&ErrorCode::InsufficientSpace)
        ));
        assert_eq!(
            recovery_attempts.load(std::sync::atomic::Ordering::Acquire),
            1
        );
    }

    #[test_r::test]
    async fn p2_adapter_preserves_access_error_when_backend_is_healthy() {
        let runtime =
            crate::services::agent_filesystem::AgentFilesystemRuntime::new_for_test_with_observations(
                None,
                None,
                healthy_capacity(),
            );
        let mut adapter = P2MutationAdapter::new(runtime.clone());
        adapter.begin_attempt();

        assert!(matches!(
            adapter
                .failure(ErrorCode::Access.into(), P2MutationPostcondition::NoEffect, true)
                .await,
            P2MutationAction::Error(error)
                if error.downcast_ref() == Some(&ErrorCode::Access)
        ));
        assert!(runtime.begin_effect().await.is_ok());
    }
}
