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

//! Retention by count. A job keeps its own snapshot and the newest older snapshots of the same
//! kind, and deletes the rest of its kind that are older than its own.

use crate::filesystem_snapshot::{SnapshotInfo, SnapshotName};
use golem_common::model::oplog::FilesystemSnapshotName;
use std::time::Duration;

/// The largest difference between the clocks of two executors that retention allows for. A
/// snapshot whose time is this close to the time of the own snapshot is neither counted nor
/// deleted, because another executor gave it its time.
pub(super) const CLOCK_SKEW_MARGIN: Duration = Duration::from_secs(2 * 60);

/// Gives the names of `listing` that retention deletes after the save of `own`, whose info is
/// `own_info`. Of the kind of `own`, it keeps `own` and the `keep - 1` newest names whose time is
/// more than [`CLOCK_SKEW_MARGIN`] before the time of `own`, and gives the older ones. It never
/// gives `own`, a name of `kept`, a name of another kind, or a name whose time is within the
/// margin or later. A name of `kept` does not count either. `listing` is the listing of an agent,
/// in any order.
pub(super) fn victims(
    listing: &[(SnapshotName, SnapshotInfo)],
    own: &SnapshotName,
    own_info: &SnapshotInfo,
    keep: usize,
    kept: &[SnapshotName],
) -> Box<[SnapshotName]> {
    let prefix = if own
        .as_str()
        .starts_with(FilesystemSnapshotName::UPDATE_PREFIX)
    {
        FilesystemSnapshotName::UPDATE_PREFIX
    } else {
        FilesystemSnapshotName::PERIODIC_PREFIX
    };
    let margin = u64::try_from(CLOCK_SKEW_MARGIN.as_millis()).unwrap_or(u64::MAX);
    let older_than = own_info.created_at.to_millis().saturating_sub(margin);
    let mut older = listing
        .iter()
        .filter(|(name, info)| {
            name != own
                && !kept.contains(name)
                && name.as_str().starts_with(prefix)
                && info.created_at.to_millis() < older_than
        })
        .collect::<Vec<_>>();
    older.sort_by_key(|(_, info)| std::cmp::Reverse(info.created_at.to_millis()));
    older
        .into_iter()
        .skip(keep.saturating_sub(1))
        .map(|(name, _)| name.clone())
        .collect()
}
