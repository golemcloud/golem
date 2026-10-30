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

use crate::filesystem_snapshot::backend::rustic::RusticSnapshotStore;
use crate::filesystem_snapshot::backend::rustic::pruning::PruneSettings;
use crate::filesystem_snapshot::backend::rustic::pruning::eligibility::Percent;
use crate::filesystem_snapshot::backend::rustic::store_policy::StorePolicy;
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    LONG_DEADLINE, NEVER, is_failed, listed_names, name, one_file_tree, one_run, policy,
    publish_bound_for, repository_key, restored_listing, save_numbered, store, store_with_deadline,
};

use crate::filesystem_snapshot::clock::SystemClock;
use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{
    CallError, FilesystemSnapshotStore, ReadError, RestoreFailure, Unlimited,
};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use test_r::{test, timeout};

use golem_service_base::storage::blob::BlobStorage;
#[test]
#[timeout("120s")]
async fn scope_snapshots_reads_the_snapshot_files_concurrently_and_in_order() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_numbered(
        &store_with_deadline(inner.clone(), STORAGE_CALL_DEADLINE),
        &scope,
        0..40,
    )
    .await;
    let counting = Arc::new(
        crate::filesystem_snapshot::backend::rustic::test_support::counting::CountingBlobStorage::new(
            inner,
            Duration::from_millis(20),
        ),
    );
    let store_with_deadline = store_with_deadline(counting.clone(), STORAGE_CALL_DEADLINE);

    let listed = store_with_deadline
        .list(&scope, &Unlimited)
        .await
        .unwrap()
        .iter()
        .map(|(name, _)| name.as_str().to_string())
        .collect::<Vec<_>>();

    assert_eq!(
        listed,
        (0..40)
            .rev()
            .map(|index| format!("p-{index}"))
            .collect::<Vec<_>>()
    );
    assert_eq!(counting.count("read", "snapshots"), 40);
    let at_once = counting.most_snapshot_reads_at_once();
    assert!(
        (2..=crate::filesystem_snapshot::backend::rustic::blob_backend::SNAPSHOT_FILE_READS)
            .contains(&at_once),
        "{at_once} reads of snapshot files ran at once"
    );
}

#[test]
#[timeout("120s")]
async fn the_reads_ahead_of_a_store_take_the_read_slots_that_the_store_got() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_numbered(
        &store_with_deadline(inner.clone(), STORAGE_CALL_DEADLINE),
        &scope,
        0..12,
    )
    .await;
    let counting = Arc::new(
        crate::filesystem_snapshot::backend::rustic::test_support::counting::CountingBlobStorage::new(
            inner,
            Duration::from_millis(20),
        ),
    );
    let store_with_deadline = store_with_deadline(counting.clone(), STORAGE_CALL_DEADLINE)
        .reading_under(Arc::new(tokio::sync::Semaphore::new(2)));

    let listed = store_with_deadline.list(&scope, &Unlimited).await.unwrap();

    assert_eq!(
        (
            listed.len(),
            counting.count("read", "snapshots"),
            counting.most_snapshot_reads_at_once() <= 2
        ),
        (12, 12, true)
    );
}

#[test]
#[timeout("120s")]
async fn a_read_ahead_that_fails_reads_no_file_twice_and_starts_no_new_read() {
    // Each read of a snapshot file is refused, with an answer that a new try can change.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_numbered(
        &store_with_deadline(inner.clone(), STORAGE_CALL_DEADLINE),
        &scope,
        0..40,
    )
    .await;
    let storage = ScriptedBlobStorage::new(inner, |op_label, path| {
        if op_label == "read" && path.starts_with("snapshots") {
            Script::Refuse
        } else {
            Script::Pass
        }
    });
    let store_with_deadline = RusticSnapshotStore::with_policy(
        storage.clone(),
        repository_key(),
        StorePolicy {
            deadline: STORAGE_CALL_DEADLINE,
            save_threads: None,
            restore_reader_threads: NonZeroUsize::new(6).unwrap(),
            prune: PruneSettings {
                fast_repack: true,
                keep_delete: Duration::from_secs(15 * 60),
                threshold: Percent(u16::MAX),
            },
            retry: one_run(),
            publish_bound: publish_bound_for(STORAGE_CALL_DEADLINE, Duration::from_secs(15 * 60)),
            in_call_tries: crate::filesystem_snapshot::backend::rustic::blob_io::IN_CALL_TRIES,
        },
        Arc::new(SystemClock),
    );

    let listed = store_with_deadline.list(&scope, &Unlimited).await;
    let reads = storage
        .calls()
        .into_iter()
        .filter(|(op_label, path)| *op_label == "read" && path.starts_with("snapshots"))
        .fold(
            std::collections::HashMap::<String, usize>::new(),
            |mut reads, (_, path)| {
                *reads.entry(path).or_default() += 1;
                reads
            },
        );

    assert!(
        matches!(listed, Err(CallError::Failed(_))),
        "a listing whose snapshot file reads fail gives Failed: {listed:?}"
    );
    assert!(
        reads.values().all(|count| *count
            <= crate::filesystem_snapshot::backend::rustic::blob_io::IN_CALL_TRIES as usize),
        "a snapshot file was read more than its tries: {reads:?}"
    );
    assert!(
        reads.len()
            <= crate::filesystem_snapshot::backend::rustic::blob_backend::SNAPSHOT_FILE_READS,
        "{} snapshot files were read",
        reads.len()
    );
}

#[test]
#[timeout("60s")]
async fn a_snapshot_file_that_fails_its_check_is_left_out_of_list_and_makes_an_unknown_name_corrupt()
 {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(
            &scope,
            &name("p-kept"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    storage
        .put_raw(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new(&format!("snapshots/{}", "ab".repeat(32))),
            b"not a snapshot",
        )
        .await
        .unwrap();

    let names = listed_names(&store, &scope).await;
    let kept = store.stat(&scope, &name("p-kept")).await;
    let unknown = store.stat(&scope, &name("p-unknown")).await;
    let restore_unknown = restored_listing(&store, &scope, &name("p-unknown")).await;

    assert!(matches!(unknown, Err(ReadError::Corrupt(_))), "{unknown:?}");
    assert!(
        matches!(restore_unknown, Err(RestoreFailure::Corrupt(_))),
        "{restore_unknown:?}"
    );
    assert_eq!(
        (names, kept.map(|info| info.is_some()).ok()),
        (vec!["p-kept".to_string()], Some(true))
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_read_of_a_snapshot_file_fails_stat_and_list_with_failed() {
    let refuse = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refuse = refuse.clone();
        move |op_label, path| {
            if refuse.load(Ordering::SeqCst) && op_label == "read" && path.starts_with("snapshots")
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(
            &scope,
            &name("p-kept"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    refuse.store(true, Ordering::SeqCst);

    let stat = store.stat(&scope, &name("p-kept")).await;
    let list = store.list(&scope, &Unlimited).await;

    assert!(stat.as_ref().is_err_and(is_failed), "{stat:?}");
    assert!(list.as_ref().is_err_and(is_failed), "{list:?}");
}

#[test]
#[timeout("60s")]
async fn a_snapshot_file_that_is_gone_after_the_listing_is_left_out() {
    let gone = format!("snapshots/{}", "cd".repeat(32));
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let gone = gone.clone();
        move |op_label, path| {
            if op_label == "read" && path == Path::new(&gone) {
                Script::Vanish
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = one_file_tree("kept");
    store
        .save(
            &scope,
            &name("p-kept"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    inner
        .put_raw(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new(&gone),
            b"listed",
        )
        .await
        .unwrap();

    let unknown = store.stat(&scope, &name("p-unknown")).await;
    let names = listed_names(&store, &scope).await;

    assert!(matches!(unknown, Ok(None)), "{unknown:?}");
    assert_eq!(names, vec!["p-kept".to_string()]);
}
