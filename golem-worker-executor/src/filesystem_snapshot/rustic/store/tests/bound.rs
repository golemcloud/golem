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

//! The publish bound of a save and the lease of a prune, judged by the storage.
//!
//! The time is real, with a short grace period and a short deadline, because the backup of a save
//! runs on blocking threads, where the paused time of tokio would move on by itself. So a case
//! lasts more than one grace period, and the sweep runs few cases: 8 with the marking prune
//! before the save and 8 with it during the save, in two tests that run at the same time.
//!
//! A case saves a name of an agent whose packs a prune of the same agent marks before or during
//! the save. One grace period after the end of that prune, a second prune of the agent runs,
//! which can delete each pack that the first one marked. The save and the prunes go through two
//! stores over the same storage, as two executors do, so the record of each store holds only its
//! own calls. The cases generate slow and refused pack writes, failed runs of the save, a late
//! index write, late deletes of the marking prune, slow backend calls of the marking prune, and
//! refresh writes of its claim that fail, are slow, or lose their answer.
//!
//! The judges read the record of the calls and the answers:
//! - each write of the save lands before the first index read of the second prune, or the save
//!   answers `Failed` and lands nothing after that read;
//! - each write of the save lands before the bound of the formula;
//! - after the second prune, a saved name restores the whole tree, and a failed save left no name;
//! - the marking prune makes no storage call later than one lease span after the start of its
//!   newest marker write that succeeded.
//!
//! The sweep does not crash the executor at a step; the shell tests of the runs cover a stop at
//! each wait.

use super::super::super::prune::{lease_span, refresh_period};
use super::super::super::publish::index_read_bound;
use super::super::super::tests::scripted::CallEvent;
use super::*;
use crate::filesystem_snapshot::{RunSlots, Slot, Withdrawal};
use futures::future::BoxFuture;
use golem_common::model::RetryConfig;
use pretty_assertions::assert_eq;
use proptest::prelude::*;
use std::sync::Mutex;
use std::time::Instant;
use test_r::{test, timeout};

/// The grace period of a marked pack in these tests.
const GRACE: Duration = Duration::from_secs(10);

/// The storage call deadline in these tests: the shortest that the configuration allows, because a
/// publish needs one deadline of [`super::super::super::publish::MIN_PUBLISH_TRY`] or more.
const DEADLINE: Duration = Duration::from_secs(1);

/// The storage calls of the backend of rustic, which a prune makes inside its lease.
const BACKEND_CALLS: [&str; 6] = ["stat", "list", "read", "read_range", "write", "delete"];

/// The policy of these tests: the bound is on, each blob call has its tries, and a failed run
/// runs again after a short wait.
fn bound_policy() -> StorePolicy {
    StorePolicy {
        deadline: DEADLINE,
        prune: PruneSettings {
            fast_repack: true,
            keep_delete: GRACE,
        },
        prune_threshold: ALWAYS,
        retry: RetryConfig {
            max_attempts: 5,
            min_delay: Duration::from_millis(50),
            max_delay: Duration::from_millis(200),
            multiplier: 2.0,
            max_jitter_factor: None,
        },
        publish_bound: PublishBound::On,
        in_call_tries: super::super::super::files::IN_CALL_TRIES,
        ..StorePolicy::from_config(&config())
    }
}

/// A limiter without a bound that keeps the instant of its first grant. The shell takes the
/// instant of the first slot of a call after the grant, so this instant is not later.
#[derive(Default)]
struct FirstGrant(Mutex<Option<Instant>>);

impl RunSlots for FirstGrant {
    fn take(&self, immediate: bool) -> BoxFuture<'_, Result<Slot, Withdrawal>> {
        Box::pin(async move {
            let slot = crate::filesystem_snapshot::Unlimited
                .take(immediate)
                .await?;
            self.0
                .lock()
                .unwrap()
                .get_or_insert_with(super::super::super::runs::now);
            Ok(slot)
        })
    }

    fn withdrawn(&self) -> BoxFuture<'_, Withdrawal> {
        Box::pin(std::future::pending())
    }
}

/// What the refresh writes of the claim of the marking prune do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Refresh {
    Pass,
    /// Each refresh write fails, and never lands.
    Refused,
    /// Each refresh write takes this time.
    Slow(Duration),
    /// Each refresh write lands and loses its answer.
    LostAnswer,
}

/// What a case generates.
#[derive(Clone, Copy, Debug)]
struct Case {
    /// The time of each pack write of the save.
    pack_delay: Duration,
    /// The pack writes of the save that the storage refuses first, so its first runs fail.
    refused_packs: usize,
    /// The first index write of the save gets no answer and lands after this time.
    late_index: Duration,
    /// Each index delete of the marking prune gets no answer and lands after this time.
    late_delete: Duration,
    /// The time between the start of the first of the save and the marking prune and the start of
    /// the other.
    offset: Duration,
    /// Whether the marking prune starts after the save.
    marking_after: bool,
    /// What the refresh writes of the marking prune do.
    refresh: Refresh,
    /// The time of each backend call of the marking prune.
    prune_call_delay: Duration,
}

fn case_strategy(marking_after: bool) -> impl Strategy<Value = Case> {
    (
        prop_oneof![Just(0u64), 100u64..1000, 1000u64..3000, 3000u64..7000],
        0usize..3,
        0u64..1000,
        0u64..1000,
        0u64..1000,
        prop_oneof![
            Just(Refresh::Pass),
            Just(Refresh::Refused),
            (0u64..1000).prop_map(|millis| Refresh::Slow(Duration::from_millis(millis))),
            Just(Refresh::LostAnswer),
        ],
        prop_oneof![Just(0u64), 50u64..300],
    )
        .prop_map(
            move |(pack, refused_packs, late_index, late_delete, offset, refresh, prune_call)| {
                Case {
                    pack_delay: Duration::from_millis(pack),
                    refused_packs,
                    late_index: Duration::from_millis(late_index),
                    late_delete: Duration::from_millis(late_delete),
                    offset: Duration::from_millis(offset),
                    marking_after,
                    refresh,
                    prune_call_delay: Duration::from_millis(prune_call),
                }
            },
        )
}

/// The flags of the phases of a case, which the rules of the storages read.
#[derive(Default)]
struct Phases {
    /// The save and the marking prune run.
    racing: AtomicBool,
    /// The pack writes of the save that the storage refused.
    refused_packs: AtomicUsize,
    /// The first index write of the save got its script.
    first_index: AtomicBool,
}

/// Gives a script that passes after `delay`.
fn delayed(delay: Duration) -> Script {
    if delay.is_zero() {
        Script::Pass
    } else {
        Script::Delay(delay)
    }
}

/// Gives the script of a call of the store of the save while the save runs.
fn save_script(case: Case, phases: &Phases, op_label: &str, path: &Path) -> Script {
    if !phases.racing.load(Ordering::SeqCst) {
        return Script::Pass;
    }
    match op_label {
        "write" if path.starts_with("data") => {
            if phases.refused_packs.fetch_add(1, Ordering::SeqCst) < case.refused_packs {
                Script::Refuse
            } else {
                delayed(case.pack_delay)
            }
        }
        "write"
            if path.starts_with("index") && !phases.first_index.swap(true, Ordering::SeqCst) =>
        {
            Script::LandAfter(case.late_index)
        }
        _ => Script::Pass,
    }
}

/// Gives the script of a call of the store of the prunes while the marking prune runs.
fn prune_script(case: Case, phases: &Phases, op_label: &str, path: &Path) -> Script {
    if !phases.racing.load(Ordering::SeqCst) {
        return Script::Pass;
    }
    match (op_label, case.refresh) {
        ("delete", _) if path.starts_with("index") => Script::LandAfter(case.late_delete),
        ("refresh_claim", Refresh::Refused) => Script::Refuse,
        ("refresh_claim", Refresh::Slow(delay)) => delayed(delay),
        ("refresh_claim", Refresh::LostAnswer) => Script::LoseTheAnswer,
        (label, _) if BACKEND_CALLS.contains(&label) => delayed(case.prune_call_delay),
        _ => Script::Pass,
    }
}

/// Each write of the save that started at or after `t0` and its late landings, judged against
/// the bound of a save whose first slot was at `t0`: a call ends one deadline before the bound,
/// and a late change lands before it.
fn writes_after_the_bound(
    events: &[CallEvent],
    saving: &BlobStorageNamespace,
    t0: Instant,
) -> Vec<String> {
    let bound = index_read_bound(t0, GRACE, DEADLINE, refresh_period(GRACE, DEADLINE));
    events
        .iter()
        .filter(|event| {
            event.namespace == *saving
                && event.started >= t0
                && matches!(event.op_label, "write" | "publish")
        })
        .filter(|event| {
            if event.landed_late {
                event.ended > bound
            } else {
                event.ended + DEADLINE > bound
            }
        })
        .map(|event| {
            format!(
                "{} {} {} ms after the first slot, the bound is {} ms",
                if event.landed_late {
                    "the late"
                } else {
                    "the call"
                },
                event.op_label,
                event.ended.duration_since(t0).as_millis(),
                bound.duration_since(t0).as_millis()
            )
        })
        .collect()
}

/// Each write of the save that landed after `read`, the first index read of the second prune, when
/// the save answered success, or when a write of the save landed after it at all. A save that
/// answered `Failed` lands nothing after the read.
fn writes_after_the_second_read(
    events: &[CallEvent],
    saving: &BlobStorageNamespace,
    read: Instant,
) -> Vec<String> {
    events
        .iter()
        .filter(|event| event.namespace == *saving && matches!(event.op_label, "write" | "publish"))
        .filter(|event| event.ended > read)
        .map(|event| {
            format!(
                "the save {} {} {} ended {} ms after the first index read of the second prune",
                if event.landed_late { "late" } else { "call" },
                event.op_label,
                event.path.display(),
                event.ended.duration_since(read).as_millis()
            )
        })
        .collect()
}

/// Each storage call of the prune that started before `until` and ended later than one lease span
/// after the start of the newest marker write that had succeeded when the call started.
fn calls_after_the_lease(
    events: &[CallEvent],
    pruning: &BlobStorageNamespace,
    refreshes_fail: bool,
    until: Instant,
) -> Vec<String> {
    let of_prune = events
        .iter()
        .filter(|event| event.namespace == *pruning && event.started < until)
        .collect::<Vec<_>>();
    let markers = of_prune
        .iter()
        .filter(|event| {
            event.op_label == "write_claim"
                || (event.op_label == "refresh_claim" && !refreshes_fail)
        })
        .collect::<Vec<_>>();
    let Some(claimed) = markers.iter().map(|marker| marker.started).min() else {
        return Vec::new();
    };
    let span = lease_span(GRACE, DEADLINE);
    of_prune
        .iter()
        .filter(|event| {
            event.started >= claimed
                && (BACKEND_CALLS.contains(&event.op_label) || event.op_label == "write_ledger")
        })
        .filter_map(|event| {
            let expiry = markers
                .iter()
                .filter(|marker| marker.ended <= event.started)
                .map(|marker| marker.started + span)
                .max()?;
            (event.ended > expiry).then(|| {
                format!(
                    "the prune call {} {} ended {} ms after the end of its lease",
                    event.op_label,
                    event.path.display(),
                    event.ended.duration_since(expiry).as_millis()
                )
            })
        })
        .collect()
}

/// Runs one case and gives each broken judgement.
async fn broken(case: Case) -> Vec<String> {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let scope = new_scope();
    let phases = Arc::new(Phases::default());
    let save_storage = ScriptedBlobStorage::new(inner.clone(), {
        let phases = phases.clone();
        move |op_label, path| save_script(case, &phases, op_label, path)
    });
    let prune_storage = ScriptedBlobStorage::new(inner, {
        let phases = phases.clone();
        move |op_label, path| prune_script(case, &phases, op_label, path)
    });
    let saving_store = store(save_storage.clone(), bound_policy());
    let pruning_store = store(prune_storage.clone(), bound_policy());
    save_each(&pruning_store, &scope, &["p-1", "p-2", "p-4"]).await;

    // The marking prune of the delete of `p-1` marks the packs of `p-1` before or during the save
    // of `p-3`, which reuses the blob of `p-1`.
    phases.racing.store(true, Ordering::SeqCst);
    let slots = FirstGrant::default();
    let tree = one_file_tree("p-1");
    let saving = async {
        saving_store
            .save(
                &scope,
                &name("p-3"),
                tree.path(),
                None,
                crate::filesystem_snapshot::never_cancelled(),
                &slots,
            )
            .await
    };
    let marking = async {
        let marked = pruning_store
            .delete(
                &scope,
                &[name("p-1")],
                &crate::filesystem_snapshot::Unlimited,
            )
            .await;
        (marked, Instant::now())
    };
    let later = |late: bool| tokio::time::sleep(if late { case.offset } else { Duration::ZERO });
    let (saved, (marked, marking_end)) = tokio::join!(
        async {
            later(!case.marking_after).await;
            saving.await
        },
        async {
            later(case.marking_after).await;
            marking.await
        }
    );
    phases.racing.store(false, Ordering::SeqCst);
    // A marking prune that its lease stopped keeps its claim for the whole hold, so no second
    // prune of the agent runs in the case.
    let marking_pruned = pruning_store.take_failed_prunes().is_empty();

    // One grace period after the end of the marking prune, the second prune can delete each pack
    // that the marking prune marked.
    tokio::time::sleep_until(tokio::time::Instant::from_std(
        marking_end + GRACE + Duration::from_millis(100),
    ))
    .await;
    let second_start = Instant::now();
    let deleted = pruning_store
        .delete(
            &scope,
            &[name("p-2")],
            &crate::filesystem_snapshot::Unlimited,
        )
        .await;
    // Each late change lands within one deadline.
    tokio::time::sleep(DEADLINE * 2).await;
    let restored = match &saved {
        Ok(_) => Some(restored_listing(&pruning_store, &scope, &name("p-3")).await),
        Err(_) => None,
    };
    let stat = pruning_store.stat(&scope, &name("p-3")).await;
    saving_store.shut_down().await;
    pruning_store.shut_down().await;

    let (save_events, prune_events) = (save_storage.events(), prune_storage.events());
    let t0 = *slots.0.lock().unwrap();
    let second_read = prune_events
        .iter()
        .filter(|event| {
            event.started >= second_start
                && event.path.starts_with("index")
                && matches!(event.op_label, "list" | "read")
        })
        .map(|event| event.started)
        .min();
    let second_pruned = prune_events
        .iter()
        .any(|event| event.started >= second_start && event.op_label == "write_claim");
    // Without a second prune, a save that reused a blob of a pack that the marking prune marked
    // gives `Failed` with the cause of a marked pack, as the doc of `restore` says.
    let saved_well = match (&saved, &restored, &stat) {
        (Ok(_), Some(Ok(restored)), _) if *restored == listing(tree.path()) => Vec::new(),
        (Ok(_), Some(Err(RestoreFailure::Failed(failed))), _)
            if !second_pruned && failed.cause().to_string().contains("marked for deletion") =>
        {
            Vec::new()
        }
        (Err(_), _, Ok(None)) => Vec::new(),
        _ => vec![format!(
            "the save gave {saved:?}, the restore after the second prune {restored:?} and the stat {stat:?}"
        )],
    };
    let refreshes_fail = case.refresh == Refresh::Refused;
    [
        marked
            .err()
            .map(|error| format!("the marking delete failed: {error:?}"))
            .into_iter()
            .collect(),
        deleted
            .err()
            .map(|error| format!("the second delete failed: {error:?}"))
            .into_iter()
            .collect(),
        (marking_pruned && !second_pruned)
            .then(|| "the second delete did not prune".to_string())
            .into_iter()
            .collect(),
        saved_well,
        second_read
            .map(|read| writes_after_the_second_read(&save_events, &scope.0, read))
            .unwrap_or_default(),
        t0.map(|t0| writes_after_the_bound(&save_events, &scope.0, t0))
            .unwrap_or_default(),
        calls_after_the_lease(&prune_events, &scope.0, refreshes_fail, marking_end),
    ]
    .concat()
}

/// Runs `case` on a runtime of its own.
fn broken_in_runtime(case: Case) -> Vec<String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(broken(case))
}

/// Runs 8 cases of `strategy` on a thread of the blocking pool, each on a runtime of its own, and
/// gives the first case that breaks the bound, shrunk for at most 2 min. A case runs in real time,
/// for about 20 s.
async fn no_case_breaks_the_bound(strategy: impl Strategy<Value = Case> + Send + 'static) {
    let outcome = tokio::task::spawn_blocking(move || {
        proptest::test_runner::TestRunner::new(ProptestConfig {
            cases: 8,
            max_shrink_time: 120_000,
            failure_persistence: None,
            ..ProptestConfig::default()
        })
        .run(&strategy, |case| {
            prop_assert_eq!(broken_in_runtime(case), Vec::<String>::new(), "{:?}", case);
            Ok(())
        })
        .map_err(|error| error.to_string())
    })
    .await;

    assert!(matches!(outcome, Ok(Ok(()))), "{outcome:?}");
}

#[test]
#[timeout("6m")]
async fn every_write_of_a_save_lands_before_the_bound() {
    no_case_breaks_the_bound(case_strategy(true)).await;
}

#[test]
#[timeout("6m")]
async fn every_write_of_a_save_after_a_marking_prune_lands_before_the_bound() {
    no_case_breaks_the_bound(case_strategy(false)).await;
}

#[test]
fn the_judges_of_the_bound_find_a_write_after_the_bound_and_a_call_after_the_lease() {
    // The judges themselves: a write of the save that ends after one deadline before the bound,
    // a late change after the bound, and a prune call after the lease of its only marker.
    let saving = BlobStorageNamespace::InitialAgentFiles {
        environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::new_v4()),
    };
    let pruning = BlobStorageNamespace::InitialAgentFiles {
        environment_id: golem_common::model::environment::EnvironmentId(uuid::Uuid::new_v4()),
    };
    let t0 = Instant::now();
    let bound = index_read_bound(t0, GRACE, DEADLINE, refresh_period(GRACE, DEADLINE));
    let event =
        |namespace: &BlobStorageNamespace, op_label, started, ended, landed_late| CallEvent {
            namespace: namespace.clone(),
            op_label,
            path: Path::new("index/0").into(),
            started,
            ended,
            landed_late,
        };
    let claim = event(&pruning, "write_claim", t0, t0, false);
    let span = lease_span(GRACE, DEADLINE);
    let events = [
        event(&saving, "write", t0, bound - DEADLINE, false),
        event(
            &saving,
            "write",
            t0,
            bound - DEADLINE + Duration::from_millis(1),
            false,
        ),
        event(&saving, "publish", bound, bound, true),
        event(
            &saving,
            "publish",
            bound,
            bound + Duration::from_millis(1),
            true,
        ),
        claim,
        event(&pruning, "read", t0 + span, t0 + span, false),
        event(
            &pruning,
            "read",
            t0 + span,
            t0 + span + Duration::from_millis(1),
            false,
        ),
    ];

    assert_eq!(
        (
            writes_after_the_bound(&events, &saving, t0).len(),
            calls_after_the_lease(&events, &pruning, true, t0 + span + DEADLINE).len(),
            writes_after_the_second_read(&events, &saving, bound - DEADLINE).len(),
        ),
        (2, 1, 3)
    );
}
