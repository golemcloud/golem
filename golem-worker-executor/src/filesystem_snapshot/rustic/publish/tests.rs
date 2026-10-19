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

use super::super::files::{LateWrites, SnapshotFiles, lands_by};
use super::super::runs::{RunEnd, now};
use super::super::tests::files_of;
use super::super::tests::scripted::{Script, ScriptedBlobStorage};
use super::{
    MIN_PUBLISH_TRY, Published, SnapshotStage, StagedSnapshot, backup_end, index_read_bound,
    publish, publish_try,
};
use bytes::Bytes;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use test_r::{test, timeout};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const SNAPSHOT_PATH: &str =
    "snapshots/cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

fn staged() -> StagedSnapshot {
    StagedSnapshot {
        path: Arc::from(Path::new(SNAPSHOT_PATH)),
        content: Bytes::from_static(b"snapshot"),
    }
}

/// Gives the snapshot files of a new namespace over a scripted storage whose `n`-th publish try
/// follows `script(n)`, counted from 1, with the tries of a blob call of the store. It also gives
/// the snapshot files of the same namespace over the in-memory storage below it.
fn files(
    script: impl Fn(usize) -> Script + Send + Sync + 'static,
    deadline: Duration,
) -> (
    SnapshotFiles,
    Arc<ScriptedBlobStorage>,
    SnapshotFiles,
    Arc<LateWrites>,
) {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage =
        ScriptedBlobStorage::with_state(inner.clone(), 0usize, move |tries, op_label, _| {
            match op_label {
                "publish" => (script(tries + 1), tries + 1),
                _ => (Script::Pass, *tries),
            }
        });
    let namespace = BlobStorageNamespace::InitialAgentFiles {
        environment_id: EnvironmentId(Uuid::new_v4()),
    };
    let over = |storage: Arc<dyn BlobStorage>| {
        files_of(
            storage,
            namespace.clone(),
            deadline,
            CancellationToken::new(),
        )
        .with_tries(super::super::files::IN_CALL_TRIES)
    };
    let late = Arc::new(LateWrites::default());
    (
        over(storage.clone()).recording(late.clone()),
        storage,
        over(inner),
        late,
    )
}

/// Gives the content of the snapshot file, when the storage below the script holds it.
async fn stored(inner: &SnapshotFiles) -> Option<Vec<u8>> {
    inner.get("test", Path::new(SNAPSHOT_PATH)).await.unwrap()
}

/// Gives what a publish gave without its failure, and whether a try of it can still land.
fn shape(published: &Published, late: &LateWrites) -> (Option<RunEnd>, bool) {
    let can_land = late.latest().is_some();
    match published {
        Published::Written => (None, can_land),
        Published::NotWritten { end, .. } => (Some(*end), can_land),
    }
}

#[test]
#[timeout("60s")]
async fn a_publish_writes_the_staged_file() {
    let (files, _, inner, late) = files(|_| Script::Pass, Duration::from_secs(2));

    let published = publish(&files, &staged(), None, &CancellationToken::new()).await;

    assert_eq!(
        (shape(&published, &late), stored(&inner).await),
        ((None, false), Some(b"snapshot".to_vec()))
    );
}

#[test]
#[timeout("60s")]
async fn a_publish_of_a_file_that_is_there_succeeds_and_keeps_the_file() {
    let (files, _, inner, late) = files(|_| Script::Pass, Duration::from_secs(2));

    let first = publish(&files, &staged(), None, &CancellationToken::new()).await;
    let second = publish(&files, &staged(), None, &CancellationToken::new()).await;

    assert_eq!(
        (
            shape(&first, &late),
            shape(&second, &late),
            stored(&inner).await
        ),
        ((None, false), (None, false), Some(b"snapshot".to_vec()))
    );
}

#[test]
#[timeout("60s")]
async fn a_try_whose_answer_was_lost_is_recorded_and_a_later_try_counts_the_landed_file_as_written()
{
    let (files, storage, inner, late) = files(
        |tried| match tried {
            1 => Script::LoseTheAnswer,
            _ => Script::AnswerAlreadyExists,
        },
        Duration::from_secs(2),
    );

    let published = publish(&files, &staged(), None, &CancellationToken::new()).await;

    assert_eq!(
        (
            shape(&published, &late),
            stored(&inner).await,
            storage.calls().len()
        ),
        // The lost try is recorded although a later try counts the file as written: it can
        // still land, and the save waits for it.
        ((None, true), Some(b"snapshot".to_vec()), 2)
    );
}

#[test]
#[timeout("60s")]
async fn a_publish_whose_tries_all_lose_their_answer_keeps_the_file_and_gives_a_late_window() {
    let (files, storage, inner, late) = files(|_| Script::LoseTheAnswer, Duration::from_secs(2));
    let started = now();

    let published = publish(&files, &staged(), None, &CancellationToken::new()).await;
    let until = lands_by(late.latest(), Duration::from_secs(2));

    assert_eq!(
        (
            shape(&published, &late),
            stored(&inner).await,
            storage.calls().len(),
            until.is_some_and(|until| until >= started + Duration::from_secs(2)),
        ),
        (
            (Some(RunEnd::CallFailed), true),
            Some(b"snapshot".to_vec()),
            3,
            true
        )
    );
}

#[test]
#[timeout("60s")]
async fn a_publish_whose_cancel_fired_sends_no_try() {
    let (files, storage, inner, late) = files(|_| Script::Pass, Duration::from_secs(2));
    let cancel = CancellationToken::new();
    cancel.cancel();

    let published = publish(&files, &staged(), None, &cancel).await;

    assert_eq!(
        (
            shape(&published, &late),
            stored(&inner).await,
            storage.calls().len()
        ),
        ((Some(RunEnd::Cancelled), false), None, 0)
    );
}

#[test]
#[timeout("60s")]
async fn a_publish_with_no_time_before_the_bound_sends_no_try() {
    let (files, storage, inner, late) = files(|_| Script::Pass, Duration::from_secs(2));
    let bound = now() + Duration::from_secs(2);

    let published = publish(&files, &staged(), Some(bound), &CancellationToken::new()).await;

    assert_eq!(
        (
            shape(&published, &late),
            stored(&inner).await,
            storage.calls().len()
        ),
        ((Some(RunEnd::BoundPassed), false), None, 0)
    );
}

#[test]
fn the_earliest_index_read_is_the_grace_less_two_deadlines_and_one_period_after_t0() {
    let t0 = Instant::now();
    let minute = Duration::from_secs(60);
    let grace = Duration::from_secs(15 * 60);
    let period = Duration::from_secs(210);
    let bound = index_read_bound(t0, grace, minute, period);

    assert_eq!(
        (
            bound.duration_since(t0),
            backup_end(bound, minute).duration_since(t0),
            index_read_bound(t0, Duration::from_secs(60), minute, period),
        ),
        (
            Duration::from_secs(9 * 60 + 30),
            Duration::from_secs(7 * 60 + 30),
            t0,
        )
    );
}

#[test]
fn a_publish_try_is_cut_at_the_least_of_the_window_and_the_bound_and_needs_at_least_one_second() {
    let now = Instant::now();
    let minute = Duration::from_secs(60);
    let bound = now + Duration::from_secs(150);

    assert_eq!(
        [
            publish_try(now, Duration::ZERO, Some(bound), minute),
            publish_try(now, Duration::from_secs(10), Some(bound), minute),
            publish_try(
                now,
                Duration::ZERO,
                Some(now + Duration::from_secs(70)),
                minute
            ),
            publish_try(
                now,
                Duration::ZERO,
                Some(now + minute + MIN_PUBLISH_TRY),
                minute
            ),
            publish_try(
                now,
                Duration::ZERO,
                Some(now + minute + MIN_PUBLISH_TRY - Duration::from_millis(1)),
                minute
            ),
            publish_try(now, minute - MIN_PUBLISH_TRY, Some(bound), minute),
            publish_try(now, minute - Duration::from_millis(1), Some(bound), minute),
            publish_try(now, Duration::from_secs(59), None, minute),
            publish_try(now, minute, None, minute),
        ],
        [
            Some(minute),
            Some(Duration::from_secs(50)),
            Some(Duration::from_secs(10)),
            Some(MIN_PUBLISH_TRY),
            None,
            Some(MIN_PUBLISH_TRY),
            None,
            Some(Duration::from_secs(1)),
            None,
        ]
    );
}

#[test]
fn a_stage_keeps_one_file_until_it_is_taken() {
    let stage = SnapshotStage::default();
    let other = StagedSnapshot {
        content: Bytes::from_static(b"other"),
        ..staged()
    };

    let first = stage.keep(staged());
    let second = stage.keep(other.clone());
    let taken = stage.take();
    let taken_again = stage.take();

    assert_eq!(
        (first, second, taken, taken_again),
        (Ok(()), Err(other), Some(staged()), None)
    );
}

#[test]
#[timeout("60s")]
async fn a_stop_of_the_tries_ends_the_wait_between_two_tries_of_a_publish() {
    // The first try is refused, and the tries stop 20 ms into the wait of 250 ms before the next
    // try. The publish ends then, with no second try.
    let (files, storage, _, late) = files(|_| Script::Refuse, Duration::from_secs(2));
    let retry_stop = CancellationToken::new();
    let files = files.with_retry_stop(retry_stop.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        retry_stop.cancel();
    });
    let started = now();

    let published = publish(&files, &staged(), None, &CancellationToken::new()).await;

    assert_eq!(
        (
            shape(&published, &late),
            storage.calls().len(),
            now().duration_since(started) < Duration::from_millis(200)
        ),
        ((Some(RunEnd::Cancelled), true), 1, true)
    );
}
