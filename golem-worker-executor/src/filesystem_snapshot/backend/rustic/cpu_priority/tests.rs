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
#[cfg(target_os = "linux")]
use crate::filesystem_snapshot::backend::rustic::test_support::name;
#[cfg(target_os = "linux")]
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    ALWAYS, LONG_DEADLINE, NEVER, config, fixture_tree,
};

use crate::filesystem_snapshot::backend::rustic::test_support::{
    one_file_tree, policy, restored_listing, save_each, store,
};

use crate::filesystem_snapshot::test_support::fixture::{Scratch, Spec, listing, write_tree};
use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{ChangeDetection, FilesystemSnapshotStore, Unlimited};
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::Duration;
use test_r::{test, timeout};

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn the_forget_of_a_delete_runs_its_storage_calls_in_a_rayon_pool_of_its_own() {
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    taken_calls(&calls);

    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();
    let forgets = taken_threads(&calls, "delete", "snapshots/");

    assert_eq!(
        (
            forgets.len(),
            forgets
                .iter()
                .filter(|(_, thread)| !thread.starts_with("fs-snap-delete-"))
                .collect::<Vec<_>>()
        ),
        (1, Vec::<&(String, String)>::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn the_index_load_of_a_restore_runs_its_storage_calls_in_a_rayon_pool_of_its_own() {
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    taken_calls(&calls);

    let into = Scratch::new();
    store
        .restore(&scope, &name("p-1"), into.path(), &Unlimited)
        .await
        .unwrap();
    let index_reads = taken_threads(&calls, "read", "index/");

    assert_eq!(
        (
            index_reads.is_empty(),
            index_reads
                .iter()
                .filter(|(_, thread)| !thread.starts_with("fs-snap-restore-"))
                .collect::<Vec<_>>()
        ),
        (false, Vec::<&(String, String)>::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn the_storage_calls_of_a_save_run_at_nice_19() {
    // The publish of the snapshot file runs on the async runtime after the work, so it keeps
    // the normal priority. The second save reads the config, the index, the snapshot files and
    // the trees of its parent, and writes the added file.
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();
    store
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    std::fs::write(tree.path().join("added.txt"), b"added").unwrap();
    taken_calls(&calls);

    store
        .save(
            &scope,
            &name("p-2"),
            tree.path(),
            Some((&name("p-1"), ChangeDetection::SizeMtime)),
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let work = taken_calls(&calls)
        .into_iter()
        .filter(|(op_label, _, _)| op_label != "publish")
        .collect::<Vec<_>>();

    assert_eq!(
        labels_and_calls_not_at_nice_19(&work),
        (vec!["list", "read", "stat", "write"], Vec::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn the_storage_calls_of_a_prune_run_at_nice_19() {
    // The forget of a delete runs before the ledger read at the normal priority. The ledger calls,
    // the calls of the freed records, the listing of the packs and the claim calls run on the
    // async runtime.
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, ALWAYS, Duration::ZERO));
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(
            &scope,
            &name("p-deleted"),
            deleted_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    store
        .save(
            &scope,
            &name("p-kept"),
            kept_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    taken_calls(&calls);

    store
        .delete(&scope, &[name("p-deleted")], &Unlimited)
        .await
        .unwrap();
    let prune = taken_calls(&calls)
        .into_iter()
        .skip_while(|(op_label, _, _)| op_label != "read_ledger")
        .filter(|(op_label, _, _)| {
            ![
                "read_ledger",
                "write_ledger",
                "list_data",
                "list_claims",
                "write_marker",
                "write_claim",
                "final_marker",
                "delete_marker",
                "delete_claims",
                "write_freed",
                "list_freed",
                "delete_freed",
                "list_ledgers",
                "delete_ledger",
                "refresh_claim",
                "list_claim_directories",
                "list_claim_blobs",
                "read_freed",
                "list_snapshots",
            ]
            .contains(&op_label.as_str())
        })
        .collect::<Vec<_>>();

    assert_eq!(
        labels_and_calls_not_at_nice_19(&prune),
        (vec!["delete", "list", "read", "stat", "write"], Vec::new())
    );
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn the_storage_calls_of_a_restore_run_at_the_nice_value_of_the_process() {
    let process_nice = crate::filesystem_snapshot::backend::rustic::cpu_priority::own_nice();
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, NEVER, Duration::ZERO));
    let scope = new_scope();
    let tree = fixture_tree();
    store
        .save(
            &scope,
            &name("p-1"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    std::mem::take(&mut *calls.lock().unwrap());

    let restored = restored_listing(&store, &scope, &name("p-1")).await;
    let recorded = std::mem::take(&mut *calls.lock().unwrap());

    assert_eq!(
        (
            restored.ok(),
            recorded.is_empty(),
            recorded
                .iter()
                .filter(|(_, _, _, nice)| *nice != process_nice)
                .collect::<Vec<_>>()
        ),
        (
            Some(listing(tree.path())),
            false,
            Vec::<&(String, String, String, i32)>::new()
        )
    );
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn after_saves_and_prunes_the_pools_keep_the_nice_value_of_the_process() {
    // All tasks wait for each other, so each runs on its own thread of the blocking pool, and the
    // idle threads that ran the saves and the prune are among them.
    const TASKS: usize = 16;
    let process_nice = crate::filesystem_snapshot::backend::rustic::cpu_priority::own_nice();
    let store = store(
        Arc::new(InMemoryBlobStorage::new()),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), fixture_tree());
    store
        .save(
            &scope,
            &name("p-deleted"),
            deleted_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    store
        .save(
            &scope,
            &name("p-kept"),
            kept_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    store
        .delete(&scope, &[name("p-deleted")], &Unlimited)
        .await
        .unwrap();

    let barrier = Arc::new(std::sync::Barrier::new(TASKS));
    let blocking = futures::future::join_all((0..TASKS).map(|_| {
        let barrier = barrier.clone();
        tokio::task::spawn_blocking(move || {
            barrier.wait();
            crate::filesystem_snapshot::backend::rustic::cpu_priority::own_nice()
        })
    }))
    .await
    .into_iter()
    .map(Result::unwrap)
    .collect::<Vec<_>>();
    let rayon =
        rayon::broadcast(|_| crate::filesystem_snapshot::backend::rustic::cpu_priority::own_nice());

    assert_eq!(
        (
            blocking.iter().all(|nice| *nice == process_nice),
            rayon.iter().all(|nice| *nice == process_nice),
        ),
        (true, true),
        "{blocking:?} {rayon:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn the_storage_calls_of_the_rayon_workers_of_a_prune_that_repacks_run_at_nice_19() {
    // The deleted snapshot shares a pack with the kept one, so the prune repacks that pack. The
    // prune reads the index files and repacks with rayon, on the workers of the pool of the prune.
    let (storage, calls) = nice_recording_storage();
    let store = store(storage, policy(LONG_DEADLINE, ALWAYS, Duration::ZERO));
    let scope = new_scope();
    let file = |content: &[u8]| Spec::File {
        content: Box::from(content),
        mode: 0o644,
    };
    let both = Scratch::new();
    write_tree(
        both.path(),
        &[
            ("kept.txt", file(b"kept content")),
            ("deleted.txt", file(b"deleted content")),
        ],
    );
    let kept = Scratch::new();
    write_tree(kept.path(), &[("kept.txt", file(b"kept content"))]);
    store
        .save(
            &scope,
            &name("p-both"),
            both.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    store
        .save(
            &scope,
            &name("p-kept"),
            kept.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    std::mem::take(&mut *calls.lock().unwrap());

    store
        .delete(&scope, &[name("p-both")], &Unlimited)
        .await
        .unwrap();
    let recorded = std::mem::take(&mut *calls.lock().unwrap());
    let from_workers = recorded
        .iter()
        .filter(|(_, _, thread, _)| thread.starts_with("fs-snap-prune-"))
        .collect::<Vec<_>>();

    assert_eq!(
        (
            recorded
                .iter()
                .any(|(op, path, _, _)| op == "write" && path.starts_with("data/")),
            from_workers.is_empty(),
            from_workers
                .iter()
                .filter(|(_, _, _, nice)| *nice != 19)
                .count(),
            restored_listing(&store, &scope, &name("p-kept")).await.ok(),
        ),
        (true, false, 0, Some(listing(kept.path())))
    );
}

#[cfg(target_os = "linux")]
#[test]
#[timeout("60s")]
async fn the_global_rayon_pool_keeps_the_nice_value_of_the_process_after_saves_without_their_pool()
{
    // Without its own pool, the rayon work of a save goes to the global pool from a thread at
    // nice 19. The store starts the global pool when it is made, so its threads keep the normal
    // priority also when a save is the first rayon work of the process.
    let process_nice = crate::filesystem_snapshot::backend::rustic::cpu_priority::own_nice();
    let store = RusticSnapshotStore {
        low_priority: crate::filesystem_snapshot::backend::rustic::cpu_priority::LowPriority {
            build_pool: no_pool,
            ..crate::filesystem_snapshot::backend::rustic::cpu_priority::LowPriority::new(
                NonZeroUsize::new(2),
            )
        },
        ..RusticSnapshotStore::new(Arc::new(InMemoryBlobStorage::new()), &config())
    };
    let scope = new_scope();
    let (first, second) = (one_file_tree("first"), fixture_tree());
    store
        .save(
            &scope,
            &name("p-1"),
            first.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    store
        .save(
            &scope,
            &name("p-2"),
            second.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();

    let global =
        rayon::broadcast(|_| crate::filesystem_snapshot::backend::rustic::cpu_priority::own_nice());

    assert!(
        global.iter().all(|nice| *nice == process_nice),
        "{global:?}"
    );
}

/// A pool builder that cannot start a thread.
#[cfg(target_os = "linux")]
fn no_pool(
    _: &str,
    _: Option<NonZeroUsize>,
) -> Result<rayon::ThreadPool, rayon::ThreadPoolBuildError> {
    rayon::ThreadPoolBuilder::new()
        .num_threads(1)
        .spawn_handler(|_| Err(std::io::Error::other("no thread can start here")))
        .build()
}

mod value_rules {
    use crate::filesystem_snapshot::backend::rustic::cpu_priority::{
        LOW_PRIORITY, LowPriority, lower_own_priority, own_nice,
    };
    use pretty_assertions::assert_eq;
    use rayon::{ThreadPool, ThreadPoolBuildError, ThreadPoolBuilder};
    use std::num::NonZeroUsize;
    use test_r::test;

    /// What the work sees: its nice value, its thread name, whether it runs in a rayon pool, and
    /// the thread count of the current rayon pool.
    fn seen() -> anyhow::Result<(i32, Option<String>, bool, usize)> {
        Ok((
            own_nice(),
            std::thread::current().name().map(str::to_string),
            rayon::current_thread_index().is_some(),
            rayon::current_num_threads(),
        ))
    }

    /// A pool builder that cannot start a thread.
    fn no_pool(_: &str, _: Option<NonZeroUsize>) -> Result<ThreadPool, ThreadPoolBuildError> {
        ThreadPoolBuilder::new()
            .num_threads(1)
            .spawn_handler(|_| Err(std::io::Error::other("no thread can start here")))
            .build()
    }

    #[test]
    fn work_at_low_priority_runs_at_nice_19_in_a_pool_of_its_own_and_the_caller_keeps_its_priority()
    {
        let before = own_nice();

        let inside = LowPriority::new(NonZeroUsize::new(3)).run("fs-snap-test", seen);

        assert_eq!(
            (
                inside.ok().map(|(nice, name, in_pool, threads)| (
                    nice,
                    name.is_some_and(|name| name.starts_with("fs-snap-test-")),
                    in_pool,
                    threads
                )),
                own_nice()
            ),
            (Some((LOW_PRIORITY, true, true, 3)), before)
        );
    }

    #[test]
    fn work_whose_priority_step_does_nothing_runs_on_its_own_thread_in_a_pool_of_its_own() {
        // Off Linux, the step that lowers the priority does nothing. The rest of the path is the same
        // on each platform.
        let before = own_nice();
        let low_priority = LowPriority {
            lower: || Ok(()),
            ..LowPriority::new(NonZeroUsize::new(2))
        };

        let inside = low_priority.run("fs-snap-test", seen);

        assert_eq!(
            inside.ok().map(|(nice, name, in_pool, threads)| (
                nice,
                name.is_some_and(|name| name.starts_with("fs-snap-test-")),
                in_pool,
                threads
            )),
            Some((before, true, true, 2))
        );
    }

    #[test]
    fn work_whose_priority_cannot_be_lowered_still_runs_at_the_normal_priority() {
        let before = own_nice();
        let low_priority = LowPriority {
            lower: || Err(std::io::Error::other("the priority cannot change here")),
            ..LowPriority::new(NonZeroUsize::new(2))
        };

        let inside = low_priority.run("fs-snap-test", seen);

        assert_eq!(
            inside.ok().map(|(nice, _, in_pool, _)| (nice, in_pool)),
            Some((before, true))
        );
    }

    #[test]
    fn work_without_its_pool_still_runs_at_nice_19_on_its_own_thread() {
        let low_priority = LowPriority {
            build_pool: no_pool,
            ..LowPriority::new(NonZeroUsize::new(2))
        };

        let inside = low_priority.run("fs-snap-test", seen);

        assert_eq!(
            inside
                .ok()
                .map(|(nice, name, in_pool, _)| (nice, name, in_pool)),
            Some((LOW_PRIORITY, Some("fs-snap-test".to_string()), false))
        );
    }

    #[test]
    fn a_panic_of_the_work_gives_an_error() {
        let done = LowPriority::new(NonZeroUsize::new(1))
            .run::<()>("fs-snap-test", || panic!("the work panics"));

        assert!(done.is_err());
    }

    #[test]
    fn lowering_the_own_priority_gives_ok_and_nice_19() {
        // The thread cannot raise its priority again, so the test lowers a thread of its own.
        let lowered = std::thread::spawn(|| {
            (
                lower_own_priority().map_err(|error| error.to_string()),
                own_nice(),
            )
        })
        .join();

        assert_eq!(lowered.ok(), Some((Ok(()), LOW_PRIORITY)));
    }
}

/// The operation label, the path, and the name and the nice value of the calling thread of each
/// storage call.
#[cfg(target_os = "linux")]
type NiceCalls = Arc<std::sync::Mutex<Vec<(String, String, String, i32)>>>;

/// A storage that records the nice value of the thread of each call.
#[cfg(target_os = "linux")]
fn nice_recording_storage() -> (Arc<ScriptedBlobStorage>, NiceCalls) {
    let calls = NiceCalls::default();
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let calls = calls.clone();
        move |op_label, path| {
            calls
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push((
                    op_label.to_string(),
                    path.display().to_string(),
                    std::thread::current()
                        .name()
                        .unwrap_or_default()
                        .to_string(),
                    crate::filesystem_snapshot::backend::rustic::cpu_priority::own_nice(),
                ));
            Script::Pass
        }
    });
    (storage, calls)
}

/// Takes the recorded calls as the operation label, the path and the nice value.
#[cfg(target_os = "linux")]
fn taken_calls(calls: &NiceCalls) -> Vec<(String, String, i32)> {
    std::mem::take(
        &mut *calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_iter()
    .map(|(op_label, path, _, nice)| (op_label, path, nice))
    .collect()
}

/// Takes the recorded calls with the operation label and a path below the directory, as the path
/// and the name of the thread of each call.
#[cfg(target_os = "linux")]
fn taken_threads(calls: &NiceCalls, op_label: &str, directory: &str) -> Vec<(String, String)> {
    std::mem::take(
        &mut *calls
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    )
    .into_iter()
    .filter(|(op, path, _, _)| op == op_label && path.starts_with(directory))
    .map(|(_, path, thread, _)| (path, thread))
    .collect()
}

/// Gives the operation labels of the calls, and each call that does not run at nice 19.
#[cfg(target_os = "linux")]
fn labels_and_calls_not_at_nice_19(
    calls: &[(String, String, i32)],
) -> (Vec<&str>, Vec<&(String, String, i32)>) {
    let mut labels = calls
        .iter()
        .map(|(op_label, _, _)| op_label.as_str())
        .collect::<Vec<_>>();
    labels.sort_unstable();
    labels.dedup();
    let not_at_nice_19 = calls.iter().filter(|(_, _, nice)| *nice != 19).collect();
    (labels, not_at_nice_19)
}
