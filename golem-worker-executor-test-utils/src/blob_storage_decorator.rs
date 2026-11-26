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

//! A blob storage that changes some operations of the storage it wraps.
//!
//! A test implements [`BlobStorageDecorator`] for its fault or observation state and overrides
//! only the operations it changes. [`DecoratedBlobStorage`] is the [`BlobStorageBackend`] that
//! runs the decorator over the wrapped storage.

#![allow(clippy::too_many_arguments)]

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use futures::{StreamExt, TryStreamExt};
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobMissingError, BlobRangeError, BlobRangeStream, BlobStorage,
    BlobStorageBackend, BlobStorageNamespace, ExistsResult, ListedBlob, NormalizedBlobPath,
    PutIfAbsent,
};
use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;

type Error = anyhow::Error;
type ByteStream = BoxStream<'static, Result<Bytes, Error>>;

/// The blob storage that the executor passes to `TestExecutorOverrides::wrap_blob_storage`. A
/// decorator of it names its `inner` storage with this type.
pub type WrappedBlobStorage = dyn BlobStorage;

/// The storage that a [`DecoratedBlobStorage`] wraps.
///
/// It is implemented for `dyn BlobStorage`, the storage that
/// `TestExecutorOverrides::wrap_blob_storage` gets, through its public operations, and for every
/// concrete [`BlobStorageBackend`], through its `_at` operations.
#[async_trait]
pub trait BlobStorageInner: Debug + Send + Sync {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error>;

    async fn get_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<ByteStream>, Error>;

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error>;

    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error>;

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error>;

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error>;

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error>;

    async fn put_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error>;

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error>;

    async fn delete_many_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[NormalizedBlobPath<'_>],
    ) -> Result<(), Error>;

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error>;

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error>;

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error>;

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<u64>, Error>;

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error>;

    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error>;
}

#[async_trait]
impl BlobStorageInner for dyn BlobStorage {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.get_raw(target_label, op_label, namespace, path).await
    }

    async fn get_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<ByteStream>, Error> {
        self.get_stream(target_label, op_label, namespace, path)
            .await
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        self.get_range_stream(target_label, op_label, namespace, path, offset, length)
            .await
    }

    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.get_raw_slice(target_label, op_label, namespace, path, start, end)
            .await
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error> {
        self.get_metadata(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error> {
        self.put_raw(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        self.put_raw_if_absent(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        self.put_stream(target_label, op_label, namespace, path, stream)
            .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        self.delete(target_label, op_label, namespace, path).await
    }

    async fn delete_many_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[NormalizedBlobPath<'_>],
    ) -> Result<(), Error> {
        let paths = paths
            .iter()
            .map(|path| path.to_path_buf())
            .collect::<Vec<_>>();
        self.delete_many(target_label, op_label, namespace, &paths)
            .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        self.create_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error> {
        self.list_dir(target_label, op_label, namespace, path).await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error> {
        self.list_blobs_below(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<u64>, Error> {
        self.delete_dir(target_label, op_label, namespace, path)
            .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error> {
        self.exists(target_label, op_label, namespace, path).await
    }

    /// Maps the [`BlobMissingError`] of `copy_between` back to the `false` of `copy_between_at`.
    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        match self
            .copy_between(
                target_label,
                op_label,
                from_namespace,
                from,
                to_namespace,
                to,
            )
            .await
        {
            Ok(()) => Ok(true),
            Err(error) if error.is::<BlobMissingError>() => Ok(false),
            Err(error) => Err(error),
        }
    }
}

#[async_trait]
impl<B: BlobStorageBackend> BlobStorageInner for B {
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error> {
        BlobStorageBackend::get_raw_at(self, target_label, op_label, namespace, path).await
    }

    async fn get_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<ByteStream>, Error> {
        BlobStorageBackend::get_stream_at(self, target_label, op_label, namespace, path).await
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        BlobStorageBackend::get_range_stream_at(
            self,
            target_label,
            op_label,
            namespace,
            path,
            offset,
            length,
        )
        .await
    }

    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        BlobStorageBackend::get_raw_slice_at(
            self,
            target_label,
            op_label,
            namespace,
            path,
            start,
            end,
        )
        .await
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error> {
        BlobStorageBackend::get_metadata_at(self, target_label, op_label, namespace, path).await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error> {
        BlobStorageBackend::put_raw_at(self, target_label, op_label, namespace, path, data).await
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        BlobStorageBackend::put_raw_if_absent_at(
            self,
            target_label,
            op_label,
            namespace,
            path,
            data,
        )
        .await
    }

    async fn put_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        BlobStorageBackend::put_stream_at(self, target_label, op_label, namespace, path, stream)
            .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        BlobStorageBackend::delete_at(self, target_label, op_label, namespace, path).await
    }

    async fn delete_many_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[NormalizedBlobPath<'_>],
    ) -> Result<(), Error> {
        BlobStorageBackend::delete_many_at(self, target_label, op_label, namespace, paths).await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        BlobStorageBackend::create_dir_at(self, target_label, op_label, namespace, path).await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error> {
        BlobStorageBackend::list_dir_at(self, target_label, op_label, namespace, path).await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error> {
        BlobStorageBackend::list_blobs_below_at(self, target_label, op_label, namespace, path).await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<u64>, Error> {
        BlobStorageBackend::delete_dir_at(self, target_label, op_label, namespace, path).await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error> {
        BlobStorageBackend::exists_at(self, target_label, op_label, namespace, path).await
    }

    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        BlobStorageBackend::copy_between_at(
            self,
            target_label,
            op_label,
            from_namespace,
            from,
            to_namespace,
            to,
        )
        .await
    }
}

/// The bytes from `start` to `end` of `blob`, both inclusive, as the default of
/// `BlobStorageBackend::get_raw_slice_at` gives them: a range with a byte that is not in the blob
/// is a [`BlobRangeError`]. A copy of the private `golem_service_base::storage::blob::blob_range`.
fn blob_range(blob: &[u8], start: u64, end: u64) -> Result<Vec<u8>, BlobRangeError> {
    usize::try_from(start)
        .ok()
        .zip(usize::try_from(end).ok())
        .filter(|&(first, last)| first <= last && last < blob.len())
        .map(|(first, last)| blob[first..=last].to_vec())
        .ok_or(BlobRangeError { start, end })
}

/// The operations of a [`DecoratedBlobStorage`]. Each one gets the wrapped storage as `inner`.
///
/// An operation that [`BlobStorageBackend`] requires calls the same operation of `inner` by
/// default. An operation that [`BlobStorageBackend`] defines from other operations
/// (`get_stream_at`, `get_raw_slice_at`, `put_stream_at` and `delete_many_at`) keeps that
/// definition by default, built from this decorator's operations, so it reaches the operations
/// the decorator changes. A decorator that sends one of them straight to `inner` overrides it.
/// `has_blob_at` also keeps its definition, from `exists_at`; `inner` has no such operation.
#[async_trait]
pub trait BlobStorageDecorator<I: BlobStorageInner + ?Sized = WrappedBlobStorage>:
    Debug + Send + Sync + 'static
{
    /// The value of `TestExecutorOverrides::wrap_blob_storage` that runs this decorator over the
    /// executor's blob storage.
    fn wrap_blob_storage(self: Arc<Self>) -> Arc<crate::WrapBlobStorageFn>
    where
        Self: Sized + BlobStorageDecorator<dyn BlobStorage>,
    {
        Arc::new(move |inner| Arc::new(DecoratedBlobStorage::new(self.clone(), inner)))
    }

    async fn get_raw_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error> {
        inner
            .get_raw_at(target_label, op_label, namespace, path)
            .await
    }

    /// By default, reads the blob with `get_raw_at` and gives it as one chunk.
    async fn get_stream_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<ByteStream>, Error> {
        Ok(self
            .get_raw_at(inner, target_label, op_label, namespace, path)
            .await?
            .map(|data| futures::stream::iter([Ok(Bytes::from(data))]).boxed()))
    }

    async fn get_range_stream_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        inner
            .get_range_stream_at(target_label, op_label, namespace, path, offset, length)
            .await
    }

    /// By default, reads the full blob with `get_raw_at` and gives the range of it.
    async fn get_raw_slice_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.get_raw_at(inner, target_label, op_label, namespace, path)
            .await?
            .map(|data| blob_range(&data, start, end).map_err(Error::from))
            .transpose()
    }

    async fn get_metadata_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error> {
        inner
            .get_metadata_at(target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error> {
        inner
            .put_raw_at(target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        inner
            .put_raw_if_absent_at(target_label, op_label, namespace, path, data)
            .await
    }

    /// By default, reads the full stream and writes it with `put_raw_at`.
    async fn put_stream_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        let data = stream
            .make_stream_erased()
            .await?
            .try_collect::<Vec<_>>()
            .await?
            .concat();
        self.put_raw_at(inner, target_label, op_label, namespace, path, &data)
            .await
    }

    async fn delete_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        inner
            .delete_at(target_label, op_label, namespace, path)
            .await
    }

    /// By default, removes the paths with `delete_at`, one after the other, and stops at the
    /// first error.
    async fn delete_many_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[NormalizedBlobPath<'_>],
    ) -> Result<(), Error> {
        for path in paths {
            self.delete_at(inner, target_label, op_label, namespace.clone(), path)
                .await?;
        }
        Ok(())
    }

    async fn create_dir_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        inner
            .create_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error> {
        inner
            .list_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error> {
        inner
            .list_blobs_below_at(target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<u64>, Error> {
        inner
            .delete_dir_at(target_label, op_label, namespace, path)
            .await
    }

    async fn exists_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error> {
        inner
            .exists_at(target_label, op_label, namespace, path)
            .await
    }

    /// By default, reads `exists_at`.
    async fn has_blob_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        Ok(self
            .exists_at(inner, target_label, op_label, namespace, path)
            .await?
            == ExistsResult::File)
    }

    async fn copy_between_at(
        &self,
        inner: &I,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        inner
            .copy_between_at(
                target_label,
                op_label,
                from_namespace,
                from,
                to_namespace,
                to,
            )
            .await
    }
}

/// A [`BlobStorageBackend`] that runs `decorator` over `inner`.
///
/// Published blobs keep the defaults of [`BlobStorageBackend`]: `put_published_file_if_absent_at`
/// refuses them.
#[derive(Debug)]
pub struct DecoratedBlobStorage<D, I: ?Sized = WrappedBlobStorage> {
    decorator: Arc<D>,
    inner: Arc<I>,
}

impl<D, I: ?Sized> DecoratedBlobStorage<D, I> {
    pub fn new(decorator: Arc<D>, inner: Arc<I>) -> Self {
        Self { decorator, inner }
    }
}

impl<D: Default, I: Default> Default for DecoratedBlobStorage<D, I> {
    fn default() -> Self {
        Self::new(Arc::default(), Arc::default())
    }
}

/// A test reads its decorator's state through the storage.
impl<D, I: ?Sized> std::ops::Deref for DecoratedBlobStorage<D, I> {
    type Target = D;

    fn deref(&self) -> &D {
        &self.decorator
    }
}

#[async_trait]
impl<D, I> BlobStorageBackend for DecoratedBlobStorage<D, I>
where
    D: BlobStorageDecorator<I>,
    I: BlobStorageInner + ?Sized + 'static,
{
    async fn get_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.decorator
            .get_raw_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn get_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<ByteStream>, Error> {
        self.decorator
            .get_stream_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn get_range_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        offset: u64,
        length: u64,
    ) -> Result<Option<BlobRangeStream>, Error> {
        self.decorator
            .get_range_stream_at(
                &self.inner,
                target_label,
                op_label,
                namespace,
                path,
                offset,
                length,
            )
            .await
    }

    async fn get_raw_slice_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        start: u64,
        end: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        self.decorator
            .get_raw_slice_at(
                &self.inner,
                target_label,
                op_label,
                namespace,
                path,
                start,
                end,
            )
            .await
    }

    async fn get_metadata_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<BlobMetadata>, Error> {
        self.decorator
            .get_metadata_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn put_raw_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<(), Error> {
        self.decorator
            .put_raw_at(&self.inner, target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_raw_if_absent_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        data: &[u8],
    ) -> Result<PutIfAbsent, Error> {
        self.decorator
            .put_raw_if_absent_at(&self.inner, target_label, op_label, namespace, path, data)
            .await
    }

    async fn put_stream_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
        stream: &dyn ErasedReplayableStream<Item = Result<Vec<u8>, Error>, Error = Error>,
    ) -> Result<(), Error> {
        self.decorator
            .put_stream_at(&self.inner, target_label, op_label, namespace, path, stream)
            .await
    }

    async fn delete_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        self.decorator
            .delete_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn delete_many_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[NormalizedBlobPath<'_>],
    ) -> Result<(), Error> {
        self.decorator
            .delete_many_at(&self.inner, target_label, op_label, namespace, paths)
            .await
    }

    async fn create_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<(), Error> {
        self.decorator
            .create_dir_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn list_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Vec<PathBuf>, Error> {
        self.decorator
            .list_dir_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn list_blobs_below_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Box<[ListedBlob]>, Error> {
        self.decorator
            .list_blobs_below_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn delete_dir_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<Option<u64>, Error> {
        self.decorator
            .delete_dir_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn exists_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<ExistsResult, Error> {
        self.decorator
            .exists_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn has_blob_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        self.decorator
            .has_blob_at(&self.inner, target_label, op_label, namespace, path)
            .await
    }

    async fn copy_between_at(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        from_namespace: BlobStorageNamespace,
        from: &NormalizedBlobPath<'_>,
        to_namespace: BlobStorageNamespace,
        to: &NormalizedBlobPath<'_>,
    ) -> Result<bool, Error> {
        self.decorator
            .copy_between_at(
                &self.inner,
                target_label,
                op_label,
                from_namespace,
                from,
                to_namespace,
                to,
            )
            .await
    }
}
