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

use fs_set_times::{SystemTimeSpec, set_symlink_times};
use metrohash::MetroHash128;
use wasmtime::component::Resource;
use wasmtime_wasi::filesystem::{FilePerms, WasiFilesystemView as _};
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
use crate::durable_host::filesystem::durable::{
    DurableFileOutputStream, DurableFilesystem, FilesystemEntryStorage,
    FilesystemOutputStreamState, FilesystemPreview, FilesystemSizeChange, FilesystemWriteMode,
    directory_entry_identity, replaced_file_storage, rewrite_descendant_path,
};
use crate::durable_host::io::streams::reconcile_all_pending_filesystem_streams_locked;
use crate::durable_host::{DurabilityHost, DurableWorkerCtx, FilesystemInputStreamState};
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

fn descriptor_path(descriptor: &Descriptor) -> &std::path::Path {
    match descriptor {
        Descriptor::File(file) => &file.path,
        Descriptor::Dir(dir) => &dir.path,
    }
}

async fn filesystem_mutation_guard<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
) -> Result<tokio::sync::OwnedMutexGuard<()>, FsError> {
    let guard = ctx.filesystem_mutation_lock().lock_owned().await;
    reconcile_all_pending_filesystem_streams_locked(ctx)
        .await
        .map_err(|err| {
            FsError::trap(wasmtime::Error::msg(format!(
                "failed to complete pending filesystem stream write: {err:?}"
            )))
        })?;
    Ok(guard)
}

impl<Ctx: WorkerCtx> HostDescriptor for DurableWorkerCtx<Ctx> {
    fn read_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<InputStream>, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "read_via_stream");
        let file = match self.table().get(&self_)? {
            Descriptor::File(file) => file.file.clone(),
            Descriptor::Dir(_) => return Err(FsError::from(ErrorCode::IsDirectory)),
        };
        let stream =
            HostDescriptor::read_via_stream(&mut self.as_wasi_view().filesystem(), self_, offset)?;
        self.state.open_filesystem_input_streams.insert(
            stream.rep(),
            FilesystemInputStreamState {
                file,
                position: offset,
            },
        );
        Ok(stream)
    }

    fn write_via_stream(
        &mut self,
        fd: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<OutputStream>, FsError> {
        self.fail_if_read_only(&fd)?;
        self.observe_function_call("filesystem::types::descriptor", "write_via_stream");
        let (mutation_path, file) = match self.table().get(&fd)? {
            Descriptor::File(file) if file.perms.contains(FilePerms::WRITE) => {
                (file.path.clone(), file.file.clone())
            }
            Descriptor::File(_) => return Err(FsError::from(ErrorCode::BadDescriptor)),
            Descriptor::Dir(_) => return Err(FsError::from(ErrorCode::IsDirectory)),
        };
        let stream = self.table().push(
            DurableFileOutputStream::new(file.clone(), FilesystemWriteMode::At(offset)).into_dyn(),
        )?;
        self.filesystem.register_output_stream(
            stream.rep(),
            FilesystemOutputStreamState::new(mutation_path, file, Some(offset)),
        );
        Ok(stream)
    }

    fn append_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<OutputStream>, FsError> {
        self.fail_if_read_only(&self_)?;
        self.observe_function_call("filesystem::types::descriptor", "append_via_stream");
        let (mutation_path, file) = match self.table().get(&self_)? {
            Descriptor::File(file) if file.perms.contains(FilePerms::WRITE) => {
                (file.path.clone(), file.file.clone())
            }
            Descriptor::File(_) => return Err(FsError::from(ErrorCode::BadDescriptor)),
            Descriptor::Dir(_) => return Err(FsError::from(ErrorCode::IsDirectory)),
        };
        let stream = self.table().push(
            DurableFileOutputStream::new(file.clone(), FilesystemWriteMode::Append).into_dyn(),
        )?;
        self.filesystem.register_output_stream(
            stream.rep(),
            FilesystemOutputStreamState::new(mutation_path, file, None),
        );
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
        let mut view = self.as_wasi_view();
        HostDescriptor::sync_data(&mut view.filesystem(), self_).await
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
        self.fail_if_read_only(&fd)?;
        let file = match self.table().get(&fd)? {
            Descriptor::File(file) => file.file.clone(),
            Descriptor::Dir(_) => return Err(FsError::from(ErrorCode::IsDirectory)),
        };
        let _mutation_guard = filesystem_mutation_guard(self).await?;
        let account_storage =
            !self.state.is_replay() && !self.filesystem.is_file_unlinked(file.clone()).await;

        // Determine whether this is a growth and charge the delta.
        // We borrow fd to stat before consuming it in set_size.
        let current_size = if !account_storage {
            size
        } else {
            let fd_borrow = Resource::new_borrow(fd.rep());
            let mut view = self.as_wasi_view();
            HostDescriptor::stat(&mut view.filesystem(), fd_borrow)
                .await?
                .size
        };

        let size_change = FilesystemSizeChange::between(current_size, size);
        let reservation = if let FilesystemSizeChange::Grow(growth) = size_change {
            self.reserve_filesystem_storage(growth)
                .await
                .map_err(|e| FsError::trap(wasmtime::Error::from_anyhow(e)))?
        } else {
            None
        };
        // size == current_size: no-op

        self.observe_function_call("filesystem::types::descriptor", "set_size");

        let result = {
            let mut view = self.as_wasi_view();
            HostDescriptor::set_size(&mut view.filesystem(), fd, size).await
        };

        if let Some(reservation) = reservation {
            if result.is_ok() {
                let FilesystemSizeChange::Grow(growth) = size_change else {
                    unreachable!("storage was reserved without file growth")
                };
                self.commit_filesystem_storage_reservation(reservation, growth)
                    .await;
            } else {
                drop(reservation);
            }
        } else if result.is_ok()
            && let FilesystemSizeChange::Shrink(shrink) = size_change
        {
            self.release_filesystem_storage_space(shrink).await;
        }

        result
    }

    async fn set_times(
        &mut self,
        fd: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        self.fail_if_read_only(&fd)?;

        self.observe_function_call("filesystem::types::descriptor", "set_times");

        let mut view = self.as_wasi_view();
        HostDescriptor::set_times(
            &mut view.filesystem(),
            fd,
            data_access_timestamp,
            data_modification_timestamp,
        )
        .await
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
            Descriptor::File(file) => file.file.clone(),
            Descriptor::Dir(_) => return Err(FsError::from(ErrorCode::IsDirectory)),
        };
        let _mutation_guard = filesystem_mutation_guard(self).await?;
        let account_storage =
            !self.state.is_replay() && !self.filesystem.is_file_unlinked(file.clone()).await;

        let current_size = if !account_storage {
            offset.saturating_add(buffer.len() as u64)
        } else {
            let fd_borrow = Resource::new_borrow(fd.rep());
            let mut view = self.as_wasi_view();
            HostDescriptor::stat(&mut view.filesystem(), fd_borrow)
                .await?
                .size
        };

        let requested_end = offset.saturating_add(buffer.len() as u64);
        let requested_growth = requested_end.saturating_sub(current_size);
        let reservation = if requested_growth > 0 {
            self.reserve_filesystem_storage(requested_growth)
                .await
                .map_err(|e| FsError::trap(wasmtime::Error::from_anyhow(e)))?
        } else {
            None
        };

        self.observe_function_call("filesystem::types::descriptor", "write");
        let result = {
            let mut view = self.as_wasi_view();
            HostDescriptor::write(&mut view.filesystem(), fd, buffer, offset).await
        };

        if let Some(reservation) = reservation {
            match result {
                Ok(written) => {
                    let actual_end = offset.saturating_add(written);
                    let actual_growth = actual_end.saturating_sub(current_size);
                    self.commit_filesystem_storage_reservation(reservation, actual_growth)
                        .await;
                    Ok(written)
                }
                Err(err) => {
                    drop(reservation);
                    Err(err)
                }
            }
        } else {
            result
        }
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
        let mut view = self.as_wasi_view();
        HostDescriptor::sync(&mut view.filesystem(), self_).await
    }

    async fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let _mutation_guard = filesystem_mutation_guard(self).await?;
        self.observe_function_call("filesystem::types::descriptor", "create_directory_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::create_directory_at(&mut view.filesystem(), self_, path).await
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        let path = descriptor_path(self.table().get(&self_)?).to_path_buf();

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
        let full_path = descriptor_path(self.table().get(&self_)?).join(&path);

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
        self.fail_if_read_only(&fd)?;

        self.observe_function_call("filesystem::types::descriptor", "set_times_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::set_times_at(
            &mut view.filesystem(),
            fd,
            path_flags,
            path,
            data_access_timestamp,
            data_modification_timestamp,
        )
        .await
    }

    async fn link_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path_flags: PathFlags,
        old_path: String,
        new_descriptor: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        let _mutation_guard = filesystem_mutation_guard(self).await?;
        self.observe_function_call("filesystem::types::descriptor", "link_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::link_at(
            &mut view.filesystem(),
            self_,
            old_path_flags,
            old_path,
            new_descriptor,
            new_path.clone(),
        )
        .await
    }

    async fn open_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
        open_flags: OpenFlags,
        flags: DescriptorFlags,
    ) -> Result<Resource<Descriptor>, FsError> {
        let _mutation_guard = filesystem_mutation_guard(self).await?;
        let truncated_size = if open_flags.contains(OpenFlags::TRUNCATE) && !self.state.is_replay()
        {
            let fd_borrow = Resource::new_borrow(self_.rep());
            let mut view = self.as_wasi_view();
            match HostDescriptor::stat_at(
                &mut view.filesystem(),
                fd_borrow,
                path_flags,
                path.clone(),
            )
            .await
            {
                Ok(s) => s.size,
                Err(_) => 0,
            }
        } else {
            0
        };

        self.observe_function_call("filesystem::types::descriptor", "open_at");
        let result = {
            let mut view = self.as_wasi_view();
            HostDescriptor::open_at(
                &mut view.filesystem(),
                self_,
                path_flags,
                path,
                open_flags,
                flags,
            )
            .await
        };

        let opened_file = if let Ok(descriptor) = &result {
            self.filesystem
                .register_descriptor(FilesystemPreview::P2, descriptor.rep());
            match self.table().get_mut(descriptor) {
                Ok(Descriptor::File(file)) => {
                    file.path = DurableFilesystem::normalized_path(&file.path);
                    Some(Arc::clone(&file.file))
                }
                Ok(Descriptor::Dir(directory)) => {
                    directory.path = DurableFilesystem::normalized_path(&directory.path);
                    None
                }
                Err(_) => None,
            }
        } else {
            None
        };
        if let Some(file) = opened_file {
            self.filesystem.mark_file_linked(file).await;
        }

        if result.is_ok() && truncated_size > 0 {
            self.release_filesystem_storage_space(truncated_size).await;
        }

        result
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
        let _mutation_guard = filesystem_mutation_guard(self).await?;
        self.observe_function_call("filesystem::types::descriptor", "remove_directory_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::remove_directory_at(&mut view.filesystem(), self_, path.clone()).await
    }

    async fn rename_at(
        &mut self,
        old_fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        self.fail_if_read_only(&old_fd)?;
        self.fail_if_read_only(&new_fd)?;
        let old_target = DurableFilesystem::normalized_path(
            &descriptor_path(self.table().get(&old_fd)?).join(&old_path),
        );
        let new_target = DurableFilesystem::normalized_path(
            &descriptor_path(self.table().get(&new_fd)?).join(&new_path),
        );
        let old_directory = match self.table().get(&old_fd)? {
            Descriptor::Dir(directory) => Some(directory.clone()),
            Descriptor::File(_) => None,
        };
        let new_directory = match self.table().get(&new_fd)? {
            Descriptor::Dir(directory) => Some(directory.clone()),
            Descriptor::File(_) => None,
        };
        let _mutation_guard = filesystem_mutation_guard(self).await?;
        let replaced = if self.state.is_replay() {
            Default::default()
        } else {
            let old_fd_ref = Resource::new_borrow(old_fd.rep());
            let mut view = self.as_wasi_view();
            let filesystem = &mut view.filesystem();
            let source_stat = HostDescriptor::stat_at(
                filesystem,
                old_fd_ref,
                PathFlags::empty(),
                old_path.clone(),
            )
            .await;
            let destination_stat = HostDescriptor::stat_at(
                filesystem,
                Resource::new_borrow(new_fd.rep()),
                PathFlags::empty(),
                new_path.clone(),
            )
            .await;
            match (source_stat, destination_stat) {
                (Ok(_), Ok(destination)) if old_directory.is_some() && new_directory.is_some() => {
                    let (source_identity, destination_identity) = tokio::join!(
                        directory_entry_identity(
                            old_directory.expect("checked above"),
                            old_path.clone().into()
                        ),
                        directory_entry_identity(
                            new_directory.expect("checked above"),
                            new_path.clone().into()
                        )
                    );
                    replaced_file_storage(
                        source_identity.ok().flatten(),
                        FilesystemEntryStorage {
                            identity: destination_identity.ok().flatten(),
                            size: destination.size,
                            is_regular_file: destination.type_ == DescriptorType::RegularFile,
                            link_count: destination.link_count,
                        },
                    )
                }
                _ => Default::default(),
            }
        };

        self.observe_function_call("filesystem::types::descriptor", "rename_at");
        let result = {
            let mut view = self.as_wasi_view();
            HostDescriptor::rename_at(
                &mut view.filesystem(),
                old_fd,
                old_path.clone(),
                new_fd,
                new_path.clone(),
            )
            .await
        };
        if result.is_ok() {
            self.filesystem.mark_object_unlinked(replaced.object_id);
            self.rename_open_filesystem_stream_paths(&old_target, &new_target);
            let descriptor_reps = self.filesystem.descriptor_reps(FilesystemPreview::P2);
            for descriptor_rep in descriptor_reps {
                let descriptor = Resource::<Descriptor>::new_borrow(descriptor_rep);
                if let Ok(descriptor) = self.table().get_mut(&descriptor) {
                    let path = match descriptor {
                        Descriptor::File(file) => &mut file.path,
                        Descriptor::Dir(directory) => &mut directory.path,
                    };
                    rewrite_descendant_path(path, &old_target, &new_target);
                }
            }
            if replaced.bytes > 0 {
                self.release_filesystem_storage_space(replaced.bytes).await;
            }
        }
        result
    }

    async fn symlink_at(
        &mut self,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        self.fail_if_read_only(&fd)?;
        let _mutation_guard = filesystem_mutation_guard(self).await?;

        self.observe_function_call("filesystem::types::descriptor", "symlink_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::symlink_at(&mut view.filesystem(), fd, old_path, new_path.clone()).await
    }

    async fn unlink_file_at(
        &mut self,
        fd: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        self.fail_if_read_only(&fd)?;
        let directory = match self.table().get(&fd)? {
            Descriptor::Dir(directory) => Some(directory.clone()),
            Descriptor::File(_) => None,
        };
        let _mutation_guard = filesystem_mutation_guard(self).await?;

        // Stat the target file before unlinking to know how many bytes to release.
        // Use the upstream (non-durable) stat_at to avoid oplog side effects.
        let released = if self.state.is_replay() {
            Default::default()
        } else {
            let target_identity = match directory {
                Some(directory) => directory_entry_identity(directory, path.clone().into())
                    .await
                    .ok()
                    .flatten(),
                None => None,
            };
            let fd_borrow = Resource::new_borrow(fd.rep());
            let mut view = self.as_wasi_view();
            match HostDescriptor::stat_at(
                &mut view.filesystem(),
                fd_borrow,
                PathFlags::empty(),
                path.clone(),
            )
            .await
            {
                Ok(stat) => FilesystemEntryStorage {
                    identity: target_identity,
                    size: stat.size,
                    is_regular_file: stat.type_ == DescriptorType::RegularFile,
                    link_count: stat.link_count,
                }
                .released_by_unlink(),
                _ => Default::default(),
            }
        };

        self.observe_function_call("filesystem::types::descriptor", "unlink_file_at");
        let result = {
            let mut view = self.as_wasi_view();
            HostDescriptor::unlink_file_at(&mut view.filesystem(), fd, path.clone()).await
        };

        // Only release permits if the unlink actually succeeded.
        if result.is_ok() {
            self.filesystem.mark_object_unlinked(released.object_id);
            self.release_filesystem_storage_space(released.bytes).await;
        }

        result
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
        let descriptor_rep = rep.rep();
        let result = HostDescriptor::drop(&mut self.as_wasi_view().filesystem(), rep);
        if result.is_ok() {
            self.filesystem
                .unregister_descriptor(FilesystemPreview::P2, descriptor_rep);
        }
        result
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
