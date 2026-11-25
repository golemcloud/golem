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
use crate::oplog_export::model::{COVERAGE_LIST_CAP, ReportAgentRef, ReportNode};
use base64::Engine;
use golem_common::model::account::AccountId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::public_oplog_entry::{
    CreateParams, EndParams, LogParams, StartParams,
};
use golem_common::model::oplog::{
    HostRequest, HostRequestGolemRpcInvoke, LogLevel, PublicDurableFunctionType,
    PublicOplogEntryAttribution, host_functions,
};
use golem_common::model::{AgentId, IdempotencyKey, InvocationRequestKey, OplogIndex, Timestamp};
use golem_common::schema::{IntoTypedSchemaValue, SchemaValue, TypedSchemaValue};
use golem_schema::Empty;
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use test_r::test;

const MAX: u64 = 1000;
const REVISION: u64 = 3;

#[derive(Debug, Clone, PartialEq)]
enum Event {
    Page {
        from: Option<u64>,
        count: u64,
        cursor: Option<u64>,
        query: Option<String>,
    },
    Entry {
        index: u64,
        revision: u64,
    },
    RangePage {
        from: u64,
        to: u64,
        cursor: Option<u64>,
        allowance: Option<OplogScanAllowance>,
    },
    SinkHeader,
    SinkEntry(u64),
    SinkCoverage,
}

fn root_read() -> Event {
    Event::Page {
        from: Some(1),
        count: 1,
        cursor: None,
        query: None,
    }
}

type Log = Arc<Mutex<Vec<Event>>>;

struct FakeEntry {
    entry: PublicOplogEntryWithIndex,
    /// The size of the entry when the page omits it.
    omitted_size: Option<u64>,
    hydration_fails: bool,
}

/// An oplog in memory. Pages hold `count` entries from the cursor, `from` or index 1; a read
/// with a query returns only the entries whose JSON contains the query. The `Create` at index 1
/// gets the `n`-th instance ID at the `n`-th root read (`from = 1`, `count = 1`).
struct FakeReads {
    entries: Vec<FakeEntry>,
    log: Log,
    empty_page_at: Mutex<HashSet<u64>>,
    instance_ids: Vec<Uuid>,
    root_reads: AtomicUsize,
    fail_root_read_at: Option<usize>,
    /// The configured scan allowance of the fake server.
    scan: OplogScanAllowance,
    /// The time that each range read charges.
    millis_per_read: u64,
    /// The range read, counted from 0, that spends the whole allowance before it reads anything.
    exhaust_range_read_at: Option<usize>,
    range_reads: AtomicUsize,
}

impl FakeReads {
    fn new(entries: Vec<FakeEntry>) -> Self {
        Self {
            entries,
            log: Arc::default(),
            empty_page_at: Mutex::default(),
            instance_ids: vec![instance(1)],
            root_reads: AtomicUsize::new(0),
            fail_root_read_at: None,
            scan: OplogScanAllowance {
                entries: 1000,
                millis: 1000,
            },
            millis_per_read: 1,
            exhaust_range_read_at: None,
            range_reads: AtomicUsize::new(0),
        }
    }

    fn last_index(&self) -> u64 {
        self.entries
            .iter()
            .map(|e| e.entry.oplog_index.as_u64())
            .max()
            .unwrap_or(0)
    }

    fn page_entry(&self, fake: &FakeEntry, instance_id: Uuid) -> PublicOplogPageEntry {
        let mut entry = fake.entry.clone();
        if let PublicOplogEntry::Create(create) = &mut entry.entry {
            create.instance_id = instance_id;
        }
        match fake.omitted_size {
            None => PublicOplogPageEntry::Included(entry),
            Some(size) => PublicOplogPageEntry::Omitted(OmittedOplogEntry {
                oplog_index: entry.oplog_index,
                attribution: entry.attribution.clone(),
                timestamp: Timestamp::from(0),
                entry_type: entry_type(&entry.entry),
                size,
                component_revision: ComponentRevision::new(REVISION).unwrap(),
            }),
        }
    }
}

#[async_trait]
impl OplogReads for FakeReads {
    async fn read_page(
        &self,
        from: Option<u64>,
        count: u64,
        cursor: Option<&OplogCursor>,
        query: Option<&str>,
    ) -> anyhow::Result<OplogPage> {
        self.log.lock().unwrap().push(Event::Page {
            from,
            count,
            cursor: cursor.map(|c| c.next_oplog_index),
            query: query.map(str::to_string),
        });
        let mut instance_id = self.instance_ids[0];
        if from == Some(1) && count == 1 && cursor.is_none() && query.is_none() {
            let n = self.root_reads.fetch_add(1, Ordering::SeqCst);
            if self.fail_root_read_at == Some(n) {
                anyhow::bail!("agent not found");
            }
            instance_id = self.instance_ids[n.min(self.instance_ids.len() - 1)];
        }
        let start = cursor.map(|c| c.next_oplog_index).or(from).unwrap_or(1);
        if let Some(cursor) = cursor
            && self.empty_page_at.lock().unwrap().remove(&start)
        {
            return Ok(OplogPage {
                entries: vec![],
                next: Some(cursor.clone()),
                last_index: self.last_index(),
            });
        }
        let matching: Vec<&FakeEntry> = self
            .entries
            .iter()
            .filter(|e| e.entry.oplog_index.as_u64() >= start)
            .filter(|e| query.is_none_or(|q| serde_json::to_string(&e.entry).unwrap().contains(q)))
            .collect();
        let taken: Vec<&FakeEntry> = matching.iter().take(count as usize).copied().collect();
        let next = (matching.len() > taken.len()).then(|| OplogCursor {
            next_oplog_index: taken.last().unwrap().entry.oplog_index.as_u64() + 1,
            current_component_revision: REVISION,
            query_hash: OplogCursor::query_hash(query),
            scan_to: None,
        });
        Ok(OplogPage {
            entries: taken
                .into_iter()
                .map(|e| self.page_entry(e, instance_id))
                .collect(),
            next,
            last_index: self.last_index(),
        })
    }

    /// Accounts the scan as the server does: a read without a cursor charges the entries before
    /// `from`, which it reads to find the component revision. Every read charges the entries of its
    /// page and `millis_per_read`. A read without an allowance has no limit. The fake clamps an
    /// allowance to the configured one.
    async fn read_range_page(
        &self,
        from: u64,
        to: u64,
        count: u64,
        cursor: Option<&OplogCursor>,
        allowance: Option<OplogScanAllowance>,
    ) -> anyhow::Result<OplogRangePage> {
        self.log.lock().unwrap().push(Event::RangePage {
            from,
            to,
            cursor: cursor.map(|c| c.next_oplog_index),
            allowance,
        });
        let limit = allowance.map_or(
            OplogScanAllowance {
                entries: u64::MAX,
                millis: u64::MAX,
            },
            |a| OplogScanAllowance {
                entries: a.entries.min(self.scan.entries),
                millis: a.millis.min(self.scan.millis),
            },
        );
        let start = cursor.map_or(from, |c| c.next_oplog_index);
        let cursor_at = |index: u64| OplogCursor {
            next_oplog_index: index,
            current_component_revision: REVISION,
            query_hash: OplogCursor::query_hash(None),
            scan_to: None,
        };
        let scan = |charged_entries: u64, charged_millis: u64, exhausted: bool| OplogScan {
            limit,
            charged_entries,
            charged_millis,
            exhausted,
        };
        if self.range_reads.fetch_add(1, Ordering::SeqCst)
            == self.exhaust_range_read_at.unwrap_or(usize::MAX)
        {
            return Ok(OplogRangePage {
                entries: vec![],
                next: Some(cursor_at(start)),
                scan: scan(limit.entries, limit.millis, true),
            });
        }
        let mut charged = 0;
        if cursor.is_none() {
            charged = from - 1;
            if charged > limit.entries {
                return Ok(OplogRangePage {
                    entries: vec![],
                    next: None,
                    scan: scan(limit.entries, self.millis_per_read, true),
                });
            }
        }
        let end = to.min(self.last_index());
        let read = count.min(end + 1 - start).min(limit.entries - charged);
        charged += read;
        let entries = self
            .entries
            .iter()
            .filter(|e| (start..start + read).contains(&e.entry.oplog_index.as_u64()))
            .map(|e| self.page_entry(e, self.instance_ids[0]))
            .collect();
        let complete = start + read > end;
        let spent = charged >= limit.entries || self.millis_per_read >= limit.millis;
        Ok(OplogRangePage {
            entries,
            next: (!complete).then(|| cursor_at(start + read)),
            scan: scan(charged, self.millis_per_read, !complete && spent),
        })
    }

    async fn read_entry(
        &self,
        oplog_index: u64,
        component_revision: ComponentRevision,
    ) -> anyhow::Result<PublicOplogEntryWithIndex> {
        self.log.lock().unwrap().push(Event::Entry {
            index: oplog_index,
            revision: component_revision.get(),
        });
        let fake = self
            .entries
            .iter()
            .find(|e| e.entry.oplog_index.as_u64() == oplog_index)
            .ok_or_else(|| anyhow::anyhow!("no entry"))?;
        if fake.hydration_fails {
            anyhow::bail!("read failed");
        }
        Ok(fake.entry.clone())
    }
}

#[derive(Default)]
struct RecordingSink {
    log: Log,
    documents: Vec<serde_json::Value>,
    entries: Vec<ReportEntry>,
    header: Option<OplogExportHeader>,
    coverage: Option<OplogExportCoverage>,
}

impl RecordingSink {
    fn new(log: &Log) -> Self {
        Self {
            log: log.clone(),
            ..Default::default()
        }
    }

    fn text(&self) -> String {
        serde_json::to_string(&self.documents).unwrap()
    }
}

impl OplogExportSink for RecordingSink {
    fn header(&mut self, header: &OplogExportHeader) -> anyhow::Result<()> {
        self.log.lock().unwrap().push(Event::SinkHeader);
        self.documents.push(serde_json::to_value(header)?);
        self.header = Some(header.clone());
        Ok(())
    }

    fn entry(&mut self, entry: &ReportEntry) -> anyhow::Result<()> {
        self.log.lock().unwrap().push(Event::SinkEntry(entry.index));
        self.documents.push(serde_json::to_value(entry)?);
        self.entries.push(entry.clone());
        Ok(())
    }

    fn coverage(&mut self, coverage: &OplogExportCoverage) -> anyhow::Result<()> {
        self.log.lock().unwrap().push(Event::SinkCoverage);
        self.documents.push(serde_json::to_value(coverage)?);
        self.coverage = Some(coverage.clone());
        Ok(())
    }
}

fn key_from(bytes: [u8; 32]) -> &'static TokenKey {
    Box::leak(Box::new(TokenKey::from_bytes(bytes)))
}

fn key() -> &'static TokenKey {
    key_from([7; 32])
}

fn options() -> OplogExportOptions {
    OplogExportOptions {
        keep: KeepFlags::default(),
        max_entry_bytes: MAX,
        from: None,
        query: None,
        page_size: 2,
        cli_version: "test".to_string(),
        generated_at: "2026-01-01T00:00:00Z".to_string(),
    }
}

fn instance(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn entry_type(entry: &PublicOplogEntry) -> String {
    serde_json::to_value(entry).unwrap()["type"]
        .as_str()
        .unwrap()
        .to_string()
}

fn root_agent() -> AgentId {
    AgentId::from_parts(77, 0x7007)
}

fn full(index: u64, entry: PublicOplogEntry) -> FakeEntry {
    FakeEntry {
        entry: PublicOplogEntryWithIndex {
            oplog_index: OplogIndex::from_u64(index),
            attribution: PublicOplogEntryAttribution::agent(),
            entry,
        },
        omitted_size: None,
        hydration_fails: false,
    }
}

fn omitted(mut fake: FakeEntry, size: u64) -> FakeEntry {
    fake.omitted_size = Some(size);
    fake
}

fn create() -> FakeEntry {
    full(
        1,
        PublicOplogEntry::Create(CreateParams {
            timestamp: Timestamp::from(0),
            agent_id: root_agent(),
            agent_type_name: Some(golem_common::model::agent::AgentTypeName(
                "Counter".to_string(),
            )),
            owner_kind: Default::default(),
            agent_mode: golem_common::model::agent::AgentMode::Durable,
            component_revision: ComponentRevision::new(REVISION).unwrap(),
            env: Default::default(),
            created_by: AccountId(instance(5)),
            environment_id: EnvironmentId(instance(6)),
            parent: None,
            component_size: 10,
            initial_total_linear_memory_size: 20,
            initial_active_plugins: Default::default(),
            local_agent_config: vec![],
            instance_id: instance(1),
            never_live: false,
            standalone_tool: None,
        }),
    )
}

fn log_entry(index: u64, message: &str) -> FakeEntry {
    full(
        index,
        PublicOplogEntry::Log(LogParams {
            timestamp: Timestamp::from(0),
            level: LogLevel::Stdout,
            context: String::new(),
            message: message.to_string(),
            trace_context: None,
        }),
    )
}

fn start_entry(index: u64, function: &str, request: Option<TypedSchemaValue>) -> FakeEntry {
    full(
        index,
        PublicOplogEntry::Start(StartParams {
            timestamp: Timestamp::from(0),
            parent_start_index: None,
            function_name: function.to_string(),
            invocation_id: None,
            observational_owner: None,
            driver: None,
            request,
            durable_function_type: PublicDurableFunctionType::WriteRemote(Empty {}),
            span_started: None,
            denial: None,
        }),
    )
}

fn end_entry(index: u64, start_index: u64, response: TypedSchemaValue) -> FakeEntry {
    full(
        index,
        PublicOplogEntry::End(EndParams {
            timestamp: Timestamp::from(0),
            start_index: OplogIndex::from_u64(start_index),
            response: Some(response),
            forced_commit: false,
            span_finished: None,
            span_attributes: None,
        }),
    )
}

fn string_value(text: &str) -> TypedSchemaValue {
    text.to_string().into_typed_schema_value().unwrap()
}

fn rpc_invoke_start(index: u64) -> FakeEntry {
    let request = HostRequest::GolemRpcInvoke(HostRequestGolemRpcInvoke {
        remote_agent_id: root_agent(),
        request_key: InvocationRequestKey::Internal(IdempotencyKey::new("key".into())),
        method_name: "do-it".into(),
        input: SchemaValue::String("value".into()),
        logical_streaming_origin: None,
        remote_agent_type: None,
        remote_agent_parameters: None,
        scope_card: None,
    });
    start_entry(
        index,
        <host_functions::GolemRpcWasmRpcInvokeAndAwaitResult as golem_common::model::oplog::HostPayloadPair>::FQFN,
        Some(request.into_typed_schema_value().unwrap()),
    )
}

async fn run(
    reads: FakeReads,
    options: OplogExportOptions,
) -> (anyhow::Result<()>, RecordingSink, Log) {
    let log = reads.log.clone();
    let mut sink = RecordingSink::new(&log);
    let result = export_oplog(reads, key(), options, &mut sink).await;
    (result, sink, log)
}

fn events(log: &Log) -> Vec<Event> {
    log.lock().unwrap().clone()
}

fn page_reads(log: &Log) -> Vec<Event> {
    events(log)
        .into_iter()
        .filter(|e| matches!(e, Event::Page { .. }))
        .collect()
}

fn entry_reads(log: &Log) -> Vec<Event> {
    events(log)
        .into_iter()
        .filter(|e| matches!(e, Event::Entry { .. }))
        .collect()
}

fn export_error(result: anyhow::Result<()>) -> OplogExportError {
    result
        .unwrap_err()
        .downcast::<OplogExportError>()
        .expect("an OplogExportError")
}

#[test]
async fn export_follows_the_cursor_to_the_end() {
    let reads = FakeReads::new(vec![
        create(),
        log_entry(2, "a"),
        log_entry(3, "b"),
        log_entry(4, "c"),
    ]);
    reads.empty_page_at.lock().unwrap().insert(3);
    let (result, sink, log) = run(reads, options()).await;
    result.unwrap();

    let sink_events: Vec<Event> = events(&log)
        .into_iter()
        .filter(|e| !matches!(e, Event::Page { .. } | Event::Entry { .. }))
        .collect();
    assert_eq!(
        sink_events,
        vec![
            Event::SinkHeader,
            Event::SinkEntry(1),
            Event::SinkEntry(2),
            Event::SinkEntry(3),
            Event::SinkEntry(4),
            Event::SinkCoverage,
        ]
    );
    let cursor_reads: Vec<Option<u64>> = page_reads(&log)
        .into_iter()
        .filter_map(|e| match e {
            Event::Page {
                count: 2, cursor, ..
            } => Some(cursor),
            _ => None,
        })
        .collect();
    assert_eq!(cursor_reads, vec![None, Some(3), Some(3)]);
    assert!(!sink.coverage.unwrap().incomplete);
}

#[test]
async fn entries_are_written_before_the_next_read() {
    let reads = FakeReads::new(vec![create(), log_entry(2, "a"), log_entry(3, "b")]);
    let (result, _, log) = run(reads, options()).await;
    result.unwrap();
    let events = events(&log);
    let second_page = events
        .iter()
        .position(|e| {
            matches!(
                e,
                Event::Page {
                    cursor: Some(3),
                    ..
                }
            )
        })
        .unwrap();
    let written = |index| {
        events
            .iter()
            .position(|e| *e == Event::SinkEntry(index))
            .unwrap()
    };
    assert!(written(1) < second_page);
    assert!(written(2) < second_page);
    assert!(written(3) > second_page);
}

#[test]
async fn export_pins_and_checks_the_root() {
    let reads = FakeReads::new(vec![create(), log_entry(2, "a"), log_entry(3, "b")]);
    let (result, _, log) = run(reads, options()).await;
    result.unwrap();
    let pages = page_reads(&log);
    assert_eq!(pages.first(), Some(&root_read()));
    assert_eq!(pages.last(), Some(&root_read()));
    assert!(events(&log).first() == Some(&root_read()));
}

#[test]
async fn root_recreated_between_pages_fails() {
    let mut reads = FakeReads::new(vec![create(), log_entry(2, "a"), log_entry(3, "b")]);
    reads.instance_ids = vec![instance(1), instance(2)];
    let (result, sink, _) = run(reads, options()).await;
    assert_eq!(export_error(result), OplogExportError::RootChanged);
    assert!(sink.header.is_some());
    assert!(sink.coverage.is_none());
}

#[test]
async fn root_deleted_during_the_export_fails() {
    let mut reads = FakeReads::new(vec![create(), log_entry(2, "a")]);
    reads.fail_root_read_at = Some(1);
    let (result, sink, _) = run(reads, options()).await;
    assert!(result.unwrap_err().to_string().contains("agent not found"));
    assert!(sink.coverage.is_none());
}

#[test]
async fn create_above_the_limit_fails_without_a_full_read() {
    let reads = FakeReads::new(vec![omitted(create(), MAX + 1), log_entry(2, "a")]);
    let (result, sink, log) = run(reads, options()).await;
    assert_eq!(
        export_error(result),
        OplogExportError::CreateTooLarge {
            size: MAX + 1,
            max_entry_bytes: MAX
        }
    );
    assert!(entry_reads(&log).is_empty());
    assert!(sink.header.is_none());
}

#[test]
async fn missing_create_fails() {
    let reads = FakeReads::new(vec![log_entry(1, "a")]);
    let (result, _, _) = run(reads, options()).await;
    assert_eq!(export_error(result), OplogExportError::CreateMissing);
}

#[test]
async fn omitted_entry_at_the_limit_is_hydrated() {
    let reads = FakeReads::new(vec![
        create(),
        omitted(log_entry(2, "a"), MAX),
        log_entry(3, "b"),
    ]);
    let (result, sink, log) = run(reads, options()).await;
    result.unwrap();
    assert_eq!(
        entry_reads(&log),
        vec![Event::Entry {
            index: 2,
            revision: REVISION
        }]
    );
    let indices: Vec<u64> = sink.entries.iter().map(|e| e.index).collect();
    assert_eq!(indices, vec![1, 2, 3]);
    assert!(sink.entries[1].body.is_some());
    assert_eq!(sink.entries[1].kind, "Log");
    assert_eq!(sink.coverage.unwrap().hydration_failures.discovered, 0);
}

#[test]
async fn omitted_entry_above_the_limit_is_never_read() {
    let reads = FakeReads::new(vec![create(), omitted(log_entry(2, "a"), MAX + 1)]);
    let (result, sink, log) = run(reads, options()).await;
    result.unwrap();
    assert!(entry_reads(&log).is_empty());
    let omission = sink.entries[1].omitted.as_ref().unwrap();
    assert_eq!(omission.reason, OmittedReason::TooLarge);
    assert_eq!(omission.size, MAX + 1);
    assert!(sink.entries[1].body.is_none());
    let coverage = sink.coverage.unwrap();
    assert_eq!(
        coverage.hydration_failures.shown,
        vec![OmittedEntryRef {
            index: 2,
            kind: "Log".to_string(),
            size: MAX + 1,
            reason: OmittedReason::TooLarge
        }]
    );
    assert!(coverage.incomplete);
}

#[test]
async fn failed_hydration_stays_omitted() {
    let mut failing = omitted(log_entry(2, "a"), 10);
    failing.hydration_fails = true;
    let reads = FakeReads::new(vec![create(), failing, log_entry(3, "b")]);
    let (result, sink, _) = run(reads, options()).await;
    result.unwrap();
    let indices: Vec<u64> = sink.entries.iter().map(|e| e.index).collect();
    assert_eq!(indices, vec![1, 2, 3]);
    let omission = sink.entries[1].omitted.as_ref().unwrap();
    assert_eq!(omission.reason, OmittedReason::HydrationFailed);
    assert_eq!(omission.size, 10);
    let coverage = sink.coverage.unwrap();
    assert_eq!(coverage.hydration_failures.discovered, 1);
    assert_eq!(
        coverage.hydration_failures.shown[0].reason,
        OmittedReason::HydrationFailed
    );
    assert!(coverage.incomplete);
}

#[test]
async fn start_function_comes_from_the_map() {
    let reads = FakeReads::new(vec![
        create(),
        start_entry(2, "custom:pkg/iface.{fn}", None),
        end_entry(3, 2, string_value("x")),
    ]);
    let (result, sink, log) = run(reads, options()).await;
    result.unwrap();
    assert!(!page_reads(&log).iter().any(|e| matches!(
        e,
        Event::Page {
            from: Some(2),
            count: 1,
            ..
        }
    )));
    assert_eq!(sink.coverage.unwrap().start_unknown.discovered, 0);
}

#[test]
async fn start_outside_the_read_is_read_once() {
    let reads = FakeReads::new(vec![
        create(),
        omitted(start_entry(2, "custom:pkg/iface.{fn}", None), 10),
        omitted(start_entry(3, "custom:pkg/iface.{other}", None), MAX + 1),
        end_entry(4, 2, string_value("x")),
        end_entry(5, 3, string_value("y")),
    ]);
    let mut options = options();
    options.from = Some(4);
    options.page_size = 10;
    let (result, sink, log) = run(reads, options).await;
    result.unwrap();

    let lookups = |index| {
        page_reads(&log)
            .into_iter()
            .filter(|e| {
                *e == Event::Page {
                    from: Some(index),
                    count: 1,
                    cursor: None,
                    query: None,
                }
            })
            .count()
    };
    assert_eq!(lookups(2), 1);
    assert_eq!(lookups(3), 1);
    assert_eq!(
        entry_reads(&log),
        vec![Event::Entry {
            index: 2,
            revision: REVISION
        }]
    );
    let coverage = sink.coverage.unwrap();
    assert_eq!(
        coverage.start_unknown.shown,
        vec![StartUnknownRef {
            index: 5,
            start_index: 3
        }]
    );
    assert!(coverage.incomplete);
}

#[test]
async fn start_lookup_with_a_query_sends_no_query() {
    let reads = FakeReads::new(vec![
        create(),
        start_entry(2, "custom:pkg/iface.{fn}", None),
        end_entry(3, 2, string_value("MARKER-Q")),
    ]);
    let mut options = options();
    options.query = Some("MARKER-Q".to_string());
    let (result, sink, log) = run(reads, options).await;
    result.unwrap();
    assert_eq!(sink.entries.len(), 1);
    assert!(page_reads(&log).contains(&Event::Page {
        from: Some(2),
        count: 1,
        cursor: None,
        query: None,
    }));
    for event in page_reads(&log) {
        if let Event::Page {
            query: Some(_),
            from,
            ..
        } = event
        {
            assert_eq!(from, None);
        }
    }
    assert_eq!(sink.coverage.unwrap().start_unknown.discovered, 0);
}

#[test]
async fn a_query_from_an_index_exports_only_the_later_matches() {
    let reads = FakeReads::new(vec![
        create(),
        log_entry(2, "MARKER-R"),
        log_entry(3, "other"),
        log_entry(4, "MARKER-R"),
        log_entry(5, "MARKER-R"),
    ]);
    let mut options = options();
    options.from = Some(3);
    options.query = Some("MARKER-R".to_string());
    options.page_size = 1;
    let (result, sink, log) = run(reads, options).await;
    result.unwrap();
    let indices: Vec<u64> = sink.entries.iter().map(|e| e.index).collect();
    assert_eq!(indices, vec![4, 5]);
    assert!(page_reads(&log).iter().any(|event| matches!(
        event,
        Event::Page {
            from: Some(3),
            query: Some(_),
            ..
        }
    )));
}

#[test]
async fn coverage_lists_are_capped() {
    let n = 150u64;
    let mut entries = vec![
        create(),
        omitted(start_entry(2, "custom:pkg/iface.{fn}", None), MAX + 1),
    ];
    let mut index = 3;
    for _ in 0..n {
        entries.push(omitted(log_entry(index, "a"), MAX + 1));
        entries.push(end_entry(index + 1, 2, string_value("x")));
        entries.push(rpc_invoke_start(index + 2));
        index += 3;
    }
    let reads = FakeReads::new(entries);
    let mut options = options();
    options.from = Some(3);
    options.page_size = 7;
    let (result, sink, _) = run(reads, options).await;
    result.unwrap();
    let coverage = sink.coverage.unwrap();
    assert_eq!(coverage.hydration_failures.discovered, n);
    assert_eq!(coverage.hydration_failures.shown.len(), COVERAGE_LIST_CAP);
    assert_eq!(coverage.start_unknown.discovered, n);
    assert_eq!(coverage.start_unknown.shown.len(), COVERAGE_LIST_CAP);
    assert_eq!(coverage.schema_less_rpc_values.discovered, n);
    assert_eq!(
        coverage.schema_less_rpc_values.shown.len(),
        COVERAGE_LIST_CAP
    );
    assert_eq!(coverage.hydration_failures.shown[0].index, 3);
    assert!(coverage.incomplete);
}

#[test]
async fn query_sets_filtered_and_never_enters_the_output() {
    let reads = FakeReads::new(vec![
        create(),
        log_entry(2, "MARKER-Q one"),
        log_entry(3, "other"),
        log_entry(4, "MARKER-Q two"),
    ]);
    let mut options = options();
    options.query = Some("MARKER-Q".to_string());
    let (result, sink, _) = run(reads, options).await;
    result.unwrap();
    assert!(sink.header.as_ref().unwrap().filtered);
    let indices: Vec<u64> = sink.entries.iter().map(|e| e.index).collect();
    assert_eq!(indices, vec![2, 4]);
    assert!(!sink.text().contains("MARKER-Q"));

    let reads = FakeReads::new(vec![create(), log_entry(2, "a")]);
    let (result, sink, _) = run(reads, self::options()).await;
    result.unwrap();
    assert!(!sink.header.unwrap().filtered);
}

#[test]
async fn keep_flags_reach_the_context_and_the_header() {
    let scalar = 7391204117u64.into_typed_schema_value().unwrap();
    for (flags, note) in [
        (KeepFlags::default(), false),
        (
            KeepFlags {
                scalars: true,
                ..KeepFlags::default()
            },
            true,
        ),
        (
            KeepFlags {
                shape: true,
                ..KeepFlags::default()
            },
            false,
        ),
        (
            KeepFlags {
                error_text: true,
                ..KeepFlags::default()
            },
            false,
        ),
        (
            KeepFlags {
                scalars: true,
                shape: true,
                error_text: false,
            },
            false,
        ),
    ] {
        let reads = FakeReads::new(vec![
            create(),
            start_entry(2, "custom:pkg/iface.{fn}", Some(scalar.clone())),
        ]);
        let mut options = options();
        options.keep = flags;
        let (result, sink, _) = run(reads, options).await;
        result.unwrap();
        let header = sink.header.as_ref().unwrap();
        assert!(header.obfuscate);
        assert_eq!(header.keep_scalars, flags.scalars);
        assert_eq!(header.keep_shape, flags.shape);
        assert_eq!(header.keep_error_text, flags.error_text);
        let entry = serde_json::to_string(&sink.entries[1]).unwrap();
        assert_eq!(entry.contains("7391204117"), flags.scalars, "{entry}");
        let coverage = sink.coverage.as_ref().unwrap();
        assert_eq!(!coverage.notes.is_empty(), note, "{flags:?}");
        if note {
            assert_eq!(coverage.notes, vec![KEEP_SCALARS_WITHOUT_SHAPE_NOTE]);
        }
    }
}

#[test]
async fn key_bytes_never_reach_an_output() {
    let mut bytes = [0u8; 32];
    for (i, b) in bytes.iter_mut().enumerate() {
        *b = 0xa0 + i as u8;
    }
    let reads = FakeReads::new(vec![
        create(),
        log_entry(2, "a"),
        start_entry(3, "custom:pkg/iface.{fn}", Some(string_value("x"))),
    ]);
    let log = reads.log.clone();
    let mut sink = RecordingSink::new(&log);
    let mut options = options();
    options.keep = KeepFlags {
        scalars: true,
        shape: true,
        error_text: true,
    };
    export_oplog(reads, key_from(bytes), options, &mut sink)
        .await
        .unwrap();
    let text = sink.text();
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let decimal = bytes
        .iter()
        .map(|b| b.to_string())
        .collect::<Vec<_>>()
        .join(",");
    for needle in [
        hex.clone(),
        hex.to_uppercase(),
        base64::engine::general_purpose::STANDARD.encode(bytes),
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes),
        decimal,
    ] {
        assert!(!text.contains(&needle[..16]), "{needle} in {text}");
    }
}

fn without_tokens(text: &str) -> String {
    let re = regex::Regex::new(r#""[a-z:]+:[0-9a-f]{32}""#).unwrap();
    re.replace_all(text, "\"T\"").to_string()
}

fn sample_oplog() -> Vec<FakeEntry> {
    vec![
        create(),
        log_entry(2, "hello"),
        start_entry(3, "custom:pkg/iface.{fn}", Some(string_value("x"))),
        end_entry(4, 3, string_value("y")),
    ]
}

#[test]
async fn two_keys_give_the_same_structure_and_other_tokens() {
    let mut texts = vec![];
    for bytes in [[1u8; 32], [2u8; 32]] {
        let reads = FakeReads::new(sample_oplog());
        let log = reads.log.clone();
        let mut sink = RecordingSink::new(&log);
        export_oplog(reads, key_from(bytes), options(), &mut sink)
            .await
            .unwrap();

        let root = sink.header.as_ref().unwrap().root.clone();
        let Some(ReportNode::Record(body)) = &sink.entries[0].body else {
            panic!("the Create body is a record");
        };
        let agent = body
            .fields
            .iter()
            .find(|f| f.name.as_deref() == Some("agentId"))
            .map(|f| &f.value);
        assert_eq!(agent, Some(&ReportNode::Agent(root.clone())));
        assert_eq!(
            root,
            ReportAgentRef {
                agent_id: root_agent(),
                agent_type: Some(golem_common::model::agent::AgentTypeName(
                    "Counter".to_string()
                )),
            }
        );
        texts.push(sink.text());
    }
    assert_ne!(texts[0], texts[1]);
    assert_eq!(without_tokens(&texts[0]), without_tokens(&texts[1]));
}

#[test]
async fn no_environment_or_component_id_in_documents() {
    for query in [None, Some("hello")] {
        let reads = FakeReads::new(sample_oplog());
        let log = reads.log.clone();
        let mut sink = RecordingSink::new(&log);
        let options = OplogExportOptions {
            query: query.map(str::to_string),
            ..options()
        };
        export_oplog(reads, key_from([3; 32]), options, &mut sink)
            .await
            .unwrap();
        assert!(sink.coverage.is_some());
        let text = sink.text();
        let root = root_agent().0.hyphenated().to_string();
        assert_eq!(
            sink.header.as_ref().unwrap().root.agent_id.to_string(),
            root
        );
        assert!(text.contains(&root), "{text}");
        // The environment ID and the account ID of the `Create` entry.
        for id in [instance(6), instance(5)] {
            assert!(!text.contains(&id.hyphenated().to_string()), "{text}");
            assert!(!text.contains(&id.simple().to_string()), "{text}");
        }
        assert!(!text.contains("hello"), "{text}");
        assert!(!text.contains("http"), "{text}");
    }
}

#[test]
async fn session_pages_check_the_root_before_and_after() {
    let mut reads = FakeReads::new(vec![
        create(),
        start_entry(2, "custom:pkg/iface.{fn}", None),
        end_entry(3, 2, string_value("x")),
    ]);
    // Root reads: the pin, two checks around each of two pages, then a recreated agent.
    reads.instance_ids = vec![instance(1); 5];
    reads.instance_ids.push(instance(2));
    let log = reads.log.clone();
    let mut session = OplogExportSession::pin(reads, key(), options())
        .await
        .unwrap();

    session.check_root().await.unwrap();
    let first = session.render_page(Some(2), None, 1, None).await.unwrap();
    session.check_root().await.unwrap();
    assert_eq!(first.entries.len(), 1);
    assert!(first.next.is_some());

    session.check_root().await.unwrap();
    let second = session
        .render_page(None, first.next.as_ref(), 1, None)
        .await
        .unwrap();
    session.check_root().await.unwrap();
    assert_eq!(second.entries[0].index, 3);
    assert_eq!(second.coverage.start_unknown.discovered, 0);
    // The first page itself is the only read at index 2; the End reuses its Start.
    let reads_at_start = page_reads(&log)
        .into_iter()
        .filter(|e| matches!(e, Event::Page { from: Some(2), .. }))
        .count();
    assert_eq!(reads_at_start, 1);

    let error = session.check_root().await.unwrap_err();
    assert_eq!(
        error.downcast::<OplogExportError>().unwrap(),
        OplogExportError::RootChanged
    );
}

fn range_oplog() -> Vec<FakeEntry> {
    vec![
        create(),
        log_entry(2, "SAME-MARKER"),
        log_entry(3, "OTHER-MARKER"),
        log_entry(4, "SAME-MARKER"),
        log_entry(5, "THIRD-MARKER"),
        log_entry(6, "SAME-MARKER"),
        log_entry(7, "FOURTH-MARKER"),
        log_entry(8, "FIFTH-MARKER"),
        log_entry(9, "SIXTH-MARKER"),
    ]
}

async fn run_range(
    reads: FakeReads,
    from: u64,
    to: u64,
    limit_bytes: usize,
) -> (anyhow::Result<OplogExportReport>, Log) {
    let log = reads.log.clone();
    let result = export_oplog_range(
        reads,
        key(),
        options(),
        OplogRange { from, to },
        limit_bytes,
        document_bytes,
    )
    .await;
    (result, log)
}

/// The size of the document of `report` in these tests: its JSON and a newline.
fn document_bytes(report: &OplogExportReport) -> anyhow::Result<usize> {
    Ok(serde_json::to_vec(report)?.len() + 1)
}

fn range_reads(log: &Log) -> Vec<Event> {
    events(log)
        .into_iter()
        .filter(|e| matches!(e, Event::RangePage { .. }))
        .collect()
}

fn range_error(result: anyhow::Result<OplogExportReport>) -> OplogExportError {
    result
        .unwrap_err()
        .downcast::<OplogExportError>()
        .expect("an OplogExportError")
}

#[test]
async fn a_range_report_holds_exactly_the_range_in_index_order() {
    let (result, log) = run_range(FakeReads::new(range_oplog()), 3, 6, usize::MAX).await;
    let report = result.unwrap();

    let indexes = report.entries.iter().map(|e| e.index).collect::<Vec<_>>();
    assert_eq!(indexes, vec![3, 4, 5, 6]);
    assert_eq!((report.header.from, report.header.to), (3, 6));
    assert!(!report.header.export.filtered);
    assert!(report.header.export.obfuscate);
    assert!(!report.coverage.incomplete);

    // Equal customer values have equal tokens inside the report, and no value is in clear.
    let body = |index: u64| {
        let entry = report.entries.iter().find(|e| e.index == index).unwrap();
        serde_json::to_string(&entry.body).unwrap()
    };
    assert_eq!(body(4), body(6));
    assert_ne!(body(3), body(4));
    let text = serde_json::to_string(&report).unwrap();
    assert!(!text.contains("MARKER"), "{text}");

    // Two pages of two entries, both bounded by `to`, between the two root reads.
    assert_eq!(
        range_reads(&log)
            .iter()
            .map(|e| match e {
                Event::RangePage { to, cursor, .. } => (*to, *cursor),
                _ => unreachable!(),
            })
            .collect::<Vec<_>>(),
        vec![(6, None), (6, Some(5))]
    );
    let pages = page_reads(&log);
    assert_eq!(pages, vec![root_read(), root_read()]);
}

#[test]
async fn a_range_needs_valid_bounds_within_the_oplog() {
    for (from, to) in [(0, 3), (5, 4)] {
        let (result, log) = run_range(FakeReads::new(range_oplog()), from, to, usize::MAX).await;
        assert_eq!(
            range_error(result),
            OplogExportError::RangeInvalid { from, to }
        );
        assert!(events(&log).is_empty());
    }
    let (result, log) = run_range(FakeReads::new(range_oplog()), 2, 10, usize::MAX).await;
    assert_eq!(
        range_error(result),
        OplogExportError::RangePastEnd {
            to: 10,
            last_index: 9
        }
    );
    assert!(range_reads(&log).is_empty());
    // The last index itself is in the oplog.
    let (result, _) = run_range(FakeReads::new(range_oplog()), 9, 9, usize::MAX).await;
    assert_eq!(result.unwrap().entries.len(), 1);
}

#[test]
async fn small_pages_share_one_allowance_and_completion_at_the_allowance_succeeds() {
    // From 2, the server reads entry 1 for the revision, then the 8 entries of the range: 9.
    let mut reads = FakeReads::new(range_oplog());
    reads.scan = OplogScanAllowance {
        entries: 9,
        millis: 1000,
    };
    reads.millis_per_read = 10;
    let (result, log) = run_range(reads, 2, 9, usize::MAX).await;
    assert_eq!(result.unwrap().entries.len(), 8);
    let allowances = range_reads(&log)
        .into_iter()
        .map(|e| match e {
            Event::RangePage { allowance, .. } => allowance.map(|a| (a.entries, a.millis)),
            _ => unreachable!(),
        })
        .collect::<Vec<_>>();
    assert_eq!(
        allowances,
        vec![
            Some((u64::MAX, u64::MAX)),
            Some((6, 990)),
            Some((4, 980)),
            Some((2, 970))
        ]
    );
}

#[test]
async fn a_range_that_the_entry_allowance_does_not_cover_fails_without_a_report() {
    let mut reads = FakeReads::new(range_oplog());
    reads.scan = OplogScanAllowance {
        entries: 8,
        millis: 1000,
    };
    let (result, log) = run_range(reads, 2, 9, usize::MAX).await;
    assert_eq!(
        range_error(result),
        OplogExportError::ScanBudgetExceeded { from: 2, to: 9 }
    );
    // No root check after the failure: the export stopped at the spent allowance.
    assert_eq!(page_reads(&log), vec![root_read()]);
    assert_eq!(range_reads(&log).len(), 4);
}

#[test]
async fn a_range_that_the_time_allowance_does_not_cover_fails() {
    let mut reads = FakeReads::new(range_oplog());
    reads.scan = OplogScanAllowance {
        entries: 1000,
        millis: 25,
    };
    reads.millis_per_read = 10;
    let (result, log) = run_range(reads, 2, 9, usize::MAX).await;
    assert_eq!(
        range_error(result),
        OplogExportError::ScanBudgetExceeded { from: 2, to: 9 }
    );
    assert_eq!(range_reads(&log).len(), 3);
}

#[test]
async fn an_empty_page_that_spent_the_allowance_fails() {
    for at in [0, 1] {
        let mut reads = FakeReads::new(range_oplog());
        reads.exhaust_range_read_at = Some(at);
        let (result, _) = run_range(reads, 2, 9, usize::MAX).await;
        assert_eq!(
            range_error(result),
            OplogExportError::ScanBudgetExceeded { from: 2, to: 9 },
            "{at}"
        );
    }
    // The allowance runs out before the server finds the revision at `from`.
    let mut reads = FakeReads::new(range_oplog());
    reads.scan = OplogScanAllowance {
        entries: 5,
        millis: 1000,
    };
    let (result, log) = run_range(reads, 8, 9, usize::MAX).await;
    assert_eq!(
        range_error(result),
        OplogExportError::ScanBudgetExceeded { from: 8, to: 9 }
    );
    assert_eq!(range_reads(&log).len(), 1);
}

#[test]
async fn a_report_that_does_not_fit_stops_reading() {
    // The complete report of 2 to 3, which one page reads, fits exactly in its own size.
    let (complete, _) = run_range(FakeReads::new(range_oplog()), 2, 3, usize::MAX).await;
    let complete = complete.unwrap();
    let exact = document_bytes(&complete).unwrap();
    let (result, _) = run_range(FakeReads::new(range_oplog()), 2, 3, exact).await;
    assert_eq!(result.unwrap().entries.len(), 2);
    // One byte less fails at the last entry, which makes the count known, before the root check.
    let (result, log) = run_range(FakeReads::new(range_oplog()), 2, 3, exact - 1).await;
    assert_eq!(
        range_error(result),
        OplogExportError::OutputTooLarge {
            limit_bytes: exact - 1,
            matched_entries: Some(2),
        }
    );
    assert_eq!(page_reads(&log), vec![root_read()]);

    // The first page of 2 to 9 holds the same two entries. With their separators they fit in the
    // limit, but the header and the coverage do not fit beside them: the export reads no more.
    let limit = complete
        .entries
        .iter()
        .map(|entry| serde_json::to_vec(entry).unwrap().len() + 1)
        .sum::<usize>();
    let (result, log) = run_range(FakeReads::new(range_oplog()), 2, 9, limit).await;
    assert_eq!(
        range_error(result),
        OplogExportError::OutputTooLarge {
            limit_bytes: limit,
            matched_entries: None,
        }
    );
    assert_eq!(range_reads(&log).len(), 1);
    assert_eq!(page_reads(&log), vec![root_read()]);
}

#[test]
async fn coverage_that_the_entries_add_counts_toward_the_size() {
    // Entry 3 stays omitted with a hydration failure, which adds a coverage item.
    let oplog = || {
        let mut oplog = range_oplog();
        let mut failing = omitted(log_entry(3, "OTHER-MARKER"), 10);
        failing.hydration_fails = true;
        oplog[2] = failing;
        oplog
    };
    let (first_page, _) = run_range(FakeReads::new(oplog()), 2, 3, usize::MAX).await;
    let first_page = first_page.unwrap();
    assert_eq!(first_page.coverage.hydration_failures.discovered, 1);
    // The entries of the first page and the document with the empty coverage fit exactly; the
    // coverage item does not fit beside them.
    let empty_frame = OplogExportReport {
        header: first_page.header.clone(),
        entries: Vec::new(),
        coverage: OplogExportCoverage::new(first_page.coverage.notes.clone()),
    };
    let entry_bytes = first_page
        .entries
        .iter()
        .map(|entry| serde_json::to_vec(entry).unwrap().len())
        .sum::<usize>()
        + first_page.entries.len()
        - 1;
    let limit = document_bytes(&empty_frame).unwrap() + entry_bytes;

    // The second range read would spend the allowance: the export must fail on size before it.
    let mut reads = FakeReads::new(oplog());
    reads.exhaust_range_read_at = Some(1);
    let (result, log) = run_range(reads, 2, 9, limit).await;
    assert_eq!(
        range_error(result),
        OplogExportError::OutputTooLarge {
            limit_bytes: limit,
            matched_entries: None,
        }
    );
    assert_eq!(range_reads(&log).len(), 1);
}

#[test]
async fn a_range_with_a_missing_index_fails_without_a_report() {
    // A gap inside the range, and a range whose last index the server does not return.
    for (missing, from, to) in [(5, 2, 9), (8, 7, 8)] {
        let mut oplog = range_oplog();
        oplog.retain(|e| e.entry.oplog_index.as_u64() != missing);
        let (result, _) = run_range(FakeReads::new(oplog), from, to, usize::MAX).await;
        assert_eq!(
            range_error(result),
            OplogExportError::RangePageInvalid,
            "{missing}"
        );
    }
}

#[test]
async fn a_range_report_fails_when_the_root_changes_or_goes() {
    let mut reads = FakeReads::new(range_oplog());
    reads.instance_ids = vec![instance(1), instance(2)];
    let (result, _) = run_range(reads, 2, 5, usize::MAX).await;
    assert_eq!(range_error(result), OplogExportError::RootChanged);

    let mut reads = FakeReads::new(range_oplog());
    reads.fail_root_read_at = Some(1);
    let (result, _) = run_range(reads, 2, 5, usize::MAX).await;
    let error = result.unwrap_err();
    assert!(!format!("{error:#}").contains("MARKER"), "{error:#}");
}
