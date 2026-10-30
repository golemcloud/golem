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

use super::super::timing::CLOCK_SKEW_MARGIN;
use super::*;
use crate::filesystem_snapshot::backend::rustic::test_support::files_of;
use crate::filesystem_snapshot::clock::SystemClock;
use golem_common::model::environment::EnvironmentId;
use golem_service_base::storage::blob::memory::InMemoryBlobStorage;
use golem_service_base::storage::blob::{BlobStorage, BlobStorageNamespace};
use pretty_assertions::assert_eq;
use std::sync::Arc;
use std::time::Duration;
use test_r::{test, timeout};
use uuid::Uuid;

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

const DEADLINE: Duration = Duration::from_secs(2);

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
