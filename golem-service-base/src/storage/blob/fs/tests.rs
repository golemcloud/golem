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

use super::{
    Commit, CommitGate, FileSystemBlobStorage, STAGING_DIRECTORY, STAGING_FILE_AGE,
    absent_on_not_found, add_files, copy_staged, first_error, listed_entry, remove_unless_dropped,
    staging_file_is_old, write_if_absent, write_staged,
};
use crate::storage::blob::{
    BlobRangeError, BlobStorage, BlobStorageNamespace, ListedBlob, PutIfAbsent,
};
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::{AgentFingerprint, AgentId};
use pretty_assertions::assert_eq;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use test_r::test;
use uuid::Uuid;

fn namespace() -> BlobStorageNamespace {
    BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId(Uuid::nil()),
    }
}

/// Makes a blob storage in a new temporary directory with the blob `ranges/blob`, which holds
/// `abcdef`, and the empty blob `ranges/empty`.
async fn storage_with_blobs() -> (tempfile::TempDir, FileSystemBlobStorage) {
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemBlobStorage::new(root.path()).await.unwrap();
    storage
        .put_raw(
            "test",
            "put-raw",
            namespace(),
            Path::new("ranges/blob"),
            b"abcdef",
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "test",
            "put-raw",
            namespace(),
            Path::new("ranges/empty"),
            b"",
        )
        .await
        .unwrap();
    (root, storage)
}

#[test]
async fn get_raw_slice_reads_the_inclusive_range_of_the_file() {
    let (_root, storage) = storage_with_blobs().await;
    let read = |path: &'static str, start, end| {
        storage.get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new(path),
            start,
            end,
        )
    };

    let inside = read("ranges/blob", 1, 3).await.unwrap();
    let first_byte = read("ranges/blob", 0, 0).await.unwrap();
    let whole = read("ranges/blob", 0, 5).await.unwrap();
    let last_byte = read("ranges/blob", 5, 5).await.unwrap();
    let missing = read("ranges/missing", 0, 0).await.unwrap();
    let below_a_file = read("ranges/blob/below", 0, 0).await.unwrap();

    assert_eq!(
        (inside, first_byte, whole, last_byte, missing, below_a_file),
        (
            Some(b"bcd".to_vec()),
            Some(b"a".to_vec()),
            Some(b"abcdef".to_vec()),
            Some(b"f".to_vec()),
            None,
            None
        )
    );
}

#[test]
async fn get_raw_slice_gives_a_range_error_for_a_range_that_is_not_in_the_file() {
    let (_root, storage) = storage_with_blobs().await;
    let storage = &storage;
    let outside = [
        ("ranges/blob", 0, 6),
        ("ranges/blob", 6, 6),
        ("ranges/blob", 3, 2),
        ("ranges/blob", u64::MAX, 2),
        ("ranges/blob", u64::MAX, u64::MAX),
        ("ranges/empty", 0, 0),
        // A start after the end gives the error before the backend looks for the file.
        ("ranges/missing", 3, 2),
    ];

    let errors = futures::future::join_all(outside.map(|(path, start, end)| async move {
        storage
            .get_raw_slice(
                "test",
                "get-raw-slice",
                namespace(),
                Path::new(path),
                start,
                end,
            )
            .await
            .map_err(|error| error.downcast_ref::<BlobRangeError>().copied())
    }))
    .await;

    assert_eq!(
        errors,
        outside
            .map(|(_, start, end)| Err(Some(BlobRangeError { start, end })))
            .to_vec()
    );
}

#[test]
async fn get_raw_slice_gives_the_error_of_get_raw_for_a_directory() {
    let (_root, storage) = storage_with_blobs().await;
    let io_error_kind = |error: anyhow::Error| {
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind)
    };

    let slice = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("ranges"),
            0,
            0,
        )
        .await;
    let raw = storage
        .get_raw("test", "get-raw", namespace(), Path::new("ranges"))
        .await;

    assert_eq!(
        (slice.map_err(io_error_kind), raw.map_err(io_error_kind)),
        (
            Err(Some(ErrorKind::IsADirectory)),
            Err(Some(ErrorKind::IsADirectory))
        )
    );
}

#[test]
async fn copy_makes_the_directory_of_the_target() {
    let (_root, storage) = storage_with_blobs().await;

    storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("ranges/blob"),
            Path::new("other/below/blob"),
        )
        .await
        .unwrap();
    let copied = storage
        .get_raw(
            "test",
            "get-raw",
            namespace(),
            Path::new("other/below/blob"),
        )
        .await
        .unwrap();

    assert_eq!(copied, Some(b"abcdef".to_vec()));
}

fn snapshots() -> BlobStorageNamespace {
    BlobStorageNamespace::FilesystemSnapshots {
        environment_id: EnvironmentId(Uuid::nil()),
        agent_id: AgentId {
            component_id: ComponentId(Uuid::nil()),
            agent_id: "agent".to_string(),
        },
        fingerprint: AgentFingerprint(Uuid::nil()),
    }
}

fn not_found() -> std::io::Error {
    std::io::Error::from(ErrorKind::NotFound)
}

/// The files of the staging directory of `root`.
fn staged_files(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root.join(STAGING_DIRECTORY))
        .map(|entries| entries.map(|entry| entry.unwrap().path()).collect())
        .unwrap_or_default()
}

/// A gate of the blocking writes of a storage: a write that reaches it tells `reached`, and waits
/// until the test sends to `release`.
struct Gate {
    function: Arc<dyn Fn() + Send + Sync>,
    reached: tokio::sync::mpsc::UnboundedReceiver<()>,
    release: std::sync::mpsc::Sender<()>,
}

impl Gate {
    fn new() -> Self {
        let (reached_sender, reached) = tokio::sync::mpsc::unbounded_channel();
        let (release, release_receiver) = std::sync::mpsc::channel::<()>();
        let release_receiver = Mutex::new(release_receiver);
        Self {
            function: Arc::new(move || {
                reached_sender.send(()).unwrap();
                release_receiver.lock().unwrap().recv().unwrap();
            }),
            reached,
            release,
        }
    }

    /// Lets the write at the gate go on, and waits until its blocking work returned.
    async fn release_and_wait(&self) {
        self.release.send(()).unwrap();
        // The test and the storage hold the gate; the blocking work holds one more until it
        // returns.
        tokio::time::timeout(
            Duration::from_secs(10),
            futures::StreamExt::for_each(
                futures::StreamExt::take_while(
                    futures::StreamExt::then(futures::stream::repeat(()), |()| {
                        tokio::time::sleep(Duration::from_millis(5))
                    }),
                    |()| std::future::ready(Arc::strong_count(&self.function) > 2),
                ),
                |()| std::future::ready(()),
            ),
        )
        .await
        .unwrap();
    }
}

/// A storage over `root` whose blocking writes stop at `gate`.
fn gated_storage(root: &Path, gate: &Gate) -> FileSystemBlobStorage {
    FileSystemBlobStorage {
        root: std::fs::canonicalize(root).unwrap(),
        before_commit: Some(CommitGate(gate.function.clone())),
        after_metadata: None,
    }
}

/// Runs `call` until its blocking work reaches the gate, and then drops it.
async fn drop_at_the_gate<T>(gate: &mut Gate, call: impl std::future::Future<Output = T>) {
    let call = std::pin::pin!(call);
    tokio::select! {
        _ = call => panic!("the call ended before its write reached the gate"),
        _ = gate.reached.recv() => {}
    }
}

#[test]
fn listed_entry_gives_none_only_for_a_missing_entry() {
    let answers = (
        listed_entry(Ok::<_, std::io::Error>(1)).unwrap(),
        listed_entry::<u8>(Err(not_found())).unwrap(),
        listed_entry::<u8>(Err(std::io::Error::from(ErrorKind::PermissionDenied)))
            .map_err(|error| error.kind()),
    );

    assert_eq!(answers, (Some(1), None, Err(ErrorKind::PermissionDenied)));
}

#[test]
async fn a_whole_and_a_partial_read_of_a_blob_removed_after_its_metadata_check_give_none() {
    let (root, storage) = storage_with_blobs().await;
    let blob = std::fs::canonicalize(root.path())
        .unwrap()
        .join("custom_data")
        .join(EnvironmentId(Uuid::nil()).to_string())
        .join("ranges/blob");
    let removed = blob.clone();
    let storage = FileSystemBlobStorage {
        after_metadata: Some(CommitGate(Arc::new(move || {
            let _ = std::fs::remove_file(&removed);
        }))),
        ..storage
    };

    let whole = storage
        .get_raw("test", "get-raw", namespace(), Path::new("ranges/blob"))
        .await
        .map_err(|error| error.to_string());
    std::fs::write(&blob, b"abcdef").unwrap();
    let partial = storage
        .get_raw_slice(
            "test",
            "get-raw-slice",
            namespace(),
            Path::new("ranges/blob"),
            1,
            2,
        )
        .await
        .map_err(|error| error.to_string());

    assert_eq!((whole, partial), (Ok(None), Ok(None)));
}

#[test]
fn a_read_of_a_blob_deleted_after_its_metadata_gives_none() {
    let root = tempfile::tempdir().unwrap();
    let removed = root.path().join("removed");
    std::fs::write(&removed, b"bytes").unwrap();
    let metadata = std::fs::metadata(&removed).map(|_| ()).ok();
    std::fs::remove_file(&removed).unwrap();

    let read = absent_on_not_found(std::fs::read(&removed)).unwrap();
    let other = absent_on_not_found(std::fs::read(root.path())).map_err(|error| error.kind());

    assert_eq!(
        (metadata, read, other),
        (Some(()), None, Err(ErrorKind::IsADirectory))
    );
}

#[cfg(unix)]
#[test]
fn a_listing_leaves_out_the_entries_that_a_remove_took_away_and_goes_on() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("kept"), b"kept").unwrap();
    std::fs::write(root.path().join("removed"), b"removed").unwrap();
    std::fs::create_dir(root.path().join("gone")).unwrap();
    std::fs::write(root.path().join("gone/file"), b"file").unwrap();
    let entries = std::fs::read_dir(root.path()).unwrap().collect::<Vec<_>>();
    std::fs::remove_file(root.path().join("removed")).unwrap();
    std::fs::remove_dir_all(root.path().join("gone")).unwrap();

    let listed = add_files(
        std::iter::once(Err(not_found())).chain(entries),
        root.path(),
        Vec::new(),
    )
    .unwrap();

    assert_eq!(
        listed,
        vec![ListedBlob {
            path: Path::new("kept").into(),
            size: 4,
        }]
    );
}

#[test]
fn a_staging_file_is_old_only_after_the_staging_file_age() {
    let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10 * 60 * 60);
    let at = |age: Duration| staging_file_is_old(now - age, now);

    assert_eq!(
        (
            at(STAGING_FILE_AGE + Duration::from_secs(1)),
            at(STAGING_FILE_AGE),
            at(Duration::ZERO),
            staging_file_is_old(now + Duration::from_secs(1), now),
        ),
        (true, false, false, false)
    );
}

#[test]
async fn old_staging_files_are_removed_at_start() {
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join(STAGING_DIRECTORY);
    std::fs::create_dir_all(&staging).unwrap();
    let old = std::fs::File::create(staging.join("old")).unwrap();
    old.set_modified(SystemTime::now() - STAGING_FILE_AGE - Duration::from_secs(60))
        .unwrap();
    std::fs::write(staging.join("young"), b"young").unwrap();

    FileSystemBlobStorage::new(root.path()).await.unwrap();

    assert_eq!(
        staged_files(root.path()),
        vec![std::fs::canonicalize(&staging).unwrap().join("young")]
    );
}

#[cfg(unix)]
#[test]
async fn a_start_whose_old_staging_file_cannot_be_removed_goes_on() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let staging = root.path().join(STAGING_DIRECTORY);
    std::fs::create_dir_all(&staging).unwrap();
    let old = std::fs::File::create(staging.join("old")).unwrap();
    old.set_modified(SystemTime::now() - STAGING_FILE_AGE - Duration::from_secs(60))
        .unwrap();
    std::fs::write(staging.join("probe"), b"probe").unwrap();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o555)).unwrap();
    // A process that may write into any directory removes the file anyway, and the case does not
    // arise for it.
    let privileged = std::fs::remove_file(staging.join("probe")).is_ok();

    let started = FileSystemBlobStorage::new(root.path())
        .await
        .map(|_| ())
        .map_err(|error| error.to_string());
    let old_stays = staging.join("old").exists();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(started, Ok(()));
    assert_eq!(old_stays, !privileged);
}

#[test]
fn a_write_whose_call_was_dropped_before_it_started_makes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let commit = Commit {
        dropped: Arc::new(AtomicBool::new(true)),
        gate: None,
    };
    let staging = root.path().join(STAGING_DIRECTORY);
    let target = root.path().join("below/blob");

    let written = write_staged(&commit, &staging, &target, b"bytes").map_err(|error| error.kind());

    assert_eq!(
        (written, staging.exists(), target.parent().unwrap().exists()),
        (Err(ErrorKind::Interrupted), false, false)
    );
}

#[test]
fn a_write_if_absent_a_copy_and_a_remove_whose_calls_were_dropped_before_they_started_change_nothing()
 {
    let root = tempfile::tempdir().unwrap();
    let commit = Commit {
        dropped: Arc::new(AtomicBool::new(true)),
        gate: None,
    };
    let staging = root.path().join(STAGING_DIRECTORY);
    let source = root.path().join("source");
    std::fs::write(&source, b"source").unwrap();
    let kept = root.path().join("kept");
    std::fs::write(&kept, b"kept").unwrap();

    let written = write_if_absent(
        &commit,
        &staging,
        &root.path().join("written/blob"),
        b"bytes",
    )
    .map(|_| ())
    .map_err(|error| error.kind());
    let copied = copy_staged(&commit, &source, &staging, &root.path().join("copied/blob"))
        .map(|_| ())
        .map_err(|error| error.kind());
    let removed = remove_unless_dropped(&commit, &kept).map_err(|error| error.kind());

    assert_eq!(
        (
            [written, copied, removed],
            staging.exists(),
            root.path().join("written").exists(),
            root.path().join("copied").exists(),
            std::fs::read(&kept).unwrap(),
        ),
        (
            [
                Err(ErrorKind::Interrupted),
                Err(ErrorKind::Interrupted),
                Err(ErrorKind::Interrupted)
            ],
            false,
            false,
            false,
            b"kept".to_vec(),
        )
    );
}

#[cfg(unix)]
#[test]
async fn a_snapshot_put_replaces_the_blob_whole_with_the_usual_mode() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemBlobStorage::new(root.path()).await.unwrap();
    let blob = Path::new("blob");
    storage
        .put_raw("test", "put-raw", snapshots(), blob, b"first and longer")
        .await
        .unwrap();
    let full = storage.path_of(
        &snapshots(),
        &crate::storage::blob::normalized_blob_path(blob).unwrap(),
    );
    let first_inode = std::fs::metadata(&full).unwrap().ino();
    storage
        .put_raw("test", "put-raw", snapshots(), blob, b"second")
        .await
        .unwrap();
    storage
        .put_raw("test", "put-raw", namespace(), blob, b"other")
        .await
        .unwrap();
    let other = storage.path_of(
        &namespace(),
        &crate::storage::blob::normalized_blob_path(blob).unwrap(),
    );
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

    assert_eq!(
        (
            std::fs::read(&full).unwrap(),
            std::fs::metadata(&full).unwrap().ino() != first_inode,
            mode(&full),
            staged_files(root.path()),
        ),
        (b"second".to_vec(), true, mode(&other), Vec::new())
    );
}

#[cfg(unix)]
#[test]
async fn a_put_of_another_namespace_writes_in_place() {
    use std::os::unix::fs::MetadataExt;
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemBlobStorage::new(root.path()).await.unwrap();
    let blob = Path::new("blob");
    let full = storage.path_of(
        &namespace(),
        &crate::storage::blob::normalized_blob_path(blob).unwrap(),
    );
    storage
        .put_raw("test", "put-raw", namespace(), blob, b"first")
        .await
        .unwrap();
    let first_inode = std::fs::metadata(&full).unwrap().ino();

    storage
        .put_raw("test", "put-raw", namespace(), blob, b"second")
        .await
        .unwrap();

    assert_eq!(
        (
            std::fs::read(&full).unwrap(),
            std::fs::metadata(&full).unwrap().ino(),
            root.path().join(STAGING_DIRECTORY).exists()
        ),
        (b"second".to_vec(), first_inode, false)
    );
}

#[test]
async fn a_dropped_snapshot_put_never_lands() {
    let root = tempfile::tempdir().unwrap();
    FileSystemBlobStorage::new(root.path()).await.unwrap();
    let mut gate = Gate::new();
    let storage = gated_storage(root.path(), &gate);
    let blob = Path::new("blob");

    drop_at_the_gate(
        &mut gate,
        storage.put_raw("test", "put-raw", snapshots(), blob, b"bytes"),
    )
    .await;
    gate.release_and_wait().await;
    let read = FileSystemBlobStorage::new(root.path())
        .await
        .unwrap()
        .get_raw("test", "get-raw", snapshots(), blob)
        .await
        .unwrap();

    assert_eq!((read, staged_files(root.path())), (None, Vec::new()));
}

#[test]
async fn a_dropped_put_if_absent_never_lands() {
    let root = tempfile::tempdir().unwrap();
    FileSystemBlobStorage::new(root.path()).await.unwrap();
    let mut gate = Gate::new();
    let storage = gated_storage(root.path(), &gate);
    let blob = Path::new("blob");

    drop_at_the_gate(
        &mut gate,
        storage.put_raw_if_absent("test", "put-if-absent", snapshots(), blob, b"bytes"),
    )
    .await;
    gate.release_and_wait().await;
    let written = FileSystemBlobStorage::new(root.path())
        .await
        .unwrap()
        .put_raw_if_absent("test", "put-if-absent", snapshots(), blob, b"later")
        .await
        .unwrap();

    assert_eq!(
        (written, staged_files(root.path())),
        (PutIfAbsent::Written, Vec::new())
    );
}

#[test]
async fn a_dropped_copy_never_lands() {
    let root = tempfile::tempdir().unwrap();
    let plain = FileSystemBlobStorage::new(root.path()).await.unwrap();
    plain
        .put_raw(
            "test",
            "put-raw",
            snapshots(),
            Path::new("source"),
            b"bytes",
        )
        .await
        .unwrap();
    let mut gate = Gate::new();
    let storage = gated_storage(root.path(), &gate);

    drop_at_the_gate(
        &mut gate,
        storage.copy_between(
            "test",
            "copy-between",
            snapshots(),
            Path::new("source"),
            namespace(),
            Path::new("target"),
        ),
    )
    .await;
    gate.release_and_wait().await;
    let read = plain
        .get_raw("test", "get-raw", namespace(), Path::new("target"))
        .await
        .unwrap();

    assert_eq!((read, staged_files(root.path())), (None, Vec::new()));
}

#[test]
async fn a_dropped_snapshot_delete_never_lands() {
    let root = tempfile::tempdir().unwrap();
    let plain = FileSystemBlobStorage::new(root.path()).await.unwrap();
    let blob = Path::new("blob");
    plain
        .put_raw("test", "put-raw", snapshots(), blob, b"bytes")
        .await
        .unwrap();
    let mut gate = Gate::new();
    let storage = gated_storage(root.path(), &gate);

    drop_at_the_gate(
        &mut gate,
        storage.delete("test", "delete", snapshots(), blob),
    )
    .await;
    gate.release_and_wait().await;
    let read = plain
        .get_raw("test", "get-raw", snapshots(), blob)
        .await
        .unwrap();

    assert_eq!(read, Some(b"bytes".to_vec()));
}

#[test]
async fn a_delete_of_a_missing_blob_succeeds_in_each_namespace() {
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemBlobStorage::new(root.path()).await.unwrap();

    let deleted = (
        storage
            .delete("test", "delete", snapshots(), Path::new("missing"))
            .await
            .map_err(|error| error.to_string()),
        storage
            .delete("test", "delete", namespace(), Path::new("missing"))
            .await
            .map_err(|error| error.to_string()),
    );

    assert_eq!(deleted, (Ok(()), Ok(())));
}

#[test]
fn a_failed_step_does_not_stop_the_steps_after_it_and_the_first_error_is_given() {
    let ran = std::cell::Cell::new(0);
    let step = |result: std::io::Result<()>| {
        ran.set(ran.get() + 1);
        result
    };

    let given = first_error(
        [
            Err(std::io::Error::from(ErrorKind::PermissionDenied)),
            Ok(()),
            Err(std::io::Error::from(ErrorKind::Other)),
        ]
        .into_iter()
        .map(step),
    );

    assert_eq!(
        (given.map_err(|error| error.kind()), ran.get()),
        (Err(ErrorKind::PermissionDenied), 3)
    );
}

#[test]
fn a_copy_from_a_directory_or_from_below_a_file_gives_false_and_makes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let commit = Commit {
        dropped: Arc::new(AtomicBool::new(false)),
        gate: None,
    };
    let staging = root.path().join(STAGING_DIRECTORY);
    let directory = root.path().join("directory");
    std::fs::create_dir(&directory).unwrap();
    let file = root.path().join("file");
    std::fs::write(&file, b"file").unwrap();

    let copied = [directory.clone(), file.join("below")].map(|source| {
        copy_staged(
            &commit,
            &source,
            &staging,
            &root.path().join("target").join("blob"),
        )
        .map_err(|error| error.kind())
    });

    assert_eq!(
        (
            copied,
            root.path().join("target").exists(),
            staging.exists()
        ),
        ([Ok(false), Ok(false)], false, false)
    );
}
