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

//! Shared inputs and storage fixtures for prune tests.

use super::super::blob_io::SnapshotFiles;
use super::super::test_support::files_of;
use super::records::{PruneLedger, parse_freed};
use crate::filesystem_snapshot::agent_snapshots::AgentSnapshots;
use golem_common::model::Timestamp;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

/// Runs a repository prune directly for shared race fixtures without a deletion's claim gate.
pub(in crate::filesystem_snapshot::backend::rustic) fn prune_repository(
    backend: Arc<super::super::blob_backend::BlobBackend>,
    key: &super::super::repository::RepositoryKey,
    settings: &super::PruneSettings,
) -> anyhow::Result<Option<super::PruneReport>> {
    super::prune(backend, key, settings)
}

pub(super) const GRACE: Duration = Duration::from_secs(15 * 60);
pub(super) const DEADLINE: Duration = Duration::from_secs(2);

/// The grace period and the margin for clock skew, in milliseconds.
pub(super) const HELD_MILLIS: u64 = 15 * 60 * 1000 + 2 * 60 * 1000;

/// The grace period less one deadline, then the skew margin and two deadlines.
pub(super) const HOLD_MILLIS: u64 = HELD_MILLIS + 2 * 1000;

pub(super) fn at(millis: u64) -> Timestamp {
    Timestamp::from(millis)
}

pub(super) fn ledger(last_prune_millis: Option<u64>, awaiting_removal: bool) -> PruneLedger {
    PruneLedger {
        last_prune: last_prune_millis.map(Timestamp::from),
        awaiting_removal,
    }
}

pub(super) fn new_files() -> SnapshotFiles {
    files_over(Arc::new(InMemoryBlobStorage::new()))
}

pub(super) fn files_over(storage: Arc<dyn BlobStorage>) -> SnapshotFiles {
    files_with_cancel(storage, tokio_util::sync::CancellationToken::new())
}

pub(super) fn files_with_cancel(
    storage: Arc<dyn BlobStorage>,
    cancel: tokio_util::sync::CancellationToken,
) -> SnapshotFiles {
    files_of(
        storage,
        BlobStorageNamespace::InitialAgentFiles {
            environment_id: EnvironmentId(Uuid::new_v4()),
        },
        DEADLINE,
        cancel,
    )
}

/// Gives the sum of the bytes in the names of the records of freed bytes of the scope.
pub(in crate::filesystem_snapshot::backend::rustic) async fn freed<S: BlobStorage + 'static>(
    storage: &Arc<S>,
    agent: &AgentSnapshots,
) -> u64 {
    let listed = storage
        .list_blobs_below(
            "test",
            "test",
            agent.namespace().clone(),
            Path::new("golem/prune-freed"),
        )
        .await
        .unwrap();
    listed
        .iter()
        .filter_map(|blob| parse_freed(blob.path.file_name()?.to_str()?))
        .fold(0, u64::saturating_add)
}
