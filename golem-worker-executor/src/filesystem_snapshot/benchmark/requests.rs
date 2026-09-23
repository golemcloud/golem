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

//! A blob storage that records each request that it passes to another blob storage.

use super::agents::AGENTS;
use super::report::{RequestSummary, RequestTimes, millis};
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, PutIfAbsent,
};
use std::future::Future;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// One request that the storage passed on.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct RequestRecord {
    pub(super) call: &'static str,
    pub(super) file_type: &'static str,
    /// The bytes that the request wrote or read.
    pub(super) bytes: u64,
    pub(super) written: bool,
    pub(super) time: Duration,
    pub(super) ok: bool,
}

/// A blob storage that passes each call to `inner` and records it.
///
/// A record holds the name of the call, the file type of the path, the bytes that the call wrote
/// or read, and the time from the start of the call to its end. The time includes the retries of
/// the inner storage. Each method of the trait is passed on, also a method with a default, so the
/// inner storage does each call in its own way.
#[derive(Debug)]
pub(super) struct MeasuredBlobStorage {
    inner: Arc<dyn BlobStorage>,
    records: Mutex<Vec<RequestRecord>>,
}

impl MeasuredBlobStorage {
    pub(super) fn new(inner: Arc<dyn BlobStorage>) -> Self {
        Self {
            inner,
            records: Mutex::new(Vec::new()),
        }
    }

    /// Gives the records since the last call, and removes them.
    pub(super) fn take(&self) -> Box<[RequestRecord]> {
        std::mem::take(&mut *self.records.lock().unwrap_or_else(PoisonError::into_inner))
            .into_boxed_slice()
    }

    async fn record<T>(
        &self,
        call: &'static str,
        path: &Path,
        bytes: impl FnOnce(&T) -> (u64, bool),
        future: impl Future<Output = anyhow::Result<T>>,
    ) -> anyhow::Result<T> {
        let started = Instant::now();
        let result = future.await;
        let time = started.elapsed();
        let (bytes, written) = result.as_ref().map_or((0, false), bytes);
        self.records
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(RequestRecord {
                call,
                file_type: file_type(path),
                bytes,
                written,
                time,
                ok: result.is_ok(),
            });
        result
    }
}

/// Gives the file type of a repository path from its first name: `config`, `pack`, `index`,
/// `snapshot`, `key`, or `other`. The path of the repository of an agent starts with
/// `agents/<agent>` (see [`super::agents`]), and the first name after it gives the type.
pub(super) fn file_type(path: &Path) -> &'static str {
    let names = path
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_str()),
            _ => None,
        })
        .collect::<Box<[_]>>();
    let in_repository = match &*names {
        [Some(AGENTS), Some(_), rest @ ..] => rest,
        all => all,
    };
    match in_repository.first() {
        Some(Some("config")) => "config",
        Some(Some("data")) => "pack",
        Some(Some("index")) => "index",
        Some(Some("snapshots")) => "snapshot",
        Some(Some("keys")) => "key",
        _ => "other",
    }
}

fn nothing<T>(_: &T) -> (u64, bool) {
    (0, false)
}

fn read_bytes(data: &Option<Vec<u8>>) -> (u64, bool) {
    (data.as_ref().map_or(0, |data| data.len() as u64), false)
}

/// Gives a summary of the records for each call and file type, in the order of the call and the
/// file type.
pub(super) fn summarize(records: &[RequestRecord]) -> Box<[RequestSummary]> {
    let mut sorted = records.to_vec();
    sorted.sort_by(|left, right| {
        (left.call, left.file_type, left.time).cmp(&(right.call, right.file_type, right.time))
    });
    sorted
        .chunk_by(|left, right| (left.call, left.file_type) == (right.call, right.file_type))
        .filter_map(|group| {
            group.first().map(|first| RequestSummary {
                call: first.call,
                file_type: first.file_type,
                count: group.len() as u64,
                errors: group.iter().filter(|record| !record.ok).count() as u64,
                bytes: group.iter().map(|record| record.bytes).sum(),
                time_ms: times(&group.iter().map(|record| record.time).collect::<Box<[_]>>()),
            })
        })
        .collect()
}

/// Gives the minimum, the percentiles by the nearest rank, the maximum and the total of times
/// that are in ascending order. Each value of an empty list is zero.
pub(super) fn times(sorted: &[Duration]) -> RequestTimes {
    let rank = |percent: usize| {
        let index = (percent * sorted.len()).div_ceil(100).max(1) - 1;
        sorted.get(index).copied().map(millis).unwrap_or_default()
    };
    RequestTimes {
        min: sorted.first().copied().map(millis).unwrap_or_default(),
        p50: rank(50),
        p90: rank(90),
        p99: rank(99),
        max: sorted.last().copied().map(millis).unwrap_or_default(),
        total: millis(sorted.iter().sum()),
    }
}

/// Gives the bytes that the records wrote and the bytes that they read.
pub(super) fn written_and_read(records: &[RequestRecord]) -> (u64, u64) {
    records.iter().fold((0, 0), |(written, read), record| {
        if record.written {
            (written + record.bytes, read)
        } else {
            (written, read + record.bytes)
        }
    })
}

#[async_trait]
impl BlobStorage for MeasuredBlobStorage {
    async fn get_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.record(
            "get_raw",
            path,
            read_bytes,
            self.inner.get_raw(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn get_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BoxStream<'static, anyhow::Result<Bytes>>>> {
        self.record(
            "get_stream",
            path,
            nothing,
            self.inner
                .get_stream(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn get_raw_slice(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        start: u64,
        end: u64,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        self.record(
            "get_raw_slice",
            path,
            read_bytes,
            self.inner
                .get_raw_slice(target_label, op_label, namespace, path, start, end),
        )
        .await
    }

    async fn get_metadata(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        self.record(
            "get_metadata",
            path,
            nothing,
            self.inner
                .get_metadata(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn put_raw(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<()> {
        let length = data.len() as u64;
        self.record(
            "put_raw",
            path,
            |_| (length, true),
            self.inner
                .put_raw(target_label, op_label, namespace, path, data),
        )
        .await
    }

    async fn put_raw_if_absent(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        let length = data.len() as u64;
        self.record(
            "put_raw_if_absent",
            path,
            |outcome| match outcome {
                PutIfAbsent::Written => (length, true),
                PutIfAbsent::AlreadyExists => (0, true),
            },
            self.inner
                .put_raw_if_absent(target_label, op_label, namespace, path, data),
        )
        .await
    }

    async fn put_stream(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
        stream: &dyn ErasedReplayableStream<Item = anyhow::Result<Vec<u8>>, Error = anyhow::Error>,
    ) -> anyhow::Result<()> {
        self.record(
            "put_stream",
            path,
            nothing,
            self.inner
                .put_stream(target_label, op_label, namespace, path, stream),
        )
        .await
    }

    async fn delete(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.record(
            "delete",
            path,
            nothing,
            self.inner.delete(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn delete_many(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        paths: &[PathBuf],
    ) -> anyhow::Result<()> {
        self.record(
            "delete_many",
            paths.first().map_or(Path::new(""), PathBuf::as_path),
            nothing,
            self.inner
                .delete_many(target_label, op_label, namespace, paths),
        )
        .await
    }

    async fn create_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<()> {
        self.record(
            "create_dir",
            path,
            nothing,
            self.inner
                .create_dir(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn list_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        self.record(
            "list_dir",
            path,
            nothing,
            self.inner.list_dir(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn list_blobs_below(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        self.record(
            "list_blobs_below",
            path,
            nothing,
            self.inner
                .list_blobs_below(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn delete_dir(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<bool> {
        self.record(
            "delete_dir",
            path,
            nothing,
            self.inner
                .delete_dir(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn exists(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        path: &Path,
    ) -> anyhow::Result<ExistsResult> {
        self.record(
            "exists",
            path,
            nothing,
            self.inner.exists(target_label, op_label, namespace, path),
        )
        .await
    }

    async fn copy(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> anyhow::Result<()> {
        self.record(
            "copy",
            from,
            nothing,
            self.inner.copy(target_label, op_label, namespace, from, to),
        )
        .await
    }

    async fn r#move(
        &self,
        target_label: &'static str,
        op_label: &'static str,
        namespace: BlobStorageNamespace,
        from: &Path,
        to: &Path,
    ) -> anyhow::Result<()> {
        self.record(
            "move",
            from,
            nothing,
            self.inner
                .r#move(target_label, op_label, namespace, from, to),
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::{
        MeasuredBlobStorage, RequestRecord, file_type, summarize, times, written_and_read,
    };
    use crate::filesystem_snapshot::benchmark::report::RequestTimes;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::test;
    use uuid::Uuid;

    fn record(call: &'static str, file_type: &'static str, millis: u64) -> RequestRecord {
        RequestRecord {
            call,
            file_type,
            bytes: 10,
            written: call == "put_raw",
            time: Duration::from_millis(millis),
            ok: millis != 0,
        }
    }

    #[test]
    fn the_times_are_the_nearest_ranks_of_the_sorted_times() {
        let hundred = (1..=100).map(Duration::from_millis).collect::<Vec<_>>();
        let three = [1, 2, 3].map(Duration::from_millis);

        assert_eq!(
            (
                times(&hundred),
                times(&three),
                times(&[Duration::from_millis(7)])
            ),
            (
                RequestTimes {
                    min: 1.0,
                    p50: 50.0,
                    p90: 90.0,
                    p99: 99.0,
                    max: 100.0,
                    total: 5050.0,
                },
                RequestTimes {
                    min: 1.0,
                    p50: 2.0,
                    p90: 3.0,
                    p99: 3.0,
                    max: 3.0,
                    total: 6.0,
                },
                RequestTimes {
                    min: 7.0,
                    p50: 7.0,
                    p90: 7.0,
                    p99: 7.0,
                    max: 7.0,
                    total: 7.0,
                }
            )
        );
    }

    #[test]
    fn a_summary_groups_the_records_by_call_and_file_type() {
        let records = [
            record("put_raw", "pack", 3),
            record("get_raw_slice", "pack", 5),
            record("put_raw", "pack", 1),
            record("put_raw", "index", 2),
            record("put_raw", "pack", 0),
        ];

        let summary = summarize(&records);

        assert_eq!(
            (
                summary
                    .iter()
                    .map(|summary| (
                        summary.call,
                        summary.file_type,
                        summary.count,
                        summary.errors,
                        summary.bytes,
                        summary.time_ms.max
                    ))
                    .collect::<Vec<_>>(),
                written_and_read(&records)
            ),
            (
                vec![
                    ("get_raw_slice", "pack", 1, 0, 10, 5.0),
                    ("put_raw", "index", 1, 0, 10, 2.0),
                    ("put_raw", "pack", 3, 1, 30, 3.0),
                ],
                (40, 10)
            )
        );
    }

    #[test]
    fn the_file_type_is_the_first_name_of_the_repository_path() {
        assert_eq!(
            [
                "config",
                "data/ab/abcd",
                "index/ab",
                "snapshots/ab",
                "keys/ab",
                "results/base/save.json",
                "",
                "./data/ab/abcd",
                "agents/0/config",
                "agents/x8-7/data/ab/abcd",
                "agents/0",
                "agents"
            ]
            .map(|path| file_type(Path::new(path))),
            [
                "config", "pack", "index", "snapshot", "key", "other", "other", "pack", "config",
                "pack", "other", "other"
            ]
        );
    }

    #[test]
    async fn the_storage_records_each_call_and_passes_it_on() {
        let inner = Arc::new(InMemoryBlobStorage::new());
        let storage = MeasuredBlobStorage::new(inner.clone());
        let namespace = BlobStorageNamespace::InitialAgentFiles {
            environment_id: EnvironmentId(Uuid::new_v4()),
        };
        let pack = Path::new("data/ab/abcd");

        storage
            .put_raw("test", "test", namespace.clone(), pack, b"0123456789")
            .await
            .unwrap();
        let before = storage.take();
        let slice = storage
            .get_raw_slice("test", "test", namespace.clone(), pack, 2, 4)
            .await
            .unwrap();
        let missing = storage
            .get_raw("test", "test", namespace.clone(), Path::new("index/ef"))
            .await
            .unwrap();
        let after = storage.take();
        let empty = storage.take();
        let stored = inner
            .get_raw("test", "test", namespace, pack)
            .await
            .unwrap();

        assert_eq!(
            (
                before
                    .iter()
                    .map(|record| (
                        record.call,
                        record.file_type,
                        record.bytes,
                        record.written,
                        record.ok
                    ))
                    .collect::<Vec<_>>(),
                after
                    .iter()
                    .map(|record| (
                        record.call,
                        record.file_type,
                        record.bytes,
                        record.written,
                        record.ok
                    ))
                    .collect::<Vec<_>>(),
                empty.len(),
                slice,
                missing,
                stored,
            ),
            (
                vec![("put_raw", "pack", 10, true, true)],
                vec![
                    ("get_raw_slice", "pack", 3, false, true),
                    ("get_raw", "index", 0, false, true)
                ],
                0,
                Some(b"234".to_vec()),
                None,
                Some(b"0123456789".to_vec()),
            )
        );
    }
}
