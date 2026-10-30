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

use super::claim::{ClaimChoice, ClaimEntry, next_claim, parse_claim_entry};
use super::eligibility::Percent;
use super::records::PruneLedger;
use super::test_support::freed;
use super::timing::{CLOCK_SKEW_MARGIN, claim_hold};
use super::{PruneReport, leaves_marked_packs, prune_options};
use crate::filesystem_snapshot::backend::rustic::RusticSnapshotStore;
use crate::filesystem_snapshot::backend::rustic::failure_classification::CallFailure;
use crate::filesystem_snapshot::backend::rustic::failure_classification::call_failure;
use crate::filesystem_snapshot::backend::rustic::pruning::PruneSettings;
use crate::filesystem_snapshot::backend::rustic::repository_layout::LEDGERS_PATH;
use crate::filesystem_snapshot::backend::rustic::store_policy::StorePolicy;
use crate::filesystem_snapshot::backend::rustic::test_support::holding::{
    holding_storage, reached_deadline,
};
use crate::filesystem_snapshot::backend::rustic::test_support::scripted::{
    Script, ScriptedBlobStorage,
};
use crate::filesystem_snapshot::backend::rustic::test_support::{
    ALWAYS, LIMIT, LONG_DEADLINE, NEVER, REACH_LIMIT, SHORT_DEADLINE, backend_of, blobs, calls_of,
    data_packs, dropped_within_limit, eventually, fixture_tree, key, ledger, listed_names, name,
    one_file_tree, policy, polled_until, prunes, repository_fixture_tree, repository_key,
    restored_listing, save_each, snapshot_files, store, store_over, store_with_deadline,
    stored_paths, written_times,
};

use crate::filesystem_snapshot::backend::rustic::tracked_tasks::run_blocking;
use crate::filesystem_snapshot::test_support::clock::TestClock;
use crate::filesystem_snapshot::test_support::fixture::{Scratch, Spec, listing, write_tree};
use crate::filesystem_snapshot::test_support::{never_cancelled, new_scope};
use crate::filesystem_snapshot::{AgentSnapshots, FilesystemSnapshotStore, Unlimited};
use crate::services::golem_config::DEFAULT_FILESYSTEM_SNAPSHOT_STORAGE_CALL_DEADLINE as STORAGE_CALL_DEADLINE;
use futures::{StreamExt, TryStreamExt};
use golem_service_base::storage::blob::BlobStorage;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use test_r::{test, timeout};
use tokio_util::task::AbortOnDropHandle;

/// Gives the length of the span in seconds.
fn seconds(span: rustic_core::jiff::Span) -> f64 {
    span.total(rustic_core::jiff::Unit::Second).unwrap()
}

#[test]
fn the_off_side_of_each_prune_setting_goes_into_its_rustic_option() {
    let prune = prune_options(&PruneSettings {
        fast_repack: false,
        keep_delete: Duration::from_secs(23 * 3600),
        ..PruneSettings::default()
    })
    .unwrap();
    assert_eq!(
        (prune.fast_repack, seconds(prune.keep_delete)),
        (false, 82_800.0)
    );
}

#[test]
fn each_prune_setting_goes_into_its_rustic_option() {
    let prune = prune_options(&PruneSettings {
        fast_repack: true,
        keep_delete: Duration::ZERO,
        ..PruneSettings::default()
    })
    .unwrap();
    assert_eq!(
        (
            prune.fast_repack,
            seconds(prune.keep_delete),
            format!("{:?}", (prune.max_unused, prune.max_repack))
        ),
        (
            true,
            0.0,
            format!(
                "{:?}",
                (
                    rustic_core::LimitOption::Percentage(5),
                    rustic_core::LimitOption::Percentage(10)
                )
            )
        )
    );
}

/// Makes the ledger one entry with the marked packs of `ledger` and a time before it by two hours,
/// which is more than the grace period of each test and the margin for clock skew.
async fn age_ledger<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    agent: &AgentSnapshots,
    ledger: &PruneLedger,
) {
    let back = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap() + 2 * 3_600_000;
    let aged = ledger
        .last_prune
        .map_or(0, |last| last.to_millis().saturating_sub(back));
    storage
        .delete_dir(
            "test",
            "test",
            agent.namespace().clone(),
            Path::new(LEDGERS_PATH),
        )
        .await
        .unwrap();
    put_ledger_entry(
        storage,
        agent,
        &format!("{aged}-{}-aged", u8::from(ledger.awaiting_removal)),
    )
    .await;
}

/// Moves each marker of the claims of the scope back by two hours and the margin, so no marker
/// holds its ledger.
async fn age_claims<S: BlobStorage + 'static>(storage: &Arc<S>, agent: &AgentSnapshots) {
    let stale = golem_common::model::Timestamp::now_utc()
        .to_millis()
        .saturating_sub(2 * 3_600_000 + 2 * 60_000);
    let markers = storage
        .list_blobs_below(
            "test",
            "test",
            agent.namespace().clone(),
            Path::new("golem/prune-claims"),
        )
        .await
        .unwrap()
        .iter()
        .filter_map(|blob| {
            let entry = parse_claim_entry(blob.path.file_name()?.to_str()?)?;
            match entry {
                ClaimEntry::Marker(number, _) => Some((blob.path.clone(), number)),
                ClaimEntry::Claim(_) => None,
            }
        })
        .collect::<Vec<_>>();
    futures::stream::iter(markers)
        .for_each(|(path, number)| {
            let (storage, agent) = (storage.clone(), agent.clone());
            async move {
                storage
                    .delete("test", "test", agent.namespace().clone(), &path)
                    .await
                    .unwrap();
                let aged = path
                    .parent()
                    .unwrap_or(Path::new(""))
                    .join(format!("{number}@{stale}-aged"));
                storage
                    .put_raw("test", "test", agent.namespace().clone(), &aged, b"")
                    .await
                    .unwrap();
            }
        })
        .await;
}

/// Gives the entries of the claims of the scope.
async fn claim_entries(storage: &ScriptedBlobStorage, agent: &AgentSnapshots) -> Vec<ClaimEntry> {
    blobs(storage, agent.namespace(), "golem/prune-claims/")
        .await
        .iter()
        .filter_map(|path| parse_claim_entry(Path::new(path).file_name()?.to_str()?))
        .collect()
}

/// Gives the number of markers of the claims of the scope.
async fn markers(storage: &ScriptedBlobStorage, agent: &AgentSnapshots) -> usize {
    claim_entries(storage, agent)
        .await
        .iter()
        .filter(|entry| matches!(entry, ClaimEntry::Marker(..)))
        .count()
}

/// Writes a ledger entry with the name.
async fn put_ledger_entry<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    agent: &AgentSnapshots,
    name: &str,
) {
    storage
        .put_raw(
            "test",
            "test",
            agent.namespace().clone(),
            &Path::new(LEDGERS_PATH).join(name),
            b"",
        )
        .await
        .unwrap();
}

#[test]
#[timeout("120s")]
async fn a_prune_whose_tree_pack_reads_get_no_answer_fails_and_stops_its_threads() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let tree = repository_fixture_tree();
    store_with_deadline(inner.clone(), STORAGE_CALL_DEADLINE)
        .save(
            &scope,
            &name("p-first"),
            tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    // A prune reads the trees of the snapshots, and each tree read is a full read of a pack.
    let (storage, _gate, dropped) = holding_storage(inner, |op_label, path| {
        op_label == "read" && path.starts_with("data")
    });

    let pruned = tokio::time::timeout(
        REACH_LIMIT,
        prune_with(
            storage,
            &scope,
            SHORT_DEADLINE,
            PruneSettings {
                fast_repack: true,
                keep_delete: Duration::ZERO,
                ..PruneSettings::default()
            },
        ),
    )
    .await;
    let stopped = dropped_within_limit(dropped).await;

    assert_eq!(
        (
            pruned
                .ok()
                .map(|result| result.is_err_and(|error| reached_deadline(error.as_ref()))),
            stopped
        ),
        (Some(true), true)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_after_a_delete_deletes_the_packs_of_that_name_and_the_other_name_restores() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let store_with_deadline = store_with_deadline(inner.clone(), STORAGE_CALL_DEADLINE);
    let first_tree = named_file_tree("first.txt", "only in the first tree");
    let second_tree = named_file_tree("second.txt", "only in the second tree");
    store_with_deadline
        .save(
            &scope,
            &name("p-first"),
            first_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let first_packs = pack_paths(&inner, &scope).await;
    store_with_deadline
        .save(
            &scope,
            &name("p-second"),
            second_tree.path(),
            None,
            never_cancelled(),
            &Unlimited,
        )
        .await
        .unwrap();
    let second_packs = pack_paths(&inner, &scope)
        .await
        .into_iter()
        .filter(|path| !first_packs.contains(path))
        .collect::<Vec<_>>();
    let into = Scratch::new();

    // The first prune marks the packs that only the deleted name used, and the second prune
    // deletes them, because they stay marked for no time.
    store_with_deadline
        .delete(&scope, &[name("p-first")], &Unlimited)
        .await
        .unwrap();
    let settings = PruneSettings {
        fast_repack: true,
        keep_delete: Duration::ZERO,
        ..PruneSettings::default()
    };
    prune_with(inner.clone(), &scope, STORAGE_CALL_DEADLINE, settings)
        .await
        .unwrap();
    prune_with(inner.clone(), &scope, STORAGE_CALL_DEADLINE, settings)
        .await
        .unwrap();
    let restored = store_with_deadline
        .restore(&scope, &name("p-second"), into.path(), &Unlimited)
        .await;

    assert_eq!(
        (
            first_packs.is_empty(),
            second_packs.is_empty(),
            pack_paths(&inner, &scope).await,
            restored.is_ok(),
            listing(into.path()),
        ),
        (
            false,
            false,
            second_packs.clone(),
            true,
            listing(second_tree.path())
        )
    );
}

#[test]
#[timeout("60s")]
async fn two_prunes_without_a_grace_period_under_the_limits_of_rustic_give_back_the_data_that_no_snapshot_uses()
 {
    // The first save puts both files in one pack. The second save rewrites one of them. After the
    // delete, that pack holds a used and an unused blob. The used blob is far below 10% of the
    // repository, and the unused blob is above 5% of the used data. So a prune under the limits of
    // rustic repacks it.
    let prune_twice = async |fast_repack| {
        let storage = Arc::new(InMemoryBlobStorage::new());
        let scope = new_scope();
        let store_with_deadline = store_with_deadline(storage.clone(), STORAGE_CALL_DEADLINE);
        let tree = Scratch::new();
        std::fs::write(tree.path().join("kept.txt"), b"kept in both snapshots").unwrap();
        std::fs::write(
            tree.path().join("changed.txt"),
            b"only in the first snapshot",
        )
        .unwrap();
        store_with_deadline
            .save(
                &scope,
                &name("p-first"),
                tree.path(),
                None,
                never_cancelled(),
                &Unlimited,
            )
            .await
            .unwrap();
        let first_packs = data_packs(&storage, &scope).await;
        std::fs::write(tree.path().join("changed.txt"), b"in the second snapshot").unwrap();
        store_with_deadline
            .save(
                &scope,
                &name("p-second"),
                tree.path(),
                None,
                never_cancelled(),
                &Unlimited,
            )
            .await
            .unwrap();
        store_with_deadline
            .delete(&scope, &[name("p-first")], &Unlimited)
            .await
            .unwrap();
        let settings = PruneSettings {
            fast_repack,
            keep_delete: Duration::ZERO,
            ..PruneSettings::default()
        };

        let stored = |paths: Vec<String>| {
            first_packs
                .iter()
                .map(|pack| paths.iter().any(|path| path.ends_with(&**pack)))
                .collect::<Vec<_>>()
        };

        let marked = prune_with(storage.clone(), &scope, STORAGE_CALL_DEADLINE, settings)
            .await
            .unwrap()
            .unwrap();
        let after_mark = stored(pack_paths(&storage, &scope).await);
        let deleted = prune_with(storage.clone(), &scope, STORAGE_CALL_DEADLINE, settings)
            .await
            .unwrap()
            .unwrap();
        let after_delete = stored(pack_paths(&storage, &scope).await);
        let into = Scratch::new();
        store_with_deadline
            .restore(&scope, &name("p-second"), into.path(), &Unlimited)
            .await
            .unwrap();
        (
            marked.packs_repacked,
            first_packs.len(),
            after_mark,
            after_delete,
            deleted.packs_repacked,
            listing(into.path()) == listing(tree.path()),
        )
    };
    // The first prune repacks the data pack of the first save and marks it. The second prune
    // deletes it.
    let expected = (1, 1, vec![true], vec![false], 0, true);

    assert_eq!(
        (prune_twice(false).await, prune_twice(true).await),
        (expected.clone(), expected)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_of_a_scope_without_a_repository_gives_nothing() {
    let storage = Arc::new(InMemoryBlobStorage::new());

    let pruned = prune_with(
        storage,
        &new_scope(),
        STORAGE_CALL_DEADLINE,
        PruneSettings {
            fast_repack: false,
            keep_delete: Duration::ZERO,
            ..PruneSettings::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(pruned, None);
}

#[test]
#[timeout("60s")]
async fn a_delete_past_the_threshold_prunes_and_the_packs_go_after_the_grace_period() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
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
    let packs_before = blobs(&*storage, scope.namespace(), "data/").await;

    store
        .delete(&scope, &[name("p-deleted")], &Unlimited)
        .await
        .unwrap();
    let after_first = ledger(&storage, &scope).await;
    let after_first_freed = freed(&storage, &scope).await;
    // The margin for clock skew keeps the next prune back, so the ledger moves back.
    age_ledger(&storage, &scope, &after_first).await;
    store
        .delete(&scope, &[name("p-none")], &Unlimited)
        .await
        .unwrap();
    let packs_after = blobs(&*storage, scope.namespace(), "data/").await;

    assert_eq!(
        (
            after_first_freed,
            after_first.last_prune.is_some(),
            after_first.awaiting_removal,
            packs_after.len() < packs_before.len(),
            packs_after.iter().all(|pack| packs_before.contains(pack)),
            restored_listing(&store, &scope, &name("p-kept")).await.ok(),
        ),
        (0, true, true, true, true, Some(listing(kept_tree.path())))
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_within_the_grace_period_does_not_list_the_packs() {
    // The ledger has no marked packs, so only the grace period keeps the first delete from a
    // listing. The second delete comes after the grace period and lists the packs one time.
    let grace = Duration::from_secs(3600);
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(storage.clone(), policy(LONG_DEADLINE, NEVER, grace));
    let scope = new_scope();
    let (first, second) = (one_file_tree("first"), one_file_tree("second"));
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
    let now = golem_common::model::Timestamp::now_utc();
    set_last_prune(&storage, &scope, now).await;
    let before_first = storage.calls().len();

    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();
    let before_second = storage.calls().len();
    set_last_prune(
        &storage,
        &scope,
        golem_common::model::Timestamp::from(now.to_millis().saturating_sub(2 * 3_600_000)),
    )
    .await;
    store
        .delete(&scope, &[name("p-2")], &Unlimited)
        .await
        .unwrap();
    let calls = storage.calls();

    assert_eq!(
        (
            data_listings(&calls[before_first..before_second]),
            data_listings(&calls[before_second..]),
        ),
        (0, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_listing_of_the_packs_leaves_the_delete_its_success_and_records_no_prune() {
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "list_data" {
            Script::Refuse
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), one_file_tree("kept"));
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

    let deleted = store.delete(&scope, &[name("p-deleted")], &Unlimited).await;
    let after = ledger(&storage, &scope).await;
    let after_freed = freed(&storage, &scope).await;

    assert!(matches!(&deleted, Ok(())), "{deleted:?}");
    assert_eq!((after_freed > 0, after.last_prune), (true, None));
}

#[test]
#[timeout("120s")]
async fn a_delete_that_paused_after_its_ledger_read_does_not_put_back_the_old_ledger() {
    // The gate holds the first delete after its record write and its ledger read. The second
    // delete prunes to its end. The first delete then goes on with the ledger that it read.
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let held = held.clone();
        move |op_label, _| {
            if op_label == "list_freed" && !held.swap(true, Ordering::SeqCst) {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let paused = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    // The second delete lists the freed records too. So the gate would hold it for ever when the
    // first delete did not reach its listing first.
    let paused_held = polled_until(REACH_LIMIT, || held.load(Ordering::SeqCst)).await;
    assert!(
        paused_held,
        "the first delete did not reach its listing of the freed records"
    );

    let pruned = store.delete(&scope, &[name("p-2")], &Unlimited).await;
    let after_prune = ledger(&storage, &scope).await;
    storage.open_gate();
    let paused = tokio::time::timeout(LIMIT, paused).await;
    let after_all = ledger(&storage, &scope).await;

    assert!(pruned.is_ok(), "{pruned:?}");
    assert!(matches!(paused, Ok(Ok(Ok(())))), "{paused:?}");
    assert_eq!(
        (
            paused_held,
            prunes(&storage.calls()),
            after_prune.last_prune.is_some(),
            after_all,
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, 1, true, after_prune, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_record_that_a_delete_adds_during_a_prune_stays_for_the_next_prune() {
    // The gate holds the prune at its listing of the packs, and a record comes in meanwhile.
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let pruning = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let prune_held = eventually(|| held.load(Ordering::SeqCst)).await;

    storage
        .put_raw(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new("golem/prune-freed/7-late"),
            GONE_SNAPSHOT.as_bytes(),
        )
        .await
        .unwrap();
    storage.open_gate();
    let pruned = tokio::time::timeout(LIMIT, pruning).await;

    assert!(matches!(pruned, Ok(Ok(Ok(())))), "{pruned:?}");
    assert_eq!(
        (
            prune_held,
            blobs(&*storage, scope.namespace(), "golem/prune-freed/").await,
            freed(&storage, &scope).await,
        ),
        (true, vec!["golem/prune-freed/7-late".to_string()], 7)
    );
}

#[test]
#[timeout("60s")]
async fn a_late_older_ledger_entry_does_not_win() {
    // The newer entry is inside the grace period, so a delete does not prune.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let now = golem_common::model::Timestamp::now_utc().to_millis();
    let newer = now.saturating_sub(60_000);
    put_ledger_entry(&storage, &scope, &format!("{newer}-0-newer")).await;
    put_ledger_entry(
        &storage,
        &scope,
        &format!("{}-1-older", now.saturating_sub(2 * 3_600_000)),
    )
    .await;

    let read = ledger(&storage, &scope).await;
    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();

    assert_eq!(
        (
            read.last_prune.map(|last| last.to_millis()),
            read.awaiting_removal,
            prunes(&storage.calls()),
        ),
        (Some(newer), false, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_deletes_the_older_ledger_entries_and_keeps_a_newer_one() {
    // The gate holds the prune at its listing of the packs. An older and a newer entry come in
    // meanwhile.
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let pruning = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let prune_held = eventually(|| held.load(Ordering::SeqCst)).await;

    let newer = format!(
        "{}-0-newer",
        golem_common::model::Timestamp::now_utc().to_millis() + 60_000
    );
    put_ledger_entry(&storage, &scope, "1000-0-older").await;
    put_ledger_entry(&storage, &scope, &newer).await;
    storage.open_gate();
    let pruned = tokio::time::timeout(LIMIT, pruning).await;
    let entries = blobs(&*storage, scope.namespace(), "golem/prune-ledgers/").await;

    assert!(matches!(pruned, Ok(Ok(Ok(())))), "{pruned:?}");
    assert_eq!(
        (
            prune_held,
            entries.len(),
            entries.contains(&format!("golem/prune-ledgers/{newer}")),
            entries.contains(&"golem/prune-ledgers/1000-0-older".to_string()),
        ),
        (true, 2, true, false)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_refreshes_the_claim_with_its_own_number() {
    // Old claims 0 and 1 with markers older than the hold stay in the claim directory, so the
    // delete takes claim 2. The gate holds the prune at its listing of the packs while the claim
    // gets new markers.
    let grace = Duration::from_millis(400);
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let old = golem_common::model::Timestamp::now_utc()
        .to_millis()
        .saturating_sub(2 * 3_600_000);
    futures::stream::iter([
        "golem/prune-claims/none/0".to_string(),
        format!("golem/prune-claims/none/0@{old}-old"),
        "golem/prune-claims/none/1".to_string(),
        format!("golem/prune-claims/none/1@{old}-old"),
    ])
    .for_each(|path| {
        let (storage, scope) = (storage.clone(), scope.clone());
        async move {
            storage
                .put_raw(
                    "test",
                    "test",
                    scope.namespace().clone(),
                    Path::new(&path),
                    &[],
                )
                .await
                .unwrap();
        }
    })
    .await;
    let pruning = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let refreshes = || {
        storage
            .calls()
            .iter()
            .filter(|(op_label, _)| *op_label == "refresh_claim")
            .count()
    };
    let refreshed = eventually(|| held.load(Ordering::SeqCst) && refreshes() >= 2).await;

    let entries = claim_entries(&storage, &scope).await;
    storage.open_gate();
    let pruned = tokio::time::timeout(LIMIT, pruning).await;
    let young = entries
        .iter()
        .filter_map(|entry| match entry {
            ClaimEntry::Marker(number, at) if at.to_millis() > old => Some(*number),
            _ => None,
        })
        .collect::<Vec<_>>();
    let live_markers = entries
        .iter()
        .filter(|entry| matches!(entry, ClaimEntry::Marker(2, _)))
        .cloned()
        .collect::<Vec<_>>();
    let hold = claim_hold(grace, LONG_DEADLINE);

    assert!(matches!(pruned, Ok(Ok(Ok(())))), "{pruned:?}");
    assert_eq!(
        (
            refreshed,
            entries.contains(&ClaimEntry::Claim(2)),
            young.len() >= 3,
            young.iter().all(|number| *number == 2),
            next_claim(
                &live_markers,
                golem_common::model::Timestamp::now_utc(),
                hold
            ),
        ),
        (true, true, true, true, ClaimChoice::Held)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_that_finds_a_snapshot_file_gone_at_each_attempt_after_refreshes_deletes_each_marker_of_its_claim()
 {
    // Each listing of the snapshot files by the prune takes 250 ms, and the grace period gives a
    // new marker each 100 ms, so the claim gets new markers before the release. Each read of a
    // snapshot file after the claim finds it gone.
    let claimed = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let claimed = claimed.clone();
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            let after_claim = claimed.load(Ordering::SeqCst);
            if after_claim && op_label == "list" && path == Path::new("snapshots") {
                Script::Delay(Duration::from_millis(250))
            } else if after_claim && path.starts_with("snapshots") && !is_forget(op_label, path) {
                Script::Vanish
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_millis(400)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let refreshes = storage
        .calls()
        .iter()
        .filter(|(op_label, _)| *op_label == "refresh_claim")
        .count();

    assert!(matches!(&failed, Ok(())), "{failed:?}");
    assert_eq!(
        (
            refreshes > 0,
            prunes(&storage.calls()),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, 0, Vec::<String>::new())
    );
}

#[test]
#[timeout("120s")]
async fn a_prune_slower_than_the_grace_period_keeps_its_claim_fresh() {
    // The gate holds the prune at its listing of the packs for longer than the grace period.
    let grace = Duration::from_millis(400);
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let pruning = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    // The second delete claims and lists the packs too. So the gate would hold its prune for ever
    // when the first delete did not reach its listing first.
    let prune_held = polled_until(REACH_LIMIT, || held.load(Ordering::SeqCst)).await;
    assert!(
        prune_held,
        "the first delete did not reach its listing of the packs"
    );
    let first = claim_time(&storage, &scope).await.unwrap_or(u64::MAX);
    let wanted = first.saturating_add(u64::try_from(grace.as_millis()).unwrap_or(u64::MAX));

    let refreshed = tokio::time::timeout(
        LIMIT,
        futures::stream::repeat(())
            .then(|()| async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                claim_time(&storage, &scope).await
            })
            .filter(|time| std::future::ready(time.is_some_and(|time| time >= wanted)))
            .boxed()
            .next(),
    )
    .await
    .is_ok();
    let second = store.delete(&scope, &[name("p-2")], &Unlimited).await;
    let prunes_while_held = prunes(&storage.calls());
    storage.open_gate();
    let pruned = tokio::time::timeout(LIMIT, pruning).await;

    assert!(second.is_ok(), "{second:?}");
    assert!(matches!(pruned, Ok(Ok(Ok(())))), "{pruned:?}");
    assert_eq!(
        (
            prune_held,
            refreshed,
            prunes_while_held,
            prunes(&storage.calls()),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, true, 1, 1, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_whose_refreshes_fail_stops_when_its_lease_runs_out_and_keeps_its_claim() {
    // A zero grace period and a deadline of 200 ms give a lease of 199 ms. The claim write and the
    // second read of the ledger each take 150 ms, so the lease has run out when the prune makes its
    // first call. Each refresh fails.
    let deadline = Duration::from_millis(200);
    let slow = Duration::from_millis(150);
    let claimed = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let claimed = claimed.clone();
        move |op_label, _| match op_label {
            "write_claim" => {
                claimed.store(true, Ordering::SeqCst);
                Script::Delay(slow)
            }
            "read_ledger" if claimed.load(Ordering::SeqCst) => Script::Delay(slow),
            "refresh_claim" => Script::Refuse,
            _ => Script::Pass,
        }
    });
    let store = store(storage.clone(), policy(deadline, ALWAYS, Duration::ZERO));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let calls = storage.calls();
    let backend_calls_after_claim = calls
        .iter()
        .position(|(op_label, _)| *op_label == "write_claim")
        .map(|claimed_at| {
            calls[claimed_at..]
                .iter()
                .filter(|(op_label, _)| is_backend_call(op_label))
                .count()
        });
    let entries = claim_entries(&storage, &scope).await;
    let hold = claim_hold(Duration::ZERO, deadline);
    let now = golem_common::model::Timestamp::now_utc();
    let after_hold = golem_common::model::Timestamp::from(
        now.to_millis() + u64::try_from((hold + Duration::from_secs(1)).as_millis()).unwrap(),
    );

    // The prune fails when its lease runs out; the delete answers success, because the names
    // stopped resolving.
    assert!(matches!(&deleted, Ok(())), "{deleted:?}");
    assert_eq!(failed_prunes(&store), vec![CallFailure::LeaseExpired]);
    assert_eq!(
        (
            backend_calls_after_claim,
            entries.contains(&ClaimEntry::Claim(0)),
            next_claim(&entries, now, hold),
            next_claim(&entries, after_hold, hold),
        ),
        (Some(0), true, ClaimChoice::Held, ClaimChoice::Claim(1))
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_goes_on_after_one_failed_refresh_when_the_later_refreshes_succeed() {
    // A zero grace period and a deadline of 200 ms give a lease of 199 ms and a refresh about each
    // 50 ms. Each call of the prune takes 40 ms, so the prune runs for more than one lease. The
    // first refresh fails, and the later ones succeed.
    let deadline = Duration::from_millis(200);
    let claimed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, refused) = (claimed.clone(), refused.clone());
        move |op_label, _| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "refresh_claim" && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else if is_backend_call(op_label) && claimed.load(Ordering::SeqCst) {
                Script::Delay(Duration::from_millis(40))
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), policy(deadline, ALWAYS, Duration::ZERO));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let calls = storage.calls();
    let backend_calls_after_claim = calls
        .iter()
        .position(|(op_label, _)| *op_label == "write_claim")
        .map_or(0, |claimed_at| {
            calls[claimed_at..]
                .iter()
                .filter(|(op_label, _)| is_backend_call(op_label))
                .count()
        });

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            refused.load(Ordering::SeqCst),
            backend_calls_after_claim * 40 > 200,
            prunes(&calls),
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (true, true, 1, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_marker_ahead_within_the_margin_after_a_slow_claim_listing_holds_the_claim() {
    // Another delete wrote a marker whose time is 130 s ahead of the clock at the start of the
    // delete, beyond the margin of 120 s. The gate holds the claim listing while the clock goes
    // 20 s on, so after the listing the marker is 110 s ahead, within the margin, and it holds.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "list_claims" {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(RusticSnapshotStore::with_policy(
        storage.clone(),
        key(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        clock.clone(),
    ));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let ahead = golem_common::model::Timestamp::now_utc().to_millis() + 130_000;
    futures::stream::iter([
        "golem/prune-claims/none/0".to_string(),
        format!("golem/prune-claims/none/0@{ahead}-{}", uuid::Uuid::new_v4()),
    ])
    .for_each(|path| {
        let (storage, scope) = (storage.clone(), scope.clone());
        async move {
            storage
                .put_raw(
                    "test",
                    "test",
                    scope.namespace().clone(),
                    Path::new(&path),
                    &[],
                )
                .await
                .unwrap();
        }
    })
    .await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "list_claims")
    })
    .await;

    clock.advance(Duration::from_secs(20));
    storage.open_gate();
    let deleted = tokio::time::timeout(LIMIT, deleting).await;
    let calls = storage.calls();

    assert!(matches!(deleted, Ok(Ok(Ok(())))), "{deleted:?}");
    assert_eq!(
        (
            held,
            prunes(&calls),
            calls
                .iter()
                .filter(|(op_label, _)| *op_label == "write_claim")
                .count(),
        ),
        (true, 0, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_ledger_entry_ahead_within_the_margin_of_the_moved_clock_holds_the_prune() {
    // The entry is 130 s ahead of the clock of the host, beyond the margin of 120 s. The clock of
    // the store is 20 s ahead, so the entry is within the margin of that clock, and it is the
    // ledger. Its hold has not passed, so the delete does not prune.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(RusticSnapshotStore::with_policy(
        storage.clone(),
        key(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
        clock.clone(),
    ));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let ahead = golem_common::model::Timestamp::now_utc().to_millis() + 130_000;
    put_ledger_entry(&storage, &scope, &format!("{ahead}-0-ahead")).await;
    clock.advance(Duration::from_secs(20));

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(prunes(&storage.calls()), 0);
}

#[test]
#[timeout("60s")]
async fn the_refresh_markers_of_a_prune_take_their_times_from_the_injected_clock() {
    // A short grace period makes the claim get new markers while the gate holds the prune at its
    // listing of the packs.
    let storage = holding_the_prune();
    let (store, ahead) = store_an_hour_ahead(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_millis(400)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let pruning = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let refreshed =
        eventually(|| written_times(&storage.calls(), "refresh_claim").len() >= 2).await;
    storage.open_gate();
    let pruned = tokio::time::timeout(LIMIT, pruning).await;
    let refreshes = written_times(&storage.calls(), "refresh_claim");

    assert!(matches!(pruned, Ok(Ok(Ok(())))), "{pruned:?}");
    assert_eq!(
        (refreshed, refreshes.iter().all(|time| *time >= ahead)),
        (true, true),
        "{refreshes:?}"
    );
}

#[test]
#[timeout("60s")]
async fn the_second_ledger_read_of_a_delete_compares_with_the_injected_clock() {
    // The gate holds the claim write while the test adds a ledger entry 130 s ahead of the clock of
    // the host. That is beyond the margin of 120 s of the clock of the host. It is within the
    // margin of the clock of the store, which is 20 s ahead. So only the clock of the store makes
    // the second read of the ledger see a new ledger, and then the delete does not prune.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "write_claim" {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(RusticSnapshotStore::with_policy(
        storage.clone(),
        key(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
        clock.clone(),
    ));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    clock.advance(Duration::from_secs(20));
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let claiming = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "write_claim")
    })
    .await;
    let ahead = golem_common::model::Timestamp::now_utc().to_millis() + 130_000;
    put_ledger_entry(&storage, &scope, &format!("{ahead}-0-ahead")).await;
    storage.open_gate();
    let deleted = tokio::time::timeout(LIMIT, deleting).await;

    assert!(matches!(deleted, Ok(Ok(Ok(())))), "{deleted:?}");
    assert_eq!((claiming, prunes(&storage.calls())), (true, 0));
}

#[test]
#[timeout("60s")]
async fn the_final_marker_of_a_dropped_delete_takes_its_time_from_the_injected_clock() {
    // The gate holds the first call of the prune, and the test drops the delete there. So the
    // claim guard writes the final marker in its own task.
    let storage = holding_the_prune();
    let (store, ahead) = store_an_hour_ahead(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let started = eventually(|| prunes(&storage.calls()) == 1).await;
    deleting.abort();
    let _ = deleting.await;
    let ended = eventually(|| store.work_in_flight() == 0).await;
    storage.open_gate();
    let finals = written_times(&storage.calls(), "final_marker");

    assert_eq!(
        (
            started,
            ended,
            finals.len(),
            finals.iter().all(|time| *time >= ahead)
        ),
        (true, true, 1, true),
        "{finals:?}"
    );
}

#[test]
#[timeout("60s")]
async fn a_ledger_write_after_the_lease_ran_out_is_not_sent_and_the_claim_stays() {
    // A zero grace period and a deadline of 1 s give a lease of 999 ms. Each refresh fails, and
    // the final marker takes longer than its deadline, so it fails at 1 s, after the end of the
    // lease. The prune itself ends well within the lease.
    let deadline = Duration::from_secs(1);
    let storage =
        ScriptedBlobStorage::new(
            Arc::new(InMemoryBlobStorage::new()),
            |op_label, _| match op_label {
                "refresh_claim" => Script::Refuse,
                "final_marker" => Script::RefuseAfter(Duration::from_millis(1200)),
                _ => Script::Pass,
            },
        );
    let store = store(storage.clone(), policy(deadline, ALWAYS, Duration::ZERO));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let calls = storage.calls();
    let entries = claim_entries(&storage, &scope).await;

    // The prune fails when its lease runs out; the delete answers success, because the names
    // stopped resolving.
    assert!(matches!(&deleted, Ok(())), "{deleted:?}");
    assert_eq!(failed_prunes(&store), vec![CallFailure::LeaseExpired]);
    assert_eq!(
        (
            prunes(&calls),
            calls
                .iter()
                .filter(|(op_label, _)| *op_label == "write_ledger")
                .count(),
            ledger(&storage, &scope).await.last_prune,
            entries.contains(&ClaimEntry::Claim(0)),
        ),
        (1, 0, None, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_ledger_write_that_starts_within_the_lease_ends_at_the_end_of_the_lease() {
    // A zero grace period and a deadline of 2 s give a lease of 1999 ms. The final marker takes
    // 1.5 s, so the ledger write starts with about 0.5 s of the lease left. The ledger write takes
    // 10 s, so the end of the lease ends it before its own deadline.
    let deadline = Duration::from_secs(2);
    let storage =
        ScriptedBlobStorage::new(
            Arc::new(InMemoryBlobStorage::new()),
            |op_label, _| match op_label {
                "final_marker" => Script::Delay(Duration::from_millis(1500)),
                "write_ledger" => Script::Delay(Duration::from_secs(10)),
                _ => Script::Pass,
            },
        );
    let store = store(storage.clone(), policy(deadline, ALWAYS, Duration::ZERO));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let calls = storage.calls();
    let entries = claim_entries(&storage, &scope).await;

    // The prune fails when its lease runs out; the delete answers success, because the names
    // stopped resolving.
    assert!(matches!(&deleted, Ok(())), "{deleted:?}");
    assert_eq!(
        (
            calls
                .iter()
                .filter(|(op_label, _)| *op_label == "write_ledger")
                .count(),
            ledger(&storage, &scope).await.last_prune,
            entries.contains(&ClaimEntry::Claim(0)),
        ),
        (1, None, true)
    );
}

#[test]
#[timeout("60s")]
async fn the_lease_of_a_prune_starts_at_its_first_marker_so_a_prune_without_a_refresh_prunes() {
    // A grace period of one hour gives a refresh period of fifteen minutes, so the prune ends
    // before its first refresh, and only the first marker gives the lease.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let calls = storage.calls();

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            prunes(&calls),
            calls
                .iter()
                .filter(|(op_label, _)| *op_label == "refresh_claim")
                .count(),
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (1, 0, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_within_the_hold_reads_no_record_of_freed_bytes_and_lists_no_pack() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2", "p-3"]).await;
    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();
    let pruned = ledger(&storage, &scope).await.last_prune.is_some();
    let from = storage.calls().len();

    let deleted = store.delete(&scope, &[name("p-2")], &Unlimited).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (pruned, decision_reads(&storage, from)),
        (true, Vec::<&str>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_whose_named_bytes_are_below_the_threshold_reads_no_record_content() {
    // A threshold of all the bytes of the repository is above the bytes that one delete frees.
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, Percent(100), Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let from = storage.calls().len();

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            decision_reads(&storage, from),
            ledger(&storage, &scope).await.last_prune.is_some(),
            freed(&storage, &scope).await > 0,
        ),
        (vec!["list_freed", "list_data"], false, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_whose_named_bytes_reach_the_threshold_reads_and_settles_the_records_and_prunes() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let from = storage.calls().len();

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            decision_reads(&storage, from),
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
            freed(&storage, &scope).await,
        ),
        (DECISION_READS.to_vec(), 1, true, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_deletes_the_claims_of_old_ledgers() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    futures::stream::iter(["golem/prune-claims/100/0", "golem/prune-claims/200/4"])
        .for_each(|path| {
            let storage = storage.clone();
            let scope = scope.clone();
            async move {
                storage
                    .put_raw(
                        "test",
                        "test",
                        scope.namespace().clone(),
                        Path::new(path),
                        b"100",
                    )
                    .await
                    .unwrap();
            }
        })
        .await;

    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();

    assert_eq!(
        (
            ledger(&storage, &scope).await.last_prune.is_some(),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_keeps_the_claims_of_a_newer_ledger() {
    // A newer claim directory can hold a live claim of a later prune, so the cleanup of a prune
    // leaves it. Its time is far after the end of this prune.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let newer = "golem/prune-claims/99999999999999/0";
    let not_a_time = "golem/prune-claims/later/0";
    futures::stream::iter(["golem/prune-claims/100/0", newer, not_a_time])
        .for_each(|path| {
            let storage = storage.clone();
            let scope = scope.clone();
            async move {
                storage
                    .put_raw(
                        "test",
                        "test",
                        scope.namespace().clone(),
                        Path::new(path),
                        b"",
                    )
                    .await
                    .unwrap();
            }
        })
        .await;

    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();

    assert_eq!(
        (
            ledger(&storage, &scope).await.last_prune.is_some(),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, vec![newer.to_string(), not_a_time.to_string()])
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_deletes_an_empty_claim_directory_of_an_old_ledger() {
    // A listing of the blobs does not find an empty directory, so only a listing of the
    // directories finds it.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    storage
        .create_dir(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new("golem/prune-claims/100"),
        )
        .await
        .unwrap();
    let list_claims = || {
        storage.list_dir(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new("golem/prune-claims"),
        )
    };
    let before = list_claims().await.unwrap();

    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();

    assert_eq!(
        (
            before,
            ledger(&storage, &scope).await.last_prune.is_some(),
            list_claims().await.unwrap(),
        ),
        (
            vec![std::path::PathBuf::from("golem/prune-claims/100")],
            true,
            Vec::<std::path::PathBuf>::new()
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_ledger_write_after_a_prune_keeps_the_claim_so_no_second_prune_runs() {
    // The first prune runs and its ledger write fails. A delete right after it finds the claim
    // and does not prune. When the claim is older than the grace period and the margin, the next
    // delete prunes.
    let refused = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refused = refused.clone();
        move |op_label, _| {
            if op_label == "write_ledger" && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2", "p-3"]).await;

    let failed = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let claims_after_failure = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;
    let second = store.delete(&scope, &[name("p-2")], &Unlimited).await;
    let prunes_after_second = prunes(&storage.calls());
    age_claims(&storage, &scope).await;
    let third = store.delete(&scope, &[name("p-3")], &Unlimited).await;

    assert!(matches!(&failed, Ok(())), "{failed:?}");
    assert_eq!(failed_prunes(&store), vec![CallFailure::Failed]);
    assert!(second.is_ok(), "{second:?}");
    assert!(third.is_ok(), "{third:?}");
    assert_eq!(
        (
            claims_after_failure
                .iter()
                .filter(|path| !path.contains('@'))
                .count(),
            prunes_after_second,
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (1, 1, 2, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_keeps_the_record_of_a_delete_that_has_not_forgotten_its_snapshot() {
    // The first delete writes its record and waits at its forget. The second delete prunes and
    // must not count that record. After the forget, the next due prune counts it.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let plain = store(
        inner.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&plain, &scope, &["p-1", "p-2"]).await;
    let held = ScriptedBlobStorage::new(inner.clone(), |op_label, path| {
        if is_forget(op_label, path) {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let pausing = store(
        held.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let paused = AbortOnDropHandle::new(tokio::spawn({
        let scope = scope.clone();
        async move { pausing.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let at_forget = eventually(|| {
        held.calls()
            .iter()
            .any(|(op_label, path)| is_forget(op_label, Path::new(path)))
    })
    .await;
    let paused_record = records(&inner, &scope).await;

    plain
        .delete(&scope, &[name("p-2")], &Unlimited)
        .await
        .unwrap();
    let after_prune = records(&inner, &scope).await;
    let pruned = ledger(&inner, &scope).await;
    age_ledger(&inner, &scope, &pruned).await;
    held.open_gate();
    let resumed = tokio::time::timeout(LIMIT, paused).await;

    assert!(matches!(resumed, Ok(Ok(Ok(())))), "{resumed:?}");
    assert_eq!(
        (
            at_forget,
            paused_record.len(),
            after_prune == paused_record,
            pruned.last_prune.is_some(),
            records(&inner, &scope).await,
            ledger(&inner, &scope).await.last_prune > pruned.last_prune,
        ),
        (true, 1, true, true, Vec::<String>::new(), true)
    );
}

#[test]
#[timeout("60s")]
async fn a_forget_that_lands_while_a_prune_runs_keeps_its_record_for_the_next_prune() {
    // The prune of the second delete checks the records before the first delete forgets. The
    // forget lands while that prune runs, so the record was not settled at the check, and it must
    // stay for the next prune.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let plain = store(
        inner.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&plain, &scope, &["p-1", "p-2", "p-3"]).await;
    let forgetting = ScriptedBlobStorage::new(inner.clone(), |op_label, path| {
        if is_forget(op_label, path) {
            Script::Step {
                refuse: false,
                late: false,
            }
        } else {
            Script::Pass
        }
    });
    let claimed = Arc::new(AtomicBool::new(false));
    let pruning = ScriptedBlobStorage::new(inner.clone(), {
        let claimed = claimed.clone();
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list" && path == Path::new("data") && claimed.load(Ordering::SeqCst) {
                Script::Step {
                    refuse: false,
                    late: false,
                }
            } else {
                Script::Pass
            }
        }
    });
    let first = AbortOnDropHandle::new(tokio::spawn({
        let deleting = store(
            forgetting.clone(),
            policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        );
        let scope = scope.clone();
        async move { deleting.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let first_at_forget = eventually(|| forgetting.waiting_steps() > 0).await;
    let first_record = records(&inner, &scope).await;
    let second = AbortOnDropHandle::new(tokio::spawn({
        let deleting = store(
            pruning.clone(),
            policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        );
        let scope = scope.clone();
        async move { deleting.delete(&scope, &[name("p-2")], &Unlimited).await }
    }));
    let second_at_prune = eventually(|| pruning.waiting_steps() > 0).await;

    forgetting.step();
    let first = tokio::time::timeout(LIMIT, first).await;
    pruning.step();
    let second = tokio::time::timeout(LIMIT, second).await;
    let after_prune = records(&inner, &scope).await;
    let pruned = ledger(&inner, &scope).await;
    age_ledger(&inner, &scope, &pruned).await;
    plain
        .delete(&scope, &[name("p-3")], &Unlimited)
        .await
        .unwrap();

    assert!(matches!(first, Ok(Ok(Ok(())))), "{first:?}");
    assert!(matches!(second, Ok(Ok(Ok(())))), "{second:?}");
    assert_eq!(
        (
            first_at_forget,
            second_at_prune,
            first_record.len(),
            after_prune == first_record,
            pruned.last_prune.is_some(),
            records(&inner, &scope).await,
        ),
        (true, true, 1, true, true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_record_whose_snapshot_still_exists_counts_nothing_and_does_not_make_a_prune_due() {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        inner.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;
    let snapshot = snapshot_files(inner.clone(), &scope)
        .await
        .first()
        .map(|snapshot| snapshot.id.to_hex().to_string())
        .unwrap_or_default();
    inner
        .put_raw(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new("golem/prune-freed/1000000000-kept"),
            snapshot.as_bytes(),
        )
        .await
        .unwrap();

    store
        .delete(&scope, &[name("p-unknown")], &Unlimited)
        .await
        .unwrap();
    store
        .delete(&scope, &[name("p-unknown")], &Unlimited)
        .await
        .unwrap();

    assert_eq!(
        (
            snapshot.len(),
            ledger(&inner, &scope).await.last_prune,
            records(&inner, &scope).await,
        ),
        (
            64,
            None,
            vec!["golem/prune-freed/1000000000-kept".to_string()]
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_record_write_that_answers_already_exists_counts_as_written() {
    // A new try of a record write whose first answer was lost finds the record of the first try.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "write_freed" {
            Script::AnswerAlreadyExists
        } else {
            Script::Pass
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            freed(&storage, &scope).await > 0,
            listed_names(&store, &scope).await
        ),
        (true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_sees_the_marker_of_a_claim_that_is_not_taken_yet_and_does_not_prune() {
    // The gate holds the second of the two writes of the first delete: its marker and its claim.
    // The second delete runs meanwhile.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let writes = Arc::new(AtomicUsize::new(0));
    let first = ScriptedBlobStorage::new(inner.clone(), {
        let writes = writes.clone();
        move |op_label, _| {
            if matches!(op_label, "write_marker" | "write_claim")
                && writes.fetch_add(1, Ordering::SeqCst) == 1
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let second = ScriptedBlobStorage::new(inner.clone(), |_, _| Script::Pass);
    let grace = Duration::from_secs(3600);
    let scope = new_scope();
    save_each(
        &store(inner.clone(), policy(LONG_DEADLINE, NEVER, grace)),
        &scope,
        &["p-1", "p-2"],
    )
    .await;
    let holding = AbortOnDropHandle::new(tokio::spawn({
        let deleting = store(first.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
        let scope = scope.clone();
        async move { deleting.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let held = eventually(|| writes.load(Ordering::SeqCst) >= 2).await;

    let seen = store(second.clone(), policy(LONG_DEADLINE, ALWAYS, grace))
        .delete(&scope, &[name("p-2")], &Unlimited)
        .await;
    first.open_gate();
    let holding = tokio::time::timeout(LIMIT, holding).await;

    assert!(seen.is_ok(), "{seen:?}");
    assert!(matches!(holding, Ok(Ok(Ok(())))), "{holding:?}");
    assert_eq!(
        (held, prunes(&second.calls()), prunes(&first.calls())),
        (true, 0, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_claim_without_a_marker_does_not_unblock_a_live_holder() {
    // The first delete holds claim 0 and waits at the start of its prune. A claim 1 without a
    // marker is in the same directory.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let claimed = Arc::new(AtomicBool::new(false));
    let first = ScriptedBlobStorage::new(inner.clone(), {
        let claimed = claimed.clone();
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list" && path == Path::new("data") && claimed.load(Ordering::SeqCst) {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let second = ScriptedBlobStorage::new(inner.clone(), |_, _| Script::Pass);
    let grace = Duration::from_secs(3600);
    let scope = new_scope();
    save_each(
        &store(inner.clone(), policy(LONG_DEADLINE, NEVER, grace)),
        &scope,
        &["p-1", "p-2"],
    )
    .await;
    let holding = AbortOnDropHandle::new(tokio::spawn({
        let deleting = store(first.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
        let scope = scope.clone();
        async move { deleting.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let held = eventually(|| {
        first
            .calls()
            .iter()
            .any(|(op_label, path)| *op_label == "list" && path == "data")
    })
    .await;
    inner
        .put_raw(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new("golem/prune-claims/none/1"),
            b"",
        )
        .await
        .unwrap();

    let seen = store(second.clone(), policy(LONG_DEADLINE, ALWAYS, grace))
        .delete(&scope, &[name("p-2")], &Unlimited)
        .await;
    first.open_gate();
    let holding = tokio::time::timeout(LIMIT, holding).await;

    assert!(seen.is_ok(), "{seen:?}");
    assert!(matches!(holding, Ok(Ok(Ok(())))), "{holding:?}");
    assert_eq!((held, prunes(&second.calls())), (true, 0));
}

#[test]
#[timeout("120s")]
async fn two_deletes_that_read_the_same_ledger_make_one_prune() {
    // The first read after the first claim is the start of the first prune. The gate holds it,
    // so the second delete reads the ledger that the first delete read.
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, _| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "read"
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let first = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    // The second delete claims and reads too, so the gate would hold it for ever when the first
    // delete did not reach its read first.
    let first_held = polled_until(REACH_LIMIT, || held.load(Ordering::SeqCst)).await;
    assert!(
        first_held,
        "the first delete did not reach its first read after its claim"
    );

    let second = store.delete(&scope, &[name("p-2")], &Unlimited).await;
    storage.open_gate();
    let first = tokio::time::timeout(LIMIT, first).await;
    let calls = storage.calls();

    assert!(matches!(first, Ok(Ok(Ok(())))), "{first:?}");
    assert!(second.is_ok(), "{second:?}");
    assert_eq!(
        (
            first_held,
            prunes(&calls),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, 1, Vec::<String>::new())
    );
}

#[test]
#[timeout("120s")]
async fn a_delete_that_claims_after_another_prune_removed_the_claims_does_not_prune() {
    // The gate holds the first delete after its ledger read and before its listing of the claims.
    // The second delete prunes to its end, so the first delete claims in a removed directory.
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let held = held.clone();
        move |op_label, _| {
            if op_label == "list_claims" && !held.swap(true, Ordering::SeqCst) {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let late = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        let scope = scope.clone();
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    // The second delete lists the claims too, so the gate would hold it for ever when the first
    // delete did not reach its listing first.
    let late_held = polled_until(REACH_LIMIT, || held.load(Ordering::SeqCst)).await;
    assert!(
        late_held,
        "the first delete did not reach its listing of the claims"
    );

    let pruned = store.delete(&scope, &[name("p-2")], &Unlimited).await;
    storage.open_gate();
    let late = tokio::time::timeout(LIMIT, late).await;

    assert!(pruned.is_ok(), "{pruned:?}");
    assert!(matches!(late, Ok(Ok(Ok(())))), "{late:?}");
    assert_eq!(
        (
            late_held,
            prunes(&storage.calls()),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, 1, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_failed_second_read_of_the_ledger_deletes_the_claim_and_a_retry_of_the_delete_prunes() {
    // The first call after the first claim is the second read of the ledger, and it fails. So the
    // first delete does not prune, and only the retry counts as a prune.
    let claimed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, refused) = (claimed.clone(), refused.clone());
        move |op_label, _| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
                Script::Pass
            } else if claimed.load(Ordering::SeqCst) && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let claims_after_failure = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;
    let retried = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let after = ledger(&storage, &scope).await;

    assert!(matches!(&failed, Ok(())), "{failed:?}");
    assert!(retried.is_ok(), "{retried:?}");
    assert_eq!(
        (
            claims_after_failure,
            after.last_prune.is_some(),
            prunes(&storage.calls())
        ),
        (Vec::<String>::new(), true, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_dropped_after_a_failed_second_read_still_releases_its_claim() {
    // The second read of the ledger fails, and the gate holds the delete of the claim. The test
    // drops the delete there, and then opens the gate.
    let storage = failing_after_the_claim(false);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "delete_claim")
    })
    .await;

    deleting.abort();
    let dropped = deleting.await;
    let claims_at_drop = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;
    storage.open_gate();
    let ended = eventually(|| store.work_in_flight() == 0).await;

    assert!(
        dropped.as_ref().is_err_and(|error| error.is_cancelled()),
        "{dropped:?}"
    );
    assert_eq!(
        (
            held,
            claims_at_drop.is_empty(),
            ended,
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, false, true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_dropped_after_its_claim_and_before_its_prune_releases_the_claim() {
    // The gate holds the second read of the ledger. The test drops the delete there, before its
    // prune starts.
    let storage = holding_after_the_claim(false);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let claimed = |calls: &[(&'static str, String)]| {
        calls
            .iter()
            .filter(|(op_label, _)| *op_label == "read_ledger")
            .count()
            >= 2
    };
    let held = eventually(|| claimed(&storage.calls())).await;
    let claims_at_drop = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;

    deleting.abort();
    let dropped = deleting.await;
    let ended = eventually(|| store.work_in_flight() == 0).await;
    let claims_after = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;
    storage.open_gate();

    assert!(
        dropped.as_ref().is_err_and(|error| error.is_cancelled()),
        "{dropped:?}"
    );
    assert_eq!(
        (
            held,
            claims_at_drop.len(),
            ended,
            prunes(&storage.calls()),
            claims_after,
        ),
        (true, 2, true, 0, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn shut_down_waits_for_the_release_of_a_delete_dropped_after_its_claim() {
    // The gate holds the second read of the ledger and each delete of the release. The test drops
    // the delete at the read, so its guard starts the release, which waits at the gate.
    let storage = holding_after_the_claim(true);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .filter(|(op_label, _)| *op_label == "read_ledger")
            .count()
            >= 2
    })
    .await;
    deleting.abort();
    let _ = deleting.await;
    let releasing = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "delete_claim")
    })
    .await;

    let shutting = store.shut_down();
    tokio::pin!(shutting);
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut shutting)
        .await
        .is_err();
    storage.open_gate();
    // A finished future must not be polled again, so the second wait runs only after a first wait
    // that timed out.
    let stopped = !waited || tokio::time::timeout(LIMIT, &mut shutting).await.is_ok();

    assert_eq!(
        (
            held,
            releasing,
            waited,
            stopped,
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, true, true, true, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_shut_down_during_a_started_prune_still_writes_its_final_marker_and_waits_for_it() {
    // The gate holds the first call of the prune, and the shut down cancels it. A grace period of
    // one hour gives no refresh, so the claim has its first marker and its final marker.
    let storage = holding_the_prune();
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let started = eventually(|| prunes(&storage.calls()) == 1).await;

    let stopped = tokio::time::timeout(LIMIT, store.shut_down()).await.is_ok();
    let markers_at_shut_down = markers(&storage, &scope).await;
    storage.open_gate();
    let deleted = tokio::time::timeout(LIMIT, deleting).await;

    assert!(matches!(deleted, Ok(Ok(Ok(())))), "{deleted:?}");
    assert_eq!(
        (
            started,
            stopped,
            markers_at_shut_down,
            final_markers(&storage.calls())
        ),
        (true, true, 2, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_dropped_during_a_started_prune_writes_its_final_marker() {
    // The gate holds the first call of the prune, and the test drops the delete there.
    let storage = holding_the_prune();
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let started = eventually(|| prunes(&storage.calls()) == 1).await;

    deleting.abort();
    let dropped = deleting.await;
    let ended = eventually(|| store.work_in_flight() == 0).await;
    let markers_after = markers(&storage, &scope).await;
    storage.open_gate();

    assert!(
        dropped.as_ref().is_err_and(|error| error.is_cancelled()),
        "{dropped:?}"
    );
    assert_eq!(
        (
            started,
            ended,
            markers_after,
            final_markers(&storage.calls())
        ),
        (true, true, 2, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_writes_one_final_marker() {
    let storage =
        ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |_, _| Script::Pass);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let ended = eventually(|| store.work_in_flight() == 0).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            ended,
            prunes(&storage.calls()),
            final_markers(&storage.calls())
        ),
        (true, 1, 1)
    );
}

#[test]
#[timeout("60s")]
async fn a_shut_down_after_the_claim_still_releases_it() {
    // The gate holds the second read of the ledger, and the shut down cancels that read. The
    // gate stays shut for the read, and it opens only for the delete of the claim.
    let storage = failing_after_the_claim(true);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let held = eventually(|| {
        storage
            .calls()
            .iter()
            .skip_while(|(op_label, _)| *op_label != "write_claim")
            .any(|(op_label, _)| *op_label == "read_ledger")
    })
    .await;

    let shutting_down = AbortOnDropHandle::new(tokio::spawn({
        let store = store.clone();
        async move { store.shut_down().await }
    }));
    let reached_release = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "delete_claim")
    })
    .await;
    let claims_at_release = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;
    storage.open_gate();
    let shut_down = tokio::time::timeout(LIMIT, shutting_down).await;
    let deleted = tokio::time::timeout(LIMIT, deleting).await;

    assert!(matches!(shut_down, Ok(Ok(()))), "{shut_down:?}");
    assert!(matches!(deleted, Ok(Ok(Ok(())))), "{deleted:?}");
    assert_eq!(
        (
            held,
            reached_release,
            claims_at_release.is_empty(),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (true, true, false, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_release_whose_claim_delete_is_refused_still_deletes_its_marker_and_the_next_delete_prunes()
 {
    let storage = refusing_the_release_call("delete_claim");
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let claims_after_failure = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;
    let retried = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(matches!(&failed, Ok(())), "{failed:?}");
    assert_eq!(failed_prunes(&store), vec![CallFailure::Failed]);
    assert!(retried.is_ok(), "{retried:?}");
    assert_eq!(
        (
            claims_after_failure,
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (vec!["golem/prune-claims/none/0".to_string()], 1, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_release_whose_marker_delete_is_refused_still_deletes_the_claim() {
    let storage = refusing_the_release_call("delete_marker");
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let claims_after_failure = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;

    assert!(matches!(&failed, Ok(())), "{failed:?}");
    assert_eq!(failed_prunes(&store), vec![CallFailure::Failed]);
    assert_eq!(
        claims_after_failure
            .iter()
            .map(|path| path.starts_with("golem/prune-claims/none/0@"))
            .collect::<Vec<_>>(),
        vec![true]
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_plans_again_when_a_snapshot_file_is_gone_at_its_read_and_prunes_once() {
    let (storage, vanished) = vanishing_snapshots_after_the_claim(1);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            vanished.load(Ordering::SeqCst),
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
        ),
        (1, 1, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_that_finds_a_snapshot_file_gone_at_each_attempt_releases_its_claim_and_the_delete_succeeds()
 {
    let (storage, vanished) = vanishing_snapshots_after_the_claim(usize::MAX);
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(matches!(&failed, Ok(())), "{failed:?}");
    assert_eq!(
        (
            vanished.load(Ordering::SeqCst),
            prunes(&storage.calls()),
            ledger(&storage, &scope).await.last_prune.is_some(),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
        ),
        (3, 0, false, Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_that_fails_keeps_its_claim_so_no_second_prune_runs_within_the_hold() {
    // The first listing of the packs by a prune after the first claim fails. Only a prune lists
    // the packs with that call, so the second read of the ledger passes and the prune fails after
    // it started. Its claim stays, so a retry within the hold does not prune, and a retry after it
    // does.
    let claimed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, refused) = (claimed.clone(), refused.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !refused.swap(true, Ordering::SeqCst)
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    let failed = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let claims_after_failure = blobs(&*storage, scope.namespace(), "golem/prune-claims/").await;
    let retried = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let after_retry = ledger(&storage, &scope).await;
    age_claims(&storage, &scope).await;
    let later = store.delete(&scope, &[name("p-1")], &Unlimited).await;
    let after = ledger(&storage, &scope).await;

    assert!(matches!(&failed, Ok(())), "{failed:?}");
    assert_eq!(failed_prunes(&store), vec![CallFailure::Failed]);
    assert!(retried.is_ok(), "{retried:?}");
    assert!(later.is_ok(), "{later:?}");
    assert_eq!(
        (
            claims_after_failure
                .iter()
                .filter(|path| !path.contains('@'))
                .count(),
            refused.load(Ordering::SeqCst),
            after_retry.last_prune,
            after.last_prune.is_some()
        ),
        (1, true, None, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_claim_without_a_marker_does_not_block_a_prune() {
    let grace = Duration::from_secs(3600);
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    storage
        .put_raw(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new("golem/prune-claims/none/0"),
            &[],
        )
        .await
        .unwrap();

    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();

    assert!(ledger(&storage, &scope).await.last_prune.is_some());
}

#[test]
#[timeout("60s")]
async fn a_prune_that_succeeds_deletes_the_claims_and_the_counted_records_of_freed_bytes() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
    );
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;

    store
        .delete(&scope, &[name("p-1")], &Unlimited)
        .await
        .unwrap();

    assert_eq!(
        (
            ledger(&storage, &scope).await.last_prune.is_some(),
            blobs(&*storage, scope.namespace(), "golem/prune-claims/").await,
            blobs(&*storage, scope.namespace(), "golem/prune-freed/").await,
        ),
        (true, Vec::<String>::new(), Vec::<String>::new())
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_below_the_threshold_does_not_prune() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, NEVER, Duration::ZERO),
    );
    let scope = new_scope();
    let (deleted_tree, kept_tree) = (one_file_tree("deleted content"), one_file_tree("kept"));
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
    let packs_before = blobs(&*storage, scope.namespace(), "data/").await;

    store
        .delete(&scope, &[name("p-deleted")], &Unlimited)
        .await
        .unwrap();
    let after = ledger(&storage, &scope).await;
    let after_freed = freed(&storage, &scope).await;

    assert_eq!(
        (
            after_freed > 0,
            after.last_prune,
            blobs(&*storage, scope.namespace(), "data/").await,
        ),
        (true, None, packs_before)
    );
}

#[test]
#[timeout("60s")]
async fn no_second_prune_runs_within_the_hold_after_a_prune() {
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
    );
    let scope = new_scope();
    let trees = [one_file_tree("a"), one_file_tree("b"), one_file_tree("c")];
    futures::stream::iter(["p-a", "p-b", "p-c"].into_iter().zip(&trees))
        .for_each(|(text, tree)| {
            let store = store.clone();
            let scope = scope.clone();
            async move {
                store
                    .save(
                        &scope,
                        &name(text),
                        tree.path(),
                        None,
                        never_cancelled(),
                        &Unlimited,
                    )
                    .await
                    .unwrap();
            }
        })
        .await;

    store
        .delete(&scope, &[name("p-a")], &Unlimited)
        .await
        .unwrap();
    let after_first = ledger(&storage, &scope).await;
    store
        .delete(&scope, &[name("p-b")], &Unlimited)
        .await
        .unwrap();
    let after_second = ledger(&storage, &scope).await;
    let after_second_freed = freed(&storage, &scope).await;

    assert_eq!(
        (
            after_first.last_prune.is_some(),
            after_second.last_prune == after_first.last_prune,
            after_second_freed > 0,
        ),
        (true, true, true)
    );
}

#[test]
#[timeout("60s")]
async fn a_delete_whose_prune_fails_succeeds_and_the_next_delete_prunes() {
    let refuse = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let refuse = refuse.clone();
        move |op_label, path| {
            if refuse.load(Ordering::SeqCst)
                && matches!(op_label, "read" | "read_range")
                && path.starts_with("data")
            {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
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
    refuse.store(true, Ordering::SeqCst);

    let failed = store.delete(&scope, &[name("p-deleted")], &Unlimited).await;
    let after_failure = ledger(&storage, &scope).await;
    let after_failure_freed = freed(&storage, &scope).await;
    refuse.store(false, Ordering::SeqCst);
    // The prune started, so its claim stays until the hold passed.
    age_claims(&storage, &scope).await;
    let retried = store.delete(&scope, &[name("p-deleted")], &Unlimited).await;
    let after_retry = ledger(&storage, &scope).await;
    let after_retry_freed = freed(&storage, &scope).await;

    assert!(matches!(&failed, Ok(())), "{failed:?}");
    assert_eq!(
        (
            after_failure_freed > 0,
            after_failure.last_prune,
            retried.is_ok(),
            after_retry_freed,
            after_retry.last_prune.is_some(),
        ),
        (true, None, true, 0, true)
    );
}

#[test]
#[timeout("60s")]
async fn shut_down_waits_for_the_step_of_a_prune_and_its_refresh_that_is_not_polled() {
    // The test drives the delete until its prune waits at the gate and its refresh runs, and then
    // does not poll it. Only the tracker of the step makes the shut down wait for it.
    let grace = Duration::from_millis(400);
    let claimed = Arc::new(AtomicBool::new(false));
    let held = Arc::new(AtomicBool::new(false));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let (claimed, held) = (claimed.clone(), held.clone());
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            if op_label == "list"
                && path == Path::new("data")
                && claimed.load(Ordering::SeqCst)
                && !held.swap(true, Ordering::SeqCst)
            {
                Script::WaitForGate
            } else {
                Script::Pass
            }
        }
    });
    let store = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, grace));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleted_name = name("p-1");
    let deleting = store.delete(&scope, std::slice::from_ref(&deleted_name), &Unlimited);
    tokio::pin!(deleting);
    let reached = tokio::select! {
        biased;
        held = eventually(|| held.load(Ordering::SeqCst)) => held,
        _ = &mut deleting => false,
    };

    let shutting = store.shut_down();
    tokio::pin!(shutting);
    let waited = tokio::time::timeout(Duration::from_millis(200), &mut shutting)
        .await
        .is_err();
    let deleted = tokio::time::timeout(LIMIT, &mut deleting).await;
    // A finished future must not be polled again, so the second wait runs only after a first wait
    // that timed out.
    let stopped = !waited || tokio::time::timeout(LIMIT, &mut shutting).await.is_ok();
    let refreshes = |calls: &[(&'static str, String)]| {
        calls
            .iter()
            .filter(|(op_label, _)| *op_label == "refresh_claim")
            .count()
    };
    let at_stop = refreshes(&storage.calls());
    tokio::time::sleep(grace).await;
    storage.open_gate();

    // The prune ends at the shutdown; the delete answers success, because its names stopped
    // resolving.
    assert!(matches!(&deleted, Ok(Ok(()))), "{deleted:?}");
    assert_eq!(failed_prunes(&store), vec![CallFailure::Cancelled]);
    assert_eq!(
        (
            reached,
            waited,
            stopped,
            store.work_in_flight(),
            refreshes(&storage.calls()) - at_stop,
        ),
        (true, true, true, 0, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_shut_down_between_the_claim_listing_and_the_claim_guard_makes_no_storage_call_after_it_returns()
 {
    // The scripted storage holds the claim listing, and the test clock holds its next read. That is
    // the read of the claim choice, after the listing and before the claim guard. So the tracker is
    // empty and `shut_down` returns while the delete waits. The claim listing is the last storage
    // call at the hold, and no claim is written, so the hold is at that point.
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, _| {
        if op_label == "list_claims" {
            Script::WaitForGate
        } else {
            Script::Pass
        }
    });
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(RusticSnapshotStore::with_policy(
        storage.clone(),
        key(),
        policy(LONG_DEADLINE, ALWAYS, Duration::from_secs(3600)),
        clock.clone(),
    ));
    let scope = new_scope();
    save_each(&store, &scope, &["p-1", "p-2"]).await;
    let deleting = AbortOnDropHandle::new(tokio::spawn({
        let (store, scope) = (store.clone(), scope.clone());
        async move { store.delete(&scope, &[name("p-1")], &Unlimited).await }
    }));
    let listing = eventually(|| {
        storage
            .calls()
            .iter()
            .any(|(op_label, _)| *op_label == "list_claims")
    })
    .await;
    clock.hold_next_read();
    storage.open_gate();
    let reached = tokio::task::spawn_blocking({
        let clock = clock.clone();
        move || clock.wait_until_held(LIMIT)
    })
    .await
    .unwrap();
    let calls_at_hold = storage.calls();

    let stopped = tokio::time::timeout(LIMIT, store.shut_down()).await.is_ok();
    let calls_at_shut_down = storage.calls();
    clock.release();
    let deleted = tokio::time::timeout(LIMIT, deleting).await;
    let ended = eventually(|| store.work_in_flight() == 0).await;

    assert!(matches!(&deleted, Ok(Ok(Ok(())))), "{deleted:?}");
    assert_eq!(
        (
            listing,
            reached,
            calls_at_hold.last().map(|(op_label, _)| *op_label),
            calls_at_hold
                .iter()
                .filter(|(op_label, _)| *op_label == "write_claim")
                .count(),
            stopped,
            ended,
            storage.calls(),
        ),
        (
            true,
            true,
            Some("list_claims"),
            0,
            true,
            true,
            calls_at_shut_down
        )
    );
}

#[test]
fn a_prune_that_marks_only_a_pack_that_no_index_lists_leaves_marked_packs() {
    assert!(leaves_marked_packs(&PruneReport {
        packs_unindexed: 1,
        ..PruneReport::default()
    }));
}

#[test]
#[timeout("60s")]
async fn a_due_prune_that_marks_a_pack_that_no_index_lists_records_the_marked_pack() {
    // The pack of the kept snapshot stays in use, so the pack that no index lists is the only pack
    // that the prune marks. The ledger holds freed bytes from an earlier delete, so a delete of an
    // unknown name prunes.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
        policy(LONG_DEADLINE, ALWAYS, Duration::ZERO),
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
    let unindexed = format!("data/ab/{}", "ab".repeat(32));
    storage
        .put_raw(
            "test",
            "test",
            scope.namespace().clone(),
            Path::new(&unindexed),
            b"a pack that no index lists",
        )
        .await
        .unwrap();
    set_last_prune(&storage, &scope, golem_common::model::Timestamp::from(0)).await;

    store
        .delete(&scope, &[name("p-unknown")], &Unlimited)
        .await
        .unwrap();
    let after = ledger(&storage, &scope).await;
    let after_freed = freed(&storage, &scope).await;

    assert_eq!(
        (
            after_freed,
            after.last_prune.is_some_and(|last| last.to_millis() > 0),
            after.awaiting_removal,
            blobs(&*storage, scope.namespace(), "data/")
                .await
                .contains(&unindexed),
            restored_listing(&store, &scope, &name("p-kept")).await.ok(),
        ),
        (0, true, true, true, Some(listing(tree.path())))
    );
}

#[test]
fn a_prune_leaves_marked_packs_when_it_marks_repacks_or_keeps_marked_packs() {
    let report = |packs_unused, packs_repacked, marked_packs_kept| PruneReport {
        packs_unused,
        packs_repacked,
        marked_packs_kept,
        ..PruneReport::default()
    };

    assert_eq!(
        [
            leaves_marked_packs(&report(0, 0, 0)),
            leaves_marked_packs(&report(1, 0, 0)),
            leaves_marked_packs(&report(0, 1, 0)),
            leaves_marked_packs(&report(0, 0, 1)),
        ],
        [false, true, true, true]
    );
}

#[test]
#[timeout("60s")]
async fn a_prune_that_fails_without_a_storage_failure_leaves_the_delete_its_success() {
    // Packs of zeros with the sizes of the index give the prune a decryption error, not a failed
    // storage call. The forget before the prune has succeeded, so the delete gives success.
    let storage = Arc::new(InMemoryBlobStorage::new());
    let store = store(
        storage.clone(),
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
    let packs = storage
        .list_blobs_below("test", "test", scope.namespace().clone(), Path::new("data"))
        .await
        .unwrap();
    futures::future::join_all(packs.iter().map(|pack| {
        let zeros = vec![0; usize::try_from(pack.size).unwrap()];
        let storage = storage.clone();
        let namespace = scope.namespace().clone();
        async move {
            storage
                .put_raw("test", "test", namespace, &pack.path, &zeros)
                .await
        }
    }))
    .await
    .into_iter()
    .collect::<anyhow::Result<Vec<()>>>()
    .unwrap();

    let deleted = store.delete(&scope, &[name("p-deleted")], &Unlimited).await;

    assert!(matches!(&deleted, Ok(())), "{deleted:?}");
}

#[test]
#[timeout("60s")]
async fn two_deletes_make_at_most_one_prune_in_each_order_with_up_to_two_switches() {
    // Each order where one delete runs some steps, the other runs some steps, and then each runs
    // to its end. This holds each pause of one delete while the other runs.
    let (shared, prepared) = prepared_scope().await;
    let schedules = (0..2).flat_map(|first| {
        (0..=SWEEP_TURN).flat_map(move |one| {
            (0..=SWEEP_TURN).map(move |two| schedules::Schedule {
                first,
                turns: vec![one, two],
                fail: None,
                late: None,
                drop: None,
            })
        })
    });
    let started = std::time::Instant::now();

    let cases = futures::stream::iter(schedules)
        .then(|schedule| {
            let (shared, prepared) = (&shared, &prepared);
            async move { schedules::run_case(shared, prepared, &schedule).await }
        })
        .try_fold(0usize, |cases, _| async move { Ok(cases + 1) })
        .await;

    println!("{cases:?} orders in {:?}", started.elapsed());
    assert!(cases.is_ok(), "{cases:?}");
}

#[test]
#[timeout("60s")]
async fn two_deletes_make_at_most_one_prune_when_one_is_dropped_at_its_first_step() {
    // A drop right after the first step cancels the forget of the delete while its storage call
    // can already wait for a step. The race depends on timing, so the test runs the order many
    // times.
    let (shared, prepared) = prepared_scope().await;
    let schedule = schedules::Schedule {
        first: 0,
        turns: vec![6, 1, 17, 14, 16, 2, 1, 12],
        fail: Some((0, 17)),
        late: None,
        drop: Some((0, 0)),
    };

    let cases = futures::stream::iter(0..DROP_REPEATS)
        .then(|_| schedules::run_case(&shared, &prepared, &schedule))
        .try_fold(0usize, |cases, _| async move { Ok(cases + 1) })
        .await;

    assert!(cases.is_ok(), "{cases:?}");
}

#[test]
#[timeout("60s")]
async fn two_deletes_make_at_most_one_prune_when_one_is_dropped_as_its_prune_starts() {
    // A drop right after the second read of the ledger can come just after the prune started. The
    // listing of the packs then waits for a step until the cancel ends it, and the guard writes
    // the final marker. The race depends on timing, so the test runs the order many times.
    let (shared, prepared) = prepared_scope().await;
    let schedule = schedules::Schedule {
        first: 1,
        turns: vec![16, 10, 3, 9, 14],
        fail: None,
        late: None,
        drop: Some((1, 10)),
    };

    let cases = futures::stream::iter(0..DROP_REPEATS)
        .then(|_| schedules::run_case(&shared, &prepared, &schedule))
        .try_fold(0usize, |cases, _| async move { Ok(cases + 1) })
        .await;

    assert!(cases.is_ok(), "{cases:?}");
}

#[test]
#[timeout("60s")]
async fn two_deletes_make_at_most_one_prune_in_random_orders_with_a_failed_call() {
    // The test generates the order of the steps, a call that fails, and a call that gets no answer
    // and reaches the storage later, and shrinks a failing case to the shortest order. The seed is
    // fixed, and no file keeps a failing case.
    use proptest::prelude::{Strategy, prop};
    use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
    let (shared, prepared) = prepared_scope().await;
    let runtime = tokio::runtime::Handle::current();
    let strategy = (
        0usize..2,
        prop::collection::vec(0usize..=SWEEP_TURN, 0..=8),
        prop::option::of((0usize..2, 0usize..SWEEP_TURN)),
        prop::option::of((0usize..2, 0usize..SWEEP_TURN, 0usize..8)),
        prop::option::of((0usize..2, 0usize..SWEEP_TURN)),
    )
        .prop_map(|(first, turns, fail, late, drop)| schedules::Schedule {
            first,
            turns,
            fail,
            late,
            drop,
        });
    let started = std::time::Instant::now();

    let outcome = tokio::task::spawn_blocking(move || {
        let mut runner = TestRunner::new_with_rng(
            Config {
                cases: SWEEP_CASES,
                failure_persistence: None,
                max_shrink_time: 20_000,
                ..Config::default()
            },
            TestRng::deterministic_rng(RngAlgorithm::ChaCha),
        );
        runner
            .run(&strategy, |schedule| {
                runtime
                    .block_on(schedules::run_case(&shared, &prepared, &schedule))
                    .map_err(proptest::test_runner::TestCaseError::fail)
            })
            .map_err(|error| error.to_string())
    })
    .await;

    println!("{SWEEP_CASES} random orders in {:?}", started.elapsed());
    assert!(matches!(outcome, Ok(Ok(()))), "{outcome:?}");
}

#[test]
#[timeout("60s")]
async fn the_claim_protocol_makes_one_store_try_for_each_call() {
    // The claim write is refused once. A blob call of the claim protocol has one try, so the
    // prune does not run, and the delete answers success.
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    save_each(&store_over(&inner), &scope, &["p-1", "p-2"]).await;
    let storage = ScriptedBlobStorage::new(inner.clone(), {
        let refused = AtomicBool::new(false);
        move |op_label, _| {
            if op_label == "write_claim" && !refused.swap(true, Ordering::SeqCst) {
                Script::Refuse
            } else {
                Script::Pass
            }
        }
    });
    let store = store(
        storage.clone(),
        StorePolicy {
            in_call_tries: 3,
            ..policy(LONG_DEADLINE, ALWAYS, Duration::ZERO)
        },
    );

    let deleted = store.delete(&scope, &[name("p-1")], &Unlimited).await;

    assert!(deleted.is_ok(), "{deleted:?}");
    assert_eq!(
        (
            calls_of(&storage, "write_claim"),
            listed_names(&store, &scope).await
        ),
        (1, vec!["p-2".to_string()])
    );
}

/// Gives the path of each pack of the scope, in the order of the paths.
async fn pack_paths(storage: &InMemoryBlobStorage, agent: &AgentSnapshots) -> Vec<String> {
    stored_paths(storage, agent)
        .await
        .into_iter()
        .filter(|path| path.starts_with("data/"))
        .collect()
}

/// Writes a tree of one file with the name and the content into a new directory, and gives the
/// directory.
fn named_file_tree(file: &'static str, content: &str) -> Scratch {
    let tree = Scratch::new();
    write_tree(
        tree.path(),
        &[(
            file,
            Spec::File {
                content: Box::from(content.as_bytes()),
                mode: 0o644,
            },
        )],
    );
    tree
}

/// Prunes the repository of the scope with the settings through the prune of the store, on a
/// blocking thread. Each call on the storage waits for at most `deadline`.
async fn prune_with(
    storage: Arc<dyn BlobStorage>,
    agent: &AgentSnapshots,
    deadline: Duration,
    settings: PruneSettings,
) -> anyhow::Result<Option<PruneReport>> {
    let backend = backend_of(storage, agent, deadline);
    run_blocking(move || {
        crate::filesystem_snapshot::backend::rustic::pruning::prune(
            backend,
            &repository_key(),
            &settings,
        )
    })
    .await
}

/// Gives the time of the newest marker of the claims of the scope.
async fn claim_time(storage: &ScriptedBlobStorage, agent: &AgentSnapshots) -> Option<u64> {
    blobs(storage, agent.namespace(), "golem/prune-claims/")
        .await
        .iter()
        .filter_map(
            |path| match parse_claim_entry(Path::new(path).file_name()?.to_str()?)? {
                ClaimEntry::Marker(_, at) => Some(at.to_millis()),
                ClaimEntry::Claim(_) => None,
            },
        )
        .max()
}

/// Gives a store over the storage with the policy, whose clock is an hour ahead of the clock of the
/// host. It also gives the first millisecond of that hour on the clock of the host.
fn store_an_hour_ahead(
    storage: Arc<dyn BlobStorage>,
    policy: StorePolicy,
) -> (Arc<RusticSnapshotStore>, u64) {
    let clock = Arc::new(TestClock::default());
    clock.set_ahead(Duration::from_secs(3600));
    let ahead = golem_common::model::Timestamp::now_utc().to_millis() + 3_600_000;
    let store = Arc::new(RusticSnapshotStore::with_policy(
        storage,
        key(),
        policy,
        clock,
    ));
    (store, ahead)
}

/// The operation labels of the calls of the prune decision that read the records of freed bytes
/// and what they name: the listing of the records, the read of a record, the listing of the
/// snapshot files, and the listing of the packs.
const DECISION_READS: [&str; 4] = ["list_freed", "read_freed", "list_snapshots", "list_data"];

/// Gives the labels of [`DECISION_READS`] that the calls from `from` on made.
fn decision_reads(storage: &ScriptedBlobStorage, from: usize) -> Vec<&'static str> {
    let calls = storage.calls();
    DECISION_READS
        .into_iter()
        .filter(|label| {
            calls
                .iter()
                .skip(from)
                .any(|(op_label, _)| op_label == label)
        })
        .collect()
}

/// Tells whether the call is the forget of a delete: the delete of a snapshot file.
fn is_forget(op_label: &str, path: &Path) -> bool {
    op_label == "delete" && path.starts_with("snapshots")
}

/// A storage that refuses the first call after the first claim write, which is the second read of
/// the ledger. It holds each claim delete, and each ledger read after the claim when `hold_read` is
/// true, until the gate opens.
fn failing_after_the_claim(hold_read: bool) -> Arc<ScriptedBlobStorage> {
    let claimed = Arc::new(AtomicBool::new(false));
    let refused = Arc::new(AtomicBool::new(false));
    ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), move |op_label, _| {
        if op_label == "write_claim" {
            claimed.store(true, Ordering::SeqCst);
            Script::Pass
        } else if op_label == "delete_claim"
            || (hold_read && op_label == "read_ledger" && claimed.load(Ordering::SeqCst))
        {
            Script::WaitForGate
        } else if !hold_read
            && claimed.load(Ordering::SeqCst)
            && !refused.swap(true, Ordering::SeqCst)
        {
            Script::Refuse
        } else {
            Script::Pass
        }
    })
}

/// A storage whose gate holds the second read of the ledger after the claim, and also each delete
/// of a claim or a marker when `hold_release` is true.
fn holding_after_the_claim(hold_release: bool) -> Arc<ScriptedBlobStorage> {
    let claimed = Arc::new(AtomicBool::new(false));
    ScriptedBlobStorage::new(
        Arc::new(InMemoryBlobStorage::new()),
        move |op_label, _| match op_label {
            "write_claim" => {
                claimed.store(true, Ordering::SeqCst);
                Script::Pass
            }
            "read_ledger" if claimed.load(Ordering::SeqCst) => Script::WaitForGate,
            "delete_claim" | "delete_marker" if hold_release => Script::WaitForGate,
            _ => Script::Pass,
        },
    )
}

/// A storage whose gate holds the listing of the packs, which is the first call of a prune, and
/// whose final marker takes 200 ms.
fn holding_the_prune() -> Arc<ScriptedBlobStorage> {
    ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), |op_label, path| {
        if op_label == "list" && path == Path::new("data") {
            Script::WaitForGate
        } else if op_label == "final_marker" {
            Script::Delay(Duration::from_millis(200))
        } else {
            Script::Pass
        }
    })
}

/// Gives the number of final marker writes in the calls.
fn final_markers(calls: &[(&'static str, String)]) -> usize {
    calls
        .iter()
        .filter(|(op_label, _)| *op_label == "final_marker")
        .count()
}

/// A storage that refuses the first call after the first claim write, which is the second read of
/// the ledger, and each call of the release with the operation label.
fn refusing_the_release_call(release_label: &'static str) -> Arc<ScriptedBlobStorage> {
    // The state is whether a claim was written, and whether the call after it was refused.
    ScriptedBlobStorage::with_state(
        Arc::new(InMemoryBlobStorage::new()),
        (false, false),
        move |&(claimed, refused), op_label, _| match op_label {
            "write_claim" => (Script::Pass, (true, refused)),
            label if label == release_label => (Script::Refuse, (claimed, refused)),
            _ if claimed && !refused => (Script::Refuse, (claimed, true)),
            _ => (Script::Pass, (claimed, refused)),
        },
    )
}

/// A storage that gives no blob to the first `vanishing` reads of a snapshot file after the first
/// claim write, as a forget of another delete between the listing and the read does. It counts
/// those reads.
fn vanishing_snapshots_after_the_claim(
    vanishing: usize,
) -> (Arc<ScriptedBlobStorage>, Arc<AtomicUsize>) {
    let claimed = Arc::new(AtomicBool::new(false));
    let vanished = Arc::new(AtomicUsize::new(0));
    let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
        let vanished = vanished.clone();
        move |op_label, path| {
            if op_label == "write_claim" {
                claimed.store(true, Ordering::SeqCst);
            }
            let snapshot_read = claimed.load(Ordering::SeqCst)
                && path.starts_with("snapshots")
                && !is_forget(op_label, path)
                && op_label != "list";
            if snapshot_read
                && vanished
                    .try_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                        (count < vanishing).then_some(count + 1)
                    })
                    .is_ok()
            {
                Script::Vanish
            } else {
                Script::Pass
            }
        }
    });
    (storage, vanished)
}

/// The largest number of steps of one turn. A delete takes at most 21 steps of the protocol, and
/// its prune writes at least one new marker of its claim, so a turn of 22 steps runs a delete to
/// its end when its prune writes one new marker.
const SWEEP_TURN: usize = 22;

/// The number of runs of the order with a drop at the first step.
const DROP_REPEATS: usize = 300;

/// The number of random orders that the property test tries.
const SWEEP_CASES: u32 = 1000;

/// Saves the two snapshots that each case deletes, in a scope that each case copies.
async fn prepared_scope() -> (Arc<InMemoryBlobStorage>, AgentSnapshots) {
    let shared = Arc::new(InMemoryBlobStorage::new());
    let prepared = new_scope();
    save_each(
        &store(shared.clone(), policy(LONG_DEADLINE, NEVER, Duration::ZERO)),
        &prepared,
        &["p-1", "p-2"],
    )
    .await;
    (shared, prepared)
}

mod schedules {
    //! Two deletes of one scope whose prunes are both due, in each order of their blob calls.
    //!
    //! Each delete has its own store over its own scripted storage, and the two storages share one
    //! in-memory storage, as two executors share a bucket. Each blob call of the prune protocol is a
    //! step: it waits until the test gives its delete one step. So the test sets the order of the
    //! calls of the two deletes. The grace period is short, so a prune writes new markers of its claim
    //! while it runs. A timer starts each such write, so its place in the order can change from run to
    //! run, and the log of a case gives the call that took each step.

    use super::*;
    use crate::filesystem_snapshot::CallError;

    use futures::TryStreamExt;

    /// The operation labels of the blob calls of the prune protocol. The listing of the packs by a
    /// prune is a step too, and it is the start of the prune. The delete of a snapshot file is the
    /// forget of a delete.
    const STEP_LABELS: &[&str] = &[
        "write_freed",
        "read_freed",
        "list_snapshots",
        "read_ledger",
        "list_freed",
        "list_data",
        "list_claims",
        "write_marker",
        "write_claim",
        "delete_marker",
        "refresh_claim",
        "final_marker",
        "write_ledger",
        "list_ledgers",
        "delete_ledger",
        "delete_freed",
        "delete_claim",
        "list_claim_directories",
        "list_claim_blobs",
        "delete_claims",
    ];

    /// The most steps that one delete takes before the test gives up on it, without the writes of
    /// new markers. Those writes end with the prune.
    const MOST_STEPS: usize = 64;

    fn is_step(op_label: &str, path: &Path) -> bool {
        STEP_LABELS.contains(&op_label)
            || (op_label == "list" && path == Path::new("data"))
            || is_forget(op_label, path)
    }

    /// Tells whether the call writes or deletes, so that it can reach the storage late.
    fn can_be_late(op_label: &str) -> bool {
        op_label.starts_with("write_")
            || op_label.starts_with("delete")
            || op_label == "final_marker"
    }

    /// Tells whether the call writes a new marker of a claim while its prune runs. A timer starts
    /// such a call, so the schedule neither counts it nor makes it fail.
    fn is_refresh(op_label: &str) -> bool {
        op_label == "refresh_claim"
    }

    /// The grace period of the deletes of a case. The claim of a prune gets a new marker at each
    /// quarter of it, so the markers come while the prune runs.
    const SWEEP_GRACE: Duration = Duration::from_millis(16);

    fn is_prune_start(op_label: &str, path: &str) -> bool {
        op_label == "list" && path == "data"
    }

    /// The order of the steps of one case: the delete that goes first, and the numbers of steps that
    /// the deletes take in turn. A turn counts each step, also the write of a new marker. After the
    /// listed turns, the delete whose turn is next runs to its end, and then the other one does.
    /// `fail` refuses one step of one delete. `late` makes one write or delete of one delete give no
    /// answer at its step, and reach the storage after the given number of further steps of the case.
    /// `drop` drops one delete right after one of its steps, as a caller that stops waiting does. The
    /// steps of `fail`, `late` and `drop` are numbered without the writes of new markers.
    #[derive(Clone, Debug)]
    pub(super) struct Schedule {
        pub(super) first: usize,
        pub(super) turns: Vec<usize>,
        pub(super) fail: Option<(usize, usize)>,
        pub(super) late: Option<(usize, usize, usize)>,
        pub(super) drop: Option<(usize, usize)>,
    }

    /// One step that a delete took, or a late call that reached the storage, in the order of all
    /// steps of the case.
    #[derive(Clone, Debug)]
    struct Step {
        delete: usize,
        op_label: &'static str,
        path: String,
        /// The call gave an error to its caller: it was refused, or it was late.
        failed: bool,
        /// The call reached the storage: a step that passed, or the landing of a late call.
        effect: bool,
        /// The entry is the landing of a late call, not a call.
        landed: bool,
    }

    /// One delete of the case: its store, its scripted storage and its task, which a drop of the case
    /// aborts.
    struct Delete {
        store: Arc<RusticSnapshotStore>,
        storage: Arc<ScriptedBlobStorage>,
        task: AbortOnDropHandle<Result<(), CallError>>,
        /// The steps that the delete took, other than the writes of new markers.
        taken: usize,
        /// Whether the case dropped the delete. A dropped delete makes a call only for the release of
        /// its claim.
        dropped: bool,
    }

    impl Delete {
        /// Tells whether the delete ended and its store has no work left, such as the release of the
        /// claim of a dropped delete.
        fn finished(&self) -> bool {
            self.task.is_finished() && self.store.work_in_flight() == 0
        }
    }

    /// The state of a case while it runs.
    struct Case {
        deletes: [Delete; 2],
        log: Vec<Step>,
        schedule: Schedule,
        /// A late call that has not reached the storage: its delete, the log length at which it
        /// lands, and its step.
        pending: Option<(usize, usize, Step)>,
    }

    /// The checks of [`until`] after which it sleeps between two checks instead of yielding.
    const YIELDS_BEFORE_SLEEP: usize = 1000;

    /// The sleep between two late checks of [`until`].
    const POLL_STEP: Duration = Duration::from_millis(1);

    /// Waits until the condition holds, and gives false when it does not hold within [`LIMIT`]. The
    /// wait yields to the runtime between its first [`YIELDS_BEFORE_SLEEP`] checks, so a step ends as
    /// soon as it can, and then sleeps [`POLL_STEP`] between two checks. On a thread of the blocking
    /// pool a yield gives nothing back, so the sleeps keep a long wait from holding its thread busy.
    async fn until(condition: impl Fn() -> bool) -> bool {
        tokio::time::timeout(LIMIT, async {
            futures::stream::iter(0usize..)
                .then(|check| async move {
                    if check < YIELDS_BEFORE_SLEEP {
                        tokio::task::yield_now().await;
                    } else {
                        tokio::time::sleep(POLL_STEP).await;
                    }
                })
                .take_while(|()| std::future::ready(!condition()))
                .for_each(|()| std::future::ready(()))
                .await
        })
        .await
        .is_ok()
    }

    /// Waits until the delete waits for a step or ended. While the prune waits for the listing of
    /// the packs, it also waits until the first new marker of the claim waits for a step, so each
    /// prune writes a new marker right after that listing. A dropped delete writes no new marker,
    /// because the cancel stops the refresh, and the cancel also ends its listing of the packs.
    async fn settle(delete: &Delete) -> bool {
        until(|| {
            let waiting = delete.storage.waiting_steps();
            let refreshing = !delete.dropped && prune_waits(&delete.storage);
            delete.finished() || waiting > usize::from(refreshing)
        })
        .await
    }

    /// Tells whether the prune of the delete waits for its step: the storage has a call for the
    /// listing of the packs that took no step yet.
    fn prune_waits(storage: &ScriptedBlobStorage) -> bool {
        let starts = |calls: Vec<(&'static str, String)>| {
            calls
                .iter()
                .filter(|(op_label, path)| is_prune_start(op_label, path))
                .count()
        };
        starts(storage.calls()) > starts(storage.took())
    }

    impl Case {
        /// Lands the pending late call when its time came, or when `now` is true.
        async fn land(&mut self, now: bool) -> Result<(), String> {
            let due = self
                .pending
                .as_ref()
                .is_some_and(|(_, at, _)| now || self.log.len() >= *at);
            if !due {
                return Ok(());
            }
            let Some((who, _, step)) = self.pending.take() else {
                return Ok(());
            };
            let storage = self.deletes[who].storage.clone();
            let before = storage.landed();
            storage.land_late();
            if !until(|| storage.landed() > before).await {
                return Err(format!(
                    "the late call {} {} did not land",
                    step.op_label, step.path
                ));
            }
            self.log.push(Step {
                failed: false,
                effect: true,
                landed: true,
                ..step
            });
            Ok(())
        }

        /// Gives the delete one step and waits until the step ended and the delete waits again or
        /// ended.
        async fn take_step(&mut self, who: usize) -> Result<(), String> {
            if self.deletes[who].finished() {
                return Ok(());
            }
            if self.deletes[who].dropped {
                let delete = &self.deletes[who];
                if !until(|| delete.storage.waiting_steps() > 0 || delete.finished()).await {
                    return Err(format!("the dropped delete {who} did not end"));
                }
                if delete.finished() {
                    return Ok(());
                }
            }
            let storage = self.deletes[who].storage.clone();
            let (before, taker) = (storage.stepped(), storage.took().len());
            storage.step();
            // The drop cancels the operation of the delete, and a call that already waited for a step
            // then ends without the step. So for a dropped delete, the step can stay with no call to
            // take it, and the test takes it back.
            let dropped = self.deletes[who].dropped;
            let untaken = || {
                dropped
                    && storage.stepped() == before
                    && storage.waiting_steps() == 0
                    && storage.took().len() == taker
            };
            if !until(|| storage.stepped() > before || untaken()).await {
                return Err(format!(
                    "a step of the dropped delete {who} was neither taken nor left; its last calls {:?}",
                    storage.calls().iter().rev().take(6).collect::<Vec<_>>()
                ));
            }
            if untaken() && storage.take_back_step() {
                return Ok(());
            }
            if !until(|| storage.stepped() > before).await {
                return Err(format!(
                    "a step of delete {who} did not end; its last calls {:?}",
                    storage.calls().iter().rev().take(6).collect::<Vec<_>>()
                ));
            }
            let took = storage.took();
            if took.len() != taker + 1 {
                return Err(format!(
                    "one step of delete {who} was taken by {:?}",
                    took.get(taker..)
                ));
            }
            let (op_label, path) = took
                .get(taker)
                .cloned()
                .ok_or_else(|| format!("a step of delete {who} ended, and no call took it"))?;
            let number = self.deletes[who].taken;
            let counted = !is_refresh(op_label);
            let refused = counted && self.schedule.fail == Some((who, number));
            let late = self.schedule.late.filter(|(late_who, late_step, _)| {
                counted && (*late_who, *late_step) == (who, number) && can_be_late(op_label)
            });
            let step = Step {
                delete: who,
                op_label,
                path,
                failed: refused || late.is_some(),
                effect: !refused && late.is_none(),
                landed: false,
            };
            self.log.push(step.clone());
            if let Some((_, _, delay)) = late {
                self.pending = Some((who, self.log.len() + delay, step));
            }
            self.deletes[who].taken += usize::from(counted);
            if self.deletes[who].taken > MOST_STEPS {
                return Err(format!("delete {who} took more than {MOST_STEPS} steps"));
            }
            self.land(false).await?;
            if counted && self.schedule.drop == Some((who, number)) {
                self.deletes[who].task.abort();
                self.deletes[who].dropped = true;
                // An abort only asks the task to end. Its waiting call counts as waiting until the
                // task ends, so the case gives no step before that.
                let task = &self.deletes[who].task;
                if !until(|| task.is_finished()).await {
                    return Err(format!("the dropped delete {who} did not end"));
                }
                // The drop counts as a failed step of the delete, so the rules on a delete that
                // stopped after its claim apply to it.
                self.log.push(Step {
                    delete: who,
                    op_label: "drop",
                    path: String::new(),
                    failed: true,
                    effect: false,
                    landed: false,
                });
            }
            if settle(&self.deletes[who]).await {
                Ok(())
            } else {
                let calls = self.deletes[who].storage.calls();
                let last = calls.iter().rev().take(6).collect::<Vec<_>>();
                Err(format!(
                    "delete {who} did not reach its next step; its last calls {last:?}; the other delete waits {}",
                    self.deletes[(who + 1) % 2].storage.waiting_steps()
                ))
            }
        }

        /// Gives the delete the steps.
        async fn take_steps(&mut self, who: usize, steps: usize) -> Result<(), String> {
            futures::stream::iter(0..steps)
                .map(Ok)
                .try_fold(self, |case, _| async move {
                    case.take_step(who).await?;
                    Ok::<_, String>(case)
                })
                .await
                .map(|_| ())
        }

        /// Gives the delete steps until it ends.
        async fn run_to_end(&mut self, who: usize) -> Result<(), String> {
            futures::stream::unfold(self, |case| async move {
                if case.deletes[who].finished() {
                    None
                } else {
                    let stepped = case.take_step(who).await;
                    Some((stepped, case))
                }
            })
            .try_for_each(|()| std::future::ready(Ok(())))
            .await
        }
    }

    /// Runs the two deletes of a copy of the prepared scope in the order of the schedule, and checks
    /// the rules of the prune protocol.
    pub(super) async fn run_case(
        shared: &Arc<InMemoryBlobStorage>,
        prepared: &AgentSnapshots,
        schedule: &Schedule,
    ) -> Result<(), String> {
        let scope = new_scope();
        store(shared.clone(), policy(LONG_DEADLINE, NEVER, Duration::ZERO))
            .copy_all(prepared, &scope, &Unlimited)
            .await
            .map_err(|error| format!("the copy of the prepared scope failed: {error}"))?;
        let deletes = [0, 1].map(|who| {
            let counter = Arc::new(AtomicUsize::new(0));
            let (fail, late) = (schedule.fail, schedule.late);
            let storage = ScriptedBlobStorage::new(shared.clone(), move |op_label, path| {
                if is_refresh(op_label) {
                    Script::Step {
                        refuse: false,
                        late: false,
                    }
                } else if is_step(op_label, path) {
                    let number = counter.fetch_add(1, Ordering::SeqCst);
                    Script::Step {
                        refuse: fail == Some((who, number)),
                        late: can_be_late(op_label)
                            && late.is_some_and(|(late_who, late_step, _)| {
                                (late_who, late_step) == (who, number)
                            }),
                    }
                } else {
                    Script::Pass
                }
            });
            let deleting = store(storage.clone(), policy(LONG_DEADLINE, ALWAYS, SWEEP_GRACE));
            let scope = scope.clone();
            let task = AbortOnDropHandle::new(tokio::spawn({
                let deleting = deleting.clone();
                async move {
                    deleting
                        .delete(&scope, &[name(["p-1", "p-2"][who])], &Unlimited)
                        .await
                }
            }));
            Delete {
                store: deleting,
                storage,
                task,
                taken: 0,
                dropped: false,
            }
        });
        let mut case = Case {
            deletes,
            log: Vec::new(),
            schedule: schedule.clone(),
            pending: None,
        };
        if !(settle(&case.deletes[0]).await && settle(&case.deletes[1]).await) {
            return Err("a delete did not reach its first step".to_string());
        }
        let turns = schedule.turns.clone();
        let ran = run_turns(&mut case, schedule, &turns).await;
        if let Err(error) = ran {
            return Err(format!(
                "{error}; schedule {schedule:?}; steps {}",
                order(&case.log)
            ));
        }
        let results =
            futures::future::join_all(case.deletes.iter_mut().map(|delete| &mut delete.task)).await;
        check(shared, &scope, schedule, &case.log, results).await
    }

    /// Runs the turns of the schedule, then each delete to its end, and then lands a late call that
    /// did not land yet.
    async fn run_turns(
        case: &mut Case,
        schedule: &Schedule,
        turns: &[usize],
    ) -> Result<(), String> {
        let next = futures::stream::iter(turns.iter().enumerate())
            .map(Ok)
            .try_fold(&mut *case, |case, (turn, steps)| async move {
                case.take_steps((schedule.first + turn) % 2, *steps).await?;
                Ok::<_, String>(case)
            })
            .await
            .map(|_| (schedule.first + turns.len()) % 2)?;
        case.run_to_end(next).await?;
        case.run_to_end((next + 1) % 2).await?;
        case.land(true).await
    }

    /// Gives the steps of the log as text: the delete and the operation label, with `!` for a call
    /// that gave an error.
    fn order(log: &[Step]) -> String {
        log.iter()
            .map(|step| {
                let mark = match (step.failed, step.effect) {
                    (true, _) => "!",
                    (false, true) => "",
                    (false, false) => "?",
                };
                format!("{}:{}{}", step.delete, step.op_label, mark)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Gives the index in the log at which the delete forgot its snapshot, when its forget reached
    /// the storage.
    fn forgotten_at(log: &[Step], who: usize) -> Option<usize> {
        log.iter().position(|step| {
            step.delete == who && step.effect && is_forget(step.op_label, Path::new(&step.path))
        })
    }

    /// Gives each claim, or marker of a claim, that a delete wrote and that stays, when the first
    /// failed call of that delete, or its drop, came after it took its claim and before its prune
    /// started: before it called for the listing of the packs, and when it made no final marker call.
    /// A claim that the other delete wrote later at the same path is not the claim of the delete. A
    /// blob whose own delete failed is left out, because no call can remove it then, and a claim or a
    /// marker that stays only delays a prune.
    fn kept_claims(log: &[Step], claims: &[String]) -> Vec<String> {
        [0, 1]
            .into_iter()
            .filter_map(|who| {
                let own = |step: &Step| step.delete == who;
                let claimed_at = log.iter().position(|step| {
                    own(step) && step.op_label == "write_claim" && step.effect && !step.failed
                })?;
                let failed_at = log.iter().position(|step| own(step) && step.failed)?;
                let pruning = log[..=failed_at]
                    .iter()
                    .any(|step| own(step) && is_prune_start(step.op_label, &step.path));
                (claimed_at < failed_at && !pruning && !marked_final(log, who))
                    .then_some((who, claimed_at))
            })
            .flat_map(|(who, claimed_at)| {
                let claim = &log[claimed_at].path;
                let taken_again = log[claimed_at..].iter().any(|step| {
                    step.delete != who
                        && step.op_label == "write_claim"
                        && step.effect
                        && step.path == *claim
                });
                let written = log
                    .iter()
                    .filter(|step| {
                        step.delete == who
                            && step.effect
                            && matches!(
                                step.op_label,
                                "write_marker" | "refresh_claim" | "final_marker"
                            )
                    })
                    .map(|step| step.path.clone())
                    .chain((!taken_again).then(|| claim.clone()))
                    .collect::<Vec<_>>();
                let refused = log
                    .iter()
                    .filter(|step| {
                        step.delete == who
                            && step.failed
                            && matches!(step.op_label, "delete_claim" | "delete_marker")
                    })
                    .map(|step| step.path.clone())
                    .collect::<Vec<_>>();
                claims
                    .iter()
                    .filter(move |path| written.contains(path) && !refused.contains(path))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Tells whether the delete called for the final marker of its claim. The guard writes it only
    /// after the prune started. A drop can come right after the start, when the cancel ends the
    /// listing of the packs before it takes a step, so the final marker is then the only step that
    /// shows the start.
    fn marked_final(log: &[Step], who: usize) -> bool {
        log.iter()
            .any(|step| step.delete == who && !step.landed && step.op_label == "final_marker")
    }

    /// Gives the claim of each delete whose prune started and that wrote no ledger entry, when that
    /// claim is gone. A prune that started keeps its claim, so the next prune waits for the hold.
    fn started_claims_gone(log: &[Step], claims: &[String]) -> Vec<String> {
        [0, 1]
            .into_iter()
            .filter_map(|who| {
                let own = |step: &&Step| step.delete == who;
                let claim = log
                    .iter()
                    .filter(own)
                    .find(|step| step.op_label == "write_claim" && step.effect && !step.failed)?;
                let started = log
                    .iter()
                    .filter(own)
                    .any(|step| is_prune_start(step.op_label, &step.path))
                    || marked_final(log, who);
                let ledger_written = log
                    .iter()
                    .filter(own)
                    .any(|step| step.op_label == "write_ledger" && step.effect);
                (started && !ledger_written && !claims.contains(&claim.path))
                    .then(|| claim.path.clone())
            })
            .collect()
    }

    /// Gives each delete whose prune started and that made no call for its final marker. The prune
    /// started when the delete called for the listing of the packs. A delete that called to delete its
    /// claim released it, because each attempt of its prune found a snapshot file gone, so it writes
    /// no final marker.
    fn final_markers_missing(log: &[Step]) -> Vec<usize> {
        [0, 1]
            .into_iter()
            .filter(|who| {
                let own = log
                    .iter()
                    .filter(|step| step.delete == *who && !step.landed);
                let (mut started, mut released, mut marked) = (false, false, false);
                own.for_each(|step| {
                    started |= is_prune_start(step.op_label, &step.path);
                    released |= step.op_label == "delete_claim";
                    marked |= step.op_label == "final_marker";
                });
                started && !released && !marked
            })
            .collect()
    }

    /// Gives each delete that called for a final marker after a final marker call of it that gave no
    /// error.
    fn final_markers_repeated(log: &[Step]) -> Vec<usize> {
        [0, 1]
            .into_iter()
            .filter(|who| {
                log.iter()
                    .filter(|step| {
                        step.delete == *who && !step.landed && step.op_label == "final_marker"
                    })
                    .skip_while(|step| step.failed)
                    .nth(1)
                    .is_some()
            })
            .collect()
    }

    /// Checks the rules on the end state of a case.
    async fn check(
        shared: &Arc<InMemoryBlobStorage>,
        agent: &AgentSnapshots,
        schedule: &Schedule,
        log: &[Step],
        results: Vec<Result<Result<(), CallError>, tokio::task::JoinError>>,
    ) -> Result<(), String> {
        let failed = log.iter().any(|step| step.failed);
        let prune_starts = log
            .iter()
            .enumerate()
            .filter(|(_, step)| step.effect && is_prune_start(step.op_label, &step.path))
            .collect::<Vec<_>>();
        let prunes = prune_starts.len();
        let fail = |rule: &str| {
            Err(format!(
                "{rule}; schedule {schedule:?}; steps {}",
                order(log)
            ))
        };
        if prunes > 1 {
            return fail("more than one prune ran");
        }
        if !failed && prunes != 1 {
            return fail(&format!(
                "no prune ran, although a prune was due and no call failed: {results:?}"
            ));
        }
        if !failed && results.iter().any(|result| !matches!(result, Ok(Ok(())))) {
            return fail(&format!("a delete failed with no failed call: {results:?}"));
        }
        let claims = blobs(&**shared, agent.namespace(), "golem/prune-claims/").await;
        if !failed && !claims.is_empty() {
            return fail(&format!("claims stay: {claims:?}"));
        }
        let released = kept_claims(log, &claims);
        if !released.is_empty() {
            return fail(&format!(
                "a delete that failed after its claim and before its prune kept its claim: {released:?}"
            ));
        }
        let dropped = started_claims_gone(log, &claims);
        if !dropped.is_empty() {
            return fail(&format!(
                "a delete whose prune started and wrote no ledger lost its claim: {dropped:?}"
            ));
        }
        let unmarked = final_markers_missing(log);
        if !unmarked.is_empty() {
            return fail(&format!(
                "a delete whose prune started and kept its claim made no final marker call: {unmarked:?}"
            ));
        }
        let repeated = final_markers_repeated(log);
        if !repeated.is_empty() {
            return fail(&format!(
                "a delete made a final marker call after one that succeeded: {repeated:?}"
            ));
        }
        let records = blobs(&**shared, agent.namespace(), "golem/prune-freed/").await;
        let entries = blobs(&**shared, agent.namespace(), "golem/prune-ledgers/").await;
        let ledger = ledger(shared, agent).await;
        let written = |path: &str| {
            log.iter()
                .find(|step| step.op_label == "write_freed" && step.effect && step.path == path)
                .map(|step| step.delete)
        };
        let lost = log
            .iter()
            .enumerate()
            .filter(|(_, step)| step.op_label == "delete_freed" && step.effect)
            .filter_map(|(deleted_at, step)| {
                let pruner = step.delete;
                let decided_at = log[..deleted_at].iter().rposition(|earlier| {
                    earlier.delete == pruner
                        && matches!(
                            earlier.op_label,
                            "list_freed" | "read_freed" | "list_snapshots"
                        )
                })?;
                let writer = written(&step.path)?;
                let settled = forgotten_at(log, writer).is_some_and(|forgot| forgot < decided_at);
                (!settled).then(|| step.path.clone())
            })
            .collect::<Vec<_>>();
        if !lost.is_empty() {
            return fail(&format!(
                "a prune deleted a record whose snapshot was not gone at its count: {lost:?}"
            ));
        }
        if let Some((start, pruner)) = prune_starts
            .first()
            .map(|(index, step)| (*index, step.delete))
        {
            let wrote_ledger = log[start..].iter().find(|step| {
                step.delete == pruner && step.op_label == "write_ledger" && step.effect
            });
            if let Some(written) = wrote_ledger {
                let written_ms = Path::new(&written.path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.split('-').next())
                    .and_then(|ms| ms.parse::<u64>().ok());
                if ledger.last_prune.map(|last| last.to_millis()) != written_ms {
                    return fail(&format!(
                        "the ledger {ledger:?} is not the entry of the prune {written_ms:?}; entries {entries:?}"
                    ));
                }
            }
            let counted_at = log[..start].iter().rposition(|step| {
                step.delete == pruner
                    && matches!(
                        step.op_label,
                        "list_freed" | "read_freed" | "list_snapshots"
                    )
            });
            let gone = log
                .iter()
                .enumerate()
                .filter(|(_, step)| step.op_label == "write_freed" && step.effect)
                .filter(|(index, _)| counted_at.is_none_or(|counted| *index > counted))
                .filter(|(_, step)| !records.contains(&step.path))
                .map(|(_, step)| step.path.clone())
                .collect::<Vec<_>>();
            if !gone.is_empty() {
                return fail(&format!(
                    "records that the prune did not count are gone: {gone:?}"
                ));
            }
        }
        Ok(())
    }
}

/// The id of a snapshot file that no scope holds, as the content of a record of freed bytes.
const GONE_SNAPSHOT: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// Counts the listings of the packs among the recorded calls.
fn data_listings(calls: &[(&'static str, String)]) -> usize {
    calls
        .iter()
        .filter(|(op_label, _)| *op_label == "list_data")
        .count()
}

/// Makes the ledger one entry with no marked packs and a last prune at the time, and writes a
/// record of one freed byte.
async fn set_last_prune<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    agent: &AgentSnapshots,
    last_prune: golem_common::model::Timestamp,
) {
    storage
        .put_raw(
            "test",
            "test",
            agent.namespace().clone(),
            Path::new("golem/prune-freed/1-test"),
            GONE_SNAPSHOT.as_bytes(),
        )
        .await
        .unwrap();
    storage
        .delete_dir(
            "test",
            "test",
            agent.namespace().clone(),
            Path::new(LEDGERS_PATH),
        )
        .await
        .unwrap();
    put_ledger_entry(
        storage,
        agent,
        &format!("{}-0-test", last_prune.to_millis()),
    )
    .await;
}

/// Counts the prunes among the recorded calls. A prune lists the packs, and no other step of a
/// delete makes that call.
/// How each prune of the store that failed since the last take failed.
fn failed_prunes(store: &RusticSnapshotStore) -> Vec<CallFailure> {
    store
        .take_failed_prunes()
        .iter()
        .map(call_failure)
        .collect()
}

/// Tells whether the operation label is a call of a rustic backend.
fn is_backend_call(op_label: &str) -> bool {
    matches!(
        op_label,
        "stat" | "list" | "read" | "read_range" | "write" | "delete"
    )
}

/// Gives the path of each record of freed bytes of the scope.
async fn records(storage: &InMemoryBlobStorage, agent: &AgentSnapshots) -> Vec<String> {
    blobs(storage, agent.namespace(), "golem/prune-freed/").await
}
