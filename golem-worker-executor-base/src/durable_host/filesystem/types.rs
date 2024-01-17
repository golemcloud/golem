use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::{Durability, DurableWorkerCtx, SerializableError};
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::WrappedFunctionType;
use wasmtime_wasi::preview2::bindings::filesystem::types::{
    Advice, Descriptor, DescriptorFlags, DescriptorStat, DescriptorType, DirectoryEntry,
    DirectoryEntryStream, Error, ErrorCode, Filesize, Host, HostDescriptor,
    HostDirectoryEntryStream, InputStream, MetadataHashValue, NewTimestamp, OpenFlags,
    OutputStream, PathFlags,
};
use wasmtime_wasi::preview2::FsError;

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
        record_host_function_call("filesystem::types::descriptor", "advise");
        HostDescriptor::advise(&mut self.as_wasi_view(), self_, offset, length, advice).await
    }

    async fn sync_data(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "sync_data");
        HostDescriptor::sync_data(&mut self.as_wasi_view(), self_).await
    }

    async fn get_flags(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorFlags, FsError> {
        record_host_function_call("filesystem::types::descriptor", "get_flags");
        HostDescriptor::get_flags(&mut self.as_wasi_view(), self_).await
    }

    async fn get_type(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorType, FsError> {
        record_host_function_call("filesystem::types::descriptor", "get_type");
        HostDescriptor::get_type(&mut self.as_wasi_view(), self_).await
    }

    async fn set_size(
        &mut self,
        self_: Resource<Descriptor>,
        size: Filesize,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "set_size");
        HostDescriptor::set_size(&mut self.as_wasi_view(), self_, size).await
    }

    async fn set_times(
        &mut self,
        self_: Resource<Descriptor>,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
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
        record_host_function_call("filesystem::types::descriptor", "read");
        Durability::<Ctx, (Vec<u8>, bool), SerializableError>::wrap(
            self,
            WrappedFunctionType::ReadLocal,
            "filesystem::read",
            |ctx| {
                Box::pin(async move {
                    HostDescriptor::read(&mut ctx.as_wasi_view(), self_, length, offset).await
                })
            },
        )
        .await
    }

    async fn write(
        &mut self,
        self_: Resource<Descriptor>,
        buffer: Vec<u8>,
        offset: Filesize,
    ) -> Result<Filesize, FsError> {
        record_host_function_call("filesystem::types::descriptor", "write");
        HostDescriptor::write(&mut self.as_wasi_view(), self_, buffer, offset).await
    }

    async fn read_directory(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<Resource<DirectoryEntryStream>, FsError> {
        record_host_function_call("filesystem::types::descriptor", "read_directory");
        HostDescriptor::read_directory(&mut self.as_wasi_view(), self_).await
    }

    async fn sync(&mut self, self_: Resource<Descriptor>) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "sync");
        HostDescriptor::sync(&mut self.as_wasi_view(), self_).await
    }

    async fn create_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "create_directory_at");
        HostDescriptor::create_directory_at(&mut self.as_wasi_view(), self_, path).await
    }

    async fn stat(&mut self, self_: Resource<Descriptor>) -> Result<DescriptorStat, FsError> {
        record_host_function_call("filesystem::types::descriptor", "stat");
        HostDescriptor::stat(&mut self.as_wasi_view(), self_).await
    }

    async fn stat_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<DescriptorStat, FsError> {
        record_host_function_call("filesystem::types::descriptor", "stat_at");
        HostDescriptor::stat_at(&mut self.as_wasi_view(), self_, path_flags, path).await
    }

    async fn set_times_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
        data_access_timestamp: NewTimestamp,
        data_modification_timestamp: NewTimestamp,
    ) -> Result<(), FsError> {
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
        record_host_function_call("filesystem::types::descriptor", "link_at");
        HostDescriptor::link_at(
            &mut self.as_wasi_view(),
            self_,
            old_path_flags,
            old_path,
            new_descriptor,
            new_path,
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
        record_host_function_call("filesystem::types::descriptor", "readlink_at");
        HostDescriptor::readlink_at(&mut self.as_wasi_view(), self_, path).await
    }

    async fn remove_directory_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "remove_directory_at");
        HostDescriptor::remove_directory_at(&mut self.as_wasi_view(), self_, path).await
    }

    async fn rename_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: String,
        new_descriptor: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "rename_at");
        HostDescriptor::rename_at(
            &mut self.as_wasi_view(),
            self_,
            old_path,
            new_descriptor,
            new_path,
        )
        .await
    }

    async fn symlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "symlink_at");
        HostDescriptor::symlink_at(&mut self.as_wasi_view(), self_, old_path, new_path).await
    }

    async fn unlink_file_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "unlink_file_at");
        HostDescriptor::unlink_file_at(&mut self.as_wasi_view(), self_, path).await
    }

    async fn is_same_object(
        &mut self,
        self_: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> anyhow::Result<bool> {
        record_host_function_call("filesystem::types::descriptor", "is_same_object");
        HostDescriptor::is_same_object(&mut self.as_wasi_view(), self_, other).await
    }

    async fn metadata_hash(
        &mut self,
        self_: Resource<Descriptor>,
    ) -> Result<MetadataHashValue, FsError> {
        record_host_function_call("filesystem::types::descriptor", "metadata_hash");
        HostDescriptor::metadata_hash(&mut self.as_wasi_view(), self_).await
    }

    async fn metadata_hash_at(
        &mut self,
        self_: Resource<Descriptor>,
        path_flags: PathFlags,
        path: String,
    ) -> Result<MetadataHashValue, FsError> {
        record_host_function_call("filesystem::types::descriptor", "metadata_hash_at");
        HostDescriptor::metadata_hash_at(&mut self.as_wasi_view(), self_, path_flags, path).await
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
