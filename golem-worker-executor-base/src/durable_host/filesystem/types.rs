// Copyright 2024-2025 Golem Cloud
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

use async_trait::async_trait;
use wasmtime::component::Resource;
use wasmtime_wasi::bindings::filesystem::types::{
    Advice, Descriptor, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry,
    DirectoryEntryStream, Error, ErrorCode, Filesize, Host, HostDescriptor,
    HostDirectoryEntryStream, InputStream, MetadataHashValue, NewTimestamp, OpenFlags,
    OutputStream, PathFlags,
};
use wasmtime_wasi::FsError;

use crate::durable_host::DurableWorkerCtx;
use crate::workerctx::WorkerCtx;

#[async_trait]
impl<Ctx: WorkerCtx> HostDescriptor for DurableWorkerCtx<Ctx> {
    fn read_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<InputStream>, FsError> {
        HostDescriptor::read_via_stream(&mut self.as_wasi_view(), self_, offset)
    }

    fn write_via_stream(
        &mut self,
        fd: Resource<Descriptor>,
        offset: Filesize,
    ) -> Result<Resource<OutputStream>, FsError> {
        self.fail_if_read_only(&fd)?;
        HostDescriptor::write_via_stream(&mut self.as_wasi_view(), fd, offset)
    }

    fn append_via_stream(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<OutputStream>, FsError> {
        HostDescriptor::append_via_stream(&mut self.as_wasi_view(), self_)
    }

    async fn advise(
        &mut self,
        self_: Resource<Descriptor>,
        offset: Filesize,
        length: Filesize,
        advice: Advice,
    ) -> Result<(), FsError> {
        HostDescriptor::advise(&mut self.as_wasi_view(), self_, offset, length, advice).await
    }

    async fn sync_data(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        HostDescriptor::sync_data(&mut self.as_wasi_view(), self_).await
    }

    async fn get_flags(&mut self, fd: Resource<Descriptor>) -> Result<DescriptorFlags, FsError> {
        let read_only = self.is_read_only(&fd)?;
        let wasi_view = &mut self.as_wasi_view();
        let mut descriptor_flags = HostDescriptor::get_flags(wasi_view, fd).await?;

        if read_only {
            descriptor_flags &= !DescriptorFlags::WRITE
        };

        Ok(descriptor_flags)
    }

    async fn get_type(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorType, FsError> {
        HostDescriptor::get_type(&mut self.as_wasi_view(), self_).await
    }

    async fn set_size(&mut self, fd: Resource<Descriptor>, size: Filesize) -> Result<(), FsError> {
        self.fail_if_read_only(&fd)?;
        HostDescriptor::set_size(&mut self.as_wasi_view(), fd, size).await
    }

    async fn set_times(
        &mut self,
        fd: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
        self.fail_if_read_only(&fd)?;
        HostDescriptor::set_times(
            &mut self.as_wasi_view(),
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
        HostDescriptor::read(&mut self.as_wasi_view(), self_, length, offset).await
    }

    async fn write(
        &mut self,
        fd: Resource<Descriptor>,
        buffer: Vec<u8>,
        offset: Filesize,
    ) -> Result<Filesize, FsError> {
        self.fail_if_read_only(&fd)?;
        HostDescriptor::write(&mut self.as_wasi_view(), fd, buffer, offset).await
    }

    async fn read_directory(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<DirectoryEntryStream>, FsError> {
        HostDescriptor::read_directory(&mut self.as_wasi_view(), self_).await
    }

    async fn sync(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        HostDescriptor::sync(&mut self.as_wasi_view(), self_).await
    }

    async fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        HostDescriptor::create_directory_at(&mut self.as_wasi_view(), self_, path).await
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        HostDescriptor::stat(&mut self.as_wasi_view(), self_).await
    }

    async fn stat_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, FsError> {
        HostDescriptor::stat_at(&mut self.as_wasi_view(), self_, path_flags, path).await
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
        HostDescriptor::set_times_at(
            &mut self.as_wasi_view(),
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
        HostDescriptor::readlink_at(&mut self.as_wasi_view(), self_, path).await
    }

    async fn remove_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        HostDescriptor::remove_directory_at(&mut self.as_wasi_view(), self_, path.clone()).await
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
        HostDescriptor::rename_at(
            &mut self.as_wasi_view(),
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
        HostDescriptor::symlink_at(&mut self.as_wasi_view(), fd, old_path, new_path.clone()).await
    }

    async fn unlink_file_at(
        &mut self,
        fd: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        self.fail_if_read_only(&fd)?;
        HostDescriptor::unlink_file_at(&mut self.as_wasi_view(), fd, path.clone()).await
    }

    async fn is_same_object(
        &mut self,
        self_: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> anyhow::Result<bool> {
        HostDescriptor::is_same_object(&mut self.as_wasi_view(), self_, other).await
    }

    async fn metadata_hash(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<MetadataHashValue, FsError> {
        HostDescriptor::metadata_hash(&mut self.as_wasi_view(), self_).await
    }

    async fn metadata_hash_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, FsError> {
        HostDescriptor::metadata_hash_at(&mut self.as_wasi_view(), self_, path_flags, path).await
    }

    fn drop(&mut self, rep: Resource<Descriptor>) -> anyhow::Result<()> {
        HostDescriptor::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> HostDirectoryEntryStream for DurableWorkerCtx<Ctx> {
    async fn read_directory_entry(
        &mut self,
        self_: Resource<DirectoryEntryStream>,
    ) -> Result<Option<DirectoryEntry>, FsError> {
        HostDirectoryEntryStream::read_directory_entry(&mut self.as_wasi_view(), self_).await
    }

    fn drop(&mut self, rep: Resource<DirectoryEntryStream>) -> anyhow::Result<()> {
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
