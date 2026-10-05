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

//! The steps of one run of a restore, as a pure rule over what the run observed.
//!
//! A run loads the repository and the index, checks that the index holds the whole tree before it
//! writes anything, and then writes the tree. A prune that runs at the same time can delete an
//! index file that the run listed, or a pack that the index of the run lists. [`next_step`] tells
//! the run to load again, to read the snapshots again, or to answer, so that a name that no delete
//! removes gives the whole tree and a name that a delete removes gives `NotFound`.

use super::runs::RunEnd;
use std::collections::HashSet;
use std::path::Path;

/// What a lookup of the name in the snapshot files found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lookup {
    Found,
    Missing,
    Corrupt,
}

/// What a second read of the snapshot files found for the name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Found {
    Name,
    NoName,
    Corrupt,
}

/// Why a run reads the snapshot files again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Reread {
    /// An index file was missing in two loads.
    Ghost,
    /// The check failed, and a listing of the index files found no new name.
    Check,
    /// A pack was missing.
    Pack,
}

/// What one step of a run of a restore observed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Observed {
    /// The first lookup of the name in a load.
    Loaded(Lookup),
    /// The config of the repository is gone.
    ConfigMissing,
    /// A listed index file was gone at its read.
    IndexFileMissing(Box<Path>),
    /// A pack was gone at its read, after the first write of the run when `written`.
    PackMissing {
        written: bool,
    },
    /// The check found a blob of the tree that the index does not hold.
    CheckFailed,
    /// A new listing of the index files gave these names.
    IndexListedAgain(HashSet<Box<Path>>),
    SnapshotsReadAgain {
        reason: Reread,
        found: Found,
    },
    /// Whether the blob that the check missed is only in packs that a prune marked for deletion.
    MarkedRead {
        only_marked: bool,
    },
    /// Whether a new load of the index lists the pack that was missing.
    PackIndexed(bool),
    /// A storage call failed after its tries, after the first write when `written`.
    CallFailed {
        written: bool,
    },
    /// A failure that no try can fix; `range` for a range error, which a damaged pack gives.
    Permanent {
        range: bool,
    },
    /// The directory could not take the tree.
    Destination,
    /// The token of the run was cancelled.
    Cancelled,
}

/// The next step of a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Check the index for the whole tree, then write it.
    Check,
    LoadAgain,
    ListIndexAgain,
    ReadSnapshotsAgain(Reread),
    ReadMarked,
    LoadIndexAgain,
    /// Remove what the run wrote, then end the run.
    ClearAndEnd(RunEnd),
    RunEnded(RunEnd),
    Answer(Outcome),
}

/// What a step adds to the index files that the run knows about. The run applies it before the
/// step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) enum Learned {
    #[default]
    Nothing,
    /// A load found this listed index file gone, the first time.
    Missed(Box<Path>),
    /// A new listing gave these index files, and some of them are new.
    Listed(HashSet<Box<Path>>),
}

/// The answer of a restore that a step gives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    NotFound,
    Corrupt,
    /// `Failed`: a pack of the snapshot is marked for deletion, and a later restore can succeed.
    MarkedOnly,
    Destination,
}

/// Gives the next step after `observed`, and what the run learns about its index files before it
/// takes the step. `listed` holds each index file that a listing of the run gave, and `missed`
/// each index file that a load of the run found gone. Each missing index file was listed, because
/// rustic reads only listed files, so a load runs again only for a file that was not missed
/// before, and the run then counts the file as missed; a file that is missed twice is a ghost. A
/// new listing that gives a file that the run did not list loads again, and the run then counts
/// the files of that listing as listed.
pub(super) fn next_step(
    observed: &Observed,
    listed: &HashSet<Box<Path>>,
    missed: &HashSet<Box<Path>>,
) -> (Step, Learned) {
    match observed {
        Observed::IndexFileMissing(path) if !missed.contains(path) => {
            (Step::LoadAgain, Learned::Missed(path.clone()))
        }
        Observed::IndexListedAgain(names) if names.iter().any(|name| !listed.contains(name)) => {
            (Step::LoadAgain, Learned::Listed(names.clone()))
        }
        observed => (step_after(observed), Learned::Nothing),
    }
}

/// Gives the next step after `observed`, for an observation that teaches the run nothing new
/// about its index files.
fn step_after(observed: &Observed) -> Step {
    match observed {
        Observed::Loaded(Lookup::Found) => Step::Check,
        Observed::Loaded(Lookup::Missing) | Observed::ConfigMissing => {
            Step::Answer(Outcome::NotFound)
        }
        Observed::Loaded(Lookup::Corrupt) => Step::Answer(Outcome::Corrupt),
        Observed::IndexFileMissing(_) => Step::ReadSnapshotsAgain(Reread::Ghost),
        Observed::CheckFailed => Step::ListIndexAgain,
        Observed::IndexListedAgain(_) => Step::ReadSnapshotsAgain(Reread::Check),
        Observed::PackMissing { .. } => Step::ReadSnapshotsAgain(Reread::Pack),
        Observed::SnapshotsReadAgain {
            found: Found::NoName,
            ..
        } => Step::Answer(Outcome::NotFound),
        Observed::SnapshotsReadAgain {
            found: Found::Corrupt,
            ..
        } => Step::Answer(Outcome::Corrupt),
        Observed::SnapshotsReadAgain {
            reason: Reread::Ghost,
            found: Found::Name,
        } => Step::RunEnded(RunEnd::CallFailed),
        Observed::SnapshotsReadAgain {
            reason: Reread::Check,
            found: Found::Name,
        } => Step::ReadMarked,
        Observed::SnapshotsReadAgain {
            reason: Reread::Pack,
            found: Found::Name,
        } => Step::LoadIndexAgain,
        Observed::MarkedRead { only_marked: true } => Step::Answer(Outcome::MarkedOnly),
        Observed::MarkedRead { only_marked: false } | Observed::PackIndexed(true) => {
            Step::Answer(Outcome::Corrupt)
        }
        Observed::PackIndexed(false) => Step::ClearAndEnd(RunEnd::RaceRunAgain),
        Observed::CallFailed { written: false } => Step::RunEnded(RunEnd::CallFailed),
        Observed::CallFailed { written: true } => Step::ClearAndEnd(RunEnd::CallFailed),
        Observed::Permanent { range: true } => Step::Answer(Outcome::Corrupt),
        Observed::Permanent { range: false } => Step::RunEnded(RunEnd::Permanent),
        Observed::Destination => Step::Answer(Outcome::Destination),
        Observed::Cancelled => Step::RunEnded(RunEnd::Cancelled),
    }
}

#[cfg(test)]
mod tests;
