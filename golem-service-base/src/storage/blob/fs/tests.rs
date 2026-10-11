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
    Commit, CommitGate, DIRECTORY_ATTEMPTS, FileSystemBlobStorage, PathEntry, STAGING_DIRECTORY,
    STAGING_FILE_AGE, absent_on_not_found, add_files, add_names, blob_path_of, copy_staged,
    directory_attempts, encoded_name, first_error, listed_entry, persist_with,
    remove_unless_dropped, staging_file_is_old, write_if_absent, write_staged,
};
use crate::storage::blob::{
    BlobRangeError, BlobStorage, BlobStorageNamespace, ListedBlob, NormalizedBlobPath, PutIfAbsent,
    normalized_blob_path,
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
fn listed_entry_gives_none_only_for_a_missing_entry_or_a_directory_that_became_a_file() {
    let answers = (
        listed_entry(Ok::<_, std::io::Error>(1)).unwrap(),
        listed_entry::<u8>(Err(not_found())).unwrap(),
        listed_entry::<u8>(Err(std::io::Error::from(ErrorKind::NotADirectory))).unwrap(),
        listed_entry::<u8>(Err(std::io::Error::from(ErrorKind::PermissionDenied)))
            .map_err(|error| error.kind()),
    );

    assert_eq!(
        answers,
        (Some(1), None, None, Err(ErrorKind::PermissionDenied))
    );
}

#[test]
async fn a_whole_and_a_partial_read_of_a_blob_removed_after_its_metadata_check_give_none() {
    let (_root, storage) = storage_with_blobs().await;
    let blob = storage
        .blob_of(
            &namespace(),
            &normalized_blob_path(Path::new("ranges/blob")).unwrap(),
        )
        .unwrap();
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
    let on_disk = |name: &str, entry: PathEntry| {
        root.path()
            .join(encoded_name(name, entry).collect::<PathBuf>())
    };
    std::fs::write(on_disk("kept", PathEntry::Blob), b"kept").unwrap();
    std::fs::write(on_disk("removed", PathEntry::Blob), b"removed").unwrap();
    let gone = on_disk("gone", PathEntry::Directory);
    std::fs::create_dir(&gone).unwrap();
    std::fs::write(
        gone.join(on_disk("file", PathEntry::Blob).file_name().unwrap()),
        b"file",
    )
    .unwrap();
    let entries = std::fs::read_dir(root.path()).unwrap().collect::<Vec<_>>();
    std::fs::remove_file(on_disk("removed", PathEntry::Blob)).unwrap();
    std::fs::remove_dir_all(&gone).unwrap();

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

/// The entries of a directory with the blob `sibling` and the entry `changed`, read while
/// `changed` is a directory when `was_directory` and a file otherwise. Then `changed` changes its
/// type. Gives whether the entries saw `changed` with its first type, and the blob paths that
/// the walk of the entries lists.
#[cfg(unix)]
fn listed_after_a_change_of_type(was_directory: bool) -> (bool, Result<Vec<Box<Path>>, ErrorKind>) {
    let root = tempfile::tempdir().unwrap();
    let on_disk = |name: &str| {
        root.path()
            .join(encoded_name(name, PathEntry::Blob).collect::<PathBuf>())
    };
    let changed = on_disk("changed");
    std::fs::write(on_disk("sibling"), b"sibling").unwrap();
    let make = |directory: bool| match directory {
        true => std::fs::create_dir(&changed).unwrap(),
        false => std::fs::write(&changed, b"file").unwrap(),
    };
    make(was_directory);
    let entries = std::fs::read_dir(root.path()).unwrap().collect::<Vec<_>>();
    let seen = entries.iter().any(|entry| {
        let entry = entry.as_ref().unwrap();
        entry.path() == changed && entry.file_type().unwrap().is_dir() == was_directory
    });
    match was_directory {
        true => std::fs::remove_dir(&changed).unwrap(),
        false => std::fs::remove_file(&changed).unwrap(),
    }
    make(!was_directory);

    let listed = add_files(entries.into_iter(), root.path(), Vec::new())
        .map(|listed| listed.into_iter().map(|blob| blob.path).collect::<Vec<_>>())
        .map_err(|error| error.kind());
    (seen, listed)
}

/// A directory that a write replaces with a blob after the walk saw it as a directory does not fail
/// the listing, and the sibling blob is in it.
#[cfg(unix)]
#[test]
fn a_listing_goes_on_when_a_directory_becomes_a_blob_during_the_walk() {
    assert_eq!(
        listed_after_a_change_of_type(true),
        (true, Ok(vec![Box::from(Path::new("sibling"))]))
    );
}

/// A file that a write replaces with a directory after the walk saw it as a file is not a blob of
/// the listing.
#[cfg(unix)]
#[test]
fn a_listing_never_gives_a_directory_that_was_a_file_when_the_walk_saw_it() {
    assert_eq!(
        listed_after_a_change_of_type(false),
        (true, Ok(vec![Box::from(Path::new("sibling"))]))
    );
}

/// The walk of the names of a directory goes on when the directory of a part of a long name
/// becomes a file after the walk saw it, and the sibling name is in the result.
#[cfg(unix)]
#[test]
fn a_listing_of_names_goes_on_when_a_part_directory_becomes_a_file() {
    let root = tempfile::tempdir().unwrap();
    let long = "l".repeat(200);
    let parts = encoded_name(&long, PathEntry::Blob).collect::<Vec<_>>();
    let part = root.path().join(&parts[0]);
    std::fs::create_dir(&part).unwrap();
    std::fs::write(
        root.path()
            .join(encoded_name("sibling", PathEntry::Blob).collect::<PathBuf>()),
        b"sibling",
    )
    .unwrap();
    let entries = std::fs::read_dir(root.path()).unwrap().collect::<Vec<_>>();
    std::fs::remove_dir(&part).unwrap();
    std::fs::write(&part, b"file").unwrap();

    let listed = add_names(entries.into_iter(), root.path(), Vec::new()).map_err(|e| e.kind());

    assert_eq!(
        (parts.len() > 1, listed),
        (true, Ok(vec![PathBuf::from("sibling")]))
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
    let removed = remove_unless_dropped(&commit, &kept, root.path()).map_err(|error| error.kind());

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
    let full = storage
        .blob_of(
            &snapshots(),
            &crate::storage::blob::normalized_blob_path(blob).unwrap(),
        )
        .unwrap();
    let first_inode = std::fs::metadata(&full).unwrap().ino();
    storage
        .put_raw("test", "put-raw", snapshots(), blob, b"second")
        .await
        .unwrap();
    storage
        .put_raw("test", "put-raw", namespace(), blob, b"other")
        .await
        .unwrap();
    let other = storage
        .blob_of(
            &namespace(),
            &crate::storage::blob::normalized_blob_path(blob).unwrap(),
        )
        .unwrap();
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
    let full = storage
        .blob_of(
            &namespace(),
            &crate::storage::blob::normalized_blob_path(blob).unwrap(),
        )
        .unwrap();
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

/// A source that does not open for another reason than a missing file fails the copy, and the copy
/// makes nothing.
#[cfg(unix)]
#[test]
fn a_copy_from_a_source_that_does_not_open_fails_and_makes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let commit = Commit {
        dropped: Arc::new(AtomicBool::new(false)),
        gate: None,
    };
    let staging = root.path().join(STAGING_DIRECTORY);
    let looped = root.path().join("looped");
    let other = root.path().join("other");
    std::os::unix::fs::symlink(&other, &looped).unwrap();
    std::os::unix::fs::symlink(&looped, &other).unwrap();

    let copied = copy_staged(
        &commit,
        &looped,
        &staging,
        &root.path().join("target").join("blob"),
    );

    assert!(copied.is_err(), "{copied:?}");
    assert_eq!(
        (root.path().join("target").exists(), staging.exists()),
        (false, false)
    );
}

/// A copy and a move give the target the permissions of the source: to an absent target, over a
/// target with other permissions, to another namespace, and in a move, which then removes the
/// source.
#[cfg(unix)]
#[test]
async fn a_copy_and_a_move_keep_the_permissions_of_the_source() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemBlobStorage::new(root.path()).await.unwrap();
    let on_disk = |namespace: BlobStorageNamespace, blob: &str| {
        storage
            .blob_of(&namespace, &normalized_blob_path(Path::new(blob)).unwrap())
            .unwrap()
    };
    let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let set_mode = |path: &Path, mode: u32| {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap()
    };
    let put = async |namespace: BlobStorageNamespace, blob: &str, data: &[u8], mode: u32| {
        storage
            .put_raw("test", "put-raw", namespace.clone(), Path::new(blob), data)
            .await
            .unwrap();
        set_mode(&on_disk(namespace, blob), mode);
    };
    put(namespace(), "source", b"shared payload", 0o640).await;
    put(namespace(), "existing", b"older", 0o604).await;
    put(namespace(), "moved", b"moved payload", 0o640).await;

    storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("source"),
            Path::new("absent"),
        )
        .await
        .unwrap();
    storage
        .copy(
            "test",
            "copy",
            namespace(),
            Path::new("source"),
            Path::new("existing"),
        )
        .await
        .unwrap();
    storage
        .copy_between(
            "test",
            "copy-between",
            namespace(),
            Path::new("source"),
            snapshots(),
            Path::new("other"),
        )
        .await
        .unwrap();
    storage
        .r#move(
            "test",
            "move",
            namespace(),
            Path::new("moved"),
            Path::new("target"),
        )
        .await
        .unwrap();
    let target = |namespace: BlobStorageNamespace, blob: &str| {
        let path = on_disk(namespace, blob);
        (std::fs::read(&path).unwrap(), mode(&path))
    };

    assert_eq!(
        (
            target(namespace(), "absent"),
            target(namespace(), "existing"),
            target(snapshots(), "other"),
            target(namespace(), "target"),
            on_disk(namespace(), "moved").exists(),
            staged_files(root.path()),
        ),
        (
            (b"shared payload".to_vec(), 0o640),
            (b"shared payload".to_vec(), 0o640),
            (b"shared payload".to_vec(), 0o640),
            (b"moved payload".to_vec(), 0o640),
            false,
            Vec::new(),
        )
    );
}

/// The filesystem backend writes each name of a blob path as hex, cut into parts of at most 242
/// bytes. So a `\`, a prefix of Windows and two names that differ only in case keep their
/// meaning on every host, a long name fits the name limit of the filesystem, and no part of a
/// path on disk goes up from its directory. The blob path of a file on disk is the path that
/// made it.
#[test]
fn filesystem_paths_encode_contract_components() {
    let storage = FileSystemBlobStorage {
        root: PathBuf::from("root"),
        before_commit: None,
        after_metadata: None,
    };
    let namespace = BlobStorageNamespace::CustomStorage {
        environment_id: EnvironmentId::new(),
    };
    let namespace_root = storage.namespace_path(&namespace);
    let physical = |logical: &str| {
        storage
            .blob_of(
                &namespace,
                &normalized_blob_path(Path::new(logical)).unwrap(),
            )
            .unwrap()
    };

    for logical in [
        r"photos/animals\cat.png",
        r"photos/C:\cats\kitten.png",
        r"photos/\\server\share\kitten.png",
    ] {
        let physical = physical(logical);
        assert!(
            !physical
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        );
        assert_eq!(
            blob_path_of(&physical, &namespace_root).unwrap(),
            PathBuf::from(logical)
        );
    }

    assert_ne!(
        physical("photos/animals/cat.png"),
        physical(r"photos/animals\cat.png")
    );

    let first = physical("AAA");
    let second = physical("AA[");
    assert!(
        !first
            .to_str()
            .unwrap()
            .eq_ignore_ascii_case(second.to_str().unwrap())
    );

    let long_name = "a".repeat(255);
    let long = physical(&long_name);
    assert!(
        long.strip_prefix(&namespace_root)
            .unwrap()
            .components()
            .all(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .is_some_and(|component| component.len() <= 242)
            })
    );
    assert_eq!(
        blob_path_of(&long, &namespace_root).unwrap(),
        PathBuf::from(long_name)
    );
    assert_eq!(
        storage
            .directory_of(&namespace, &NormalizedBlobPath::root())
            .unwrap(),
        namespace_root
    );
    assert_ne!(
        storage
            .directory_of(
                &namespace,
                &normalized_blob_path(Path::new("photos")).unwrap()
            )
            .unwrap(),
        physical("photos")
    );
}

/// A file below the directory of a namespace that the backend did not name, and a path that ends
/// in a part of a long name that more parts follow, are not files of the blob storage.
#[test]
fn a_file_name_that_the_codec_does_not_give_is_invalid_data() {
    let root = Path::new("root");

    let errors = [
        root.join("plain"),
        root.join("e-61"),
        root.join("b-zz"),
        root.join("c-61"),
        root.join("d-ff"),
    ]
    .map(|physical| blob_path_of(&physical, root).map_err(|error| error.kind()));

    assert_eq!(errors, [(); 5].map(|()| Err(ErrorKind::InvalidData)));
}

/// Tells if the directory of the blob path `path` is on disk.
fn directory_is_on_disk(storage: &FileSystemBlobStorage, path: &str) -> bool {
    storage
        .directory_of(
            &namespace(),
            &normalized_blob_path(Path::new(path)).unwrap(),
        )
        .unwrap()
        .exists()
}

/// A delete of a blob and a `delete_dir` remove each directory that they leave empty, so a later
/// check of a directory does not read the directories of blobs that were deleted. A directory
/// that holds another blob, or that `create_dir` made, stays.
#[test]
async fn a_delete_removes_the_directories_that_it_leaves_empty() {
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemBlobStorage::new(root.path()).await.unwrap();
    let put = |path: &'static str| {
        storage.put_raw("test", "put-raw", namespace(), Path::new(path), b"blob")
    };
    for path in ["a/b/c/blob", "a/sibling", "kept/x/blob", "z/y/blob"] {
        put(path).await.unwrap();
    }
    storage
        .create_dir("test", "create-dir", namespace(), Path::new("kept"))
        .await
        .unwrap();

    for path in ["a/b/c/blob", "kept/x/blob"] {
        storage
            .delete("test", "delete", namespace(), Path::new(path))
            .await
            .unwrap();
    }
    storage
        .delete_dir("test", "delete-dir", namespace(), Path::new("z/y"))
        .await
        .unwrap();

    assert_eq!(
        ["a/b/c", "a/b", "a", "kept/x", "kept", "z/y", "z"]
            .map(|path| (path, directory_is_on_disk(&storage, path))),
        [
            ("a/b/c", false),
            ("a/b", false),
            ("a", true),
            ("kept/x", false),
            ("kept", true),
            ("z/y", false),
            ("z", false),
        ]
    );
}

/// A delete removes a directory that it leaves empty, and that can be the directory that a write
/// made for its blob a moment before. The write then makes the directory again, and the blob
/// lands.
#[test]
async fn a_write_lands_when_a_remove_takes_its_directory_before_the_rename() {
    let root = tempfile::tempdir().unwrap();
    let storage = FileSystemBlobStorage::new(root.path()).await.unwrap();
    storage
        .put_raw(
            "test",
            "put-raw",
            snapshots(),
            Path::new("source"),
            b"source",
        )
        .await
        .unwrap();
    let directory = |namespace: BlobStorageNamespace, path: &str| {
        storage
            .directory_of(&namespace, &normalized_blob_path(Path::new(path)).unwrap())
            .unwrap()
    };
    let removing = |directory: PathBuf| {
        let removed = Arc::new(AtomicBool::new(false));
        FileSystemBlobStorage {
            root: storage.root.clone(),
            before_commit: Some(CommitGate(Arc::new(move || {
                if !removed.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    std::fs::remove_dir(&directory).unwrap();
                }
            }))),
            after_metadata: None,
        }
    };
    let blob = Path::new("dir/blob");

    let if_absent = removing(directory(namespace(), "dir"))
        .put_raw_if_absent("test", "put-if-absent", namespace(), blob, b"if-absent")
        .await
        .map_err(|error| error.to_string());
    let staged = removing(directory(snapshots(), "dir"))
        .put_raw("test", "put-raw", snapshots(), blob, b"staged")
        .await
        .map_err(|error| error.to_string());
    let copied = removing(directory(snapshots(), "other"))
        .copy(
            "test",
            "copy",
            snapshots(),
            Path::new("source"),
            Path::new("other/copy"),
        )
        .await
        .map_err(|error| error.to_string());
    let read = |namespace: BlobStorageNamespace, path: &'static str| {
        let storage = &storage;
        async move {
            storage
                .get_raw("test", "get-raw", namespace, Path::new(path))
                .await
                .unwrap()
        }
    };

    assert_eq!(
        (
            if_absent,
            staged,
            copied,
            read(namespace(), "dir/blob").await,
            read(snapshots(), "dir/blob").await,
            read(snapshots(), "other/copy").await,
        ),
        (
            Ok(PutIfAbsent::Written),
            Ok(()),
            Ok(()),
            Some(b"if-absent".to_vec()),
            Some(b"staged".to_vec()),
            Some(b"source".to_vec()),
        )
    );
}

/// A remove of an empty directory can take away a directory above the directory of a write while
/// the write makes it, and the make then gives `NotFound`. The write makes the directory again, as
/// for a directory that goes before the file lands, and gives `NotFound` after the last attempt.
#[test]
fn a_write_makes_its_directory_again_when_a_remove_takes_a_directory_above_it() {
    let make_that_fails = |failures: usize| {
        let mut made = 0;
        move |_: &Path| {
            made += 1;
            if made <= failures {
                Err(not_found())
            } else {
                Ok(())
            }
        }
    };
    let target = Path::new("dir/blob");

    let placed = directory_attempts(target, make_that_fails(1), || Ok("placed"))
        .map_err(|error| error.kind());
    let gone = directory_attempts(target, make_that_fails(DIRECTORY_ATTEMPTS), || Ok("placed"))
        .map_err(|error| error.kind());

    assert_eq!((placed, gone), (Ok("placed"), Err(ErrorKind::NotFound)));
}

/// A call that is dropped while its write makes the directory of the target never lands: the
/// write checks the drop right before the step that names the target, after the directory.
#[test]
fn a_write_dropped_while_it_makes_its_directory_never_lands() {
    let root = tempfile::tempdir().unwrap();
    let dropped = Arc::new(AtomicBool::new(false));
    let commit = Commit {
        dropped: dropped.clone(),
        gate: None,
    };
    let staged = tempfile::NamedTempFile::new_in(root.path()).unwrap();
    let target = root.path().join("dir/blob");

    let written = persist_with(
        &commit,
        staged,
        &target,
        |directory| {
            std::fs::create_dir_all(directory)?;
            dropped.store(true, std::sync::atomic::Ordering::Release);
            Ok(())
        },
        |file| file.persist(&target).map(|_| ()),
    )
    .map_err(|error| error.kind());

    assert_eq!(
        (written, target.exists()),
        (Err(ErrorKind::Interrupted), false)
    );
}
