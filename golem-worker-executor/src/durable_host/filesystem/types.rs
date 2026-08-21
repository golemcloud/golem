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
use std::time::Duration;
use std::time::SystemTime;

use bytes::Bytes;
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

use crate::durable_host::concurrent::{CallHandle, NotCancellable};
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, FilesystemOutputStreamState};
use crate::services::agent_filesystem::{
    AgentFilesystemMutationError, ClassifiedFileOutputStream, FilesystemStreamMode,
    NativeMutationGuestError, NativeOpenOptions, NativeOpenResult, RequestedTime,
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

fn p2_directory(descriptor: &Descriptor) -> Result<wasmtime_wasi::filesystem::Dir, FsError> {
    match descriptor {
        Descriptor::Dir(directory) => Ok(directory.clone()),
        Descriptor::File(_) => Err(ErrorCode::NotDirectory.into()),
    }
}

fn p2_native_guest(error: NativeMutationGuestError) -> FsError {
    match error {
        NativeMutationGuestError::Invalid => ErrorCode::Invalid.into(),
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

fn p2_write_result(result: Result<u64, AgentFilesystemMutationError>) -> Result<Filesize, FsError> {
    match result {
        Ok(written) => Ok(written),
        Err(AgentFilesystemMutationError::Guest(error)) => Err(p2_native_guest(error)),
        Err(AgentFilesystemMutationError::Native { error, .. }) => {
            Err(error.into_io_error().into())
        }
        Err(AgentFilesystemMutationError::QuotaExhausted { .. }) => Err(ErrorCode::Quota.into()),
        Err(AgentFilesystemMutationError::InsufficientSpace { .. }) => {
            Err(ErrorCode::InsufficientSpace.into())
        }
        Err(AgentFilesystemMutationError::Cancelled { completed }) => Ok(completed),
        Err(AgentFilesystemMutationError::RuntimeInvalidated { .. }) => Err(FsError::trap(
            wasmtime::Error::msg("agent filesystem mutation invalidated the runtime"),
        )),
    }
}

fn p2_mutation_result<T>(result: Result<T, AgentFilesystemMutationError>) -> Result<T, FsError> {
    match result {
        Ok(value) => Ok(value),
        Err(AgentFilesystemMutationError::Guest(error)) => Err(p2_native_guest(error)),
        Err(AgentFilesystemMutationError::Native { error, .. }) => {
            Err(error.into_io_error().into())
        }
        Err(AgentFilesystemMutationError::QuotaExhausted { .. }) => Err(ErrorCode::Quota.into()),
        Err(AgentFilesystemMutationError::InsufficientSpace { .. }) => {
            Err(ErrorCode::InsufficientSpace.into())
        }
        Err(AgentFilesystemMutationError::Cancelled { .. }) => {
            unreachable!("non-stream mutation is not cancellable")
        }
        Err(AgentFilesystemMutationError::RuntimeInvalidated { .. }) => Err(FsError::trap(
            wasmtime::Error::msg("agent filesystem mutation invalidated the runtime"),
        )),
    }
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
        self.state
            .open_filesystem_output_streams
            .insert(stream.rep(), FilesystemOutputStreamState);
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
            .insert(stream.rep(), FilesystemOutputStreamState);
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
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_sync().await)?;
        let descriptor = self.table().get(&self_)?.clone();
        p2_mutation_result(mutations.sync(admitted, descriptor, true).await)
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
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_resize().await)?;
        let descriptor = self.table().get(&fd)?.clone();
        let checked =
            p2_mutation_result(mutations.check_resize_policy(admitted, descriptor.clone(), size))?;
        let file = match &descriptor {
            Descriptor::File(file) => file.clone(),
            Descriptor::Dir(_) => return Err(ErrorCode::BadDescriptor.into()),
        };
        let prepared = p2_mutation_result(mutations.prepare_resize(checked, file).await)?;
        self.observe_function_call("filesystem::types::descriptor", "set_size");
        p2_mutation_result(mutations.resize(prepared).await.map(|_| ()))
    }

    async fn set_times(
        &mut self,
        fd: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_descriptor_times().await)?;
        let policy_descriptor = self.table().get(&fd)?.clone();
        let checked = p2_mutation_result(
            mutations.check_descriptor_times_policy(admitted, policy_descriptor),
        )?;
        self.observe_function_call("filesystem::types::descriptor", "set_times");
        let descriptor = self.table().get(&fd)?.clone();
        let validated =
            p2_mutation_result(mutations.prepare_descriptor_times(checked, descriptor))?;
        let accessed = p2_native_time(data_access_timestamp)?;
        let modified = p2_native_time(data_modification_timestamp)?;
        let prepared = mutations.bind_descriptor_times(
            validated,
            accessed,
            modified,
            p2_requested_time(data_access_timestamp),
            p2_requested_time(data_modification_timestamp),
        );
        p2_mutation_result(mutations.set_descriptor_times(prepared).await)
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
        self.observe_function_call("filesystem::types::descriptor", "write");
        let completion = self
            .filesystem_runtime()
            .mutations()
            .positioned_write(file, offset, Bytes::from(buffer))
            .map_err(|_| {
                FsError::trap(wasmtime::Error::msg(
                    "agent filesystem mutation invalidated the runtime",
                ))
            })?;
        p2_write_result(completion.await)
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
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_sync().await)?;
        let descriptor = self.table().get(&self_)?.clone();
        p2_mutation_result(mutations.sync(admitted, descriptor, false).await)
    }

    async fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        self.observe_function_call("filesystem::types::descriptor", "create_directory_at");
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_create_directory().await)?;
        let descriptor = self.table().get(&self_)?.clone();
        let checked = p2_mutation_result(
            mutations.check_create_directory_policy(admitted, descriptor, path),
        )?;
        let directory = p2_directory(self.table().get(&self_)?)?;
        let prepared = p2_mutation_result(mutations.prepare_create_directory(checked, directory))?;
        p2_mutation_result(mutations.run_namespace_mutation(prepared).await)
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        let path = match self.table().get(&self_)? {
            Descriptor::File(f) => f.path.clone(),
            Descriptor::Dir(d) => d.path.clone(),
        };
        let mutations = self.filesystem_runtime().mutations();
        let restoration = p2_mutation_result(mutations.admit_durable_times_restoration().await)?;

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
                let accessed = times
                    .data_access_timestamp
                    .as_ref()
                    .map(|t| <SerializableDateTime as Into<SystemTime>>::into(t.clone()));
                let modified = times
                    .data_modification_timestamp
                    .as_ref()
                    .map(|t| <SerializableDateTime as Into<SystemTime>>::into(t.clone()));
                p2_mutation_result(
                    mutations
                        .restore_durable_times(restoration, path, accessed, modified)
                        .await,
                )?;
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
        let mutations = self.filesystem_runtime().mutations();
        let restoration = p2_mutation_result(mutations.admit_durable_times_restoration().await)?;

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
                let accessed = times
                    .data_access_timestamp
                    .as_ref()
                    .map(|t| <SerializableDateTime as Into<SystemTime>>::into(t.clone()));
                let modified = times
                    .data_modification_timestamp
                    .as_ref()
                    .map(|t| <SerializableDateTime as Into<SystemTime>>::into(t.clone()));
                p2_mutation_result(
                    mutations
                        .restore_durable_times(restoration, full_path, accessed, modified)
                        .await,
                )?;
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
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_path_times().await)?;
        let follow = path_flags.contains(PathFlags::SYMLINK_FOLLOW);
        let policy_descriptor = self.table().get(&fd)?.clone();
        let checked = p2_mutation_result(mutations.check_path_times_policy(
            admitted,
            policy_descriptor,
            path,
            follow,
        ))?;
        self.observe_function_call("filesystem::types::descriptor", "set_times_at");
        let directory = p2_directory(self.table().get(&fd)?)?;
        let validated = p2_mutation_result(mutations.prepare_path_times(checked, directory))?;
        let accessed = p2_native_time(data_access_timestamp)?;
        let modified = p2_native_time(data_modification_timestamp)?;
        let prepared = mutations.bind_path_times(
            validated,
            accessed,
            modified,
            p2_requested_time(data_access_timestamp),
            p2_requested_time(data_modification_timestamp),
        );
        p2_mutation_result(mutations.set_path_times(prepared).await)
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
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_hard_link().await)?;
        let source_descriptor = self.table().get(&self_)?.clone();
        let source_follow = old_path_flags.contains(PathFlags::SYMLINK_FOLLOW);
        let source_checked =
            p2_mutation_result(mutations.check_hard_link_source_descriptor_policy(
                admitted,
                source_descriptor,
                old_path,
                source_follow,
            ))?;
        let destination_descriptor = self.table().get(&new_descriptor)?.clone();
        let destination_checked =
            p2_mutation_result(mutations.check_hard_link_destination_descriptor_policy(
                source_checked,
                destination_descriptor,
                new_path,
            ))?;
        let checked =
            p2_mutation_result(mutations.check_hard_link_path_policy(destination_checked))?;
        let source_directory = p2_directory(self.table().get(&self_)?)?;
        let destination_directory = p2_directory(self.table().get(&new_descriptor)?)?;
        let prepared = p2_mutation_result(mutations.prepare_hard_link(
            checked,
            source_directory,
            destination_directory,
        ))?;
        p2_mutation_result(mutations.run_namespace_mutation(prepared).await)
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
        let mutations = self.filesystem_runtime().mutations();
        let admitted = if mutating {
            Some(p2_mutation_result(mutations.admit_mutating_open().await)?)
        } else {
            None
        };
        let follow = path_flags.contains(PathFlags::SYMLINK_FOLLOW);
        let writable =
            open_flags.contains(OpenFlags::TRUNCATE) || flags.contains(DescriptorFlags::WRITE);
        let checked = match (admitted, writable) {
            (Some(admitted), true) => {
                let descriptor = self.table().get(&self_)?.clone();
                Some(p2_mutation_result(mutations.check_mutating_open_policy(
                    admitted,
                    descriptor,
                    path.clone(),
                    follow,
                ))?)
            }
            (Some(admitted), false) => {
                Some(mutations.bind_nonwritable_mutating_open(admitted, path.clone(), follow))
            }
            (None, true) => {
                let descriptor = self.table().get(&self_)?.clone();
                p2_mutation_result(mutations.check_writable_open_policy(
                    &descriptor,
                    &path,
                    follow,
                ))?;
                None
            }
            (None, false) => None,
        };
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
        let native_options = NativeOpenOptions {
            create: open_flags.contains(OpenFlags::CREATE),
            directory: open_flags.contains(OpenFlags::DIRECTORY),
            exclusive: open_flags.contains(OpenFlags::EXCLUSIVE),
            truncate: open_flags.contains(OpenFlags::TRUNCATE),
            follow,
            read: flags.contains(DescriptorFlags::READ),
            write: flags.contains(DescriptorFlags::WRITE),
        };
        let unsupported_sync_flags = flags.intersects(
            DescriptorFlags::FILE_INTEGRITY_SYNC
                | DescriptorFlags::DATA_INTEGRITY_SYNC
                | DescriptorFlags::REQUESTED_WRITE_SYNC,
        );
        let prepared = p2_mutation_result(mutations.prepare_mutating_open(
            checked.expect("mutating open has checked admission"),
            directory,
            native_options,
            unsupported_sync_flags,
        ))?;
        match p2_mutation_result(mutations.open_mutating(prepared).await)? {
            NativeOpenResult::Descriptor(descriptor) => Ok(self.table().push(descriptor)?),
            #[cfg(windows)]
            NativeOpenResult::IsDirectory => Err(ErrorCode::IsDirectory.into()),
            NativeOpenResult::NotDirectory => Err(ErrorCode::NotDirectory.into()),
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
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_remove_directory().await)?;
        let descriptor = self.table().get(&self_)?.clone();
        let checked = p2_mutation_result(
            mutations.check_remove_directory_policy(admitted, descriptor, path),
        )?;
        let directory = p2_directory(self.table().get(&self_)?)?;
        let prepared = p2_mutation_result(mutations.prepare_remove_directory(checked, directory))?;
        p2_mutation_result(mutations.run_namespace_mutation(prepared).await)
    }

    async fn rename_at(
        &mut self,
        old_fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_rename().await)?;
        let source_descriptor = self.table().get(&old_fd)?.clone();
        let source_checked = p2_mutation_result(mutations.check_rename_source_descriptor_policy(
            admitted,
            source_descriptor,
            old_path,
        ))?;
        let destination_descriptor = self.table().get(&new_fd)?.clone();
        let destination_checked =
            p2_mutation_result(mutations.check_rename_destination_descriptor_policy(
                source_checked,
                destination_descriptor,
                new_path,
            ))?;
        let checked = p2_mutation_result(mutations.check_rename_path_policy(destination_checked))?;
        self.observe_function_call("filesystem::types::descriptor", "rename_at");
        let source_directory = p2_directory(self.table().get(&old_fd)?)?;
        let destination_directory = p2_directory(self.table().get(&new_fd)?)?;
        let prepared = p2_mutation_result(mutations.prepare_rename(
            checked,
            source_directory,
            destination_directory,
        ))?;
        p2_mutation_result(mutations.run_namespace_mutation(prepared).await)
    }

    async fn symlink_at(
        &mut self,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_symlink().await)?;
        let descriptor = self.table().get(&fd)?.clone();
        let checked = p2_mutation_result(
            mutations.check_symlink_policy(admitted, descriptor, old_path, new_path),
        )?;
        self.observe_function_call("filesystem::types::descriptor", "symlink_at");
        let directory = p2_directory(self.table().get(&fd)?)?;
        let prepared = p2_mutation_result(mutations.prepare_symlink(checked, directory))?;
        p2_mutation_result(mutations.run_namespace_mutation(prepared).await)
    }

    async fn unlink_file_at(
        &mut self,
        fd: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let mutations = self.filesystem_runtime().mutations();
        let admitted = p2_mutation_result(mutations.admit_unlink_file().await)?;
        let descriptor = self.table().get(&fd)?.clone();
        let checked =
            p2_mutation_result(mutations.check_unlink_file_policy(admitted, descriptor, path))?;
        self.observe_function_call("filesystem::types::descriptor", "unlink_file_at");
        let directory = p2_directory(self.table().get(&fd)?)?;
        let prepared = p2_mutation_result(mutations.prepare_unlink_file(checked, directory))?;
        p2_mutation_result(mutations.run_namespace_mutation(prepared).await)
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
