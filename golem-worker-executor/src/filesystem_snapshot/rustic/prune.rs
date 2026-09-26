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
//! The scope keeps a ledger next to the files of the repository: an entry for each prune, whose
//! name holds the time at which the prune ended and whether it marked packs that a later prune
//! removes. The newest entry is the ledger. Only the delete that holds the claim of a prune writes
//! an entry, and an entry is never written over. Each delete that freed bytes writes a record
//! of its own, with the count in the name, and a prune that succeeds deletes the records it counted.
//!
//! A delete whose prune is due takes a claim before it prunes, so two deletes that read the same
//! ledger make one prune. The claims of a ledger are in one directory, named by the time of the last
//! prune in that ledger.

use super::files::SnapshotFiles;
use futures::{StreamExt, TryStreamExt, stream};
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::{ListedBlob, PutIfAbsent};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::warn;

/// The directory of the ledger entries, relative to the root of the namespace of the scope.
pub(super) const LEDGERS_PATH: &str = "golem/prune-ledgers";

/// How far the clock of another host can be ahead of the local clock. A time that is further
/// ahead counts as missing.
pub(super) const CLOCK_SKEW_MARGIN: Duration = Duration::from_secs(2 * 60);

/// The directory of the prune claims, relative to the root of the namespace of the scope.
const CLAIMS_PATH: &str = "golem/prune-claims";

/// The directory of the records of freed bytes, relative to the root of the namespace of the scope.
const FREED_PATH: &str = "golem/prune-freed";

/// What the scope did since its last prune.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct PruneLedger {
    /// The time of the last prune.
    pub(super) last_prune: Option<Timestamp>,
    /// Whether the last prune marked packs that a later prune removes.
    pub(super) awaiting_removal: bool,
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

/// Tells whether the grace period and the margin for clock skew passed at `now` since the last
/// prune. A time more than the margin after `now` counts as missing.
fn grace_passed(ledger: &PruneLedger, now: Timestamp, grace: Duration) -> bool {
    ledger
        .last_prune
        .filter(|last| !beyond_margin(*last, now))
        .is_none_or(|last| passed_since(last, now, grace.saturating_add(CLOCK_SKEW_MARGIN)))
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

/// Reads a ledger entry name, `<end ms>-<0|1>-<unique part>`, as the time of the prune and whether
/// it marked packs.
pub(super) fn parse_ledger_entry(name: &str) -> Option<PruneLedger> {
    let mut parts = name.splitn(3, '-');
    let ended = parts.next()?.parse::<u64>().ok()?;
    let awaiting_removal = match parts.next()? {
        "0" => false,
        "1" => true,
        _ => return None,
    };
    parts.next().filter(|unique| !unique.is_empty())?;
    Some(PruneLedger {
        last_prune: Some(Timestamp::from(ended)),
        awaiting_removal,
    })
}

/// Tells whether the time is more than [`CLOCK_SKEW_MARGIN`] after `now`.
fn beyond_margin(time: Timestamp, now: Timestamp) -> bool {
    time.to_millis()
        > now
            .to_millis()
            .saturating_add(u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap_or(u64::MAX))
}

/// Gives the ledger from the listed entries: the entry with the greatest time. A name that does not
/// parse and a time more than the margin after `now` are left out. No entry gives the default.
pub(super) fn newest_ledger(listed: &[ListedBlob], now: Timestamp) -> PruneLedger {
    listed
        .iter()
        .filter_map(|blob| parse_ledger_entry(blob.path.file_name()?.to_str()?))
        .filter(|entry| {
            entry
                .last_prune
                .is_some_and(|ended| !beyond_margin(ended, now))
        })
        .max_by_key(|entry| (entry.last_prune, entry.awaiting_removal))
        .unwrap_or_default()
}

/// Gives the paths of the listed entries whose time is before `ended`, in whole milliseconds as
/// an entry name holds it.
pub(super) fn older_entries(listed: &[ListedBlob], ended: Timestamp) -> Box<[Box<Path>]> {
    listed
        .iter()
        .filter(|blob| {
            blob.path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(parse_ledger_entry)
                .and_then(|entry| entry.last_prune)
                .is_some_and(|time| time.to_millis() < ended.to_millis())
        })
        .map(|blob| blob.path.clone())
        .collect()
}

/// Reads the ledger of the scope from one listing of its entries. No read of content is needed.
pub(super) async fn read_ledger(files: &SnapshotFiles) -> anyhow::Result<PruneLedger> {
    let listed = files
        .list_below("read_ledger", Path::new(LEDGERS_PATH))
        .await?;
    Ok(newest_ledger(&listed, Timestamp::now_utc()))
}

/// Writes a new ledger entry for a prune that ended at `ended`.
pub(super) async fn write_ledger(
    files: &SnapshotFiles,
    ended: Timestamp,
    awaiting_removal: bool,
) -> anyhow::Result<()> {
    let name = format!(
        "{}-{}-{}",
        ended.to_millis(),
        u8::from(awaiting_removal),
        uuid::Uuid::new_v4()
    );
    files
        .put_if_absent("write_ledger", &Path::new(LEDGERS_PATH).join(name), &[])
        .await
        .map(|_| ())
}

/// Deletes each ledger entry that is older than the entry of the prune that ended at `ended`. A
/// failure gives a warning, because an older entry is never the newest.
pub(super) async fn remove_older_ledgers(files: &SnapshotFiles, ended: Timestamp) {
    let listed = match files
        .list_below("list_ledgers", Path::new(LEDGERS_PATH))
        .await
    {
        Ok(listed) => listed,
        Err(error) => {
            warn!(
                error = %format!("{error:#}"),
                "Failed to list the prune ledger entries of a filesystem snapshot scope"
            );
            return;
        }
    };
    stream::iter(older_entries(&listed, ended))
        .for_each(|path| async move {
            if let Err(error) = files.delete("delete_ledger", &path).await {
                warn!(
                    error = %format!("{error:#}"),
                    "Failed to delete an old prune ledger entry of a filesystem snapshot scope"
                );
            }
        })
        .await;
}

/// The freed bytes of the settled records, and the paths of those records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct FreedRecords {
    pub(super) bytes: u64,
    pub(super) counted: Box<[Box<Path>]>,
}

/// A record of freed bytes that a delete wrote: its path, the bytes in its name, and the ids of
/// the snapshot files of that delete, when its content parses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct FreedRecord {
    pub(super) path: Box<Path>,
    pub(super) bytes: u64,
    pub(super) snapshots: Option<Box<[String]>>,
}

/// The directory of the snapshot files of a repository.
const SNAPSHOTS_PATH: &str = "snapshots";

/// Gives the content of a record: the id of each snapshot file of the delete, one on each line.
pub(super) fn record_content(snapshots: &[String]) -> String {
    snapshots.join("\n")
}

/// Reads the snapshot ids from the content of a record. Each line must be an id of 64 hex
/// characters. A content without an id does not parse, because a reader can see a record that a
/// write has not filled yet.
pub(super) fn parse_record(content: &[u8]) -> Option<Box<[String]>> {
    let text = std::str::from_utf8(content).ok()?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            (line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then(|| line.to_string())
        })
        .collect::<Option<Box<[String]>>>()
        .filter(|snapshots| !snapshots.is_empty())
}

/// Gives the settled records and the sum of their bytes. A record is settled when it names at
/// least one snapshot file and none of them exists. Any other record counts as zero bytes and
/// stays, and so does a record whose content does not parse.
pub(super) fn settle(records: &[FreedRecord], existing: &HashSet<String>) -> FreedRecords {
    let settled = records
        .iter()
        .filter(|record| {
            record.snapshots.as_ref().is_some_and(|snapshots| {
                !snapshots.is_empty() && snapshots.iter().all(|id| !existing.contains(id))
            })
        })
        .collect::<Box<[_]>>();
    FreedRecords {
        bytes: settled
            .iter()
            .map(|record| record.bytes)
            .fold(0, u64::saturating_add),
        counted: settled.iter().map(|record| record.path.clone()).collect(),
    }
}

/// Reads the freed bytes from the name of a record, `<bytes>-<unique part>`.
pub(super) fn parse_freed(name: &str) -> Option<u64> {
    let (bytes, unique) = name.split_once('-')?;
    if unique.is_empty() {
        return None;
    }
    bytes.parse().ok()
}

/// Writes a record of the freed bytes of one delete, with the ids of its snapshot files.
pub(super) async fn record_freed(
    files: &SnapshotFiles,
    bytes: u64,
    snapshots: &[String],
) -> anyhow::Result<()> {
    let path = Path::new(FREED_PATH).join(format!("{bytes}-{}", uuid::Uuid::new_v4()));
    // The name is unique, so `AlreadyExists` means that an earlier try of this call wrote it.
    files
        .put_if_absent("write_freed", &path, record_content(snapshots).as_bytes())
        .await
        .map(|_| ())
}

/// Lists and reads the records of freed bytes, lists the snapshot files one time, and gives the
/// settled records. A name that does not parse counts as zero bytes and stays, and a record that
/// a prune deleted after the listing is left out. Without records, no snapshot file is listed.
pub(super) async fn list_freed(files: &SnapshotFiles) -> anyhow::Result<FreedRecords> {
    let listed = files
        .list_below("list_freed", Path::new(FREED_PATH))
        .await?;
    let named = listed
        .iter()
        .filter_map(|blob| {
            let bytes = parse_freed(blob.path.file_name()?.to_str()?)?;
            Some((bytes, &blob.path))
        })
        .collect::<Box<[_]>>();
    if named.is_empty() {
        return Ok(FreedRecords::default());
    }
    let records = stream::iter(named.iter())
        .then(|(bytes, path)| async move {
            let content = files.get("read_freed", path).await?;
            Ok::<_, anyhow::Error>(content.map(|content| FreedRecord {
                path: (*path).clone(),
                bytes: *bytes,
                snapshots: parse_record(&content),
            }))
        })
        .try_filter_map(|record| std::future::ready(Ok(record)))
        .try_collect::<Vec<_>>()
        .await?;
    let existing = files
        .list_below("list_snapshots", Path::new(SNAPSHOTS_PATH))
        .await?
        .iter()
        .filter_map(|blob| Some(blob.path.file_name()?.to_str()?.to_string()))
        .collect::<HashSet<_>>();
    Ok(settle(&records, &existing))
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
/// until the grace period and the margin for clock skew passed since its time. A claim whose
/// content does not parse, or whose time is more than the margin after `now`, is old.
pub(super) fn next_claim(claims: &[ListedClaim], now: Timestamp, grace: Duration) -> ClaimChoice {
    let held_until = grace.saturating_add(CLOCK_SKEW_MARGIN);
    match claims.iter().max_by_key(|claim| claim.number) {
        None => ClaimChoice::Claim(0),
        Some(newest)
            if newest
                .claimed_at
                .filter(|at| !beyond_margin(*at, now))
                .is_some_and(|at| !passed_since(at, now, held_until)) =>
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
) -> anyhow::Result<Box<[ListedClaim]>> {
    let listed = files.list_below("list_claims", directory).await?;
    let numbered = listed
        .iter()
        .filter_map(|blob| {
            let number = blob.path.file_name()?.to_str()?.parse::<u64>().ok()?;
            Some((number, &blob.path))
        })
        .collect::<Box<[_]>>();
    stream::iter(numbered.iter())
        .then(|(number, path)| async move {
            let content = files.get("read_claim", path).await?;
            Ok::<_, anyhow::Error>(ListedClaim {
                number: *number,
                claimed_at: content.as_deref().and_then(parse_claim),
            })
        })
        .try_collect::<Vec<_>>()
        .await
        .map(Vec::into_boxed_slice)
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

/// Gives the time between two writes of a live claim: a fourth of the grace period, or a fourth of
/// the margin for clock skew when the grace period is zero.
pub(super) fn refresh_period(grace: Duration) -> Duration {
    if grace.is_zero() {
        CLOCK_SKEW_MARGIN / 4
    } else {
        grace / 4
    }
}

/// Writes the claim with the number again, with the current time, at each period, until the
/// caller drops the future or the operation of the files is cancelled. A failed write gives a
/// warning.
pub(super) async fn keep_claim_fresh(
    files: &SnapshotFiles,
    directory: &Path,
    number: u64,
    period: Duration,
) {
    let path = directory.join(number.to_string());
    stream::repeat(())
        .then(|()| tokio::time::sleep(period))
        .take_until(files.cancel.cancelled())
        .for_each(|()| {
            let path = &path;
            async move {
                let content = Timestamp::now_utc().to_millis().to_string();
                if let Err(error) = files.put("refresh_claim", path, content.as_bytes()).await {
                    warn!(
                        error = %format!("{error:#}"),
                        "Failed to write the prune claim of a filesystem snapshot scope again"
                    );
                }
            }
        })
        .await;
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

/// Gives each claim directory of the listed claims, other than the directory to keep.
pub(super) fn claim_directories_except(listed: &[ListedBlob], keep: &Path) -> Box<[PathBuf]> {
    let mut directories = listed
        .iter()
        .filter_map(|blob| blob.path.parent().map(Path::to_path_buf))
        .filter(|directory| directory != keep)
        .collect::<Vec<_>>();
    directories.sort();
    directories.dedup();
    directories.into_boxed_slice()
}

/// Deletes each claim directory other than the directory of the new ledger. It never deletes the
/// directory of all claims, so a live claim of the new ledger stays. A failure gives a warning,
/// because a claim only delays a prune until its grace period passed.
pub(super) async fn remove_old_claims(files: &SnapshotFiles, keep: &Path) {
    let listed = match files
        .list_below("list_claim_directories", Path::new(CLAIMS_PATH))
        .await
    {
        Ok(listed) => listed,
        Err(error) => {
            warn!(
                error = %format!("{error:#}"),
                "Failed to list the prune claims of a filesystem snapshot scope"
            );
            return;
        }
    };
    stream::iter(claim_directories_except(&listed, keep))
        .for_each(|directory| async move {
            if let Err(error) = files.delete_dir("delete_claims", &directory).await {
                warn!(
                    error = %format!("{error:#}"),
                    "Failed to delete the prune claims of a filesystem snapshot scope"
                );
            }
        })
        .await;
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
        CLAIMS_PATH, CLOCK_SKEW_MARGIN, ClaimChoice, FREED_PATH, FreedRecord, FreedRecords,
        LEDGERS_PATH, ListedClaim, Percent, PruneLedger, claim_directories_except,
        claims_directory, keep_claim_fresh, list_claims, list_freed, needs_repository_size,
        newest_ledger, next_claim, older_entries, parse_freed, parse_ledger_entry, parse_record,
        prune_due, read_ledger, record_content, record_freed, settle, take_claim, write_ledger,
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
    /// The grace period and the margin for clock skew, in milliseconds.
    const HELD_MILLIS: u64 = 15 * 60 * 1000 + 2 * 60 * 1000;
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
            tracker: tokio_util::task::TaskTracker::new(),
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
                full(last + HELD_MILLIS - 1),
                full(last + HELD_MILLIS),
                full(last + HELD_MILLIS + 1),
            ],
            [false, false, true, true]
        );
    }

    #[test]
    fn marked_packs_make_a_prune_due_after_the_grace_period_without_freed_bytes() {
        let last = 1_000_000;
        let after_grace = at(last + HELD_MILLIS);
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
                due(ledger(Some(10_000_000 - 120_000), false), 1),
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
                needs(1, false, last + HELD_MILLIS),
                needs(1, false, last + HELD_MILLIS - 1),
                needs(0, false, last + HELD_MILLIS),
                needs(1, true, last + HELD_MILLIS),
            ],
            [true, false, false, false]
        );
    }

    #[test]
    fn the_margin_extends_the_grace_period_and_a_time_more_than_the_margin_ahead_counts_as_missing()
    {
        let now = 10_000_000;
        let margin = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap();
        let due = |last| prune_due(&ledger(Some(last), false), 1, at(now), 0, Percent(0), GRACE);
        let claim = |claimed_at| {
            next_claim(
                &[ListedClaim {
                    number: 0,
                    claimed_at: Some(at(claimed_at)),
                }],
                at(now),
                GRACE,
            )
        };

        let grace = u64::try_from(GRACE.as_millis()).unwrap();

        assert_eq!(
            (
                [
                    due(now - grace),
                    due(now - grace - margin),
                    due(now + margin),
                    due(now + margin + 1)
                ],
                [
                    claim(now - grace),
                    claim(now - grace - margin),
                    claim(now + margin),
                    claim(now + margin + 1)
                ]
            ),
            (
                [false, true, false, true],
                [
                    ClaimChoice::Held,
                    ClaimChoice::Claim(1),
                    ClaimChoice::Held,
                    ClaimChoice::Claim(1)
                ]
            )
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
                choose(&[claim(0, Some(now - HELD_MILLIS)), claim(1, Some(now - 1))]),
                choose(&[claim(1, Some(now)), claim(2, Some(now - HELD_MILLIS))]),
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
    async fn the_refresh_of_a_claim_ends_when_its_operation_is_cancelled() {
        let files = new_files();
        files.cancel.cancel();

        let ended = tokio::time::timeout(
            Duration::from_secs(10),
            keep_claim_fresh(
                &files,
                &claims_directory(&ledger(None, false)),
                0,
                Duration::from_secs(3600),
            ),
        )
        .await
        .is_ok();

        assert!(ended);
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
            (
                directory.display().to_string(),
                first,
                again,
                listed.to_vec()
            ),
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
    fn each_claim_directory_other_than_the_new_one_is_old() {
        let claim = |directory: &str, number: &str| ListedBlob {
            path: Path::new(CLAIMS_PATH).join(directory).join(number).into(),
            size: 0,
        };
        let directory = |name: &str| Path::new(CLAIMS_PATH).join(name);

        assert_eq!(
            claim_directories_except(
                &[
                    claim("none", "0"),
                    claim("100", "0"),
                    claim("100", "1"),
                    claim("200", "3"),
                    claim("300", "0"),
                ],
                &directory("300")
            )
            .to_vec(),
            vec![directory("100"), directory("200"), directory("none")]
        );
    }

    #[test]
    fn a_ledger_entry_name_gives_the_end_time_and_the_marked_packs() {
        assert_eq!(
            [
                parse_ledger_entry("42-1-a"),
                parse_ledger_entry("43-0-a-b"),
                parse_ledger_entry("42-2-a"),
                parse_ledger_entry("42-1-"),
                parse_ledger_entry("42-1"),
                parse_ledger_entry("x-1-a"),
            ],
            [
                Some(ledger(Some(42), true)),
                Some(ledger(Some(43), false)),
                None,
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn the_newest_entry_within_the_margin_is_the_ledger() {
        let now = 10_000_000;
        let margin = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap();
        let entry = |name: &str| ListedBlob {
            path: Path::new(LEDGERS_PATH).join(name).into(),
            size: 0,
        };
        let newest = |names: &[&str]| {
            newest_ledger(
                &names.iter().map(|name| entry(name)).collect::<Vec<_>>(),
                at(now),
            )
        };

        assert_eq!(
            [
                newest(&[]),
                newest(&["5-0-a", "9-1-b", "7-0-c"]),
                newest(&["5-0-a", "not-an-entry"]),
                newest(&["5-0-a", &format!("{}-1-b", now + margin)]),
                newest(&["5-0-a", &format!("{}-1-b", now + margin + 1)]),
            ],
            [
                PruneLedger::default(),
                ledger(Some(9), true),
                ledger(Some(5), false),
                ledger(Some(now + margin), true),
                ledger(Some(5), false),
            ]
        );
    }

    #[test]
    fn only_entries_before_the_end_of_a_prune_are_older() {
        let entry = |name: &str| ListedBlob {
            path: Path::new(LEDGERS_PATH).join(name).into(),
            size: 0,
        };

        assert_eq!(
            older_entries(
                &[
                    entry("5-0-a"),
                    entry("9-1-own"),
                    entry("9-0-same-time"),
                    entry("12-0-newer"),
                    entry("bad"),
                ],
                at(9)
            )
            .to_vec(),
            vec![Path::new(LEDGERS_PATH).join("5-0-a").into_boxed_path()]
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
    fn a_record_counts_only_when_each_of_its_snapshot_files_is_gone() {
        let id = |digit: char| std::iter::repeat_n(digit, 64).collect::<String>();
        let record = |name: &str, bytes: u64, snapshots: Option<Vec<String>>| FreedRecord {
            path: Path::new(FREED_PATH).join(name).into(),
            bytes,
            snapshots: snapshots.map(Vec::into_boxed_slice),
        };
        let existing = [id('b')].into_iter().collect();

        let settled = settle(
            &[
                record("5-gone", 5, Some(vec![id('a')])),
                record("7-kept", 7, Some(vec![id('a'), id('b')])),
                record("11-empty", 11, Some(Vec::new())),
                record("13-bad", 13, None),
                record(&format!("{}-max", u64::MAX), u64::MAX, Some(vec![id('c')])),
            ],
            &existing,
        );

        assert_eq!(
            settled,
            FreedRecords {
                bytes: u64::MAX,
                counted: Box::new([
                    Path::new(FREED_PATH).join("5-gone").into(),
                    Path::new(FREED_PATH)
                        .join(format!("{}-max", u64::MAX))
                        .into(),
                ]),
            }
        );
    }

    #[test]
    fn the_content_of_a_record_holds_one_snapshot_id_on_each_line() {
        let id = |digit: char| std::iter::repeat_n(digit, 64).collect::<String>();
        let ids = [id('a'), id('b')];

        assert_eq!(
            [
                parse_record(record_content(&ids).as_bytes()),
                parse_record(b""),
                parse_record(b"not an id"),
                parse_record(&[0xff, 0xfe]),
            ],
            [
                Some(Box::new(ids.clone()) as Box<[String]>),
                None,
                None,
                None
            ]
        );
    }

    #[test]
    async fn a_record_of_freed_bytes_is_written_and_listed() {
        let files = new_files();

        let gone = ["0".repeat(64)];
        record_freed(&files, 40, &gone).await.unwrap();
        record_freed(&files, 2, &gone).await.unwrap();
        let listed = list_freed(&files).await.unwrap();

        assert_eq!((listed.bytes, listed.counted.len()), (42, 2));
    }

    #[test]
    async fn a_written_entry_is_the_ledger_that_a_read_gives() {
        let files = new_files();
        let ended = Timestamp::from(Timestamp::now_utc().to_millis());

        let before = read_ledger(&files).await.unwrap();
        write_ledger(&files, ended, true).await.unwrap();
        let after = read_ledger(&files).await.unwrap();

        assert_eq!(
            (before, after),
            (
                PruneLedger::default(),
                PruneLedger {
                    last_prune: Some(ended),
                    awaiting_removal: true
                }
            )
        );
    }
}
