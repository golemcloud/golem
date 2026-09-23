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

//! When a delete of the store prunes the repository of its scope.
//!
//! The scope keeps a small ledger blob next to the files of the repository. The ledger holds the
//! packed bytes that deleted snapshots added since the last prune, the time of the last prune, and
//! whether that prune marked packs that a later prune removes. The ledger is advice: two deletes at
//! the same time can lose a count, and that only makes a prune come later.

use super::backend::answer_within;
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use tracing::warn;

/// The target label of each blob storage call on the ledger.
const TARGET_LABEL: &str = "filesystem_snapshot";

/// The path of the ledger blob, relative to the root of the namespace of the scope.
pub(super) const LEDGER_PATH: &str = "golem/prune-ledger";

/// What the scope did since its last prune.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PruneLedger {
    /// The packed bytes that the deleted snapshots added, since the last prune.
    pub(super) freed_bytes: u64,
    /// The time of the last prune, in milliseconds since the Unix epoch.
    pub(super) last_prune_millis: Option<u64>,
    /// Whether the last prune marked packs that a later prune removes.
    pub(super) awaiting_removal: bool,
}

impl PruneLedger {
    /// Gives the ledger after a delete of snapshots that added `bytes` packed bytes.
    pub(super) fn with_deleted(self, bytes: u64) -> Self {
        Self {
            freed_bytes: self.freed_bytes.saturating_add(bytes),
            ..self
        }
    }

    /// Gives the ledger after a prune at `now` that marked packs or not.
    pub(super) fn after_prune(now: Timestamp, marked_packs: bool) -> Self {
        Self {
            freed_bytes: 0,
            last_prune_millis: Some(now.to_millis()),
            awaiting_removal: marked_packs,
        }
    }
}

/// Tells whether a prune is due at `now`.
///
/// A prune is due when the grace period passed since the last prune, and the freed bytes reach the
/// threshold or the last prune marked packs. A threshold of zero counts as one byte, so a prune
/// never runs for a scope that freed nothing and marked nothing.
pub(super) fn prune_due(
    ledger: &PruneLedger,
    now: Timestamp,
    threshold: u64,
    grace: Duration,
) -> bool {
    let grace_passed = ledger.last_prune_millis.is_none_or(|last| {
        now.to_millis() >= last.saturating_add(u64::try_from(grace.as_millis()).unwrap_or(u64::MAX))
    });
    let work = ledger.freed_bytes >= threshold.max(1) || ledger.awaiting_removal;
    grace_passed && work
}

/// Reads the ledger of the scope. A scope without a ledger gives an empty ledger, and so does a
/// ledger that does not parse, because the ledger is advice.
pub(super) async fn read_ledger(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    deadline: Duration,
) -> anyhow::Result<PruneLedger> {
    let content = answer_within(
        deadline,
        storage.get_raw(
            TARGET_LABEL,
            "read_ledger",
            namespace.clone(),
            Path::new(LEDGER_PATH),
        ),
    )
    .await?;
    Ok(content.map_or_else(PruneLedger::default, |content| {
        serde_json::from_slice(&content).unwrap_or_else(|error| {
            warn!(
                error = %error,
                "The prune ledger of a filesystem snapshot scope does not parse, so it starts again"
            );
            PruneLedger::default()
        })
    }))
}

/// Writes the ledger of the scope over the ledger that was there.
pub(super) async fn write_ledger(
    storage: &dyn BlobStorage,
    namespace: &BlobStorageNamespace,
    deadline: Duration,
    ledger: &PruneLedger,
) -> anyhow::Result<()> {
    let content = serde_json::to_vec(ledger)?;
    answer_within(
        deadline,
        storage.put_raw(
            TARGET_LABEL,
            "write_ledger",
            namespace.clone(),
            Path::new(LEDGER_PATH),
            &content,
        ),
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::{LEDGER_PATH, PruneLedger, prune_due, read_ledger, write_ledger};
    use golem_common::model::Timestamp;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::time::Duration;
    use test_r::test;
    use uuid::Uuid;

    const MIB: u64 = 1024 * 1024;
    const THRESHOLD: u64 = 64 * MIB;
    const GRACE: Duration = Duration::from_secs(3600);
    const DEADLINE: Duration = Duration::from_secs(2);

    fn at(millis: u64) -> Timestamp {
        Timestamp::from(millis)
    }

    fn ledger(
        freed_bytes: u64,
        last_prune_millis: Option<u64>,
        awaiting_removal: bool,
    ) -> PruneLedger {
        PruneLedger {
            freed_bytes,
            last_prune_millis,
            awaiting_removal,
        }
    }

    fn new_namespace() -> BlobStorageNamespace {
        BlobStorageNamespace::InitialAgentFiles {
            environment_id: EnvironmentId(Uuid::new_v4()),
        }
    }

    #[test]
    fn a_prune_is_due_when_the_freed_bytes_reach_the_threshold() {
        let now = at(10_000_000);

        assert_eq!(
            [
                prune_due(&ledger(THRESHOLD - 1, None, false), now, THRESHOLD, GRACE),
                prune_due(&ledger(THRESHOLD, None, false), now, THRESHOLD, GRACE),
                prune_due(&ledger(THRESHOLD + 1, None, false), now, THRESHOLD, GRACE),
            ],
            [false, true, true]
        );
    }

    #[test]
    fn no_second_prune_runs_within_the_grace_period() {
        let last = 1_000_000;
        let grace_millis = 3_600_000;
        let full = |now| {
            prune_due(
                &ledger(THRESHOLD, Some(last), true),
                at(now),
                THRESHOLD,
                GRACE,
            )
        };

        assert_eq!(
            [
                full(last),
                full(last + grace_millis - 1),
                full(last + grace_millis),
                full(last + grace_millis + 1),
            ],
            [false, false, true, true]
        );
    }

    #[test]
    fn marked_packs_make_a_prune_due_after_the_grace_period_without_freed_bytes() {
        let last = 1_000_000;
        let after_grace = at(last + 3_600_000);

        assert_eq!(
            [
                prune_due(&ledger(0, Some(last), true), after_grace, THRESHOLD, GRACE),
                prune_due(&ledger(0, Some(last), false), after_grace, THRESHOLD, GRACE),
            ],
            [true, false]
        );
    }

    #[test]
    fn a_zero_threshold_prunes_after_each_delete_that_freed_bytes() {
        let now = at(10_000_000);

        assert_eq!(
            [
                prune_due(&ledger(0, None, false), now, 0, Duration::ZERO),
                prune_due(&ledger(1, None, false), now, 0, Duration::ZERO),
                prune_due(&ledger(1, Some(10_000_000), false), now, 0, Duration::ZERO),
            ],
            [false, true, true]
        );
    }

    #[test]
    fn a_delete_adds_its_bytes_and_a_prune_starts_the_ledger_again() {
        let deleted = ledger(5, Some(7), true)
            .with_deleted(10)
            .with_deleted(u64::MAX);

        assert_eq!(
            (
                deleted,
                PruneLedger::after_prune(at(42), true),
                PruneLedger::after_prune(at(43), false)
            ),
            (
                ledger(u64::MAX, Some(7), true),
                ledger(0, Some(42), true),
                ledger(0, Some(43), false)
            )
        );
    }

    #[test]
    async fn the_ledger_is_written_and_read_back() {
        let storage = InMemoryBlobStorage::new();
        let namespace = new_namespace();
        let written = ledger(123, Some(456), true);

        let before = read_ledger(&storage, &namespace, DEADLINE).await.unwrap();
        write_ledger(&storage, &namespace, DEADLINE, &written)
            .await
            .unwrap();
        let after = read_ledger(&storage, &namespace, DEADLINE).await.unwrap();

        assert_eq!((before, after), (PruneLedger::default(), written));
    }

    #[test]
    async fn a_ledger_that_does_not_parse_reads_as_an_empty_ledger() {
        let storage = InMemoryBlobStorage::new();
        let namespace = new_namespace();
        storage
            .put_raw(
                "test",
                "test",
                namespace.clone(),
                Path::new(LEDGER_PATH),
                b"not json",
            )
            .await
            .unwrap();

        assert_eq!(
            read_ledger(&storage, &namespace, DEADLINE).await.unwrap(),
            PruneLedger::default()
        );
    }
}
