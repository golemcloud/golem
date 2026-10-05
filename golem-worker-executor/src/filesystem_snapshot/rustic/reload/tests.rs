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

use super::super::runs::RunEnd;
use super::{Found, Learned, Lookup, Observed, Outcome, Reread, Step, next_step};
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use std::path::Path;
use test_r::test;

fn paths(names: &[&str]) -> HashSet<Box<Path>> {
    names.iter().map(|name| Path::new(name).into()).collect()
}

#[test]
fn each_observation_gives_its_step_and_what_the_run_learns() {
    let listed = paths(&["index/a", "index/b"]);
    let missed = paths(&["index/a"]);
    let step = |observed: Observed| next_step(&observed, &listed, &missed);
    let again = |reason, found| Observed::SnapshotsReadAgain { reason, found };

    assert_eq!(
        [
            step(Observed::Loaded(Lookup::Found)),
            step(Observed::Loaded(Lookup::Missing)),
            step(Observed::Loaded(Lookup::Corrupt)),
            step(Observed::ConfigMissing),
            step(Observed::IndexFileMissing(Path::new("index/b").into())),
            step(Observed::IndexFileMissing(Path::new("index/a").into())),
            step(Observed::CheckFailed),
            step(Observed::IndexListedAgain(paths(&["index/a", "index/c"]))),
            step(Observed::IndexListedAgain(paths(&["index/a", "index/b"]))),
            step(Observed::IndexListedAgain(paths(&[]))),
            step(Observed::PackMissing { written: false }),
            step(Observed::PackMissing { written: true }),
            step(again(Reread::Ghost, Found::NoName)),
            step(again(Reread::Check, Found::Corrupt)),
            step(again(Reread::Ghost, Found::Name)),
            step(again(Reread::Check, Found::Name)),
            step(again(Reread::Pack, Found::Name)),
            step(Observed::MarkedRead { only_marked: true }),
            step(Observed::MarkedRead { only_marked: false }),
            step(Observed::PackIndexed(false)),
            step(Observed::PackIndexed(true)),
            step(Observed::CallFailed { written: false }),
            step(Observed::CallFailed { written: true }),
            step(Observed::Permanent { range: true }),
            step(Observed::Permanent { range: false }),
            step(Observed::Destination),
            step(Observed::Cancelled),
        ],
        [
            (Step::Check, Learned::Nothing),
            (Step::Answer(Outcome::NotFound), Learned::Nothing),
            (Step::Answer(Outcome::Corrupt), Learned::Nothing),
            (Step::Answer(Outcome::NotFound), Learned::Nothing),
            (
                Step::LoadAgain,
                Learned::Missed(Path::new("index/b").into())
            ),
            (Step::ReadSnapshotsAgain(Reread::Ghost), Learned::Nothing),
            (Step::ListIndexAgain, Learned::Nothing),
            (
                Step::LoadAgain,
                Learned::Listed(paths(&["index/a", "index/c"]))
            ),
            (Step::ReadSnapshotsAgain(Reread::Check), Learned::Nothing),
            (Step::ReadSnapshotsAgain(Reread::Check), Learned::Nothing),
            (Step::ReadSnapshotsAgain(Reread::Pack), Learned::Nothing),
            (Step::ReadSnapshotsAgain(Reread::Pack), Learned::Nothing),
            (Step::Answer(Outcome::NotFound), Learned::Nothing),
            (Step::Answer(Outcome::Corrupt), Learned::Nothing),
            (Step::RunEnded(RunEnd::CallFailed), Learned::Nothing),
            (Step::ReadMarked, Learned::Nothing),
            (Step::LoadIndexAgain, Learned::Nothing),
            (Step::Answer(Outcome::MarkedOnly), Learned::Nothing),
            (Step::Answer(Outcome::Corrupt), Learned::Nothing),
            (Step::ClearAndEnd(RunEnd::RaceRunAgain), Learned::Nothing),
            (Step::Answer(Outcome::Corrupt), Learned::Nothing),
            (Step::RunEnded(RunEnd::CallFailed), Learned::Nothing),
            (Step::ClearAndEnd(RunEnd::CallFailed), Learned::Nothing),
            (Step::Answer(Outcome::Corrupt), Learned::Nothing),
            (Step::RunEnded(RunEnd::Permanent), Learned::Nothing),
            (Step::Answer(Outcome::Destination), Learned::Nothing),
            (Step::RunEnded(RunEnd::Cancelled), Learned::Nothing),
        ]
    );
}
