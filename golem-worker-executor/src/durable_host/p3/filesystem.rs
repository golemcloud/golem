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

use crate::durable_host::p3::{DurableP3, DurableP3View, wasi_filesystem_view};
use crate::workerctx::WorkerCtx;
use wasmtime::AsContextMut;
use wasmtime::component::{Access, Accessor, FutureReader, Resource, StreamReader};
use wasmtime_wasi::filesystem::{Descriptor, WasiFilesystem, WasiFilesystemView};
use wasmtime_wasi::p3::bindings::filesystem::{preopens, types};
use wasmtime_wasi::p3::filesystem::{FilesystemError, FilesystemResult};

impl<Ctx: WorkerCtx> types::Host for DurableP3View<'_, Ctx> {
    fn convert_error_code(&mut self, error: FilesystemError) -> wasmtime::Result<types::ErrorCode> {
        types::Host::convert_error_code(&mut WasiFilesystemView::filesystem(self.0), error)
    }
}

impl<Ctx: WorkerCtx> types::HostDescriptor for DurableP3View<'_, Ctx> {
    fn drop(&mut self, fd: Resource<Descriptor>) -> wasmtime::Result<()> {
        types::HostDescriptor::drop(&mut WasiFilesystemView::filesystem(self.0), fd)
    }
}

impl<Ctx: WorkerCtx> preopens::Host for DurableP3View<'_, Ctx> {
    fn get_directories(&mut self) -> wasmtime::Result<Vec<(Resource<Descriptor>, String)>> {
        preopens::Host::get_directories(&mut WasiFilesystemView::filesystem(self.0))
    }
}

impl<Ctx: WorkerCtx> types::HostDescriptorWithStore for DurableP3<Ctx> {
    fn read_via_stream<U>(
        mut store: Access<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
    ) -> wasmtime::Result<(StreamReader<u8>, FutureReader<Result<(), types::ErrorCode>>)> {
        let store = Access::<U, WasiFilesystem>::new(
            store.as_context_mut(),
            wasi_filesystem_view::<Ctx, U>,
        );
        <WasiFilesystem as types::HostDescriptorWithStore>::read_via_stream(store, fd, offset)
    }

    fn write_via_stream<U>(
        mut store: Access<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
        offset: types::Filesize,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let store = Access::<U, WasiFilesystem>::new(
            store.as_context_mut(),
            wasi_filesystem_view::<Ctx, U>,
        );
        <WasiFilesystem as types::HostDescriptorWithStore>::write_via_stream(
            store, fd, data, offset,
        )
    }

    fn append_via_stream<U>(
        mut store: Access<U, Self>,
        fd: Resource<Descriptor>,
        data: StreamReader<u8>,
    ) -> wasmtime::Result<FutureReader<Result<(), types::ErrorCode>>> {
        let store = Access::<U, WasiFilesystem>::new(
            store.as_context_mut(),
            wasi_filesystem_view::<Ctx, U>,
        );
        <WasiFilesystem as types::HostDescriptorWithStore>::append_via_stream(store, fd, data)
    }

    async fn advise<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        offset: types::Filesize,
        length: types::Filesize,
        advice: types::Advice,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::advise(
            &store, fd, offset, length, advice,
        )
        .await
    }

    async fn sync_data<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::sync_data(&store, fd).await
    }

    async fn get_flags<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorFlags> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::get_flags(&store, fd).await
    }

    async fn get_type<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorType> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::get_type(&store, fd).await
    }

    async fn set_size<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        size: types::Filesize,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::set_size(&store, fd, size).await
    }

    async fn set_times<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::set_times(
            &store,
            fd,
            data_access_timestamp,
            data_modification_timestamp,
        )
        .await
    }

    fn read_directory<U>(
        mut store: Access<U, Self>,
        fd: Resource<Descriptor>,
    ) -> wasmtime::Result<(
        StreamReader<types::DirectoryEntry>,
        FutureReader<Result<(), types::ErrorCode>>,
    )> {
        let store = Access::<U, WasiFilesystem>::new(
            store.as_context_mut(),
            wasi_filesystem_view::<Ctx, U>,
        );
        <WasiFilesystem as types::HostDescriptorWithStore>::read_directory(store, fd)
    }

    async fn sync<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::sync(&store, fd).await
    }

    async fn create_directory_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::create_directory_at(&store, fd, path)
            .await
    }

    async fn stat<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::DescriptorStat> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::stat(&store, fd).await
    }

    async fn stat_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::DescriptorStat> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::stat_at(&store, fd, path_flags, path)
            .await
    }

    async fn set_times_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        data_access_timestamp: types::NewTimestamp,
        data_modification_timestamp: types::NewTimestamp,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::set_times_at(
            &store,
            fd,
            path_flags,
            path,
            data_access_timestamp,
            data_modification_timestamp,
        )
        .await
    }

    async fn link_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path_flags: types::PathFlags,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::link_at(
            &store,
            fd,
            old_path_flags,
            old_path,
            new_fd,
            new_path,
        )
        .await
    }

    async fn open_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
        open_flags: types::OpenFlags,
        flags: types::DescriptorFlags,
    ) -> FilesystemResult<Resource<Descriptor>> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::open_at(
            &store, fd, path_flags, path, open_flags, flags,
        )
        .await
    }

    async fn readlink_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<String> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::readlink_at(&store, fd, path).await
    }

    async fn remove_directory_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::remove_directory_at(&store, fd, path)
            .await
    }

    async fn rename_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_fd: Resource<Descriptor>,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::rename_at(
            &store, fd, old_path, new_fd, new_path,
        )
        .await
    }

    async fn symlink_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        old_path: String,
        new_path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::symlink_at(
            &store, fd, old_path, new_path,
        )
        .await
    }

    async fn unlink_file_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path: String,
    ) -> FilesystemResult<()> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::unlink_file_at(&store, fd, path).await
    }

    async fn is_same_object<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        other: Resource<Descriptor>,
    ) -> wasmtime::Result<bool> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::is_same_object(&store, fd, other).await
    }

    async fn metadata_hash<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
    ) -> FilesystemResult<types::MetadataHashValue> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::metadata_hash(&store, fd).await
    }

    async fn metadata_hash_at<U: Send>(
        store: &Accessor<U, Self>,
        fd: Resource<Descriptor>,
        path_flags: types::PathFlags,
        path: String,
    ) -> FilesystemResult<types::MetadataHashValue> {
        let store = store.with_getter::<WasiFilesystem>(wasi_filesystem_view::<Ctx, U>);
        <WasiFilesystem as types::HostDescriptorWithStore>::metadata_hash_at(
            &store, fd, path_flags, path,
        )
        .await
    }
}
