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

//! Retention by count: the newest snapshots of each kind stay, and the rest go.

use crate::filesystem_snapshot::{SnapshotInfo, SnapshotName};

/// The prefix of the name of a periodic snapshot.
const PERIODIC_PREFIX: &str = "p-";
/// The prefix of the name of a manual-update snapshot.
const UPDATE_PREFIX: &str = "u-";

/// Gives the names of `listing` that retention deletes: each periodic name after the newest
/// `keep_periodic`, and each manual-update name after the newest `keep_update`. `listing` is the
/// listing of a scope, newest first. A name of another kind stays.
pub(super) fn victims(
    listing: &[(SnapshotName, SnapshotInfo)],
    keep_periodic: usize,
    keep_update: usize,
) -> Box<[SnapshotName]> {
    let of_kind = |prefix: &'static str, keep: usize| {
        listing
            .iter()
            .filter(move |(name, _)| name.as_str().starts_with(prefix))
            .skip(keep)
            .map(|(name, _)| name.clone())
    };
    of_kind(PERIODIC_PREFIX, keep_periodic)
        .chain(of_kind(UPDATE_PREFIX, keep_update))
        .collect()
}
