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

//! Append-only prune ledgers and records of bytes freed by named deletes.
//!
//! Each ledger entry names the time at which a prune ended and whether it marked packs that a
//! later prune removes. The newest entry is the ledger. Only the delete that holds the prune
//! claim writes an entry, and an entry is never overwritten. Each delete that freed bytes writes
//! its own record with the count in the name. A successful prune deletes only the records it
//! counted.

use super::super::blob_io::SnapshotFiles;
use super::super::repository_layout::{FREED_PATH, LEDGERS_PATH, SNAPSHOTS_PATH};
use super::timing::beyond_margin;
use crate::filesystem_snapshot::clock::Clock;
use futures::{StreamExt, TryStreamExt, stream};
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::ListedBlob;
use std::collections::HashSet;
use std::path::Path;
use tracing::warn;

/// What the scope did since its last prune.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::filesystem_snapshot::backend::rustic) struct PruneLedger {
    /// The time of the last prune.
    pub(in crate::filesystem_snapshot::backend::rustic) last_prune: Option<Timestamp>,
    /// Whether the last prune marked packs that a later prune removes.
    pub(in crate::filesystem_snapshot::backend::rustic) awaiting_removal: bool,
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
pub(in crate::filesystem_snapshot::backend::rustic) async fn read_ledger(
    files: &SnapshotFiles,
    clock: &dyn Clock,
) -> anyhow::Result<PruneLedger> {
    let listed = files
        .list_below("read_ledger", Path::new(LEDGERS_PATH))
        .await?;
    Ok(newest_ledger(&listed, clock.now()))
}

/// Writes a new ledger entry for a prune that ended at `ended`.
pub(in crate::filesystem_snapshot::backend::rustic) async fn write_ledger(
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
pub(in crate::filesystem_snapshot::backend::rustic) async fn remove_older_ledgers(
    files: &SnapshotFiles,
    ended: Timestamp,
) {
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
struct FreedRecord {
    path: Box<Path>,
    bytes: u64,
    snapshots: Option<Box<[Box<str>]>>,
}

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
pub(in crate::filesystem_snapshot::backend::rustic) fn parse_freed(name: &str) -> Option<u64> {
    let (bytes, unique) = name.split_once('-')?;
    if unique.is_empty() {
        return None;
    }
    bytes.parse().ok()
}

/// Writes a record of the freed bytes of one delete, with the ids of its snapshot files.
pub(in crate::filesystem_snapshot::backend::rustic) async fn record_freed(
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

#[cfg(test)]
mod tests;
