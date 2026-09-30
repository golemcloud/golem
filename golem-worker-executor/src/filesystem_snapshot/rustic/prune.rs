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

use super::files::{Lease, SnapshotFiles};
use crate::filesystem_snapshot::clock::Clock;
use futures::{StreamExt, TryStreamExt, stream};
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::{ListedBlob, PutIfAbsent};
use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
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

/// Tells whether the hold of a claim passed at `now` since the last prune. The time of the last
/// prune is after its final marker, so the next prune waits a full hold from the newest claim
/// marker that the last prune wrote. A time more than the margin after `now` counts as missing.
pub(super) fn hold_passed(
    ledger: &PruneLedger,
    now: Timestamp,
    grace: Duration,
    deadline: Duration,
) -> bool {
    ledger
        .last_prune
        .filter(|last| !beyond_margin(*last, now))
        .is_none_or(|last| passed_since(last, now, claim_hold(grace, deadline)))
}

/// Tells whether the grace period passed at `now` since the time.
fn passed_since(time: Timestamp, now: Timestamp, grace: Duration) -> bool {
    now.to_millis()
        >= time
            .to_millis()
            .saturating_add(u64::try_from(grace.as_millis()).unwrap_or(u64::MAX))
}

/// Tells whether [`prune_due`] needs the size of the repository at `now`. Only freed bytes after
/// the hold of a claim, without marked packs, need it.
pub(super) fn needs_repository_size(
    ledger: &PruneLedger,
    freed_bytes: u64,
    now: Timestamp,
    grace: Duration,
    deadline: Duration,
) -> bool {
    hold_passed(ledger, now, grace, deadline) && freed_bytes > 0 && !ledger.awaiting_removal
}

/// Tells whether a prune is due at `now`.
///
/// A prune is due when the hold of a claim passed since the last prune, and the freed bytes reach the
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
    deadline: Duration,
) -> bool {
    let work = freed_bytes >= threshold.of(repository_bytes).max(1) || ledger.awaiting_removal;
    hold_passed(ledger, now, grace, deadline) && work
}

/// Tells whether the freed bytes in the names of the records, which are at least the settled
/// freed bytes, can make a prune due with the size of the repository. When this gives false,
/// [`prune_due`] gives false for the settled bytes too, so the records need no read.
pub(super) fn may_be_due(
    ledger: &PruneLedger,
    named_bytes: u64,
    repository_bytes: u64,
    threshold: Percent,
) -> bool {
    named_bytes >= threshold.of(repository_bytes).max(1) || ledger.awaiting_removal
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
/// The clock is read after the listing, so the margin check compares with a time from after it.
pub(super) async fn read_ledger(
    files: &SnapshotFiles,
    clock: &dyn Clock,
) -> anyhow::Result<PruneLedger> {
    let listed = files
        .list_below("read_ledger", Path::new(LEDGERS_PATH))
        .await?;
    Ok(newest_ledger(&listed, clock.now()))
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
    pub(super) snapshots: Option<Box<[Box<str>]>>,
}

/// The directory of the snapshot files of a repository.
pub(super) const SNAPSHOTS_PATH: &str = "snapshots";

/// Gives the content of a record: the id of each snapshot file of the delete, one on each line.
pub(super) fn record_content(snapshots: &[Box<str>]) -> String {
    snapshots.join("\n")
}

/// Reads the snapshot ids from the content of a record. Each line must be an id of 64 hex
/// characters. A content without an id does not parse, because a reader can see a record that a
/// write has not filled yet.
pub(super) fn parse_record(content: &[u8]) -> Option<Box<[Box<str>]>> {
    let text = std::str::from_utf8(content).ok()?;
    text.lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            (line.len() == 64 && line.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .then(|| line.into())
        })
        .collect::<Option<Box<[Box<str>]>>>()
        .filter(|snapshots| !snapshots.is_empty())
}

/// Gives the settled records and the sum of their bytes. A record is settled when it names at
/// least one snapshot file and none of them exists. Any other record counts as zero bytes and
/// stays, and so does a record whose content does not parse.
pub(super) fn settle(records: &[FreedRecord], existing: &HashSet<Box<str>>) -> FreedRecords {
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
    snapshots: &[Box<str>],
) -> anyhow::Result<()> {
    let path = Path::new(FREED_PATH).join(format!("{bytes}-{}", uuid::Uuid::new_v4()));
    // The name is unique, so `AlreadyExists` means that an earlier try of this call wrote it.
    files
        .put_if_absent("write_freed", &path, record_content(snapshots).as_bytes())
        .await
        .map(|_| ())
}

/// A record of freed bytes that a listing found: its path and the bytes in its name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ListedFreed {
    pub(super) path: Box<Path>,
    pub(super) bytes: u64,
}

/// Lists the names of the records of freed bytes, and reads no content. A name that does not
/// parse is left out, so it counts as zero bytes and stays.
pub(super) async fn list_freed_names(files: &SnapshotFiles) -> anyhow::Result<Box<[ListedFreed]>> {
    Ok(files
        .list_below("list_freed", Path::new(FREED_PATH))
        .await?
        .iter()
        .filter_map(|blob| {
            let bytes = parse_freed(blob.path.file_name()?.to_str()?)?;
            Some(ListedFreed {
                path: blob.path.clone(),
                bytes,
            })
        })
        .collect())
}

/// Gives the sum of the bytes in the names of the listed records.
pub(super) fn named_bytes(listed: &[ListedFreed]) -> u64 {
    listed
        .iter()
        .map(|record| record.bytes)
        .fold(0, u64::saturating_add)
}

/// Reads the listed records of freed bytes, lists the snapshot files one time, and gives the
/// settled records. A record that a prune deleted after the listing is left out. Without records,
/// no snapshot file is listed.
pub(super) async fn settle_freed(
    files: &SnapshotFiles,
    listed: &[ListedFreed],
) -> anyhow::Result<FreedRecords> {
    if listed.is_empty() {
        return Ok(FreedRecords::default());
    }
    let records = stream::iter(listed)
        .then(|listed| async move {
            let content = files.get("read_freed", &listed.path).await?;
            Ok::<_, anyhow::Error>(content.map(|content| FreedRecord {
                path: listed.path.clone(),
                bytes: listed.bytes,
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
        .filter_map(|blob| Some(blob.path.file_name()?.to_str()?.into()))
        .collect::<HashSet<Box<str>>>();
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

/// An entry of a claim directory that a listing found: a claim `<n>`, or a marker
/// `<n>@<ms>-<random>` with the time at which a delete wrote it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClaimEntry {
    Claim(u64),
    Marker(u64, Timestamp),
}

impl ClaimEntry {
    fn number(self) -> u64 {
        match self {
            Self::Claim(number) | Self::Marker(number, _) => number,
        }
    }
}

/// Reads the name of an entry of a claim directory.
pub(super) fn parse_claim_entry(name: &str) -> Option<ClaimEntry> {
    match name.split_once('@') {
        None => name.parse().ok().map(ClaimEntry::Claim),
        Some((number, rest)) => {
            let (millis, unique) = rest.split_once('-')?;
            if unique.is_empty() {
                return None;
            }
            Some(ClaimEntry::Marker(
                number.parse().ok()?,
                Timestamp::from(millis.parse::<u64>().ok()?),
            ))
        }
    }
}

/// What a delete whose prune is due does with the claims of its ledger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ClaimChoice {
    /// Take the claim with the number, and prune when the write of the claim succeeds.
    Claim(u64),
    /// Another prune holds the claims of the ledger, so do not prune.
    Held,
}

/// The unit of the time in the name of a marker.
const MARKER_TIME_UNIT: Duration = Duration::from_millis(1);

/// Gives the time from the start of a marker write to the end of the lease that the marker gives,
/// as the other deletes count it from the time in the name: the grace period less one storage call
/// deadline, but at least one deadline.
fn lease_bound(grace: Duration, deadline: Duration) -> Duration {
    grace.saturating_sub(deadline).max(deadline)
}

/// Gives how long a marker write that succeeds lets the prune of its claim go on: the lease bound
/// less one millisecond. The name of a marker keeps its time in whole milliseconds, so the time in
/// the name can be up to one millisecond before the time that the delete read. So the lease is above
/// zero for each deadline above one millisecond.
pub(super) fn lease_span(grace: Duration, deadline: Duration) -> Duration {
    lease_bound(grace, deadline).saturating_sub(MARKER_TIME_UNIT)
}

/// Gives how long a marker holds the claims of its ledger: the lease bound, then one margin for
/// clock skew, then two storage call deadlines. Another delete sees the marker time with up to one
/// margin of skew. A marker write that the storage received can still land up to one deadline after
/// the call gave up. A prune that its lease stopped writes its final marker after the lease ran
/// out, and that write can land up to one more deadline later. So a prune whose lease ran out stops
/// before another delete can take its claim, and the next prune waits a full hold from its final
/// marker.
pub(super) fn claim_hold(grace: Duration, deadline: Duration) -> Duration {
    lease_bound(grace, deadline)
        .saturating_add(CLOCK_SKEW_MARGIN)
        .saturating_add(deadline.saturating_mul(2))
}

/// Chooses the claim of a delete from the entries of the claim directory of its ledger. Any marker
/// younger than the hold holds the ledger, whatever its number. A marker whose time is more than
/// the margin after `now` counts as missing, and a claim without a young marker is old. Otherwise
/// the delete takes the number after the largest one, or 0.
pub(super) fn next_claim(entries: &[ClaimEntry], now: Timestamp, hold: Duration) -> ClaimChoice {
    let held = entries.iter().any(|entry| match entry {
        ClaimEntry::Marker(_, at) => !beyond_margin(*at, now) && !passed_since(*at, now, hold),
        ClaimEntry::Claim(_) => false,
    });
    if held {
        return ClaimChoice::Held;
    }
    entries
        .iter()
        .map(|entry| entry.number())
        .max()
        .map_or(ClaimChoice::Claim(0), |largest| {
            ClaimChoice::Claim(largest.saturating_add(1))
        })
}

/// Gives the directory of the claims of the ledger: the time of its last prune in milliseconds, or
/// `none`.
pub(super) fn claims_directory(ledger: &PruneLedger) -> Box<Path> {
    let generation = ledger
        .last_prune
        .map_or_else(|| "none".to_string(), |last| last.to_millis().to_string());
    Path::new(CLAIMS_PATH).join(generation).into_boxed_path()
}

/// Lists the claims and the markers in the directory, from their names. A name that does not
/// parse is left out.
pub(super) async fn list_claims(
    files: &SnapshotFiles,
    directory: &Path,
) -> anyhow::Result<Box<[ClaimEntry]>> {
    Ok(files
        .list_below("list_claims", directory)
        .await?
        .iter()
        .filter_map(|blob| parse_claim_entry(blob.path.file_name()?.to_str()?))
        .collect())
}

/// Gives the time of a new marker: the instant, from which a write of the marker moves the lease,
/// and then the wall time in the name of the marker. The instant is read first, so the lease starts
/// no later than the time in the name, and it never ends later than the hold that other deletes
/// count from the name.
pub(super) fn marker_time(clock: &dyn Clock) -> (Instant, Timestamp) {
    let started = Instant::now();
    let time = clock.now();
    (started, time)
}

/// Gives a new path of a marker of the claim with the number, with the time. The name is unique.
pub(super) fn marker_path(directory: &Path, number: u64, time: Timestamp) -> Box<Path> {
    directory
        .join(format!(
            "{number}@{}-{}",
            time.to_millis(),
            uuid::Uuid::new_v4()
        ))
        .into_boxed_path()
}

/// Writes the marker at the path. The name is unique, so `AlreadyExists` means that an earlier try
/// of this call wrote it.
async fn write_marker_at(
    files: &SnapshotFiles,
    op_label: &'static str,
    path: &Path,
) -> anyhow::Result<()> {
    let _: PutIfAbsent = files.put_if_absent(op_label, path, &[]).await?;
    Ok(())
}

/// Writes a marker of the claim with the number, with the time, and gives its path.
pub(super) async fn write_marker(
    files: &SnapshotFiles,
    op_label: &'static str,
    directory: &Path,
    number: u64,
    time: Timestamp,
) -> anyhow::Result<Box<Path>> {
    let path = marker_path(directory, number, time);
    write_marker_at(files, op_label, &path).await?;
    Ok(path)
}

/// Writes a marker of the claim with the number. A write that succeeds and started before the end
/// of the lease moves the end to `span` after the start of the write, when that is later. A write
/// that started at or after the end does not move it.
async fn write_leased_marker(
    files: &SnapshotFiles,
    op_label: &'static str,
    directory: &Path,
    number: u64,
    lease: &Lease,
    span: Duration,
    clock: &dyn Clock,
) -> anyhow::Result<Box<Path>> {
    let (started, time) = marker_time(clock);
    let marker = write_marker(files, op_label, directory, number, time).await?;
    lease.extend_from(started, span);
    Ok(marker)
}

/// Writes the first marker of the claim with the number at the path `marker`, then takes the
/// claim, and gives the lease of the prune when this delete holds the claim. The caller makes the
/// path before the write, so a guard can delete the marker when the delete stops during the write.
/// A delete that loses the claim deletes its marker. The lease starts with the marker write: it
/// ends `span` after `started`, the instant that [`marker_time`] gave with the time in the name of
/// the marker.
pub(super) async fn take_claim(
    files: &SnapshotFiles,
    directory: &Path,
    number: u64,
    marker: &Path,
    started: Instant,
    span: Duration,
) -> anyhow::Result<Option<Lease>> {
    write_marker_at(files, "write_marker", marker).await?;
    let lease = Lease::until(started + span);
    let written = files
        .put_if_absent("write_claim", &directory.join(number.to_string()), &[])
        .await?;
    if written == PutIfAbsent::Written {
        return Ok(Some(lease));
    }
    if let Err(error) = files.delete("delete_marker", marker).await {
        warn!(
            error = %format!("{error:#}"),
            "Failed to delete the marker of a prune claim that a filesystem snapshot delete lost"
        );
    }
    Ok(None)
}

/// Gives the time between two markers of a live claim: a fourth of the grace period, or a fourth
/// of the margin for clock skew when the grace period is zero, but at most a fourth of the lease, so
/// each lease has at least two refreshes.
pub(super) fn refresh_period(grace: Duration, deadline: Duration) -> Duration {
    let base = if grace.is_zero() {
        CLOCK_SKEW_MARGIN
    } else {
        grace
    };
    (base / 4).min(lease_span(grace, deadline) / 4)
}

/// Writes a new marker of the claim with the number at each period, until the caller drops the
/// future or the operation of the files is cancelled, and adds the path of each written marker to
/// `written`. A write that succeeds and started before the end of the lease moves the end to
/// `span` after its start, when that is later. A write that started at or after the end does not
/// move it. A failed write gives a warning, and the next period tries again.
pub(super) async fn keep_claim_fresh(
    files: &SnapshotFiles,
    directory: &Path,
    number: u64,
    period: Duration,
    written: &Mutex<Vec<Box<Path>>>,
    lease: &Lease,
    span: Duration,
    clock: &dyn Clock,
) {
    stream::repeat(())
        .then(|()| tokio::time::sleep(period))
        .take_until(files.cancelled())
        .for_each(|()| async move {
            let marker = write_leased_marker(
                files,
                "refresh_claim",
                directory,
                number,
                lease,
                span,
                clock,
            )
            .await;
            match marker {
                Ok(path) => written
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(path),
                Err(error) => warn!(
                    error = %format!("{error:#}"),
                    "Failed to write a new marker of the prune claim of a filesystem snapshot scope"
                ),
            }
        })
        .await;
}

/// Deletes the claim with the number when this delete took it, and then each of its markers by its
/// path. It tries each delete also when another one fails, and each failure gives a warning. A
/// claim that stays without its markers is old, and a marker that stays only delays a prune until
/// its hold passed.
pub(super) async fn release_claim(
    files: &SnapshotFiles,
    directory: &Path,
    number: u64,
    claimed: bool,
    markers: &[Box<Path>],
) {
    let claim = directory.join(number.to_string());
    stream::iter(
        claimed
            .then_some(("delete_claim", claim.as_path()))
            .into_iter()
            .chain(
                markers
                    .iter()
                    .map(|marker| ("delete_marker", marker.as_ref())),
            ),
    )
    .for_each(|(op_label, path)| async move {
        if let Err(error) = files.delete(op_label, path).await {
            warn!(
                error = %format!("{error:#}"),
                "Failed to delete the prune claim of a filesystem snapshot scope"
            );
        }
    })
    .await;
}

/// Gives the claim directory of each listed path below the directory of all claims whose ledger is
/// older than the ledger of `ended`: the directory `none`, and each directory whose time is before
/// `ended`. A newer directory can hold a live claim of a later prune, and a directory whose name is
/// not a time is not a directory of claims, so both stay.
pub(super) fn old_claim_directories(
    listed: impl IntoIterator<Item = impl AsRef<Path>>,
    ended: Timestamp,
) -> Box<[Box<Path>]> {
    let claims = Path::new(CLAIMS_PATH);
    let mut directories = listed
        .into_iter()
        .filter_map(|path| {
            let generation = path
                .as_ref()
                .strip_prefix(claims)
                .ok()?
                .components()
                .next()?
                .as_os_str()
                .to_str()?
                .to_owned();
            let old = generation == "none"
                || generation
                    .parse::<u64>()
                    .is_ok_and(|millis| millis < ended.to_millis());
            old.then(|| claims.join(generation).into_boxed_path())
        })
        .collect::<Vec<_>>();
    directories.sort();
    directories.dedup();
    directories.into_boxed_slice()
}

/// Deletes each claim directory of a ledger older than the ledger of `ended`. A listing of the
/// directories finds an empty claim directory, and a listing of the blobs finds a claim directory
/// that the storage keeps no entry for. It never deletes the directory of all claims or a newer
/// directory, so a live claim of a later ledger stays. A failure gives a warning, because a claim
/// only delays a prune until its hold passed.
pub(super) async fn remove_old_claims(files: &SnapshotFiles, ended: Timestamp) {
    let claims = Path::new(CLAIMS_PATH);
    let listed = async {
        let directories = files.list_dir("list_claim_directories", claims).await?;
        let blobs = files.list_below("list_claim_blobs", claims).await?;
        anyhow::Ok((directories, blobs))
    };
    let (directories, blobs) = match listed.await {
        Ok(listed) => listed,
        Err(error) => {
            warn!(
                error = %format!("{error:#}"),
                "Failed to list the prune claims of a filesystem snapshot scope"
            );
            return;
        }
    };
    let listed = directories
        .iter()
        .map(AsRef::as_ref)
        .chain(blobs.iter().map(|blob| blob.path.as_ref()));
    stream::iter(old_claim_directories(listed, ended))
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

#[cfg(test)]
mod tests {
    use super::super::backend::{BlobBackend, KEPT_PACKS_LIMIT};
    use super::super::fault::is_lease_expired;
    use super::super::files::SnapshotFiles;
    use super::super::tests::files_of;
    use super::super::tests::scripted::{Script, ScriptedBlobStorage};
    use super::{
        CLAIMS_PATH, CLOCK_SKEW_MARGIN, ClaimChoice, ClaimEntry, FREED_PATH, FreedRecord,
        FreedRecords, LEDGERS_PATH, Lease, Percent, PruneLedger, claim_hold, claims_directory,
        keep_claim_fresh, lease_span, list_claims, list_freed_names, marker_path, marker_time,
        may_be_due, named_bytes, needs_repository_size, newest_ledger, next_claim,
        old_claim_directories, older_entries, parse_claim_entry, parse_freed, parse_ledger_entry,
        parse_record, prune_due, read_ledger, record_content, record_freed, refresh_period, settle,
        settle_freed, take_claim, write_ledger,
    };
    use crate::filesystem_snapshot::clock::SystemClock;
    use futures::StreamExt;
    use golem_common::model::Timestamp;
    use golem_common::model::environment::EnvironmentId;
    use golem_service_base::storage::blob::BlobStorage;
    use golem_service_base::storage::blob::BlobStorageNamespace;
    use golem_service_base::storage::blob::ListedBlob;
    use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
    use pretty_assertions::assert_eq;
    use rustic_core::{FileType, ReadBackend};
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use std::time::Instant;
    use test_r::{test, timeout};
    use uuid::Uuid;

    const TEN_PERCENT: Percent = Percent(10);
    const GRACE: Duration = Duration::from_secs(15 * 60);
    /// The grace period and the margin for clock skew, in milliseconds.
    const HELD_MILLIS: u64 = 15 * 60 * 1000 + 2 * 60 * 1000;
    /// The hold of a claim with the grace period and the deadline, in milliseconds: the grace
    /// period less one deadline, then the margin, then two deadlines.
    const HOLD_MILLIS: u64 = HELD_MILLIS + 2 * 1000;
    const DEADLINE: Duration = Duration::from_secs(2);
    const MILLI: Duration = Duration::from_millis(1);

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
        files_over(Arc::new(InMemoryBlobStorage::new()))
    }

    fn files_over(storage: Arc<dyn BlobStorage>) -> SnapshotFiles {
        files_with_cancel(storage, tokio_util::sync::CancellationToken::new())
    }

    fn files_with_cancel(
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

    #[test]
    fn the_bytes_in_the_names_of_the_records_can_make_a_prune_due_only_when_they_reach_the_threshold()
     {
        let may = |awaiting_removal, named, repository_bytes| {
            may_be_due(
                &ledger(None, awaiting_removal),
                named,
                repository_bytes,
                TEN_PERCENT,
            )
        };

        assert_eq!(
            [
                may(false, 99, 1000),
                may(false, 100, 1000),
                may(false, 0, 0),
                may(false, 1, 0),
                may(true, 0, 1000),
            ],
            [false, true, false, true, true]
        );
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
                DEADLINE,
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
    fn no_second_prune_runs_within_the_hold_of_a_claim_after_the_last_prune() {
        // The grace period and the margin passed at `HELD_MILLIS`, and the full hold at
        // `HOLD_MILLIS`.
        let last = 1_000_000;
        let full = |now| {
            prune_due(
                &ledger(Some(last), true),
                1000,
                at(now),
                1000,
                TEN_PERCENT,
                GRACE,
                DEADLINE,
            )
        };

        assert_eq!(
            [
                full(last),
                full(last + HELD_MILLIS),
                full(last + HOLD_MILLIS - 1),
                full(last + HOLD_MILLIS),
                full(last + HOLD_MILLIS + 1),
            ],
            [false, false, false, true, true]
        );
    }

    #[test]
    fn marked_packs_make_a_prune_due_after_the_hold_without_freed_bytes() {
        let last = 1_000_000;
        let after_hold = at(last + HOLD_MILLIS);
        let due = |awaiting_removal| {
            prune_due(
                &ledger(Some(last), awaiting_removal),
                0,
                after_hold,
                1000,
                TEN_PERCENT,
                GRACE,
                DEADLINE,
            )
        };

        assert_eq!([due(true), due(false)], [true, false]);
    }

    #[test]
    fn a_zero_threshold_prunes_after_each_delete_that_freed_bytes() {
        let now = at(10_000_000);
        let due = |ledger, freed| {
            prune_due(
                &ledger,
                freed,
                now,
                1000,
                Percent(0),
                Duration::ZERO,
                DEADLINE,
            )
        };
        let hold = u64::try_from(claim_hold(Duration::ZERO, DEADLINE).as_millis()).unwrap();

        assert_eq!(
            [
                due(ledger(None, false), 0),
                due(ledger(None, false), 1),
                due(ledger(Some(10_000_000 - hold), false), 1),
            ],
            [false, true, true]
        );
    }

    #[test]
    fn only_freed_bytes_after_the_hold_without_marked_packs_need_the_repository_size() {
        let last = 1_000_000;
        let needs = |freed, awaiting_removal, now| {
            needs_repository_size(
                &ledger(Some(last), awaiting_removal),
                freed,
                at(now),
                GRACE,
                DEADLINE,
            )
        };

        assert_eq!(
            [
                needs(1, false, last + HOLD_MILLIS),
                needs(1, false, last + HOLD_MILLIS - 1),
                needs(0, false, last + HOLD_MILLIS),
                needs(1, true, last + HOLD_MILLIS),
            ],
            [true, false, false, false]
        );
    }

    #[test]
    fn the_hold_holds_the_ledger_and_the_claims_and_a_time_more_than_the_margin_ahead_counts_as_missing()
     {
        let now = 10_000_000;
        let margin = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap();
        let due = |last| {
            prune_due(
                &ledger(Some(last), false),
                1,
                at(now),
                0,
                Percent(0),
                GRACE,
                DEADLINE,
            )
        };
        let claim = |claimed_at| {
            next_claim(
                &[ClaimEntry::Claim(0), ClaimEntry::Marker(0, at(claimed_at))],
                at(now),
                claim_hold(GRACE, DEADLINE),
            )
        };

        assert_eq!(
            (
                [
                    due(now - HOLD_MILLIS + 1),
                    due(now - HOLD_MILLIS),
                    due(now + margin),
                    due(now + margin + 1)
                ],
                [
                    claim(now - HOLD_MILLIS + 1),
                    claim(now - HOLD_MILLIS),
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
    fn any_young_marker_holds_a_ledger_and_a_claim_without_a_marker_is_old() {
        let now = 10_000_000;
        let claim = ClaimEntry::Claim;
        let marker = |number, millis| ClaimEntry::Marker(number, at(millis));
        let choose =
            |entries: &[ClaimEntry]| next_claim(entries, at(now), claim_hold(GRACE, DEADLINE));

        assert_eq!(
            [
                choose(&[]),
                choose(&[claim(0), claim(1), marker(0, now - 1)]),
                choose(&[claim(0), claim(1), marker(1, now - HOLD_MILLIS)]),
                choose(&[claim(4)]),
                choose(&[marker(2, now - 1)]),
                choose(&[claim(0), claim(3), marker(0, now - HOLD_MILLIS)]),
            ],
            [
                ClaimChoice::Claim(0),
                ClaimChoice::Held,
                ClaimChoice::Claim(2),
                ClaimChoice::Claim(5),
                ClaimChoice::Held,
                ClaimChoice::Claim(4),
            ]
        );
    }

    #[test]
    fn the_lease_is_the_grace_period_less_one_deadline_and_at_least_one_deadline_less_one_millisecond()
     {
        let second = Duration::from_secs(1);
        let minute = Duration::from_secs(60);

        assert_eq!(
            [
                lease_span(GRACE, minute),
                lease_span(2 * minute, minute),
                lease_span(2 * minute - second, minute),
                lease_span(Duration::ZERO, minute),
                lease_span(minute, Duration::ZERO),
                lease_span(Duration::ZERO, second),
            ],
            [GRACE - minute, minute, minute, minute, minute, second].map(|span| span - MILLI)
        );
    }

    #[test]
    fn the_hold_is_the_lease_then_one_margin_then_two_deadlines() {
        let margin = CLOCK_SKEW_MARGIN;
        let minute = Duration::from_secs(60);

        assert_eq!(
            [
                claim_hold(GRACE, minute),
                claim_hold(GRACE, DEADLINE),
                claim_hold(Duration::ZERO, minute),
                claim_hold(Duration::ZERO, Duration::from_millis(1)),
            ],
            [
                GRACE + margin + minute,
                GRACE + margin + DEADLINE,
                minute + margin + 2 * minute,
                MILLI + margin + 2 * MILLI,
            ]
        );
    }

    #[test]
    fn each_lease_has_at_least_two_refreshes() {
        let minute = Duration::from_secs(60);
        let cases = [
            (GRACE, minute),
            (GRACE, DEADLINE),
            (Duration::ZERO, minute),
            (Duration::ZERO, 2 * MILLI),
            (Duration::from_millis(16), minute),
            (Duration::from_secs(2), Duration::from_secs(1)),
        ];

        assert_eq!(
            cases.map(|(grace, deadline)| refresh_period(grace, deadline)),
            [
                (GRACE - minute - MILLI) / 4,
                (GRACE - DEADLINE - MILLI) / 4,
                (minute - MILLI) / 4,
                Duration::from_micros(250),
                Duration::from_millis(4),
                Duration::from_micros(249_750),
            ]
        );
        assert!(cases.iter().all(|(grace, deadline)| {
            refresh_period(*grace, *deadline) * 2 <= lease_span(*grace, *deadline)
        }));
    }

    #[test]
    fn a_claim_entry_name_is_a_number_or_a_number_with_a_time_and_a_unique_part() {
        assert_eq!(
            [
                parse_claim_entry("3"),
                parse_claim_entry("3@42-a"),
                parse_claim_entry("3@42-a-b"),
                parse_claim_entry("3@42-"),
                parse_claim_entry("3@x-a"),
                parse_claim_entry("x"),
            ],
            [
                Some(ClaimEntry::Claim(3)),
                Some(ClaimEntry::Marker(3, at(42))),
                Some(ClaimEntry::Marker(3, at(42))),
                None,
                None,
                None,
            ]
        );
    }

    #[test]
    #[timeout("60s")]
    async fn the_refresh_of_a_claim_ends_when_its_operation_is_cancelled() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let files = files_with_cancel(Arc::new(InMemoryBlobStorage::new()), cancel.clone());
        cancel.cancel();

        let ended = tokio::time::timeout(
            Duration::from_secs(10),
            keep_claim_fresh(
                &files,
                &claims_directory(&ledger(None, false)),
                0,
                Duration::from_secs(3600),
                &std::sync::Mutex::default(),
                &Lease::until(Instant::now()),
                GRACE,
                &SystemClock,
            ),
        )
        .await
        .is_ok();

        assert!(ended);
    }

    /// Refreshes the claim at each period until the first refresh write succeeds, and gives the
    /// instant after that write. It gives `None` when the refresh ends before a write succeeded.
    async fn refresh_until_written(
        files: &SnapshotFiles,
        period: Duration,
        lease: &Lease,
        span: Duration,
    ) -> Option<Instant> {
        let written = std::sync::Mutex::<Vec<Box<Path>>>::default();
        let directory = claims_directory(&ledger(None, false));
        tokio::select! {
            () = keep_claim_fresh(files, &directory, 0, period, &written, lease, span, &SystemClock) => None,
            ended = async {
                futures::stream::repeat(())
                    .then(|()| tokio::time::sleep(Duration::from_millis(5)))
                    .take_while(|()| {
                        std::future::ready(
                            written.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_empty(),
                        )
                    })
                    .for_each(|()| std::future::ready(()))
                    .await;
                Instant::now()
            } => Some(ended),
        }
    }

    #[test]
    #[timeout("60s")]
    async fn a_refresh_that_starts_before_the_end_of_the_lease_and_ends_after_it_moves_the_lease_to_its_start_plus_the_span()
     {
        // The first refresh starts about 10 ms after the lease starts, before its end at 250 ms.
        // Each refresh write takes the delay, so the write ends after the end of the lease. A lease
        // from the end of the write would be later than a lease from its start by the delay.
        let delay = Duration::from_millis(500);
        let span = Duration::from_secs(1);
        let storage =
            ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), move |op_label, _| {
                if op_label == "refresh_claim" {
                    Script::Delay(delay)
                } else {
                    Script::Pass
                }
            });
        let files = files_over(storage);
        let started = Instant::now();
        let first_expiry = started + Duration::from_millis(250);
        let lease = Lease::until(first_expiry);

        let ended = refresh_until_written(&files, Duration::from_millis(10), &lease, span)
            .await
            .unwrap();

        let expiry = lease.expiry();
        assert!(
            ended > first_expiry,
            "the write ended before the end of the lease"
        );
        assert!(expiry >= started + span, "the lease did not move");
        assert!(
            expiry + delay / 2 < ended + span,
            "the lease moved to the end of the write"
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_refresh_that_starts_after_the_lease_ran_out_does_not_move_it_and_the_next_call_is_refused()
     {
        // The lease ends when the test starts. The first refresh write fails, and a later one
        // succeeds, but each refresh starts after the end of the lease. Nothing calls the backend
        // while the lease is out, and the first call after the refresh finds the lease still out.
        let refused = Arc::new(AtomicBool::new(false));
        let storage = ScriptedBlobStorage::new(Arc::new(InMemoryBlobStorage::new()), {
            let refused = refused.clone();
            move |op_label, _| {
                if op_label == "refresh_claim" && !refused.swap(true, Ordering::SeqCst) {
                    Script::Refuse
                } else {
                    Script::Pass
                }
            }
        });
        let files = files_over(storage.clone());
        let expiry = Instant::now();
        let lease = Arc::new(Lease::until(expiry));

        let refreshed = refresh_until_written(
            &files,
            Duration::from_millis(10),
            &lease,
            Duration::from_secs(3600),
        )
        .await
        .is_some();
        let backend = BlobBackend::new(
            files.leased(lease.clone()),
            tokio::runtime::Handle::current(),
            KEPT_PACKS_LIMIT,
        );
        let listed = tokio::task::spawn_blocking(move || backend.list(FileType::Snapshot))
            .await
            .unwrap();

        assert_eq!(
            (
                refreshed,
                refused.load(Ordering::SeqCst),
                lease.expiry() == expiry,
                listed
                    .as_ref()
                    .err()
                    .is_some_and(|error| is_lease_expired(error)),
            ),
            (true, true, true, true),
            "{listed:?}"
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_claim_is_taken_after_its_marker_and_a_loser_deletes_its_marker() {
        let files = new_files();
        let directory = claims_directory(&ledger(Some(42), false));
        let marker = || {
            let (at, time) = marker_time(&SystemClock);
            (marker_path(&directory, 0, time), at)
        };
        let (first_marker, first_at) = marker();
        let (second_marker, second_at) = marker();
        let first = take_claim(&files, &directory, 0, &first_marker, first_at, GRACE)
            .await
            .unwrap()
            .map(|lease| lease.expiry());
        let again = take_claim(&files, &directory, 0, &second_marker, second_at, GRACE)
            .await
            .unwrap()
            .map(|lease| lease.expiry());
        let listed = list_claims(&files, &directory).await.unwrap();

        assert_eq!(
            (
                directory.display().to_string(),
                first,
                again,
                listed.len(),
                listed.contains(&ClaimEntry::Claim(0)),
                listed
                    .iter()
                    .any(|entry| matches!(entry, ClaimEntry::Marker(0, _))),
            ),
            (
                "golem/prune-claims/42".to_string(),
                Some(first_at + GRACE),
                None,
                2,
                true,
                true,
            )
        );
    }

    #[test]
    fn a_claim_directory_is_old_when_it_is_none_or_its_time_is_before_the_new_ledger() {
        let claim =
            |directory: &str, number: &str| Path::new(CLAIMS_PATH).join(directory).join(number);
        let directory = |name: &str| Path::new(CLAIMS_PATH).join(name);

        assert_eq!(
            old_claim_directories(
                [
                    claim("none", "0"),
                    claim("100", "0"),
                    claim("100", "1@5-a"),
                    claim("200", "3"),
                    directory("250"),
                    claim("260", "made/below"),
                    claim("300", "0"),
                    directory("300"),
                    claim("301", "0"),
                    directory("400"),
                    claim("x1", "0"),
                    Path::new(LEDGERS_PATH).join("5-0-a"),
                    PathBuf::from(CLAIMS_PATH),
                ],
                Timestamp::from(300)
            )
            .to_vec(),
            ["100", "200", "250", "260", "none"].map(|name| directory(name).into_boxed_path())
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
        let id = |digit: char| {
            std::iter::repeat_n(digit, 64)
                .collect::<String>()
                .into_boxed_str()
        };
        let record = |name: &str, bytes: u64, snapshots: Option<Vec<Box<str>>>| FreedRecord {
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
        let id = |digit: char| {
            std::iter::repeat_n(digit, 64)
                .collect::<String>()
                .into_boxed_str()
        };
        let ids = [id('a'), id('b')];

        assert_eq!(
            [
                parse_record(record_content(&ids).as_bytes()),
                parse_record(b""),
                parse_record(b"not an id"),
                parse_record(&[0xff, 0xfe]),
            ],
            [
                Some(Box::new(ids.clone()) as Box<[Box<str>]>),
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn a_record_line_parses_only_when_it_has_64_characters_that_are_each_hex() {
        // A record whose content does not parse names no snapshot file, so it counts as zero
        // bytes and stays.
        let hex = "0123456789abcdef".repeat(4);
        let parsed = [
            parse_record("g".repeat(64).as_bytes()),
            parse_record(b"abcdef0123"),
            parse_record(hex.as_bytes()),
        ];
        let records = parsed
            .iter()
            .enumerate()
            .map(|(index, snapshots)| FreedRecord {
                path: Path::new(FREED_PATH)
                    .join(format!("10-{index}"))
                    .into_boxed_path(),
                bytes: 10,
                snapshots: snapshots.clone(),
            })
            .collect::<Vec<_>>();

        assert_eq!(
            (
                parsed.clone(),
                settle(&records, &std::collections::HashSet::new()),
            ),
            (
                [
                    None,
                    None,
                    Some(Box::new([hex.clone().into_boxed_str()]) as Box<[Box<str>]>)
                ],
                FreedRecords {
                    bytes: 10,
                    counted: Box::new([Path::new(FREED_PATH).join("10-2").into_boxed_path()]),
                },
            )
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_record_of_freed_bytes_is_written_and_listed() {
        let files = new_files();

        let gone = ["0".repeat(64).into_boxed_str()];
        record_freed(&files, 40, &gone).await.unwrap();
        record_freed(&files, 2, &gone).await.unwrap();
        let listed = list_freed_names(&files).await.unwrap();
        let settled = settle_freed(&files, &listed).await.unwrap();

        assert_eq!(
            (named_bytes(&listed), settled.bytes, settled.counted.len()),
            (42, 42, 2)
        );
    }

    #[test]
    #[timeout("60s")]
    async fn a_written_entry_is_the_ledger_that_a_read_gives() {
        let files = new_files();
        let ended = Timestamp::from(Timestamp::now_utc().to_millis());

        let before = read_ledger(&files, &SystemClock).await.unwrap();
        write_ledger(&files, ended, true).await.unwrap();
        let after = read_ledger(&files, &SystemClock).await.unwrap();

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
