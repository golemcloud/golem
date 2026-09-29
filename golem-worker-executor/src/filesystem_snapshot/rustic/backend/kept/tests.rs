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

use super::{Admit, State, Want, admit, want};
use bytes::Bytes;
use pretty_assertions::assert_eq;
use rustic_core::Id;
use test_r::test;

fn id(digits: &str) -> Id {
    digits.repeat(32).parse().unwrap()
}

fn state(kept: &[(&str, usize)], reading: &[&str], closed: bool) -> State {
    State {
        packs: kept
            .iter()
            .map(|(digits, len)| (id(digits), Bytes::from(vec![0; *len])))
            .collect(),
        bytes: kept.iter().map(|(_, len)| len).sum(),
        closed,
        reading: reading.iter().map(|digits| id(digits)).collect(),
    }
}

#[test]
fn a_pack_that_another_thread_reads_is_waited_for_also_when_the_set_is_closed_or_full() {
    let reading = state(&[], &["ab"], false);
    let closed = state(&[], &["ab"], true);
    let full = state(&[("cd", 10)], &["ab"], false);

    assert_eq!(
        [
            want(&reading, &id("ab"), 10),
            want(&closed, &id("ab"), 10),
            want(&full, &id("ab"), 10),
        ],
        [Want::Wait, Want::Wait, Want::Wait]
    );
}

#[test]
fn a_kept_pack_is_given_also_when_the_set_is_closed_or_full() {
    let open = state(&[("ab", 4)], &["cd"], false);
    let closed = state(&[("ab", 4)], &[], true);
    let full = state(&[("ab", 10)], &[], false);
    let pack = Bytes::from(vec![0; 4]);
    let full_pack = Bytes::from(vec![0; 10]);

    assert_eq!(
        [
            want(&open, &id("ab"), 10),
            want(&closed, &id("ab"), 10),
            want(&full, &id("ab"), 10),
        ],
        [Want::Kept(&pack), Want::Kept(&pack), Want::Kept(&full_pack)]
    );
}

#[test]
fn a_pack_that_is_not_kept_is_read_only_while_the_set_is_open_and_below_its_limit() {
    let open = state(&[("cd", 9)], &["ef"], false);
    let closed = state(&[], &[], true);
    let full = state(&[("cd", 10)], &[], false);
    let over = state(&[("cd", 11)], &[], false);

    assert_eq!(
        [
            want(&open, &id("ab"), 10),
            want(&closed, &id("ab"), 10),
            want(&full, &id("ab"), 10),
            want(&over, &id("ab"), 10),
        ],
        [Want::Read, Want::Skip, Want::Skip, Want::Skip]
    );
}

#[test]
fn a_pack_is_kept_when_it_fits_the_limit_and_closes_the_set_when_it_does_not() {
    assert_eq!(
        [
            admit(false, 0, 10, 10),
            admit(false, 4, 5, 10),
            admit(false, 4, 7, 10),
            admit(false, 0, 11, 10),
            admit(false, usize::MAX, 1, usize::MAX),
            admit(false, 0, 0, 0),
        ],
        [
            Admit::Keep { bytes: 10 },
            Admit::Keep { bytes: 9 },
            Admit::Close,
            Admit::Close,
            Admit::Keep { bytes: usize::MAX },
            Admit::Keep { bytes: 0 },
        ]
    );
}

#[test]
fn a_closed_set_keeps_no_pack_whether_it_fits_or_not() {
    assert_eq!(
        [admit(true, 0, 60, 100), admit(true, 0, 200, 100)],
        [Admit::Close, Admit::Close]
    );
}
