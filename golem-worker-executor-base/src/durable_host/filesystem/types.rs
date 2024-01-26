use async_trait::async_trait;
use bincode::{Decode, Encode};
use cap_std::fs::{Dir, File, Metadata};
use fs_set_times::{SetTimes, SystemTimeSpec};
use serde::{Deserialize, Serialize};
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;
use tracing::info;
use wasmtime::component::Resource;

use crate::durable_host::{Durability, DurableWorkerCtx, SerializableDateTime, SerializableError};
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
        HostDescriptor::read(&mut self.as_wasi_view(), self_, length, offset).await
    }

    async fn write(
        &mut self,
        self_: Resource<Descriptor>,
        buffer: Vec<u8>,
        offset: Filesize,
    ) -> Result<Filesize, FsError> {
        record_host_function_call("filesystem::types::descriptor", "write");
        let f = Descriptor::file(self.table.get(&self_)?)?;
        let f = f.file.clone();

        let result = HostDescriptor::write(&mut self.as_wasi_view(), self_, buffer, offset).await?;
        self.durable_file_times(f, "filesystem::types::descriptor::write")
            .await?;
        Ok(result)
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
        let f = Descriptor::dir(self.table.get(&self_)?)?;
        let f = f.dir.clone();

        let result =
            HostDescriptor::create_directory_at(&mut self.as_wasi_view(), self_, path).await?;
        self.durable_file_times(f, "filesystem::types::descriptor::create_directory_at")
            .await?;
        Ok(result)
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
        info!("stat_at: path={}, flags={:?}", path, path_flags);
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

        let dir = Descriptor::dir(self.table.get(&self_)?)?;
        let dir = dir.dir.clone();

        let result = HostDescriptor::link_at(
            &mut self.as_wasi_view(),
            self_,
            old_path_flags,
            old_path,
            new_descriptor,
            new_path.clone(),
        )
        .await?;

        let f = Arc::new(dir.open(new_path.clone())?);
        self.durable_file_times(f, "filesystem::types::descriptor::link_at/file")
            .await?;

        let target_path: PathBuf = new_path.into();
        let parent = match target_path.parent() {
            Some(p) => Arc::new(dir.open_dir(p)?),
            None => dir,
        };

        self.durable_file_times(parent, "filesystem::types::descriptor::link_at/dir")
            .await?;

        Ok(result)
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
        let dir = Descriptor::dir(self.table.get(&self_)?)?;
        let dir = dir.dir.clone();

        let result =
            HostDescriptor::remove_directory_at(&mut self.as_wasi_view(), self_, path.clone())
                .await;

        let target_path: PathBuf = path.into();
        let parent = match target_path.parent() {
            Some(p) => Arc::new(dir.open_dir(p)?),
            None => dir,
        };

        self.durable_file_times(parent, "filesystem::types::descriptor::remove_directory_at")
            .await?;
        result
    }

    async fn rename_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: String,
        new_descriptor: Resource<Descriptor>,
        new_path: String,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "rename_at");

        let source_dir = Descriptor::dir(self.table.get(&self_)?)?;
        let source_dir = source_dir.dir.clone();
        let target_dir = Descriptor::dir(self.table.get(&self_)?)?;
        let target_dir = target_dir.dir.clone();

        let result = HostDescriptor::rename_at(
            &mut self.as_wasi_view(),
            self_,
            old_path.clone(),
            new_descriptor,
            new_path.clone(),
        )
        .await;

        let target_f = Arc::new(target_dir.open(new_path.clone())?);
        self.durable_file_times(
            target_f,
            "filesystem::types::descriptor::rename_at/target-file",
        )
        .await?;

        let source_path: PathBuf = old_path.into();
        let source_parent = match source_path.parent() {
            Some(p) => Arc::new(source_dir.open_dir(p)?),
            None => source_dir,
        };

        let target_path: PathBuf = new_path.into();
        let target_parent = match target_path.parent() {
            Some(p) => Arc::new(target_dir.open_dir(p)?),
            None => target_dir,
        };

        self.durable_file_times(
            source_parent,
            "filesystem::types::descriptor::rename_at/source-dir",
        )
        .await?;
        self.durable_file_times(
            target_parent,
            "filesystem::types::descriptor::rename_at/target-dir",
        )
        .await?;

        result
    }

    async fn symlink_at(
        &mut self,
        self_: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "symlink_at");

        let dir = Descriptor::dir(self.table.get(&self_)?)?;
        let dir = dir.dir.clone();

        let result =
            HostDescriptor::symlink_at(&mut self.as_wasi_view(), self_, old_path, new_path.clone())
                .await;

        let f = Arc::new(dir.open(new_path.clone())?);
        self.durable_file_times(f, "filesystem::types::descriptor::symlink_at/file")
            .await?;

        let target_path: PathBuf = new_path.into();
        let parent = match target_path.parent() {
            Some(p) => Arc::new(dir.open_dir(p)?),
            None => dir,
        };

        self.durable_file_times(parent, "filesystem::types::descriptor::symlink_at/dir")
            .await?;

        result
    }

    async fn unlink_file_at(
        &mut self,
        self_: Resource<Descriptor>,
        path: String,
    ) -> Result<(), FsError> {
        record_host_function_call("filesystem::types::descriptor", "unlink_file_at");

        let dir = Descriptor::dir(self.table.get(&self_)?)?;
        let dir = dir.dir.clone();

        let result =
            HostDescriptor::unlink_file_at(&mut self.as_wasi_view(), self_, path.clone()).await;

        let target_path: PathBuf = path.into();
        let parent = match target_path.parent() {
            Some(p) => Arc::new(dir.open_dir(p)?),
            None => dir,
        };
        self.durable_file_times(parent, "filesystem::types::descriptor::unlink_file_at")
            .await?;

        result
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

trait FileTimeSupport {
    fn metadata(&self) -> std::io::Result<Metadata>;
    fn set_times(
        &self,
        accessed: Option<SystemTimeSpec>,
        modified: Option<SystemTimeSpec>,
    ) -> std::io::Result<()>;
}

impl FileTimeSupport for File {
    fn metadata(&self) -> std::io::Result<Metadata> {
        self.metadata()
    }

    fn set_times(
        &self,
        accessed: Option<SystemTimeSpec>,
        modified: Option<SystemTimeSpec>,
    ) -> std::io::Result<()> {
        SetTimes::set_times(self, accessed, modified)
    }
}

impl FileTimeSupport for Dir {
    fn metadata(&self) -> std::io::Result<Metadata> {
        self.dir_metadata()
    }

    fn set_times(
        &self,
        accessed: Option<SystemTimeSpec>,
        modified: Option<SystemTimeSpec>,
    ) -> std::io::Result<()> {
        SetTimes::set_times(self, accessed, modified)
    }
}

impl<Ctx: WorkerCtx> DurableWorkerCtx<Ctx> {
    async fn durable_file_times<Entry: FileTimeSupport + Debug + Send + Sync + 'static>(
        &mut self,
        f: Arc<Entry>,
        function_name: &str,
    ) -> Result<(), FsError> {
        let f_clone = f.clone();
        let times = Durability::<Ctx, SerializableFileTimes, SerializableError>::wrap(
            self,
            WrappedFunctionType::WriteLocal,
            function_name,
            |_ctx| {
                Box::pin(async move {
                    let metadata = f_clone.metadata()?;
                    let accessed = metadata
                        .accessed()?
                        .into_std()
                        .duration_since(SystemTime::UNIX_EPOCH)?;
                    let modified = metadata
                        .modified()?
                        .into_std()
                        .duration_since(SystemTime::UNIX_EPOCH)?;
                    Ok(SerializableFileTimes {
                        data_access_timestamp: SerializableDateTime {
                            seconds: accessed.as_secs(),
                            nanoseconds: accessed.subsec_nanos(),
                        },
                        data_modification_timestamp: SerializableDateTime {
                            seconds: modified.as_secs(),
                            nanoseconds: modified.subsec_nanos(),
                        },
                    })
                })
            },
        )
        .await
        .map_err(|err| FsError::trap(err))?;
        if self.is_replay() {
            let accessed = times.data_access_timestamp.into();
            let modified = times.data_modification_timestamp.into();
            info!(
                "Setting file times for {f:?} to {:?} and {:?}",
                accessed, modified
            );
            f.set_times(
                Some(SystemTimeSpec::Absolute(accessed)),
                Some(SystemTimeSpec::Absolute(modified)),
            )?;

            // debug:
            let metadata = f.metadata()?;
            let accessed2 = metadata
                .accessed()?
                .into_std()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap();
            let modified2 = metadata
                .modified()?
                .into_std()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap();

            assert_eq!(
                accessed.duration_since(SystemTime::UNIX_EPOCH).unwrap(),
                accessed2
            );
            assert_eq!(
                modified.duration_since(SystemTime::UNIX_EPOCH).unwrap(),
                modified2
            );
            // end debug
        }
        Ok(())
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

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
struct SerializableFileTimes {
    pub data_access_timestamp: SerializableDateTime,
    pub data_modification_timestamp: SerializableDateTime,
}
