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

//! The backend through the traits of rustic, on the in-memory blob storage.
//!
//! Each test calls the backend from the thread of the test. That thread is not a thread of the
//! runtime that the backend holds, as the threads of rustic are not.

use super::super::fault::{Operation, OperationCancelled, classify, is_config_exists};
use super::super::holding::{holding_storage, reached_deadline};
use super::super::publish::{SnapshotStage, StagedSnapshot};
use super::super::scripted::{Script, ScriptedBlobStorage};
use super::{BlobBackend, file_size};
use crate::filesystem_snapshot::SnapshotStoreError;
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use anyhow::anyhow;
use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::BoxStream;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::replayable_stream::ErasedReplayableStream;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{
    BlobMetadata, BlobStorage, BlobStorageNamespace, ExistsResult, ListedBlob, PutIfAbsent,
};
use pretty_assertions::assert_eq;
use rustic_core::{BytesList, FileType, Id, ReadBackend, RusticError, RusticResult, WriteBackend};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use test_r::test;
use tokio::runtime::Runtime;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// The longest time that a test waits for the calls on the backend.
const LIMIT: Duration = Duration::from_secs(10);

/// A backend over a new in-memory storage, with the runtime that it waits on.
struct Fixture {
    runtime: Runtime,
    storage: Arc<InMemoryBlobStorage>,
    namespace: BlobStorageNamespace,
    backend: BlobBackend,
}

impl Fixture {
    fn new() -> Self {
        let runtime = Runtime::new().unwrap();
        let storage = Arc::new(InMemoryBlobStorage::new());
        let namespace = new_namespace();
        let backend = BlobBackend::new(
            storage.clone(),
            namespace.clone(),
            runtime.handle().clone(),
            STORAGE_CALL_DEADLINE,
        );
        Self {
            runtime,
            storage,
            namespace,
            backend,
        }
    }

    /// Gives the path and the size of each blob of the namespace, in the order of the paths.
    fn stored(&self) -> Vec<(String, u64)> {
        let mut stored = self
            .runtime
            .block_on(self.storage.list_blobs_below(
                "test",
                "test",
                self.namespace.clone(),
                Path::new(""),
            ))
            .unwrap()
            .iter()
            .map(|blob| (blob.path.display().to_string(), blob.size))
            .collect::<Vec<_>>();
        stored.sort();
        stored
    }

    /// Writes the bytes as a blob at the path, past the backend.
    fn put(&self, path: &str, data: &[u8]) {
        self.runtime
            .block_on(self.storage.put_raw(
                "test",
                "test",
                self.namespace.clone(),
                Path::new(path),
                data,
            ))
            .unwrap();
    }
}

fn new_namespace() -> BlobStorageNamespace {
    BlobStorageNamespace::InitialAgentFiles {
        environment_id: EnvironmentId(Uuid::new_v4()),
    }
}

/// Gives the id whose 64 hex digits repeat the two digits.
fn id(digits: &str) -> Id {
    digits.repeat(32).parse().unwrap()
}

fn bytes(text: &str) -> BytesList {
    Bytes::copy_from_slice(text.as_bytes()).into()
}

/// Runs the calls on a new thread, which is not a thread of a runtime, and gives their result.
/// `None` means that the calls did not end within the limit.
fn within_limit<T: Send + 'static>(calls: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    on_own_thread(calls).recv_timeout(LIMIT).ok()
}

/// Starts the calls on a new thread, which is not a thread of a runtime. The receiver gets their
/// result, so a test can wait for it with a limit.
fn on_own_thread<T: Send + 'static>(
    calls: impl FnOnce() -> T + Send + 'static,
) -> std::sync::mpsc::Receiver<T> {
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || sender.send(calls()));
    receiver
}

#[test]
fn each_file_type_is_written_at_its_restic_path_and_read_back() {
    let fixture = Fixture::new();
    let files = [
        (FileType::Config, id("00"), "config file"),
        (FileType::Key, id("0a"), "key file"),
        (FileType::Snapshot, id("1b"), "snapshot file"),
        (FileType::Index, id("2c"), "index file"),
        (FileType::Pack, id("3d"), "pack file"),
    ];

    files.iter().for_each(|(tpe, id, content)| {
        fixture
            .backend
            .write_bytes(*tpe, id, false, bytes(content))
            .unwrap()
    });
    let read = files
        .iter()
        .map(|(tpe, id, _)| fixture.backend.read_full(*tpe, id).unwrap())
        .collect::<Vec<_>>();

    assert_eq!(
        (fixture.stored(), read),
        (
            vec![
                ("config".to_string(), 11),
                (format!("data/3d/{}", "3d".repeat(32)), 9),
                (format!("index/{}", "2c".repeat(32)), 10),
                (format!("keys/{}", "0a".repeat(32)), 8),
                (format!("snapshots/{}", "1b".repeat(32)), 13),
            ],
            files
                .iter()
                .map(|(_, _, content)| Bytes::from_static(content.as_bytes()))
                .collect::<Vec<_>>()
        )
    );
}

#[test]
fn a_write_of_several_parts_stores_the_parts_one_after_the_other() {
    let fixture = Fixture::new();
    let content = [Bytes::from_static(b"second "), Bytes::from_static(b"third")]
        .into_iter()
        .fold(bytes("first "), |mut content, part| {
            content.add(part);
            content
        });

    fixture
        .backend
        .write_bytes(FileType::Pack, &id("ab"), false, content)
        .unwrap();

    assert_eq!(
        fixture
            .backend
            .read_full(FileType::Pack, &id("ab"))
            .unwrap(),
        Bytes::from_static(b"first second third")
    );
}

#[test]
fn a_partial_read_gives_the_length_from_the_offset() {
    let fixture = Fixture::new();
    fixture
        .backend
        .write_bytes(FileType::Pack, &id("ab"), false, bytes("0123456789"))
        .unwrap();
    let read = |offset, length| {
        fixture
            .backend
            .read_partial(FileType::Pack, &id("ab"), false, offset, length)
            .unwrap()
    };

    assert_eq!(
        (read(0, 10), read(0, 1), read(3, 4), read(9, 1), read(4, 0)),
        (
            Bytes::from_static(b"0123456789"),
            Bytes::from_static(b"0"),
            Bytes::from_static(b"3456"),
            Bytes::from_static(b"9"),
            Bytes::new()
        )
    );
}

#[test]
fn a_partial_read_past_the_end_of_the_file_is_an_error() {
    let fixture = Fixture::new();
    fixture
        .backend
        .write_bytes(FileType::Pack, &id("ab"), false, bytes("0123456789"))
        .unwrap();
    let read = |offset, length| {
        fixture
            .backend
            .read_partial(FileType::Pack, &id("ab"), false, offset, length)
            .is_err()
    };

    assert_eq!(
        (
            read(0, 11),
            read(9, 2),
            read(10, 1),
            read(u32::MAX, u32::MAX)
        ),
        (true, true, true, true)
    );
}

#[test]
fn a_listing_gives_each_file_of_the_type_with_its_size() {
    let fixture = Fixture::new();
    [
        (FileType::Pack, id("ab"), "pack one"),
        (FileType::Pack, id("cd"), "pack number two"),
        (FileType::Index, id("ef"), "index"),
    ]
    .iter()
    .for_each(|(tpe, id, content)| {
        fixture
            .backend
            .write_bytes(*tpe, id, false, bytes(content))
            .unwrap()
    });
    fixture.put("data/ab/not-an-id", b"other");
    fixture.put("snapshots-extra/file", b"other");
    let sorted = |mut listed: Vec<(Id, u32)>| {
        listed.sort();
        listed
    };

    assert_eq!(
        (
            sorted(fixture.backend.list_with_size(FileType::Pack).unwrap()),
            sorted(fixture.backend.list_with_size(FileType::Index).unwrap()),
            fixture.backend.list_with_size(FileType::Snapshot).unwrap(),
        ),
        (
            vec![(id("ab"), 8), (id("cd"), 15)],
            vec![(id("ef"), 5)],
            vec![],
        )
    );
}

#[test]
fn the_config_file_is_listed_only_when_it_is_there() {
    let fixture = Fixture::new();

    let before = fixture.backend.list_with_size(FileType::Config).unwrap();
    fixture
        .backend
        .write_bytes(FileType::Config, &Id::default(), false, bytes("config"))
        .unwrap();
    let after = fixture.backend.list_with_size(FileType::Config).unwrap();

    assert_eq!((before, after), (vec![], vec![(Id::default(), 6)]));
}

#[test]
fn a_read_of_a_missing_file_is_an_error() {
    let fixture = Fixture::new();

    let full = fixture.backend.read_full(FileType::Snapshot, &id("ab"));
    let partial = fixture
        .backend
        .read_partial(FileType::Pack, &id("ab"), false, 0, 1);

    assert_eq!(
        (
            error_text(full).contains(&format!("snapshots/{}", "ab".repeat(32))),
            error_text(partial).contains(&format!("data/ab/{}", "ab".repeat(32))),
        ),
        (true, true)
    );
}

#[test]
fn a_remove_deletes_the_file_and_a_missing_file_is_no_error() {
    let fixture = Fixture::new();
    fixture
        .backend
        .write_bytes(FileType::Snapshot, &id("ab"), false, bytes("snapshot"))
        .unwrap();
    fixture
        .backend
        .write_bytes(FileType::Snapshot, &id("cd"), false, bytes("snapshot"))
        .unwrap();

    fixture
        .backend
        .remove(FileType::Snapshot, &id("ab"), false)
        .unwrap();
    fixture
        .backend
        .remove(FileType::Snapshot, &id("ab"), false)
        .unwrap();

    assert_eq!(
        fixture.stored(),
        vec![(format!("snapshots/{}", "cd".repeat(32)), 8)]
    );
}

#[test]
fn each_call_gives_an_error_of_the_storage_that_names_the_path() {
    let runtime = Runtime::new().unwrap();
    let backend = BlobBackend::new(
        Arc::new(FailingBlobStorage),
        new_namespace(),
        runtime.handle().clone(),
        STORAGE_CALL_DEADLINE,
    );
    let pack = format!("data/ab/{}", "ab".repeat(32));

    let texts = [
        error_text(backend.list_with_size(FileType::Config)),
        error_text(backend.list_with_size(FileType::Pack)),
        error_text(backend.read_full(FileType::Pack, &id("ab"))),
        error_text(backend.read_partial(FileType::Pack, &id("ab"), false, 0, 1)),
        error_text(backend.write_bytes(FileType::Pack, &id("ab"), false, bytes("pack"))),
        error_text(backend.remove(FileType::Pack, &id("ab"), false)),
    ];

    assert_eq!(
        texts
            .iter()
            .map(|text| (
                text.contains("the storage is broken"),
                ["config", "data", pack.as_str()]
                    .iter()
                    .any(|path| text.contains(&format!("`{path}`")))
            ))
            .collect::<Vec<_>>(),
        vec![(true, true); 6]
    );
}

#[test]
fn a_call_that_gets_no_answer_gives_a_storage_error_at_the_deadline() {
    let runtime = Runtime::new().unwrap();
    let (storage, _gate, _dropped) =
        holding_storage(Arc::new(InMemoryBlobStorage::new()), |_, _| true);
    let deadline = Duration::from_millis(100);
    let backend = BlobBackend::new(storage, new_namespace(), runtime.handle().clone(), deadline);
    let pack = format!("data/ab/{}", "ab".repeat(32));

    let outcome = within_limit(move || {
        let started = Instant::now();
        let errors = [
            (backend.list_with_size(FileType::Config).err(), "config"),
            (backend.list_with_size(FileType::Pack).err(), "data"),
            (
                backend.read_full(FileType::Pack, &id("ab")).err(),
                pack.as_str(),
            ),
            (
                backend
                    .read_partial(FileType::Pack, &id("ab"), false, 0, 1)
                    .err(),
                pack.as_str(),
            ),
            (
                backend
                    .write_bytes(FileType::Pack, &id("ab"), false, bytes("pack"))
                    .err(),
                pack.as_str(),
            ),
            (
                backend.remove(FileType::Pack, &id("ab"), false).err(),
                pack.as_str(),
            ),
        ]
        .map(|(error, path)| {
            error.map(|error| {
                let text = text_of(&error);
                (
                    text.contains(&format!("`{path}`")),
                    text.contains("the blob storage gave no answer within 100ms"),
                    reached_deadline(&*error),
                )
            })
        });
        (errors, started.elapsed() >= deadline * 6)
    });

    assert_eq!(outcome, Some(([Some((true, true, true)); 6], true)));
}

#[test]
fn a_call_that_answers_before_the_deadline_gives_its_answer() {
    let runtime = Runtime::new().unwrap();
    let (storage, gate, _dropped) =
        holding_storage(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
            op_label == "write"
        });
    let backend = BlobBackend::new(
        storage,
        new_namespace(),
        runtime.handle().clone(),
        Duration::from_secs(2),
    );
    let answer_after = Duration::from_millis(100);
    let started = Instant::now();
    runtime.spawn(async move {
        tokio::time::sleep(answer_after).await;
        drop(gate);
    });

    let outcome = within_limit(move || {
        let written = backend
            .write_bytes(FileType::Index, &id("ab"), false, bytes("index"))
            .is_ok();
        let waited = started.elapsed() >= answer_after;
        let read = backend.read_full(FileType::Index, &id("ab")).ok();
        (written, waited, read)
    });

    assert_eq!(
        outcome,
        Some((true, true, Some(Bytes::from_static(b"index"))))
    );
}

#[test]
fn a_thread_that_is_not_a_thread_of_the_runtime_can_call_the_backend() {
    let fixture = Fixture::new();
    let backend = Arc::new(fixture.backend);

    let read = within_limit({
        let backend = backend.clone();
        move || {
            backend
                .write_bytes(FileType::Index, &id("ab"), false, bytes("index"))
                .and_then(|()| backend.read_full(FileType::Index, &id("ab")))
                .ok()
        }
    });

    assert_eq!(read.flatten(), Some(Bytes::from_static(b"index")));
}

/// The content of the pack of the tests of the kept packs: 100 bytes, each its own offset.
fn pack_content() -> Vec<u8> {
    (0..100).collect()
}

/// A backend over a storage that holds one pack at the path of the id `ab`, with the rule of
/// the storage and the limit of the kept packs. The storage records each call.
struct PackFixture {
    _runtime: Runtime,
    storage: Arc<ScriptedBlobStorage>,
    backend: Arc<BlobBackend>,
}

impl PackFixture {
    fn new(limit: usize, rule: impl Fn(&str, &Path) -> Script + Send + Sync + 'static) -> Self {
        let runtime = Runtime::new().unwrap();
        let inner = Arc::new(InMemoryBlobStorage::new());
        let namespace = new_namespace();
        runtime
            .block_on(inner.put_raw(
                "test",
                "test",
                namespace.clone(),
                Path::new(&format!("data/ab/{}", "ab".repeat(32))),
                &pack_content(),
            ))
            .unwrap();
        let storage = ScriptedBlobStorage::new(inner, rule);
        let backend = BlobBackend::new(
            storage.clone(),
            namespace,
            runtime.handle().clone(),
            STORAGE_CALL_DEADLINE,
        )
        .keeping_packs_up_to(limit);
        Self {
            _runtime: runtime,
            storage,
            backend: Arc::new(backend),
        }
    }

    /// Gives the operation label of each call on the pack.
    fn pack_calls(&self) -> Vec<&'static str> {
        self.storage
            .calls()
            .into_iter()
            .filter(|(_, path)| path.starts_with("data/"))
            .map(|(op_label, _)| op_label)
            .collect()
    }
}

/// Reads the range of the pack as a range of tree blobs, which rustic marks as cacheable.
fn tree_range(backend: &BlobBackend, offset: u32, length: u32) -> RusticResult<Bytes> {
    backend.read_partial(FileType::Pack, &id("ab"), true, offset, length)
}

#[test]
fn a_later_range_of_a_kept_pack_makes_no_storage_call() {
    let fixture = PackFixture::new(1024, |_, _| Script::Pass);
    let backend = fixture.backend.clone();

    let ranges = within_limit(move || {
        (
            tree_range(&backend, 0, 10).ok(),
            tree_range(&backend, 20, 10).ok(),
            tree_range(&backend, 90, 10).ok(),
        )
    });

    assert_eq!(
        (ranges, fixture.pack_calls()),
        (
            Some((
                Some(Bytes::from_iter(0..10)),
                Some(Bytes::from_iter(20..30)),
                Some(Bytes::from_iter(90..100)),
            )),
            vec!["read"]
        )
    );
}

#[test]
fn a_range_that_is_not_cacheable_is_a_ranged_read_each_time() {
    let fixture = PackFixture::new(1024, |_, _| Script::Pass);
    let backend = fixture.backend.clone();

    let ranges = within_limit(move || {
        [(0, 10), (20, 10)].map(|(offset, length)| {
            backend
                .read_partial(FileType::Pack, &id("ab"), false, offset, length)
                .ok()
        })
    });

    assert_eq!(
        (ranges, fixture.pack_calls()),
        (
            Some([
                Some(Bytes::from_iter(0..10)),
                Some(Bytes::from_iter(20..30))
            ]),
            vec!["read_range", "read_range"]
        )
    );
}

#[test]
fn two_threads_that_miss_one_pack_make_one_storage_read() {
    // The first read waits at the gate. The second thread starts while it waits, and the gate
    // opens only after the second thread had time to ask for the pack.
    let fixture = PackFixture::new(1024, |op_label, _| {
        if op_label == "read" {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let first = on_own_thread({
        let backend = fixture.backend.clone();
        move || tree_range(&backend, 0, 10).ok()
    });
    let first_read_started = (0..1000).any(|_| {
        std::thread::sleep(Duration::from_millis(10));
        !fixture.pack_calls().is_empty()
    });
    let second = on_own_thread({
        let backend = fixture.backend.clone();
        move || tree_range(&backend, 50, 10).ok()
    });
    std::thread::sleep(Duration::from_millis(200));
    fixture.storage.open_gate();

    assert_eq!(
        (
            first_read_started,
            first.recv_timeout(LIMIT).ok().flatten(),
            second.recv_timeout(LIMIT).ok().flatten(),
            fixture.pack_calls()
        ),
        (
            true,
            Some(Bytes::from_iter(0..10)),
            Some(Bytes::from_iter(50..60)),
            vec!["read"]
        )
    );
}

#[test]
fn a_pack_over_the_limit_is_read_again_at_its_next_range() {
    let fixture = PackFixture::new(99, |_, _| Script::Pass);
    let backend = fixture.backend.clone();

    let ranges = within_limit(move || {
        (
            tree_range(&backend, 0, 10).ok(),
            tree_range(&backend, 20, 10).ok(),
        )
    });

    assert_eq!(
        (ranges, fixture.pack_calls()),
        (
            Some((
                Some(Bytes::from_iter(0..10)),
                Some(Bytes::from_iter(20..30))
            )),
            vec!["read", "read"]
        )
    );
}

#[test]
fn a_failed_read_of_a_pack_is_not_kept_and_the_next_range_reads_again() {
    let refused = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fixture = PackFixture::new(1024, {
        let refused = refused.clone();
        move |op_label, _| {
            if op_label == "read" && !refused.swap(true, std::sync::atomic::Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let backend = fixture.backend.clone();

    let ranges = within_limit(move || {
        (
            tree_range(&backend, 0, 10).is_err(),
            tree_range(&backend, 20, 10).ok(),
            tree_range(&backend, 40, 10).ok(),
        )
    });

    assert_eq!(
        (ranges, fixture.pack_calls()),
        (
            Some((
                true,
                Some(Bytes::from_iter(20..30)),
                Some(Bytes::from_iter(40..50))
            )),
            vec!["read", "read"]
        )
    );
}

#[test]
fn a_range_outside_a_kept_pack_gives_the_error_of_a_ranged_read_outside_a_blob() {
    let fixture = PackFixture::new(1024, |_, _| Script::Pass);
    let backend = fixture.backend.clone();

    let outside = within_limit(move || {
        [(90, 11), (100, 1), (u32::MAX, 1)].map(|(offset, length)| {
            tree_range(&backend, offset, length).err().map(|error| {
                (
                    text_of(&error).contains("is not in the blob"),
                    classify(Operation::Restore, anyhow::Error::new(error))
                        .to_string()
                        .contains("storage"),
                )
            })
        })
    });

    assert_eq!(outside, Some([Some((true, true)); 3]));
}

#[test]
fn a_tracked_backend_counts_in_its_tracker_until_it_drops() {
    let fixture = Fixture::new();
    let tracker = tokio_util::task::TaskTracker::new();
    let backend = BlobBackend::new(
        fixture.storage.clone(),
        fixture.namespace.clone(),
        fixture.runtime.handle().clone(),
        STORAGE_CALL_DEADLINE,
    )
    .tracked_by(tracker.token());

    let while_alive = tracker.len();
    drop(backend);

    assert_eq!((while_alive, tracker.len()), (1, 0));
}

#[test]
fn a_cancelled_backend_makes_no_storage_call() {
    let runtime = Runtime::new().unwrap();
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let cancel = CancellationToken::new();
    let backend = BlobBackend::new(
        storage.clone(),
        new_namespace(),
        runtime.handle().clone(),
        STORAGE_CALL_DEADLINE,
    )
    .cancelled_by(cancel.clone());
    cancel.cancel();

    let cancelled = [
        backend.list_with_size(FileType::Config).err(),
        backend.list_with_size(FileType::Pack).err(),
        backend.read_full(FileType::Pack, &id("ab")).err(),
        backend
            .read_partial(FileType::Pack, &id("ab"), false, 0, 1)
            .err(),
        backend
            .write_bytes(FileType::Pack, &id("ab"), false, bytes("pack"))
            .err(),
        backend.remove(FileType::Pack, &id("ab"), false).err(),
    ]
    .map(|error| error.is_some_and(|error| was_cancelled(&error)));

    assert_eq!((cancelled, storage.calls()), ([true; 6], Vec::new()));
}

#[test]
fn a_cancel_ends_a_call_that_runs() {
    let runtime = Runtime::new().unwrap();
    let (storage, _gate, _dropped) =
        holding_storage(Arc::new(InMemoryBlobStorage::new()), |_, _| true);
    let cancel = CancellationToken::new();
    let backend = BlobBackend::new(
        storage,
        new_namespace(),
        runtime.handle().clone(),
        Duration::from_secs(60),
    )
    .cancelled_by(cancel.clone());
    runtime.spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
    });

    let outcome = within_limit(move || {
        backend
            .read_full(FileType::Pack, &id("ab"))
            .err()
            .map(|error| (was_cancelled(&error), reached_deadline(&*error)))
    });

    assert_eq!(outcome, Some(Some((true, false))));
}

#[test]
fn a_backend_whose_token_is_not_cancelled_answers() {
    let fixture = Fixture::new();
    let backend = BlobBackend::new(
        fixture.storage.clone(),
        fixture.namespace.clone(),
        fixture.runtime.handle().clone(),
        STORAGE_CALL_DEADLINE,
    )
    .cancelled_by(CancellationToken::new());

    let read = backend
        .write_bytes(FileType::Pack, &id("ab"), false, bytes("pack"))
        .and_then(|()| backend.read_full(FileType::Pack, &id("ab")));

    assert_eq!(read.ok(), Some(Bytes::from_static(b"pack")));
}

#[test]
fn the_config_file_is_written_only_when_the_repository_has_none() {
    let fixture = Fixture::new();

    let first =
        fixture
            .backend
            .write_bytes(FileType::Config, &Id::default(), false, bytes("first"));
    let second =
        fixture
            .backend
            .write_bytes(FileType::Config, &Id::default(), false, bytes("second"));

    assert_eq!(
        (
            first.is_ok(),
            second
                .as_ref()
                .is_err_and(|error| is_config_exists(&**error)),
            fixture.stored(),
        ),
        (true, true, vec![("config".to_string(), 5)])
    );
}

#[test]
fn an_index_file_that_is_there_is_kept_and_its_write_succeeds() {
    let fixture = Fixture::new();
    let path = format!("index/{}", "ab".repeat(32));
    fixture.put(&path, b"kept");

    let written =
        fixture
            .backend
            .write_bytes(FileType::Index, &id("ab"), false, bytes("replacement"));
    let read = fixture.backend.read_full(FileType::Index, &id("ab"));

    assert_eq!(
        (written.is_ok(), read.ok()),
        (true, Some(Bytes::from_static(b"kept")))
    );
}

#[test]
fn a_pack_file_that_is_there_is_written_again() {
    let fixture = Fixture::new();
    fixture.put(&format!("data/ab/{}", "ab".repeat(32)), b"old");

    let written = fixture
        .backend
        .write_bytes(FileType::Pack, &id("ab"), false, bytes("new"));
    let read = fixture.backend.read_full(FileType::Pack, &id("ab"));

    assert_eq!(
        (written.is_ok(), read.ok()),
        (true, Some(Bytes::from_static(b"new")))
    );
}

#[test]
fn a_backend_with_a_stage_keeps_the_snapshot_file_and_does_not_write_it() {
    let fixture = Fixture::new();
    let stage = Arc::new(SnapshotStage::default());
    let backend = BlobBackend::new(
        fixture.storage.clone(),
        fixture.namespace.clone(),
        fixture.runtime.handle().clone(),
        STORAGE_CALL_DEADLINE,
    )
    .staging_in(stage.clone());
    let content = [Bytes::from_static(b"snap"), Bytes::from_static(b"shot")]
        .into_iter()
        .fold(BytesList::default(), |mut content, part| {
            content.add(part);
            content
        });

    let kept = backend.write_bytes(FileType::Snapshot, &id("cd"), false, content);
    let second = backend.write_bytes(FileType::Snapshot, &id("ef"), false, bytes("other"));
    let pack = backend.write_bytes(FileType::Pack, &id("ab"), false, bytes("pack"));

    assert_eq!(
        (
            kept.is_ok(),
            second.is_err(),
            pack.is_ok(),
            stage.take(),
            fixture.stored(),
        ),
        (
            true,
            true,
            true,
            Some(StagedSnapshot {
                path: Arc::from(PathBuf::from(format!("snapshots/{}", "cd".repeat(32)))),
                content: Bytes::from_static(b"snapshot"),
            }),
            vec![(format!("data/ab/{}", "ab".repeat(32)), 4)]
        )
    );
}

#[test]
fn each_failed_call_is_a_storage_failure_to_the_classification() {
    let runtime = Runtime::new().unwrap();
    let backend = BlobBackend::new(
        Arc::new(FailingBlobStorage),
        new_namespace(),
        runtime.handle().clone(),
        STORAGE_CALL_DEADLINE,
    );

    let classified = [
        backend.list_with_size(FileType::Pack).err(),
        backend.read_full(FileType::Pack, &id("ab")).err(),
        backend
            .write_bytes(FileType::Pack, &id("ab"), false, bytes("pack"))
            .err(),
    ]
    .map(|error| {
        error.map(|error| {
            matches!(
                classify(Operation::Restore, anyhow::Error::new(error)),
                SnapshotStoreError::Storage {
                    retryable: true,
                    ..
                }
            )
        })
    });

    assert_eq!(classified, [Some(true); 3]);
}

#[test]
fn a_size_that_does_not_fit_in_32_bits_is_an_error() {
    let path = Path::new("data/ab/file");

    assert_eq!(
        (
            file_size(path, u64::from(u32::MAX)).ok(),
            file_size(path, u64::from(u32::MAX) + 1).is_err()
        ),
        (Some(u32::MAX), true)
    );
}

/// Gives the text of the error of the result, with the text of its source.
fn error_text<T>(result: RusticResult<T>) -> String {
    match result {
        Ok(_) => String::new(),
        Err(error) => text_of(&error),
    }
}

/// Gives the text of the error, with the text of each error in its chain of sources.
fn text_of(error: &RusticError) -> String {
    std::iter::successors(std::error::Error::source(error), |error| error.source())
        .fold(error.to_string(), |text, source| format!("{text} {source}"))
}

/// Tells whether the error or an error in its chain of sources is [`OperationCancelled`].
fn was_cancelled(error: &RusticError) -> bool {
    std::iter::successors(Some(error as &(dyn std::error::Error + 'static)), |error| {
        error.source()
    })
    .any(|error| error.is::<OperationCancelled>())
}

/// A blob storage that fails every call.
#[derive(Debug)]
struct FailingBlobStorage;

fn broken() -> anyhow::Error {
    anyhow!("the storage is broken")
}

#[async_trait]
impl BlobStorage for FailingBlobStorage {
    async fn get_raw(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<Option<Vec<u8>>> {
        Err(broken())
    }

    async fn get_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<Option<BoxStream<'static, anyhow::Result<Bytes>>>> {
        Err(broken())
    }

    async fn get_metadata(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<Option<BlobMetadata>> {
        Err(broken())
    }

    async fn put_raw(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
        _data: &[u8],
    ) -> anyhow::Result<()> {
        Err(broken())
    }

    async fn put_raw_if_absent(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
        _data: &[u8],
    ) -> anyhow::Result<PutIfAbsent> {
        Err(broken())
    }

    async fn put_stream(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
        _stream: &dyn ErasedReplayableStream<Item = anyhow::Result<Vec<u8>>, Error = anyhow::Error>,
    ) -> anyhow::Result<()> {
        Err(broken())
    }

    async fn delete(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<()> {
        Err(broken())
    }

    async fn create_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<()> {
        Err(broken())
    }

    async fn list_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<Vec<PathBuf>> {
        Err(broken())
    }

    async fn list_blobs_below(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<Box<[ListedBlob]>> {
        Err(broken())
    }

    async fn delete_dir(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<bool> {
        Err(broken())
    }

    async fn exists(
        &self,
        _target_label: &'static str,
        _op_label: &'static str,
        _namespace: BlobStorageNamespace,
        _path: &Path,
    ) -> anyhow::Result<ExistsResult> {
        Err(broken())
    }
}
