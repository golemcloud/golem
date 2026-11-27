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

use super::*;
use test_r::test;

fn log_view(index: u64, message: &str) -> AgentOplogEntryView {
    AgentOplogEntryView {
        index,
        attribution: PublicOplogEntryAttribution::agent(),
        entry: AgentOplogEntryContent::Full(PublicOplogEntry::Log(
            golem_common::model::oplog::public_oplog_entry::LogParams {
                timestamp: timestamp(),
                level: golem_common::model::oplog::LogLevel::Info,
                context: String::new(),
                message: message.to_string(),
                trace_context: None,
            },
        )),
    }
}

fn every_entry(from: Option<u64>, to: Option<u64>) -> OplogExportSelection {
    OplogExportSelection::EveryEntry(OplogRange::from_indices(from, to).unwrap())
}

#[test]
fn an_oplog_export_refuses_a_repeated_or_earlier_entry_and_a_gap_in_every_entry() {
    use OplogExportSelection::Matches;
    const EVERY: OplogExportSelection = OplogExportSelection::EveryEntry(OplogRange::ALL);
    let from = |start| every_entry(Some(start), None);
    [
        ((EVERY, None, 1), true),
        ((EVERY, None, 7), true),
        ((from(7), None, 7), true),
        ((from(7), None, 8), false),
        ((from(7), None, 6), false),
        ((from(7), Some(7), 8), true),
        ((every_entry(None, Some(9)), None, 4), true),
        ((every_entry(Some(0), None), None, 4), true),
        ((EVERY, Some(3), 4), true),
        ((EVERY, Some(3), 3), false),
        ((EVERY, Some(3), 2), false),
        ((EVERY, Some(3), 5), false),
        ((EVERY, Some(u64::MAX - 1), u64::MAX), true),
        ((Matches, None, 7), true),
        ((Matches, Some(3), 4), true),
        ((Matches, Some(3), 9), true),
        ((Matches, Some(3), 3), false),
        ((Matches, Some(3), 2), false),
    ]
    .into_iter()
    .for_each(|((selection, previous, index), accepted)| {
        assert_eq!(
            check_oplog_export_order(
                selection,
                previous.map(OplogIndex::from_u64),
                OplogIndex::from_u64(index)
            )
            .is_ok(),
            accepted,
            "{selection:?}, entry {index} after {previous:?}"
        )
    });
}

#[test]
fn an_oplog_export_of_every_entry_ends_at_the_end_of_its_range() {
    use OplogExportSelection::Matches;
    const EVERY: OplogExportSelection = OplogExportSelection::EveryEntry(OplogRange::ALL);
    let from = |start| every_entry(Some(start), None);
    [
        ((EVERY, Some(9), 9), true),
        ((EVERY, Some(8), 9), false),
        ((EVERY, Some(10), 9), false),
        ((EVERY, None, 9), false),
        ((EVERY, None, 0), true),
        ((from(5), None, 4), true),
        ((from(5), None, 5), false),
        ((from(5), Some(5), 5), true),
        ((every_entry(Some(5), Some(9)), None, 4), true),
        ((every_entry(Some(5), Some(9)), None, 9), false),
        ((every_entry(None, Some(9)), None, 9), false),
        ((Matches, None, 9), true),
        ((Matches, Some(3), 9), true),
    ]
    .into_iter()
    .for_each(|((selection, last, range_last), accepted)| {
        assert_eq!(
            check_oplog_export_end(
                selection,
                last.map(OplogIndex::from_u64),
                OplogIndex::from_u64(range_last)
            )
            .is_ok(),
            accepted,
            "{selection:?}, last entry {last:?}, range ends at {range_last}"
        )
    });
}

#[test]
fn the_jsonl_writer_writes_one_agent_oplog_document_per_line_across_pages() {
    let mut writer = OplogJsonlWriter::new(
        Vec::new(),
        crate::model::masking::MaskingConfig::hide_secrets(),
        every_entry(Some(1), None),
    );
    // Two pages; the second, last page starts right after the first one.
    [vec![(1, "a"), (2, "b")], vec![(3, "c")]]
        .into_iter()
        .flatten()
        .try_for_each(|(index, message)| writer.write(log_view(index, message)))
        .unwrap();
    // The writer refuses a page that starts again at an entry that it wrote, or after a gap,
    // and writes nothing.
    assert!(writer.write(log_view(3, "c")).is_err());
    assert!(writer.write(log_view(2, "b")).is_err());
    assert!(writer.write(log_view(5, "e")).is_err());
    let (written, count) = writer.finish(OplogIndex::from_u64(3)).unwrap();
    assert_eq!(count, 3);
    let written = String::from_utf8(written).unwrap();

    assert!(written.ends_with('\n'));
    let documents = written
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        documents
            .iter()
            .map(|document| (
                document["$type"].as_str().unwrap(),
                document["index"].as_u64().unwrap(),
                document["entry"]["message"].as_str().unwrap()
            ))
            .collect::<Vec<_>>(),
        vec![
            ("agent.oplog", 1, "a"),
            ("agent.oplog", 2, "b"),
            ("agent.oplog", 3, "c")
        ]
    );
}

fn export_cursor(next: u64) -> OplogCursor {
    OplogCursor {
        next_oplog_index: next,
        current_component_revision: 0,
        query_hash: String::new(),
        scan_to: None,
    }
}

/// A page of an export read: the indices of its entries, the next index of its cursor, and
/// the `last_index` of the response.
fn export_page(entries: &[u64], next: Option<u64>, last_index: u64) -> OplogExportPage<u64> {
    OplogExportPage {
        entries: entries.to_vec(),
        next: next.map(export_cursor),
        last_index: OplogIndex::from_u64(last_index),
    }
}

/// The reads of an export: the range and the cursor of each read.
type ExportReads = std::sync::Mutex<Vec<(OplogRange, Option<u64>)>>;

/// Runs the export of `range` with `selection` over `pages`, which the reads get in order.
/// Gives the indices of the written lines, or the error, and the reads.
async fn export_pages(
    selection: OplogExportSelection,
    range: OplogRange,
    pages: Vec<OplogExportPage<u64>>,
    agent: anyhow::Result<()>,
) -> (anyhow::Result<Vec<u64>>, Vec<(OplogRange, Option<u64>)>) {
    let reads = ExportReads::default();
    let pages = std::sync::Mutex::new(pages.into_iter());
    let writer = OplogJsonlWriter::new(
        Vec::new(),
        crate::model::masking::MaskingConfig::hide_secrets(),
        selection,
    );
    let result = write_oplog_export(
        writer,
        range,
        |range, cursor: Option<OplogCursor>| {
            reads
                .lock()
                .unwrap()
                .push((range, cursor.map(|cursor| cursor.next_oplog_index)));
            let page = pages.lock().unwrap().next();
            std::future::ready(page.ok_or_else(|| anyhow::anyhow!("no more pages")))
        },
        |index| std::future::ready(Ok(log_view(index, "entry"))),
        || std::future::ready(agent),
    )
    .await
    .map(|(written, _)| {
        String::from_utf8(written)
            .unwrap()
            .lines()
            .map(|line| {
                serde_json::from_str::<serde_json::Value>(line).unwrap()["index"]
                    .as_u64()
                    .unwrap()
            })
            .collect()
    });
    (result, reads.into_inner().unwrap())
}

#[test]
async fn an_oplog_export_writes_every_page_and_follows_each_cursor() {
    let all = OplogRange::ALL;
    let closed = |from, to| OplogRange::from_indices(from, Some(to)).unwrap();
    let rows = [
        // A page boundary in the middle of the range, and a last page without a cursor.
        (
            (every_entry(None, None), all),
            vec![
                export_page(&[1, 2], Some(3), 5),
                export_page(&[3, 4], Some(5), 5),
                export_page(&[5], None, 5),
            ],
            (
                vec![1u64, 2, 3, 4, 5],
                vec![
                    (all, None),
                    (closed(None, 5), Some(3u64)),
                    (closed(None, 5), Some(5)),
                ],
            ),
        ),
        // A page that ends exactly at the end of the range.
        (
            (every_entry(Some(2), Some(5)), closed(Some(2), 5)),
            vec![
                export_page(&[2, 3], Some(4), 9),
                export_page(&[4, 5], None, 9),
            ],
            (
                vec![2, 3, 4, 5],
                vec![(closed(Some(2), 5), None), (closed(Some(2), 5), Some(4))],
            ),
        ),
        // A search page with no match and with a cursor, and a match on the last page.
        (
            (OplogExportSelection::Matches, OplogRange::ALL),
            vec![
                export_page(&[2], Some(4), 9),
                export_page(&[], Some(7), 9),
                export_page(&[8], None, 9),
            ],
            (
                vec![2, 8],
                vec![
                    (all, None),
                    (closed(None, 9), Some(4)),
                    (closed(None, 9), Some(7)),
                ],
            ),
        ),
    ];
    futures_util::future::join_all(rows.into_iter().map(
        |((selection, range), pages, (lines, reads))| async move {
            let (written, done) = export_pages(selection, range, pages, Ok(())).await;
            assert_eq!(written.unwrap(), lines, "{selection:?}");
            assert_eq!(done, reads, "{selection:?}");
        },
    ))
    .await;
}

#[test]
async fn an_oplog_export_of_a_search_follows_a_cursor_before_the_start_of_its_range() {
    let from = OplogRange::from_indices(Some(10), None).unwrap();
    let closed = OplogRange::from_indices(Some(10), Some(20)).unwrap();
    // The scan budget ended the first search before the start of the range. Its page is
    // empty, and its cursor continues the scan at index 3.
    let scan_page = OplogExportPage {
        entries: Vec::new(),
        next: Some(OplogCursor {
            scan_to: Some(10),
            ..export_cursor(3)
        }),
        last_index: OplogIndex::from_u64(20),
    };
    let (written, reads) = export_pages(
        OplogExportSelection::Matches,
        from,
        vec![
            scan_page,
            export_page(&[12], Some(15), 20),
            export_page(&[18], None, 20),
        ],
        Ok(()),
    )
    .await;
    assert_eq!(written.unwrap(), vec![12, 18]);
    assert_eq!(
        reads,
        vec![(from, None), (closed, Some(3)), (closed, Some(15))]
    );
}

#[test]
async fn an_oplog_export_of_a_growing_oplog_ends_at_the_end_of_the_oplog_at_its_first_page() {
    // The oplog ends at index 6 at the first read, and grows by 2 entries on each read. A page
    // holds at most 2 entries, so a read that follows the end of the oplog never ends. The
    // export reads 3 pages, so the second and the third page read the closed range.
    let reads = std::sync::atomic::AtomicU64::new(0);
    let writer = OplogJsonlWriter::new(
        Vec::new(),
        crate::model::masking::MaskingConfig::hide_secrets(),
        every_entry(None, None),
    );
    let (written, count) = write_oplog_export(
        writer,
        OplogRange::ALL,
        |range: OplogRange, cursor: Option<OplogCursor>| {
            let read = reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let oplog_last = 6 + 2 * read;
            let from = cursor.map_or(1, |cursor| cursor.next_oplog_index);
            let range_last = range
                .last_index_in(OplogIndex::from_u64(oplog_last))
                .as_u64();
            let page_last = (from + 1).min(range_last);
            let page = (read < 20)
                .then(|| {
                    export_page(
                        &(from..=page_last).collect::<Vec<_>>(),
                        (page_last < range_last).then_some(page_last + 1),
                        oplog_last,
                    )
                })
                .ok_or_else(|| anyhow::anyhow!("the export read 20 pages and did not end"));
            std::future::ready(page)
        },
        |index| std::future::ready(Ok(log_view(index, "entry"))),
        || std::future::ready(Ok(())),
    )
    .await
    .unwrap();
    assert_eq!(count, 6);
    assert_eq!(String::from_utf8(written).unwrap().lines().count(), 6);
}

#[test]
async fn an_oplog_export_does_not_shorten_the_end_reported_by_the_first_page() {
    let (written, _) = export_pages(
        every_entry(None, Some(6)),
        OplogRange::from_indices(None, Some(6)).unwrap(),
        vec![
            export_page(&[1, 2], Some(3), 9),
            export_page(&[3, 4], None, 4),
        ],
        Ok(()),
    )
    .await;
    assert!(
        written.is_err(),
        "the export did not reach the pinned end at 6"
    );
}

#[test]
async fn an_oplog_export_fails_when_the_agent_became_another_incarnation() {
    let (written, reads) = export_pages(
        every_entry(None, None),
        OplogRange::ALL,
        vec![export_page(&[1, 2], Some(3), 3), export_page(&[3], None, 3)],
        Err(anyhow::anyhow!("another incarnation")),
    )
    .await;
    assert_eq!(
        written.unwrap_err().to_string(),
        "another incarnation",
        "the export checks the agent after the last page"
    );
    assert_eq!(reads.len(), 2);
}

#[test]
fn the_jsonl_writer_refuses_to_finish_before_the_end_of_the_range() {
    let writer = || {
        OplogJsonlWriter::new(
            Vec::new(),
            crate::model::masking::MaskingConfig::hide_secrets(),
            every_entry(Some(1), None),
        )
    };
    // The writer wrote entries 1 and 2, but the range ends at 3.
    let mut short = writer();
    [(1, "a"), (2, "b")]
        .into_iter()
        .try_for_each(|(index, message)| short.write(log_view(index, message)))
        .unwrap();
    assert!(short.finish(OplogIndex::from_u64(3)).is_err());
    // The writer wrote no entry, but the range ends at 3.
    assert!(writer().finish(OplogIndex::from_u64(3)).is_err());
}
