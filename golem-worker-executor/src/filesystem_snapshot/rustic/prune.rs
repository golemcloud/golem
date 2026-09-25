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
//! time of the last prune, and whether that prune marked packs that a later prune removes. Only the
//! delete that holds the claim of a prune writes it. Each delete that freed bytes writes a record
//! of its own, with the count in the name, and a prune that succeeds deletes the records it counted.
//!
//! A delete whose prune is due takes a claim before it prunes, so two deletes that read the same
//! ledger make one prune. The claims of a ledger are in one directory, named by the time of the last
//! prune in that ledger.

use super::files::SnapshotFiles;
use futures::{StreamExt, TryStreamExt, stream};
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::{ListedBlob, PutIfAbsent};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::warn;

/// The path of the ledger blob, relative to the root of the namespace of the scope.
pub(super) const LEDGER_PATH: &str = "golem/prune-ledger";

/// The directory of the prune claims, relative to the root of the namespace of the scope.
const CLAIMS_PATH: &str = "golem/prune-claims";

/// The directory of the records of freed bytes, relative to the root of the namespace of the scope.
const FREED_PATH: &str = "golem/prune-freed";

/// What the scope did since its last prune.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct PruneLedger {
    /// The time of the last prune.
    pub(super) last_prune: Option<Timestamp>,
    /// Whether the last prune marked packs that a later prune removes.
    pub(super) awaiting_removal: bool,
}

impl PruneLedger {
    /// Gives the ledger after a prune at `now` that marked packs or not.
    pub(super) fn after_prune(now: Timestamp, marked_packs: bool) -> Self {
        Self {
            last_prune: Some(now),
            awaiting_removal: marked_packs,
        }
    }
}

/// The path of the packs of a repository, relative to the root of the namespace of the scope.
const DATA_PATH: &str = "data";

/// A share of the size of a repository, in percent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Percent(pub(super) u16);

impl Percent {
    /// Gives this share of the bytes, rounded down.
    fn of(self, bytes: u64) -> u64 {
        u64::try_from(u128::from(bytes) * u128::from(self.0) / 100).unwrap_or(u64::MAX)
    }
}

/// Tells whether the grace period passed at `now` since the last prune.
fn grace_passed(ledger: &PruneLedger, now: Timestamp, grace: Duration) -> bool {
    ledger
        .last_prune
        .is_none_or(|last| passed_since(last, now, grace))
}

/// Tells whether the grace period passed at `now` since the time.
fn passed_since(time: Timestamp, now: Timestamp, grace: Duration) -> bool {
    now.to_millis()
        >= time
            .to_millis()
            .saturating_add(u64::try_from(grace.as_millis()).unwrap_or(u64::MAX))
}

/// Tells whether [`prune_due`] needs the size of the repository at `now`. Only freed bytes after
/// the grace period, without marked packs, need it.
pub(super) fn needs_repository_size(
    ledger: &PruneLedger,
    freed_bytes: u64,
    now: Timestamp,
    grace: Duration,
) -> bool {
    grace_passed(ledger, now, grace) && freed_bytes > 0 && !ledger.awaiting_removal
}

/// Tells whether a prune is due at `now`.
///
/// A prune is due when the grace period passed since the last prune, and the freed bytes reach the
/// threshold share of `repository_bytes`, rounded down to a whole byte, or the last prune marked
/// packs. A threshold of zero bytes counts as one byte, so a prune never runs for a scope that
/// freed nothing and marked nothing.
pub(super) fn prune_due(
    ledger: &PruneLedger,
    freed_bytes: u64,
    now: Timestamp,
    repository_bytes: u64,
    threshold: Percent,
    grace: Duration,
) -> bool {
    let work = freed_bytes >= threshold.of(repository_bytes).max(1) || ledger.awaiting_removal;
    grace_passed(ledger, now, grace) && work
}

/// Gives the size of the repository of the scope: the sum of the sizes of its packs.
pub(super) async fn repository_bytes(files: &SnapshotFiles) -> anyhow::Result<u64> {
    Ok(files
        .list_below("list_data", Path::new(DATA_PATH))
        .await?
        .iter()
        .map(|blob| blob.size)
        .fold(0, u64::saturating_add))
}

/// Reads the ledger of the scope. A scope without a ledger, or with a ledger that does not parse,
/// gives an empty ledger, which only delays a prune.
pub(super) async fn read_ledger(files: &SnapshotFiles) -> anyhow::Result<PruneLedger> {
    let content = files.get("read_ledger", Path::new(LEDGER_PATH)).await?;
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
    files: &SnapshotFiles,
    ledger: &PruneLedger,
) -> anyhow::Result<()> {
    let content = serde_json::to_vec(ledger)?;
    files
        .put("write_ledger", Path::new(LEDGER_PATH), &content)
        .await
}

/// The freed bytes of the records that a listing found, and the paths of the records it counted.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FreedRecords {
    pub(super) bytes: u64,
    pub(super) counted: Vec<Box<Path>>,
}

/// Reads the freed bytes from the name of a record, `<bytes>-<unique part>`.
pub(super) fn parse_freed(name: &str) -> Option<u64> {
    let (bytes, unique) = name.split_once('-')?;
    if unique.is_empty() {
        return None;
    }
    bytes.parse().ok()
}

/// Sums the freed bytes of the listed records. A record whose name does not parse counts as zero
/// bytes, and it is not counted, so a prune leaves it in place.
pub(super) fn count_freed(listed: &[ListedBlob]) -> FreedRecords {
    listed
        .iter()
        .filter_map(|blob| {
            let bytes = parse_freed(blob.path.file_name()?.to_str()?)?;
            Some((bytes, blob.path.clone()))
        })
        .fold(FreedRecords::default(), |records, (bytes, path)| {
            let mut counted = records.counted;
            counted.push(path);
            FreedRecords {
                bytes: records.bytes.saturating_add(bytes),
                counted,
            }
        })
}

/// Writes a record of the freed bytes of one delete.
pub(super) async fn record_freed(files: &SnapshotFiles, bytes: u64) -> anyhow::Result<()> {
    let path = Path::new(FREED_PATH).join(format!("{bytes}-{}", uuid::Uuid::new_v4()));
    files.put("write_freed", &path, &[]).await
}

/// Lists the records of freed bytes, and sums them.
pub(super) async fn list_freed(files: &SnapshotFiles) -> anyhow::Result<FreedRecords> {
    Ok(count_freed(
        &files
            .list_below("list_freed", Path::new(FREED_PATH))
            .await?,
    ))
}

/// Deletes the counted records after a prune. A failure gives a warning, because a record that
/// stays only makes the next prune come earlier.
pub(super) async fn remove_freed(files: &SnapshotFiles, records: &FreedRecords) {
    stream::iter(&records.counted)
        .for_each(|path| async move {
            if let Err(error) = files.delete("delete_freed", path).await {
                warn!(
                    error = %format!("{error:#}"),
                    "Failed to delete a record of freed bytes of a filesystem snapshot scope"
                );
            }
        })
        .await;
}

/// A claim of a prune that a listing found: its number, and its time when its content parses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ListedClaim {
    pub(super) number: u64,
    pub(super) claimed_at: Option<Timestamp>,
}

/// What a delete whose prune is due does with the claims of its ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClaimChoice {
    /// Take the claim with the number, and prune when the write of the claim succeeds.
    Claim(u64),
    /// Another prune holds the claims of the ledger, so do not prune.
    Held,
}

/// Chooses the claim of a delete from the claims of its ledger. The newest claim holds the ledger
/// until the grace period passed since its time. A claim whose content does not parse is old.
pub(super) fn next_claim(claims: &[ListedClaim], now: Timestamp, grace: Duration) -> ClaimChoice {
    match claims.iter().max_by_key(|claim| claim.number) {
        None => ClaimChoice::Claim(0),
        Some(newest)
            if newest
                .claimed_at
                .is_some_and(|at| !passed_since(at, now, grace)) =>
        {
            ClaimChoice::Held
        }
        Some(newest) => ClaimChoice::Claim(newest.number.saturating_add(1)),
    }
}

/// Gives the directory of the claims of the ledger: the time of its last prune in milliseconds, or
/// `none`.
pub(super) fn claims_directory(ledger: &PruneLedger) -> PathBuf {
    let generation = ledger
        .last_prune
        .map_or_else(|| "none".to_string(), |last| last.to_millis().to_string());
    Path::new(CLAIMS_PATH).join(generation)
}

/// Lists the claims in the directory. A name that is not a number is not a claim, and a claim
/// that a delete removed after the listing counts as old.
pub(super) async fn list_claims(
    files: &SnapshotFiles,
    directory: &Path,
) -> anyhow::Result<Vec<ListedClaim>> {
    let listed = files.list_below("list_claims", directory).await?;
    let numbered = listed
        .iter()
        .filter_map(|blob| {
            let number = blob.path.file_name()?.to_str()?.parse::<u64>().ok()?;
            Some((number, blob.path.clone()))
        })
        .collect::<Vec<_>>();
    stream::iter(numbered)
        .then(|(number, path)| async move {
            let content = files.get("read_claim", &path).await?;
            Ok::<_, anyhow::Error>(ListedClaim {
                number,
                claimed_at: content.as_deref().and_then(parse_claim),
            })
        })
        .try_collect()
        .await
}

/// Writes the claim with the number, and tells whether this call wrote it.
pub(super) async fn take_claim(
    files: &SnapshotFiles,
    directory: &Path,
    number: u64,
    now: Timestamp,
) -> anyhow::Result<bool> {
    let content = now.to_millis().to_string();
    let written = files
        .put_if_absent(
            "write_claim",
            &directory.join(number.to_string()),
            content.as_bytes(),
        )
        .await?;
    Ok(written == PutIfAbsent::Written)
}

/// Deletes the claim with the number. A failure gives a warning, because a claim only delays a
/// prune until its grace period passed.
pub(super) async fn release_claim(files: &SnapshotFiles, directory: &Path, number: u64) {
    if let Err(error) = files
        .delete("delete_claim", &directory.join(number.to_string()))
        .await
    {
        warn!(
            error = %format!("{error:#}"),
            "Failed to delete the prune claim of a filesystem snapshot scope"
        );
    }
}

/// Deletes the claims of a ledger after its prune. A failure gives a warning, because a claim only
/// delays a prune until its grace period passed.
pub(super) async fn end_claims(files: &SnapshotFiles, directory: &Path) {
    if let Err(error) = files.delete_dir("delete_claims", directory).await {
        warn!(
            error = %format!("{error:#}"),
            "Failed to delete the prune claims of a filesystem snapshot scope"
        );
    }
}

/// Reads the time of a claim, in milliseconds.
fn parse_claim(content: &[u8]) -> Option<Timestamp> {
    std::str::from_utf8(content)
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Timestamp::from)
}

#[cfg(test)]
mod tests {
    use super::super::files::SnapshotFiles;
    use super::{
        ClaimChoice, FREED_PATH, FreedRecords, LEDGER_PATH, ListedClaim, Percent, PruneLedger,
        claims_directory, count_freed, list_claims, list_freed, needs_repository_size, next_claim,
        parse_freed, prune_due, read_ledger, record_freed, take_claim, write_ledger,
    };
    use golem_common::model::Timestamp;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::storage::blob::BlobStorageNamespace;
    use golem_service_base::storage::blob::ListedBlob;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use pretty_assertions::assert_eq;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use test_r::test;
    use uuid::Uuid;

    const TEN_PERCENT: Percent = Percent(10);
    const GRACE: Duration = Duration::from_secs(15 * 60);
    const GRACE_MILLIS: u64 = 15 * 60 * 1000;
    const DEADLINE: Duration = Duration::from_secs(2);

    fn at(millis: u64) -> Timestamp {
        Timestamp::from(millis)
    }

    fn ledger(last_prune_millis: Option<u64>, awaiting_removal: bool) -> PruneLedger {
        PruneLedger {
            last_prune: last_prune_millis.map(Timestamp::from),
            awaiting_removal,
        }
    }

    fn new_files() -> SnapshotFiles {
        SnapshotFiles {
            storage: Arc::new(InMemoryBlobStorage::new()),
            namespace: BlobStorageNamespace::InitialAgentFiles {
                environment_id: EnvironmentId(Uuid::new_v4()),
            },
            deadline: DEADLINE,
            cancel: tokio_util::sync::CancellationToken::new(),
        }
    }

    #[test]
    fn a_prune_is_due_when_the_freed_bytes_reach_ten_percent_of_the_repository_rounded_down() {
        let now = at(10_000_000);
        let due = |freed, repository_bytes| {
            prune_due(
                &ledger(None, false),
                freed,
                now,
                repository_bytes,
                TEN_PERCENT,
                GRACE,
            )
        };

        assert_eq!(
            [
                due(99, 1000),
                due(100, 1000),
                due(101, 1000),
                due(0, 0),
                due(1, 0),
            ],
            [false, true, true, false, true]
        );
    }

    #[test]
    fn no_second_prune_runs_within_the_grace_period() {
        let last = 1_000_000;
        let full = |now| {
            prune_due(
                &ledger(Some(last), true),
                1000,
                at(now),
                1000,
                TEN_PERCENT,
                GRACE,
            )
        };

        assert_eq!(
            [
                full(last),
                full(last + GRACE_MILLIS - 1),
                full(last + GRACE_MILLIS),
                full(last + GRACE_MILLIS + 1),
            ],
            [false, false, true, true]
        );
    }

    #[test]
    fn marked_packs_make_a_prune_due_after_the_grace_period_without_freed_bytes() {
        let last = 1_000_000;
        let after_grace = at(last + GRACE_MILLIS);
        let due = |awaiting_removal| {
            prune_due(
                &ledger(Some(last), awaiting_removal),
                0,
                after_grace,
                1000,
                TEN_PERCENT,
                GRACE,
            )
        };

        assert_eq!([due(true), due(false)], [true, false]);
    }

    #[test]
    fn a_zero_threshold_prunes_after_each_delete_that_freed_bytes() {
        let now = at(10_000_000);
        let due = |ledger, freed| prune_due(&ledger, freed, now, 1000, Percent(0), Duration::ZERO);

        assert_eq!(
            [
                due(ledger(None, false), 0),
                due(ledger(None, false), 1),
                due(ledger(Some(10_000_000), false), 1),
            ],
            [false, true, true]
        );
    }

    #[test]
    fn only_freed_bytes_after_the_grace_period_without_marked_packs_need_the_repository_size() {
        let last = 1_000_000;
        let needs = |freed, awaiting_removal, now| {
            needs_repository_size(&ledger(Some(last), awaiting_removal), freed, at(now), GRACE)
        };

        assert_eq!(
            [
                needs(1, false, last + GRACE_MILLIS),
                needs(1, false, last + GRACE_MILLIS - 1),
                needs(0, false, last + GRACE_MILLIS),
                needs(1, true, last + GRACE_MILLIS),
            ],
            [true, false, false, false]
        );
    }

    #[test]
    fn the_newest_claim_holds_a_ledger_until_the_grace_period_passed_since_its_time() {
        let now = 10_000_000;
        let claim = |number, claimed_at: Option<u64>| ListedClaim {
            number,
            claimed_at: claimed_at.map(Timestamp::from),
        };
        let choose = |claims: &[ListedClaim]| next_claim(claims, at(now), GRACE);

        assert_eq!(
            [
                choose(&[]),
                choose(&[claim(0, Some(now - GRACE_MILLIS)), claim(1, Some(now - 1))]),
                choose(&[claim(1, Some(now)), claim(2, Some(now - GRACE_MILLIS))]),
                choose(&[claim(4, None)]),
                choose(&[claim(0, Some(now - 1)), claim(3, None)]),
            ],
            [
                ClaimChoice::Claim(0),
                ClaimChoice::Held,
                ClaimChoice::Claim(3),
                ClaimChoice::Claim(5),
                ClaimChoice::Claim(4),
            ]
        );
    }

    #[test]
    async fn a_claim_is_taken_one_time_and_listed_with_its_time() {
        let files = new_files();
        let directory = claims_directory(&ledger(Some(42), false));
        let now = at(10_000_000);

        let first = take_claim(&files, &directory, 0, now).await.unwrap();
        let again = take_claim(&files, &directory, 0, at(20_000_000))
            .await
            .unwrap();
        let listed = list_claims(&files, &directory).await.unwrap();

        assert_eq!(
            (directory.display().to_string(), first, again, listed),
            (
                "golem/prune-claims/42".to_string(),
                true,
                false,
                vec![ListedClaim {
                    number: 0,
                    claimed_at: Some(now)
                }]
            )
        );
    }

    #[test]
    fn a_prune_starts_the_ledger_again() {
        assert_eq!(
            (
                PruneLedger::after_prune(at(42), true),
                PruneLedger::after_prune(at(43), false)
            ),
            (ledger(Some(42), true), ledger(Some(43), false))
        );
    }

    #[test]
    fn the_freed_bytes_of_a_record_are_the_number_before_the_first_dash() {
        assert_eq!(
            [
                parse_freed("123-0f4e"),
                parse_freed("0-a-b"),
                parse_freed("123"),
                parse_freed("123-"),
                parse_freed("x-0f4e"),
                parse_freed("-0f4e"),
            ],
            [Some(123), Some(0), None, None, None, None]
        );
    }

    #[test]
    fn the_records_that_parse_are_summed_and_counted_and_the_others_stay() {
        let blob = |name: &str| ListedBlob {
            path: Path::new(FREED_PATH).join(name).into(),
            size: 0,
        };

        let records = count_freed(&[
            blob("5-a"),
            blob("not-a-count"),
            blob(&format!("{}-b", u64::MAX)),
            blob("7"),
        ]);

        assert_eq!(
            records,
            FreedRecords {
                bytes: u64::MAX,
                counted: vec![
                    Path::new(FREED_PATH).join("5-a").into(),
                    Path::new(FREED_PATH).join(format!("{}-b", u64::MAX)).into(),
                ],
            }
        );
    }

    #[test]
    async fn a_record_of_freed_bytes_is_written_and_listed() {
        let files = new_files();

        record_freed(&files, 40).await.unwrap();
        record_freed(&files, 2).await.unwrap();
        let listed = list_freed(&files).await.unwrap();

        assert_eq!((listed.bytes, listed.counted.len()), (42, 2));
    }

    #[test]
    async fn the_ledger_is_written_and_read_back_with_the_time_in_milliseconds() {
        // The ledger keeps the time of the last prune as ISO 8601 text with milliseconds.
        let files = new_files();
        let written = PruneLedger {
            last_prune: Some(Timestamp::from(Timestamp::now_utc().to_millis())),
            awaiting_removal: true,
        };

        let before = read_ledger(&files).await.unwrap();
        write_ledger(&files, &written).await.unwrap();
        let after = read_ledger(&files).await.unwrap();

        assert_eq!((before, after), (PruneLedger::default(), written));
    }

    #[test]
    async fn a_ledger_that_does_not_parse_reads_as_an_empty_ledger() {
        let files = new_files();
        files
            .storage
            .put_raw(
                "test",
                "test",
                files.namespace.clone(),
                Path::new(LEDGER_PATH),
                b"not json",
            )
            .await
            .unwrap();

        assert_eq!(read_ledger(&files).await.unwrap(), PruneLedger::default());
    }
}
