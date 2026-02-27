// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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
use std::time::SystemTime;

use fs_set_times::{set_symlink_times, SystemTimeSpec};
use metrohash::MetroHash128;
use wasmtime::component::Resource;
use wasmtime_wasi::filesystem::WasiFilesystemView as _;
use wasmtime_wasi::p2::bindings::clocks::wall_clock::Datetime;
use wasmtime_wasi::p2::bindings::filesystem::types::{
    Advice, Descriptor, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry,
    DirectoryEntryStream, Error, ErrorCode, Filesize, Host, HostDescriptor,
    HostDirectoryEntryStream, InputStream, MetadataHashValue, NewTimestamp, OpenFlags,
    OutputStream, PathFlags,
};
use wasmtime_wasi::p2::FsError;
use wasmtime_wasi::p2::ReaddirIterator;
use wasmtime_wasi::runtime::spawn_blocking;

use crate::durable_host::{Durability, DurabilityHost, DurableWorkerCtx};
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

impl<Ctx: WorkerCtx> HostDescriptor for DurableWorkerCtx<Ctx> {
    fn read_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<InputStream>, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "read_via_stream");
        HostDescriptor::read_via_stream(&mut self.as_wasi_view().filesystem(), self_, offset)
    }

    fn write_via_stream(
        &mut self,
        fd: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<OutputStream>, FsError> {
        self.fail_if_read_only(&fd)?;
        self.observe_function_call("filesystem::types::descriptor", "write_via_stream");
        HostDescriptor::write_via_stream(&mut self.as_wasi_view().filesystem(), fd, offset)
    }

    fn append_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<OutputStream>, FsError> {
        self.observe_function_call("filesystem::types::descriptor", "append_via_stream");
        HostDescriptor::append_via_stream(&mut self.as_wasi_view().filesystem(), self_)
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

        self.observe_function_call("filesystem::types::descriptor", "set_size");

        let mut view = self.as_wasi_view();
        HostDescriptor::set_size(&mut view.filesystem(), fd, size).await
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

        self.observe_function_call("filesystem::types::descriptor", "write");
        let mut view = self.as_wasi_view();
        HostDescriptor::write(&mut view.filesystem(), fd, buffer, offset).await
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
        self.observe_function_call("filesystem::types::descriptor", "create_directory_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::create_directory_at(&mut view.filesystem(), self_, path).await
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        let durability =
            Durability::<FilesystemTypesDescriptorStat>::new(self, DurableFunctionType::ReadLocal)
                .await
                .map_err(FsError::trap)?;

        let path = match self.table().get(&self_)? {
            Descriptor::File(f) => f.path.clone(),
            Descriptor::Dir(d) => d.path.clone(),
        };

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

        let result = if durability.is_live() {
            let result = stat
                .clone()
                .map(|stat| SerializableFileTimes {
                    data_access_timestamp: stat.data_access_timestamp.map(|t| t.into()),
                    data_modification_timestamp: stat.data_modification_timestamp.map(|t| t.into()),
                })
                .map_err(FileSystemError::from_result);

            durability
                .persist(
                    self,
                    HostRequestFileSystemPath {
                        path: path.to_string_lossy().to_string(),
                    },
                    HostResponseFileSystemStat { result },
                )
                .await
        } else {
            durability.replay(self).await
        }
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
        let durability = Durability::<FilesystemTypesDescriptorStatAt>::new(
            self,
            DurableFunctionType::ReadLocal,
        )
        .await
        .map_err(FsError::trap)?;

        let full_path = match self.table().get(&self_)? {
            Descriptor::File(f) => f.path.join(path.clone()),
            Descriptor::Dir(d) => d.path.join(path.clone()),
        };

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

        let result = if durability.is_live() {
            let result = stat
                .clone()
                .map(|stat| SerializableFileTimes {
                    data_access_timestamp: stat.data_access_timestamp.map(|t| t.into()),
                    data_modification_timestamp: stat.data_modification_timestamp.map(|t| t.into()),
                })
                .map_err(FileSystemError::from_result);

            durability
                .persist(
                    self,
                    HostRequestFileSystemPath {
                        path: full_path.to_string_lossy().to_string(),
                    },
                    HostResponseFileSystemStat { result },
                )
                .await
        } else {
            durability.replay(self).await
        }
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
        self.observe_function_call("filesystem::types::descriptor", "open_at");
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

        self.observe_function_call("filesystem::types::descriptor", "rename_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::rename_at(
            &mut view.filesystem(),
            old_fd,
            old_path.clone(),
            new_fd,
            new_path.clone(),
        )
        .await
    }

    async fn symlink_at(
        &mut self,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        self.fail_if_read_only(&fd)?;

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

        self.observe_function_call("filesystem::types::descriptor", "unlink_file_at");
        let mut view = self.as_wasi_view();
        HostDescriptor::unlink_file_at(&mut view.filesystem(), fd, path.clone()).await
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
        Host::filesystem_error_code(&mut self.as_wasi_view().filesystem(), err)
    }

    fn convert_error_code(&mut self, err: FsError) -> wasmtime::Result<ErrorCode> {
        Host::convert_error_code(&mut self.as_wasi_view().filesystem(), err)
    }
}

fn calculate_metadata_hash(meta: &DescriptorStat) -> MetadataHashValue {
    let mut hasher = MetroHash128::new();

    let modified = meta.data_modification_timestamp.unwrap_or(Datetime {
        seconds: 0,
        nanoseconds: 0,
    });
    hasher.write_u64(modified.seconds);
    hasher.write_u32(modified.nanoseconds);
    hasher.write_u64(meta.size);

    let (lower, upper) = hasher.finish128();
    MetadataHashValue { lower, upper }
}
