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
//! an entry, and an entry is never written over. Each delete that freed bytes writes a record of
//! its own, with the count in the name, and a prune that succeeds deletes the records it counted.
//!
//! A delete whose prune is due takes a claim before it prunes, so two deletes that read the same
//! ledger make one prune. The claims of a ledger are in one directory, named by the time of the
//! last prune in that ledger.

use super::files::{Lease, SnapshotFiles};
use crate::filesystem_snapshot::clock::Clock;
use futures::{Stream, StreamExt, TryStreamExt, stream};
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::{ListedBlob, PutIfAbsent};
use std::collections::HashSet;
use std::future::ready;
use std::path::Path;
use std::sync::Arc;
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
fn hold_passed(ledger: &PruneLedger, now: Timestamp, grace: Duration, deadline: Duration) -> bool {
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

/// Tells whether the freed bytes can make a prune due with the size of the repository: they reach
/// the threshold share of `repository_bytes`, rounded down to a whole byte, or the last prune
/// marked packs. A threshold of zero bytes counts as one byte, so a prune never runs for a scope
/// that freed nothing and marked nothing. The bytes in the names of the records are at least the
/// settled bytes, so when the named bytes give false, the records need no read.
fn may_be_due(
    ledger: &PruneLedger,
    freed_bytes: u64,
    repository_bytes: u64,
    threshold: Percent,
) -> bool {
    freed_bytes >= threshold.of(repository_bytes).max(1) || ledger.awaiting_removal
}

/// Gives the size of the repository of the scope: the sum of the sizes of its packs.
async fn repository_bytes(files: &SnapshotFiles) -> anyhow::Result<u64> {
    Ok(files
        .list_below("list_data", Path::new(DATA_PATH))
        .await?
        .iter()
        .map(|blob| blob.size)
        .fold(0, u64::saturating_add))
}

/// Reads a ledger entry name, `<end ms>-<0|1>-<unique part>`, as the time of the prune and whether
/// it marked packs.
fn parse_ledger_entry(name: &str) -> Option<PruneLedger> {
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
fn newest_ledger(listed: &[ListedBlob], now: Timestamp) -> PruneLedger {
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
fn older_entries(listed: &[ListedBlob], ended: Timestamp) -> Box<[Box<Path>]> {
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
    bytes: u64,
    counted: Box<[Box<Path>]>,
}

/// A record of freed bytes that a delete wrote: its path, the bytes in its name, and the ids of
/// the snapshot files of that delete, when its content parses.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FreedRecord {
    path: Box<Path>,
    bytes: u64,
    snapshots: Option<Box<[Box<str>]>>,
}

/// The directory of the snapshot files of a repository.
pub(super) const SNAPSHOTS_PATH: &str = "snapshots";

/// Gives the content of a record: the id of each snapshot file of the delete, one on each line.
fn record_content(snapshots: &[Box<str>]) -> String {
    snapshots.join("\n")
}

/// Reads the snapshot ids from the content of a record. Each line must be an id of 64 hex
/// characters. A content without an id does not parse, because a reader can see a record that a
/// write has not filled yet.
fn parse_record(content: &[u8]) -> Option<Box<[Box<str>]>> {
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
fn settle(records: &[FreedRecord], existing: &HashSet<Box<str>>) -> FreedRecords {
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
struct ListedFreed {
    path: Box<Path>,
    bytes: u64,
}

/// Lists the names of the records of freed bytes, and reads no content. A name that does not
/// parse is left out, so it counts as zero bytes and stays.
async fn list_freed_names(files: &SnapshotFiles) -> anyhow::Result<Box<[ListedFreed]>> {
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
fn named_bytes(listed: &[ListedFreed]) -> u64 {
    listed
        .iter()
        .map(|record| record.bytes)
        .fold(0, u64::saturating_add)
}

/// Reads the listed records of freed bytes, lists the snapshot files one time, and gives the
/// settled records. A record that a prune deleted after the listing is left out. Without records,
/// no snapshot file is listed.
async fn settle_freed(
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

/// Gives how long a marker write that succeeds lets the prune of its claim go on: one refresh
/// period and two storage call deadlines, but at most the lease bound less one millisecond. The
/// name of a marker keeps its time in whole milliseconds, so the time in the name can be up to one
/// millisecond before the time that the delete read. So the lease is above zero for each deadline
/// above one millisecond. A prune stops its reads and deletes of snapshot data at most one span
/// after the start of its newest marker write that succeeded, also when its later refresh writes
/// fail.
pub(super) fn lease_span(grace: Duration, deadline: Duration) -> Duration {
    refresh_period(grace, deadline)
        .saturating_add(deadline.saturating_mul(2))
        .min(lease_bound(grace, deadline).saturating_sub(MARKER_TIME_UNIT))
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
fn claims_directory(ledger: &PruneLedger) -> Box<Path> {
    let generation = ledger
        .last_prune
        .map_or_else(|| "none".to_string(), |last| last.to_millis().to_string());
    Path::new(CLAIMS_PATH).join(generation).into_boxed_path()
}

/// Tells whether a claim in the directory belongs to the ledger: the directory is the claim
/// directory of the time of the last prune in the ledger. Whether marked packs wait for removal
/// does not count.
pub(super) fn belongs_to_ledger(directory: &Path, ledger: &PruneLedger) -> bool {
    *claims_directory(ledger) == *directory
}

/// Lists the claims and the markers in the directory, from their names. A name that does not
/// parse is left out.
async fn list_claims(files: &SnapshotFiles, directory: &Path) -> anyhow::Result<Box<[ClaimEntry]>> {
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
    let started = super::runs::now();
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

/// Writes a marker of the claim. A write that succeeds and started before the end of the lease
/// moves the end to `span` after the start of the write, when that is later. A write that started
/// at or after the end does not move it.
async fn write_leased_marker(
    files: &SnapshotFiles,
    op_label: &'static str,
    claim: &ClaimName,
    lease: &Lease,
    span: Duration,
    clock: &dyn Clock,
) -> anyhow::Result<Box<Path>> {
    let (started, time) = marker_time(clock);
    let marker = write_marker(files, op_label, &claim.directory, claim.number, time).await?;
    lease.extend_from(started, span);
    Ok(marker)
}

/// Writes the first marker of the claim at the path `marker`, then takes the claim, and tells
/// whether this delete holds the claim. The caller makes the path and the lease before the write,
/// so the claim can delete the marker when the delete stops during the write. A delete that loses
/// the claim tries to delete its marker. A failed delete gives a warning.
pub(super) async fn take_claim(
    files: &SnapshotFiles,
    claim: &ClaimName,
    marker: &Path,
) -> anyhow::Result<bool> {
    write_marker_at(files, "write_marker", marker).await?;
    let written = files
        .put_if_absent(
            "write_claim",
            &claim.directory.join(claim.number.to_string()),
            &[],
        )
        .await?;
    if written == PutIfAbsent::Written {
        return Ok(true);
    }
    if let Err(error) = files.delete("delete_marker", marker).await {
        warn!(
            error = %format!("{error:#}"),
            "Failed to delete the marker of a prune claim that a filesystem snapshot delete lost"
        );
    }
    Ok(false)
}

/// Gives the time between two markers of a live claim: a fourth of the grace period, or a fourth
/// of the margin for clock skew when the grace period is zero, but at most a fourth of the lease
/// bound less one millisecond. It comes from the lease bound and not from the lease span, which
/// comes from it.
pub(super) fn refresh_period(grace: Duration, deadline: Duration) -> Duration {
    let base = if grace.is_zero() {
        CLOCK_SKEW_MARGIN
    } else {
        grace
    };
    (base / 4).min(lease_bound(grace, deadline).saturating_sub(MARKER_TIME_UNIT) / 4)
}

/// Writes a new marker of the claim at each period, until the caller drops the stream or the
/// operation of the files is cancelled, and gives the path of each marker whose write succeeded.
/// The first write starts `period` after `marked`, the start of the write of the first marker of
/// the claim, and each later write one period after the end of the write before it. A write that
/// succeeds and started before the end of the lease moves the end to `span` after its start, when
/// that is later. A write that started at or after the end does not move it. A failed write gives a
/// warning, and the next period tries again.
pub(super) fn keep_claim_fresh<'a>(
    files: &'a SnapshotFiles,
    claim: &'a ClaimName,
    marked: Instant,
    period: Duration,
    lease: &'a Lease,
    span: Duration,
    clock: &'a dyn Clock,
) -> impl Stream<Item = Box<Path>> + 'a {
    stream::once(tokio::time::sleep_until(tokio::time::Instant::from_std(
        marked + period,
    )))
    .chain(stream::repeat(()).then(move |()| tokio::time::sleep(period)))
        .take_until(files.cancelled())
        .then(move |()| write_leased_marker(files, "refresh_claim", claim, lease, span, clock))
        .filter_map(|written| {
            ready(
                written
                    .inspect_err(|error| {
                        warn!(
                            error = %format!("{error:#}"),
                            "Failed to write a new marker of the prune claim of a filesystem snapshot scope"
                        )
                    })
                    .ok(),
            )
        })
}

/// Deletes the claim with the number when this delete took it, and then each of its markers by its
/// path. It tries each delete also when another one fails, and each failure gives a warning. A
/// claim that stays without its markers is old, and a marker that stays only delays a prune until
/// its hold passed.
pub(super) async fn release_claim<'a>(
    files: &SnapshotFiles,
    directory: &Path,
    number: u64,
    claimed: bool,
    markers: impl IntoIterator<Item = &'a Path>,
) {
    let claim = directory.join(number.to_string());
    let deletes = claimed
        .then_some(("delete_claim", claim.as_path()))
        .into_iter()
        .chain(markers.into_iter().map(|marker| ("delete_marker", marker)))
        .collect::<Box<[_]>>();
    stream::iter(deletes)
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
fn old_claim_directories(
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

/// The values of the prune decision: the grace period of a pack that a prune marks, the deadline
/// of one storage call, and the share of the size of the repository that freed bytes must reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PrunePolicy {
    pub(super) grace: Duration,
    pub(super) deadline: Duration,
    pub(super) threshold: Percent,
}

/// A claim of a prune: the directory of the claims of its ledger, and its number.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ClaimName {
    pub(super) directory: Arc<Path>,
    pub(super) number: u64,
}

/// A prune that is due: the claim that the delete takes, and the settled records that the prune
/// counts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct DuePrune {
    pub(super) claim: ClaimName,
    pub(super) records: FreedRecords,
}

/// What a due check found.
pub(super) enum DueCheck {
    /// A prune is due, with this claim.
    Due(DuePrune),
    /// No prune is due.
    NotDue,
    /// A prune is due, and another delete holds its claims.
    Held,
}

/// What a delete found on its way to a prune. A field is `None` until its step ran.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Observed {
    /// The ledger, from the first listing of the ledger entries.
    ledger: Option<PruneLedger>,
    /// The records of freed bytes that the listing of their names found.
    freed: Option<Box<[ListedFreed]>>,
    /// The size of the repository.
    size: Option<u64>,
    /// The settled records.
    settled: Option<FreedRecords>,
    /// The entries of the claim directory of the ledger.
    claims: Option<Box<[ClaimEntry]>>,
}

/// The next step of a due check.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Next {
    /// List the ledger entries.
    ListLedgers,
    /// List the names of the records of freed bytes.
    ListFreed,
    /// List the packs for the size of the repository.
    ListPacks,
    /// Read the records of freed bytes and list the snapshot files.
    SettleFreed,
    /// List the claim directory of the ledger.
    ListClaims(Box<Path>),
    /// No prune is due, or another delete holds the claims of the ledger.
    Stop,
    /// Take the claim.
    Claim(ClaimName),
}

/// What one step of a due check found.
#[derive(Debug)]
enum Found {
    Ledgers(Box<[ListedBlob]>),
    Freed(Box<[ListedFreed]>),
    Packs(u64),
    Settled(FreedRecords),
    Claims(Box<[ClaimEntry]>),
}

impl Observed {
    /// Adds what a step found. The ledger is the newest entry within the margin of `now`.
    fn with(self, found: Found, now: Timestamp) -> Self {
        match found {
            Found::Ledgers(listed) => Self {
                ledger: Some(newest_ledger(&listed, now)),
                ..self
            },
            Found::Freed(freed) => Self {
                freed: Some(freed),
                ..self
            },
            Found::Packs(size) => Self {
                size: Some(size),
                ..self
            },
            Found::Settled(settled) => Self {
                settled: Some(settled),
                ..self
            },
            Found::Claims(claims) => Self {
                claims: Some(claims),
                ..self
            },
        }
    }
}

/// Decides the next step of a due check from what the delete found, at `now`.
///
/// A prune is due when the hold of a claim passed since the last prune, and the settled freed
/// bytes can make a prune due with the size of the repository (see [`may_be_due`]). The check
/// reads only what can still make a prune due. Within the hold it reads nothing more. It lists the
/// packs only when the records name freed bytes and the last prune marked no packs, because marked
/// packs make a prune due at any size. It reads the records only when the bytes in their names can
/// make a prune due, or when the last prune marked packs. When a prune is due, the delete takes the
/// claim that [`next_claim`] chooses from the claims of the ledger, or it stops when another delete
/// holds them.
fn next(observed: &Observed, now: Timestamp, policy: &PrunePolicy) -> Next {
    let Some(ledger) = observed.ledger else {
        return Next::ListLedgers;
    };
    if !hold_passed(&ledger, now, policy.grace, policy.deadline) {
        return Next::Stop;
    }
    let Some(freed) = &observed.freed else {
        return Next::ListFreed;
    };
    let named = named_bytes(freed);
    if named == 0 && !ledger.awaiting_removal {
        return Next::Stop;
    }
    let size = match observed.size {
        Some(size) => size,
        None if !ledger.awaiting_removal => return Next::ListPacks,
        None => 0,
    };
    if !may_be_due(&ledger, named, size, policy.threshold) {
        return Next::Stop;
    }
    let Some(settled) = &observed.settled else {
        return Next::SettleFreed;
    };
    if !may_be_due(&ledger, settled.bytes, size, policy.threshold) {
        return Next::Stop;
    }
    let directory = claims_directory(&ledger);
    let Some(claims) = &observed.claims else {
        return Next::ListClaims(directory);
    };
    match next_claim(claims, now, claim_hold(policy.grace, policy.deadline)) {
        ClaimChoice::Claim(number) => Next::Claim(ClaimName {
            directory: directory.into(),
            number,
        }),
        ClaimChoice::Held => Next::Stop,
    }
}

/// Makes the storage calls of the step, and gives what they found. `Stop` and `Claim` make no call
/// and give `None`.
async fn observe(
    files: &SnapshotFiles,
    observed: &Observed,
    step: &Next,
) -> anyhow::Result<Option<Found>> {
    Ok(Some(match step {
        Next::ListLedgers => Found::Ledgers(
            files
                .list_below("read_ledger", Path::new(LEDGERS_PATH))
                .await?,
        ),
        Next::ListFreed => Found::Freed(list_freed_names(files).await?),
        Next::ListPacks => Found::Packs(repository_bytes(files).await?),
        Next::SettleFreed => Found::Settled(
            settle_freed(files, observed.freed.as_deref().unwrap_or_default()).await?,
        ),
        Next::ListClaims(directory) => Found::Claims(list_claims(files, directory).await?),
        Next::Stop | Next::Claim(_) => return Ok(None),
    }))
}

/// The most passes of a due check: one for each kind of listing. These are the ledger entries, the
/// names of the records, the packs, the records with the snapshot files, and the claims.
const MOST_PASSES: usize = 5;

/// Tells whether a prune of the scope is due, and which claim the delete takes for it.
///
/// Each pass before a stop or a claim makes the storage calls of the step that [`next`] asked for,
/// if the step needs any, then reads the clock one time, and asks [`next`] again with that reading.
/// The passes after `Stop` or `Claim` make no call and read no clock. So each comparison with a
/// time from storage uses a clock reading from after the listing that gave that time. A listing
/// can take up to one storage call deadline, and a stale reading can put a marker that another host
/// wrote within the margin beyond the margin. A failed call gives its error.
pub(super) async fn due_prune(
    files: &SnapshotFiles,
    clock: &dyn Clock,
    policy: &PrunePolicy,
) -> anyhow::Result<DueCheck> {
    let (observed, step) = stream::iter(0..MOST_PASSES)
        .map(Ok)
        .try_fold(
            (Observed::default(), Next::ListLedgers),
            |(observed, step), _| async move {
                let Some(found) = observe(files, &observed, &step).await? else {
                    return Ok((observed, step));
                };
                let now = clock.now();
                let observed = observed.with(found, now);
                let step = next(&observed, now, policy);
                anyhow::Ok((observed, step))
            },
        )
        .await?;
    Ok(match step {
        Next::Claim(claim) => DueCheck::Due(DuePrune {
            claim,
            records: observed.settled.unwrap_or_default(),
        }),
        // [`next`] reads the claims only when a prune is due, and then stops only when another
        // delete holds them.
        Next::Stop if observed.claims.is_some() => DueCheck::Held,
        Next::ListLedgers
        | Next::ListFreed
        | Next::ListPacks
        | Next::SettleFreed
        | Next::ListClaims(_)
        | Next::Stop => DueCheck::NotDue,
    })
}

#[cfg(test)]
mod tests;
