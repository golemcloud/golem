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

//! Due-check decisions and the storage reads those decisions request.

use super::super::blob_io::SnapshotFiles;
use super::super::repository_layout::{DATA_PATH, LEDGERS_PATH};
use super::claim::{ClaimChoice, ClaimEntry, ClaimName, claims_directory, list_claims, next_claim};
use super::records::{
    FreedRecords, ListedFreed, PruneLedger, list_freed_names, named_bytes, newest_ledger,
    settle_freed,
};
use super::timing::{beyond_margin, claim_hold, passed_since};
use crate::filesystem_snapshot::clock::Clock;
use futures::{StreamExt, TryStreamExt, stream};
use golem_common::model::Timestamp;
use golem_service_base::storage::blob::ListedBlob;
use std::path::Path;
use std::time::Duration;

/// A share of the size of a repository, in percent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::filesystem_snapshot::backend::rustic) struct Percent(
    pub(in crate::filesystem_snapshot::backend::rustic) u16,
);

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

/// The values of the prune decision: the grace period of a pack that a prune marks, the deadline
/// of one storage call, and the share of the size of the repository that freed bytes must reach.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DueCriteria {
    pub(super) grace: Duration,
    pub(super) deadline: Duration,
    pub(super) threshold: Percent,
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
fn next(observed: &Observed, now: Timestamp, criteria: &DueCriteria) -> Next {
    let Some(ledger) = observed.ledger else {
        return Next::ListLedgers;
    };
    if !hold_passed(&ledger, now, criteria.grace, criteria.deadline) {
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
    if !may_be_due(&ledger, named, size, criteria.threshold) {
        return Next::Stop;
    }
    let Some(settled) = &observed.settled else {
        return Next::SettleFreed;
    };
    if !may_be_due(&ledger, settled.bytes, size, criteria.threshold) {
        return Next::Stop;
    }
    let directory = claims_directory(&ledger);
    let Some(claims) = &observed.claims else {
        return Next::ListClaims(directory);
    };
    match next_claim(claims, now, claim_hold(criteria.grace, criteria.deadline)) {
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
    criteria: &DueCriteria,
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
                let step = next(&observed, now, criteria);
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
