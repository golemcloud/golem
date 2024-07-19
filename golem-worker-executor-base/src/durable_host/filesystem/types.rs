// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::hash::Hasher;
use std::time::SystemTime;

use async_trait::async_trait;
use fs_set_times::{set_symlink_times, SystemTimeSpec};
use metrohash::MetroHash128;
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::clocks::wall_clock::Datetime;
use wasmtime_wasi::bindings::filesystem::types::{
    Advice, Descriptor, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry,
    DirectoryEntryStream, Error, ErrorCode, Filesize, Host, HostDescriptor,
    HostDirectoryEntryStream, InputStream, MetadataHashValue, NewTimestamp, OpenFlags,
    OutputStream, PathFlags,
};
use wasmtime_wasi::runtime::spawn_blocking;
use wasmtime_wasi::FsError;
use wasmtime_wasi::ReaddirIterator;

use golem_common::model::oplog::WrappedFunctionType;

use crate::durable_host::serialized::{
    SerializableDateTime, SerializableError, SerializableFileTimes,
};
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostDescriptor for DurableWorkerCtx<Ctx> {
    fn read_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<InputStream>, FsError> {
        record_host_function_call("filesystem::types::descriptor", "read_via_stream");
        HostDescriptor::read_via_stream(&mut self.as_wasi_view(), self_, offset)
    }

    fn write_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<OutputStream>, FsError> {
        record_host_function_call("filesystem::types::descriptor", "write_via_stream");
        HostDescriptor::write_via_stream(&mut self.as_wasi_view(), self_, offset)
    }

    fn append_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<OutputStream>, FsError> {
        record_host_function_call("filesystem::types::descriptor", "append_via_stream");
        HostDescriptor::append_via_stream(&mut self.as_wasi_view(), self_)
    }

    async fn advise(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
        length: Filesize,
        advice: Advice,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "advise");
        HostDescriptor::advise(&mut self.as_wasi_view(), self_, offset, length, advice).await
    }

    async fn sync_data(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "sync_data");
        HostDescriptor::sync_data(&mut self.as_wasi_view(), self_).await
    }

    async fn get_flags(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorFlags, FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "get_flags");
        HostDescriptor::get_flags(&mut self.as_wasi_view(), self_).await
    }

    async fn get_type(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorType, FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "get_type");
        HostDescriptor::get_type(&mut self.as_wasi_view(), self_).await
    }

    async fn set_size(
        &mut self,
        self_: Resource<Descriptor>,
        size: Filesize,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "set_size");
        HostDescriptor::set_size(&mut self.as_wasi_view(), self_, size).await
    }

    async fn set_times(
        &mut self,
        self_: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "set_times");
        HostDescriptor::set_times(
            &mut self.as_wasi_view(),
            self_,
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
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "read");
        HostDescriptor::read(&mut self.as_wasi_view(), self_, length, offset).await
    }

    async fn write(
        &mut self,
        self_: Resource<Descriptor>,
        buffer: Vec<u8>,
        offset: Filesize,
    ) -> Result<Filesize, FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "write");
        HostDescriptor::write(&mut self.as_wasi_view(), self_, buffer, offset).await
    }

    async fn read_directory(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<DirectoryEntryStream>, FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "read_directory");
        let stream = HostDescriptor::read_directory(&mut self.as_wasi_view(), self_).await?;
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
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "sync");
        HostDescriptor::sync(&mut self.as_wasi_view(), self_).await
    }

    async fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "create_directory_at");
        HostDescriptor::create_directory_at(&mut self.as_wasi_view(), self_, path).await
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "stat");

        let path = match self.table().get(&self_)? {
            Descriptor::File(f) => f.path.clone(),
            Descriptor::Dir(d) => d.path.clone(),
        };

        let mut stat = HostDescriptor::stat(&mut self.as_wasi_view(), self_).await?;
        stat.status_change_timestamp = None; // We cannot guarantee this to be the same during replays, so we rather not support it

        let stat_clone1 = stat;
        Durability::<Ctx, SerializableFileTimes, SerializableError>::custom_wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "filesystem::types::descriptor::stat",
            |_ctx| {
                Box::pin(async move { Ok(stat_clone1) as Result<DescriptorStat, anyhow::Error> })
            },
            |_ctx, stat| {
                Ok(SerializableFileTimes {
                    data_access_timestamp: stat.data_access_timestamp.map(|t| t.into()),
                    data_modification_timestamp: stat.data_modification_timestamp.map(|t| t.into()),
                })
            },
            |_ctx, times| {
                Box::pin(async move {
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
                    spawn_blocking(|| set_symlink_times(path, accessed, modified)).await?;
                    stat.data_access_timestamp = times.data_access_timestamp.map(|t| t.into());
                    stat.data_modification_timestamp =
                        times.data_modification_timestamp.map(|t| t.into());
                    Ok(stat)
                })
            },
        )
        .await
        .map_err(FsError::trap)
    }

    async fn stat_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "stat_at");
        let full_path = match self.table().get(&self_)? {
            Descriptor::File(f) => f.path.join(path.clone()),
            Descriptor::Dir(d) => d.path.join(path.clone()),
        };

        let mut stat =
            HostDescriptor::stat_at(&mut self.as_wasi_view(), self_, path_flags, path).await?;
        stat.status_change_timestamp = None; // We cannot guarantee this to be the same during replays, so we rather not support it

        let stat_clone1 = stat;
        Durability::<Ctx, SerializableFileTimes, SerializableError>::custom_wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "filesystem::types::descriptor::stat_at",
            |_ctx| {
                Box::pin(async move { Ok(stat_clone1) as Result<DescriptorStat, anyhow::Error> })
            },
            |_ctx, stat| {
                Ok(SerializableFileTimes {
                    data_access_timestamp: stat.data_access_timestamp.map(|t| t.into()),
                    data_modification_timestamp: stat.data_modification_timestamp.map(|t| t.into()),
                })
            },
            |_ctx, times| {
                Box::pin(async move {
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
                    spawn_blocking(|| set_symlink_times(full_path, accessed, modified)).await?;
                    stat.data_access_timestamp = times.data_access_timestamp.map(|t| t.into());
                    stat.data_modification_timestamp =
                        times.data_modification_timestamp.map(|t| t.into());
                    Ok(stat)
                })
            },
        )
        .await
        .map_err(FsError::trap)
    }

    async fn set_times_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "set_times_at");
        HostDescriptor::set_times_at(
            &mut self.as_wasi_view(),
            self_,
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
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "link_at");
        HostDescriptor::link_at(
            &mut self.as_wasi_view(),
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
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "open_at");
        HostDescriptor::open_at(
            &mut self.as_wasi_view(),
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
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "readlink_at");
        HostDescriptor::readlink_at(&mut self.as_wasi_view(), self_, path).await
    }

    async fn remove_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "remove_directory_at");
        HostDescriptor::remove_directory_at(&mut self.as_wasi_view(), self_, path.clone()).await
    }

    async fn rename_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: String,
        new_descriptor: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "rename_at");
        HostDescriptor::rename_at(
            &mut self.as_wasi_view(),
            self_,
            old_path.clone(),
            new_descriptor,
            new_path.clone(),
        )
        .await
    }

    async fn symlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "symlink_at");
        HostDescriptor::symlink_at(&mut self.as_wasi_view(), self_, old_path, new_path.clone())
            .await
    }

    async fn unlink_file_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "unlink_file_at");
        HostDescriptor::unlink_file_at(&mut self.as_wasi_view(), self_, path.clone()).await
    }

    async fn is_same_object(
        &mut self,
        self_: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> anyhow::Result<bool> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "is_same_object");
        HostDescriptor::is_same_object(&mut self.as_wasi_view(), self_, other).await
    }

    async fn metadata_hash(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<MetadataHashValue, FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "metadata_hash");

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
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call("filesystem::types::descriptor", "metadata_hash_at");
        // Using the WASI stat_at function as it guarantees the file times are preserved
        let metadata = self.stat_at(self_, path_flags, path).await?;
        Ok(calculate_metadata_hash(&metadata))
    }

    fn drop(&mut self, rep: Resource<Descriptor>) -> anyhow::Result<()> {
        record_host_function_call("filesystem::types::descriptor", "drop");
        HostDescriptor::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDirectoryEntryStream for DurableWorkerCtx<Ctx> {
    async fn read_directory_entry(
        &mut self,
        self_: Resource<DirectoryEntryStream>,
    ) -> Result<Option<DirectoryEntry>, FsError> {
        let _permit = self
            .begin_async_host_function()
            .await
            .map_err(|err| FsError::trap(err))?;
        record_host_function_call(
            "filesystem::types::directory_entry_stream",
            "read_directory_entry",
        );
        HostDirectoryEntryStream::read_directory_entry(&mut self.as_wasi_view(), self_).await
    }

    fn drop(&mut self, rep: Resource<DirectoryEntryStream>) -> anyhow::Result<()> {
        record_host_function_call("filesystem::types::directory_entry_stream", "drop");
        HostDirectoryEntryStream::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    fn filesystem_error_code(&mut self, err: Resource<Error>) -> anyhow::Result<Option<ErrorCode>> {
        Host::filesystem_error_code(&mut self.as_wasi_view(), err)
    }

    fn convert_error_code(&mut self, err: FsError) -> anyhow::Result<ErrorCode> {
        Host::convert_error_code(&mut self.as_wasi_view(), err)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDescriptor for &mut DurableWorkerCtx<Ctx> {
    fn read_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<InputStream>, FsError> {
        (*self).read_via_stream(self_, offset)
    }

    fn write_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<OutputStream>, FsError> {
        (*self).write_via_stream(self_, offset)
    }

    fn append_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<OutputStream>, FsError> {
        (*self).append_via_stream(self_)
    }

    async fn advise(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
        length: Filesize,
        advice: Advice,
    ) -> Result<(), FsError> {
        (*self).advise(self_, offset, length, advice).await
    }

    async fn sync_data(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        (*self).sync_data(self_).await
    }

    async fn get_flags(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorFlags, FsError> {
        (*self).get_flags(self_).await
    }

    async fn get_type(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorType, FsError> {
        (*self).get_type(self_).await
    }

    async fn set_size(
        &mut self,
        self_: Resource<Descriptor>,
        size: Filesize,
    ) -> Result<(), FsError> {
        (*self).set_size(self_, size).await
    }

    async fn set_times(
        &mut self,
        self_: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        (*self)
            .set_times(self_, data_access_timestamp, data_modification_timestamp)
            .await
    }

    async fn read(
        &mut self,
        self_: Resource<Descriptor>,
        length: Filesize,
        offset: Filesize,
    ) -> Result<(Vec<u8>, bool), FsError> {
        (*self).read(self_, length, offset).await
    }

    async fn write(
        &mut self,
        self_: Resource<Descriptor>,
        buffer: Vec<u8>,
        offset: Filesize,
    ) -> Result<Filesize, FsError> {
        (*self).write(self_, buffer, offset).await
    }

    async fn read_directory(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<ReaddirIterator>, FsError> {
        (*self).read_directory(self_).await
    }

    async fn sync(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        (*self).sync(self_).await
    }

    async fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        (*self).create_directory_at(self_, path).await
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        (*self).stat(self_).await
    }

    async fn stat_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, FsError> {
        (*self).stat_at(self_, path_flags, path).await
    }

    async fn set_times_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        (*self)
            .set_times_at(
                self_,
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
        (*self)
            .link_at(self_, old_path_flags, old_path, new_descriptor, new_path)
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
        (*self)
            .open_at(self_, path_flags, path, open_flags, flags)
            .await
    }

    async fn readlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<String, FsError> {
        (*self).readlink_at(self_, path).await
    }

    async fn remove_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        (*self).remove_directory_at(self_, path).await
    }

    async fn rename_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: String,
        new_descriptor: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        (*self)
            .rename_at(self_, old_path, new_descriptor, new_path)
            .await
    }

    async fn symlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        (*self).symlink_at(self_, old_path, new_path).await
    }

    async fn unlink_file_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        (*self).unlink_file_at(self_, path).await
    }

    async fn is_same_object(
        &mut self,
        self_: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> anyhow::Result<bool> {
        (*self).is_same_object(self_, other).await
    }

    async fn metadata_hash(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<MetadataHashValue, FsError> {
        (*self).metadata_hash(self_).await
    }

    async fn metadata_hash_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, FsError> {
        (*self).metadata_hash_at(self_, path_flags, path).await
    }

    fn drop(&mut self, rep: Resource<Descriptor>) -> anyhow::Result<()> {
        HostDescriptor::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDirectoryEntryStream for &mut DurableWorkerCtx<Ctx> {
    async fn read_directory_entry(
        &mut self,
        self_: Resource<DirectoryEntryStream>,
    ) -> Result<Option<DirectoryEntry>, FsError> {
        (*self).read_directory_entry(self_).await
    }

    fn drop(&mut self, rep: Resource<DirectoryEntryStream>) -> anyhow::Result<()> {
        HostDirectoryEntryStream::drop(*self, rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for &mut DurableWorkerCtx<Ctx> {
    fn filesystem_error_code(&mut self, err: Resource<Error>) -> anyhow::Result<Option<ErrorCode>> {
        (*self).filesystem_error_code(err)
    }

    fn convert_error_code(&mut self, err: FsError) -> anyhow::Result<ErrorCode> {
        (*self).convert_error_code(err)
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
