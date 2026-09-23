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

use super::super::holding::reached_deadline;
use super::super::scripted::{Script, ScriptedBlobStorage};
use super::{SnapshotFiles, SnapshotStage, StagedSnapshot, publish, retract};
use bytes::Bytes;
use futures::FutureExt;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use tokio_util::task::TaskTracker;
use uuid::Uuid;

/// The longest time that a test waits for the tasks of a tracker.
const LIMIT: Duration = Duration::from_secs(10);

const SNAPSHOT_PATH: &str =
    "snapshots/cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

fn staged() -> StagedSnapshot {
    StagedSnapshot {
        path: PathBuf::from(SNAPSHOT_PATH).into_boxed_path(),
        content: Bytes::from_static(b"snapshot"),
    }
}

/// Gives the snapshot files of a new namespace over a storage whose script for the publish is
/// `publish`, and the in-memory storage below it.
fn files(
    publish: Script,
    deadline: Duration,
) -> (
    SnapshotFiles,
    Arc<ScriptedBlobStorage>,
    Arc<InMemoryBlobStorage>,
) {
    let inner = Arc::new(InMemoryBlobStorage::new());
    let storage = ScriptedBlobStorage::new(inner.clone(), move |op_label, _| {
        if op_label == "publish" {
            publish
        } else {
            Script::Pass
        }
    });
    (
        SnapshotFiles {
            storage: storage.clone(),
            namespace: BlobStorageNamespace::InitialAgentFiles {
                environment_id: EnvironmentId(Uuid::new_v4()),
            },
            deadline,
        },
        storage,
        inner,
    )
}

/// Gives the content of the snapshot file, when the storage holds it.
async fn stored(files: &SnapshotFiles, inner: &InMemoryBlobStorage) -> Option<Vec<u8>> {
    inner
        .get_raw(
            "test",
            "test",
            files.namespace.clone(),
            Path::new(SNAPSHOT_PATH),
        )
        .await
        .unwrap()
}

#[test]
async fn a_publish_writes_the_staged_file() {
    let (files, _, inner) = files(Script::Pass, Duration::from_secs(2));

    let published = publish(&files, &staged(), &TaskTracker::new()).await;

    assert_eq!(
        (published.is_ok(), stored(&files, &inner).await),
        (true, Some(b"snapshot".to_vec()))
    );
}

#[test]
async fn a_publish_of_a_file_that_is_there_succeeds_and_keeps_the_file() {
    let (files, _, inner) = files(Script::Pass, Duration::from_secs(2));
    let tracker = TaskTracker::new();

    let first = publish(&files, &staged(), &tracker).await;
    let second = publish(&files, &staged(), &tracker).await;

    assert_eq!(
        (first.is_ok(), second.is_ok(), stored(&files, &inner).await),
        (true, true, Some(b"snapshot".to_vec()))
    );
}

#[test]
async fn a_publish_whose_answer_is_lost_deletes_the_file_and_gives_the_error() {
    let (files, storage, inner) = files(Script::LoseTheAnswer, Duration::from_secs(2));

    let published = publish(&files, &staged(), &TaskTracker::new()).await;

    assert_eq!(
        (
            published.map_err(|error| error.to_string()),
            stored(&files, &inner).await,
            storage.calls(),
        ),
        (
            Err("the answer of the call was lost".to_string()),
            None,
            vec![
                ("publish", SNAPSHOT_PATH.to_string()),
                ("retract", SNAPSHOT_PATH.to_string()),
            ]
        )
    );
}

#[test]
async fn a_publish_that_reaches_the_deadline_deletes_the_file_that_the_storage_wrote() {
    let (files, _, inner) = files(Script::NeverAnswer, Duration::from_millis(100));

    let published = publish(&files, &staged(), &TaskTracker::new()).await;

    assert_eq!(
        (
            published
                .as_ref()
                .is_err_and(|error| reached_deadline(error.as_ref())),
            stored(&files, &inner).await
        ),
        (true, None)
    );
}

#[test]
async fn a_publish_that_the_caller_drops_deletes_the_file_in_a_task_of_the_tracker() {
    let (files, _, inner) = files(Script::NeverAnswer, Duration::from_secs(60));
    let tracker = TaskTracker::new();

    let dropped = publish(&files, &staged(), &tracker).now_or_never();
    let written_before_the_drop = stored(&files, &inner).await;
    tracker.close();
    let waited = tokio::time::timeout(LIMIT, tracker.wait()).await;

    assert_eq!(
        (
            dropped.is_none(),
            written_before_the_drop,
            waited.is_ok(),
            stored(&files, &inner).await
        ),
        (true, Some(b"snapshot".to_vec()), true, None)
    );
}

#[test]
async fn a_publish_that_returns_keeps_the_file_when_the_tasks_of_the_tracker_end() {
    let (files, _, inner) = files(Script::Pass, Duration::from_secs(2));
    let tracker = TaskTracker::new();

    let published = publish(&files, &staged(), &tracker).await;
    tracker.close();
    let waited = tokio::time::timeout(LIMIT, tracker.wait()).await;

    assert_eq!(
        (
            published.is_ok(),
            waited.is_ok(),
            stored(&files, &inner).await
        ),
        (true, true, Some(b"snapshot".to_vec()))
    );
}

#[test]
async fn a_retract_of_a_path_without_a_file_succeeds() {
    let (files, _, _) = files(Script::Pass, Duration::from_secs(2));

    assert!(retract(&files, Path::new(SNAPSHOT_PATH)).await.is_ok());
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
