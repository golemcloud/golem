use super::*;
use crate::services::oplog::{CommitLevel, OrderedOplogStart, PendingUpload};
use async_trait::async_trait;
use golem_common::model::component::ComponentId;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::payload::types::{
    SerializableP3HttpBodyChunk, SerializableP3HttpConsumeBodyResult,
};
use golem_common::model::oplog::{
    AgentError, DurableFunctionType, HostRequest, HostRequestNoInput, HostRequestPollCount,
    HostResponseMonotonicClockTimestamp, HostResponseP3HttpClientConsumeBodyChunk,
    HostResponseP3HttpClientConsumeBodyResult, OplogPayload, PayloadId, RawOplogPayload,
};
use golem_common::model::{AgentId, Timestamp};
use std::collections::BTreeMap;
use std::time::Duration;
use test_r::test;

type StoredExternalPayload = (PayloadId, Vec<u8>, Vec<u8>);

/// Minimal in-memory `Oplog` used to drive a [`ReplayState`] over hand-built entries.
#[derive(Debug)]
struct InMemoryOplog {
    entries: tokio::sync::Mutex<Vec<OplogEntry>>,
    external_payloads: tokio::sync::Mutex<Vec<StoredExternalPayload>>,
}

impl InMemoryOplog {
    fn new() -> Self {
        Self {
            entries: tokio::sync::Mutex::new(Vec::new()),
            external_payloads: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    async fn store_external_request(&self, request: &HostRequest) -> OplogPayload<HostRequest> {
        let bytes = golem_common::serialization::serialize(request).unwrap();
        let payload_id = PayloadId::new();
        let md5_hash = vec![self.external_payloads.lock().await.len() as u8];
        self.external_payloads
            .lock()
            .await
            .push((payload_id.clone(), md5_hash.clone(), bytes));
        OplogPayload::External {
            payload_id,
            md5_hash,
            cached: None,
        }
    }
}

#[async_trait]
impl Oplog for InMemoryOplog {
    async fn add(&self, entry: OplogEntry) -> OplogIndex {
        let mut entries = self.entries.lock().await;
        entries.push(entry);
        OplogIndex::from_u64(entries.len() as u64)
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
    ) -> Result<OrderedOplogStart, String> {
        let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
        let index = self.add(entry.clone()).await;
        Ok(OrderedOplogStart {
            index,
            entry,
            pending_upload: PendingUpload::already_durable(),
        })
    }

    async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
        0
    }

    async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        BTreeMap::new()
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        OplogIndex::from_u64(self.entries.lock().await.len() as u64)
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }

    async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
        true
    }

    async fn read(&self, oplog_index: OplogIndex) -> OplogEntry {
        let entries = self.entries.lock().await;
        let idx: u64 = oplog_index.into();
        entries[(idx - 1) as usize].clone()
    }

    async fn read_many(&self, oplog_index: OplogIndex, n: u64) -> BTreeMap<OplogIndex, OplogEntry> {
        let entries = self.entries.lock().await;
        let start: u64 = oplog_index.into();
        let mut result = BTreeMap::new();
        for i in start..(start + n) {
            if let Some(entry) = entries.get((i - 1) as usize) {
                result.insert(OplogIndex::from_u64(i), entry.clone());
            }
        }
        result
    }

    async fn length(&self) -> u64 {
        self.entries.lock().await.len() as u64
    }

    async fn upload_raw_payload(&self, _data: Vec<u8>) -> Result<RawOplogPayload, String> {
        unimplemented!()
    }

    async fn download_raw_payload(
        &self,
        payload_id: PayloadId,
        md5_hash: Vec<u8>,
    ) -> Result<Vec<u8>, String> {
        self.external_payloads
            .lock()
            .await
            .iter()
            .find(|(id, hash, _)| id == &payload_id && hash == &md5_hash)
            .map(|(_, _, bytes)| bytes.clone())
            .ok_or_else(|| format!("missing test payload {payload_id}"))
    }

    async fn switch_persistence_level(&self, _mode: PersistenceLevel) {}
}

fn test_agent_id() -> OwnedAgentId {
    OwnedAgentId {
        environment_id: EnvironmentId::new(),
        agent_id: AgentId {
            component_id: ComponentId::new(),
            agent_id: "replay-state-test".to_string(),
        },
    }
}

fn noop() -> OplogEntry {
    OplogEntry::NoOp {
        timestamp: Timestamp::now_utc(),
    }
}

fn start_now() -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::MonotonicClockNow,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::ReadLocal,
    }
}

fn begin_atomic_region() -> OplogEntry {
    OplogEntry::BeginAtomicRegion {
        timestamp: Timestamp::now_utc(),
    }
}

fn end_for(start_index: u64, nanos: u64) -> OplogEntry {
    OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        response: Some(OplogPayload::Inline(Box::new(
            HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp { nanos }),
        ))),
        forced_commit: false,
    }
}

/// A `Start` for the sequential `golem::api` fork pair. Its only special replay behaviour is the
/// commit-only side effect in [`ReplayState::apply_commit_effects`] (recording its index in
/// `pending_fork_starts`), which the speculative-rollback test exercises.
fn fork_start() -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::GolemApiFork,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::WriteRemote,
    }
}

async fn replay_state_over(entries: Vec<OplogEntry>) -> ReplayState {
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in entries {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    ReplayState::new(test_agent_id(), oplog, DeletedRegions::default())
        .await
        .expect("failed to build replay state")
}

fn stdout_log(message: &str) -> OplogEntry {
    OplogEntry::Log {
        timestamp: Timestamp::now_utc(),
        level: LogLevel::Stdout,
        context: "stdout".to_string(),
        message: message.to_string(),
    }
}

/// Identical log entries persisted multiple times since the last non-hint entry must each be
/// deduplicated exactly once on re-run: the seen-log collection is a counted multiset, not a
/// set. Large or repetitive stdout output regularly produces identical consecutive chunks, so
/// losing multiplicity would re-persist all but the first occurrence on every recovery.
#[test]
async fn seen_log_tracks_multiplicity_of_identical_entries() {
    // All entries are hints: constructing the replay state skips them all and records the
    // log hashes.
    let rs = replay_state_over(vec![
        noop(),
        stdout_log("X"),
        stdout_log("X"),
        stdout_log("X"),
        stdout_log("Y"),
    ])
    .await;

    for remaining in (1..=3).rev() {
        assert!(
            rs.seen_log(LogLevel::Stdout, "stdout", "X").await,
            "X must still be seen with {remaining} unmatched occurrence(s) left"
        );
        rs.remove_seen_log(LogLevel::Stdout, "stdout", "X").await;
    }
    assert!(
        !rs.seen_log(LogLevel::Stdout, "stdout", "X").await,
        "all three occurrences of X are matched"
    );

    // Removing more occurrences than were recorded must not underflow or affect others.
    rs.remove_seen_log(LogLevel::Stdout, "stdout", "X").await;
    assert!(!rs.seen_log(LogLevel::Stdout, "stdout", "X").await);
    assert!(rs.seen_log(LogLevel::Stdout, "stdout", "Y").await);
    rs.remove_seen_log(LogLevel::Stdout, "stdout", "Y").await;
    assert!(!rs.seen_log(LogLevel::Stdout, "stdout", "Y").await);
}

#[test]
async fn claim_and_await_resolves_completed() {
    // [NoOp, Start, End]
    let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed {
            end_idx, response, ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
            assert!(response.is_some());
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn request_matching_downloads_uncached_external_payloads() {
    let oplog = Arc::new(InMemoryOplog::new());
    oplog.add(noop()).await;

    let first_request: HostRequest = HostRequestPollCount { count: 1 }.into();
    let second_request: HostRequest = HostRequestPollCount { count: 2 }.into();
    let first_payload = oplog.store_external_request(&first_request).await;
    let second_payload = oplog.store_external_request(&second_request).await;

    for payload in [first_payload, second_payload] {
        oplog
            .add(OplogEntry::Start {
                timestamp: Timestamp::now_utc(),
                parent_start_index: None,
                function_name: HostFunctionName::MonotonicClockNow,
                request: Some(payload),
                durable_function_type: DurableFunctionType::ReadLocal,
            })
            .await;
    }

    let oplog: Arc<dyn Oplog> = oplog;
    let rs = ReplayState::new(test_agent_id(), oplog, DeletedRegions::default())
        .await
        .unwrap();

    let second = rs
        .claim_concurrent_start_matching_request(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            &second_request,
        )
        .await
        .unwrap();
    assert_eq!(second.start_idx(), OplogIndex::from_u64(3));

    let first = rs
        .claim_concurrent_start_matching_request(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            &first_request,
        )
        .await
        .unwrap();
    assert_eq!(first.start_idx(), OplogIndex::from_u64(2));
}

#[test]
async fn identity_claim_includes_replay_target_after_full_scan_chunk() {
    let mut entries = Vec::with_capacity(CHUNK_SIZE as usize + 2);
    entries.push(noop());
    entries.extend((0..CHUNK_SIZE).map(|_| noop()));
    entries.push(start_now());
    let target = OplogIndex::from_u64(entries.len() as u64);
    let rs = replay_state_over(entries).await;

    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    assert_eq!(handle.start_idx(), target);
}

#[test]
async fn claim_any_returns_claimed_identity() {
    // The dynamic claim does not validate name/type; it returns the claimed Start's identity.
    let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
    let claimed = rs.claim_any_concurrent_start().await.unwrap();
    assert_eq!(claimed.handle.start_idx(), OplogIndex::from_u64(2));
    assert_eq!(claimed.function_name, HostFunctionName::MonotonicClockNow);
    assert_eq!(
        claimed.durable_function_type,
        DurableFunctionType::ReadLocal
    );

    match rs.await_resolution(claimed.handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn typed_claim_mismatch_does_not_leak_pending() {
    // A typed claim whose expected type does not match the recorded Start must fail AND drop the
    // resolver receiver that `claim_any_concurrent_start` registered, so no stale awaiter leaks.
    let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
    let err = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::WriteRemote, // recorded is ReadLocal
        )
        .await
        .unwrap_err();
    assert!(
        format!("{err}").contains("WriteRemote"),
        "the error must spell out the mismatched expected identity, got: {err}"
    );
    let internal = rs.cursor.state.lock().await;
    assert!(
        !internal
            .concurrent_resolver
            .is_pending(OplogIndex::from_u64(2)),
        "failed typed claim must not leave a pending awaiter"
    );
}

#[test]
async fn speculative_rollback_leaves_cursor_and_pending_unchanged() {
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — after claiming A, the cursor head
    // is the still-unclaimed, non-terminal Start(B). A speculative read whose predicate fails
    // must roll the cursor back fully (it must not steal Start(B)) and must not resolve A.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let start_idx = handle.start_idx();

    let speculative = rs.try_get_oplog_entry(|_| false).await.unwrap();
    assert!(speculative.is_none());
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(2),
        "speculative rollback must not advance the cursor past Start(B)"
    );
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal.concurrent_resolver.is_pending(start_idx),
            "speculative rollback must not resolve the handle"
        );
        assert!(
            !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(3)),
            "speculative rollback must not claim Start(B)"
        );
    }
}

#[test]
async fn speculative_rollback_does_not_apply_side_effects() {
    // A speculative read whose predicate fails rolls the cursor back AND applies none of the
    // entry's commit-only side effects. A GolemApiFork `Start` records its index in
    // `pending_fork_starts` only when permanently consumed; a rolled-back read must not.
    let rs = replay_state_over(vec![noop(), fork_start()]).await;

    let probe = rs.try_get_oplog_entry(|_| false).await.unwrap();
    assert!(probe.is_none());
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal.pending_fork_starts.is_empty(),
            "rolled-back speculative read must not apply the fork Start side effect"
        );
    }

    // The committed consume does apply the side effect.
    let (idx, _) = rs.try_get_oplog_entry(|_| true).await.unwrap().unwrap();
    assert_eq!(idx, OplogIndex::from_u64(2));
    let internal = rs.cursor.state.lock().await;
    assert!(
        internal
            .pending_fork_starts
            .contains(&OplogIndex::from_u64(2)),
        "committed read must apply the fork Start side effect"
    );
}

#[test]
async fn error_hint_between_start_and_end_resolves() {
    // [NoOp, Start, Error{retry_from: Start}, End] — Error is a hint, skipped transparently.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        OplogEntry::error(
            AgentError::TransientError("boom".to_string()),
            OplogIndex::from_u64(2),
            false,
            None,
        ),
        end_for(2, 42),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn dangling_start_without_end_errors() {
    // [NoOp, Start] — eager Start with no matching End/Cancelled (crash window).
    let rs = replay_state_over(vec![noop(), start_now()]).await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    let err = rs.await_resolution(handle).await.unwrap_err();
    let message = format!("{err}");
    assert!(
        message.contains("no matching End/Cancelled"),
        "unexpected error: {message}"
    );
}

#[test]
async fn lone_start_reports_incomplete_outcome_and_unregisters() {
    // [NoOp, Start] — same crash window as above, but via the outcome-returning API: the lone
    // committed Start (no End) must be reported as Incomplete (not an error), and the stale
    // resolver registration must be dropped so it cannot leak.
    let rs = replay_state_over(vec![noop(), start_now()]).await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let start_idx = handle.start_idx();

    match rs.await_resolution_outcome(handle).await.unwrap() {
        ResolutionOutcome::Incomplete => {}
        other => panic!("expected Incomplete, got {other:?}"),
    }
    let internal = rs.cursor.state.lock().await;
    assert!(
        !internal.concurrent_resolver.is_pending(start_idx),
        "incomplete outcome must unregister the awaiter"
    );
}

/// A claimed call whose awaiter was dropped without awaiting (the accessor future awaiting
/// the resolution was cancelled) must not wedge the cursor: when the cursor reaches the
/// call's terminal, the drain routes it to the closed receiver (the send fails silently) and
/// drops the registration, leaving no resolver residue behind.
#[test]
async fn dropped_awaiter_terminal_drains_without_residue() {
    // [NoOp, Start(2), End(2→3), NoOp(4)]
    let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42), noop()]).await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let start_idx = handle.start_idx();
    assert_eq!(start_idx, OplogIndex::from_u64(2));
    drop(handle);

    // A later positional read must drain the abandoned call's End on the way to NoOp(4).
    let consumed = rs
        .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::NoOp { .. }))
        .await
        .unwrap();
    assert_eq!(
        consumed.map(|(idx, _)| idx),
        Some(OplogIndex::from_u64(4)),
        "the positional reader must see NoOp(4), not the abandoned call's End"
    );

    let internal = rs.cursor.state.lock().await;
    assert!(
        !internal.concurrent_resolver.is_pending(start_idx),
        "draining the terminal of a dropped awaiter must drop its registration"
    );
}

/// A scan-ahead (identity-keyed) claim whose awaiter was dropped without awaiting must leave
/// no `claimed_starts` residue once the cursor passes the claimed `Start`, and no resolver
/// residue once it passes the terminal — dead registrations from cancelled accessor futures
/// must not accumulate or steal entries from later positional readers.
#[test]
async fn dropped_scan_ahead_claim_leaves_no_residue_once_cursor_passes() {
    fn owned_start_now(parent: u64) -> OplogEntry {
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(OplogIndex::from_u64(parent)),
            function_name: HostFunctionName::MonotonicClockNow,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
        }
    }

    // [NoOp, Start(A=2), Start(B=3, parent=2), End(B=3→4), End(A=2→5)]
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        owned_start_now(2),
        end_for(3, 1),
        end_for(2, 2),
    ])
    .await;

    // The head is Start(A), so the owned claim scan-ahead-claims Start(B) at 3.
    let handle_b = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    drop(handle_b);

    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));

    // Resolving A drives the cursor over the claimed Start(B) (auto-consumed) and End(B)
    // (drained to the dropped receiver) before reaching End(A).
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => {
            assert_eq!(end_idx, OplogIndex::from_u64(5));
        }
        other => panic!("expected Completed, got {other:?}"),
    }

    let internal = rs.cursor.state.lock().await;
    assert!(
        internal.claimed_starts.is_empty(),
        "passing the claimed Start must remove it from claimed_starts"
    );
    assert!(
        !internal
            .concurrent_resolver
            .is_pending(OplogIndex::from_u64(3)),
        "draining the terminal of the dropped scan-ahead claim must drop its registration"
    );
    assert!(
        !internal
            .concurrent_resolver
            .is_pending(OplogIndex::from_u64(2)),
        "the resolved call must not stay registered"
    );
}

#[test]
async fn interrupted_call_reports_incomplete_while_sibling_completes() {
    // [NoOp, Start(A=2), Start(B=3), End(B=3→4)] — a worker interrupted mid-call commits A's
    // `Start` but never its terminal, while a concurrent sibling B completed before the
    // interrupt. Replay must resolve B normally and report A as Incomplete (so A can be
    // re-executed live), not error out or misroute B's End to A.
    let rs = replay_state_over(vec![noop(), start_now(), start_now(), end_for(3, 42)]).await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

    match rs.await_resolution_outcome(handle_a).await.unwrap() {
        ResolutionOutcome::Incomplete => {}
        other => panic!("expected Incomplete for the interrupted call, got {other:?}"),
    }
    match rs.await_resolution_outcome(handle_b).await.unwrap() {
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) => {
            assert_eq!(end_idx, OplogIndex::from_u64(4));
        }
        other => panic!("expected Completed for the sibling call, got {other:?}"),
    }
}

#[test]
async fn replay_resolves_cancelled_without_partial() {
    // [NoOp, Start, Cancelled { partial: None }] — a call dropped mid-flight live and
    // recorded as `Cancelled` with no partial result replays to a `Cancelled` resolution
    // carrying no payload. (The caller decides how to surface it; the accessor replay path
    // rejects it as an unexpected entry when a response is required.)
    let rs = replay_state_over(vec![noop(), start_now(), cancelled_for(2)]).await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Cancelled {
            cancelled_idx,
            partial,
        } => {
            assert_eq!(cancelled_idx, OplogIndex::from_u64(3));
            assert!(partial.is_none());
        }
        other => panic!("expected Cancelled, got {other:?}"),
    }
}

#[test]
async fn replay_resolves_cancelled_with_partial_result() {
    // [NoOp, Start, Cancelled { partial: Some(..) }] — a call cancelled live with a partial
    // result replays to a `Cancelled` resolution that preserves the recorded partial
    // response payload (the CallHandle replay path downloads and converts it).
    let rs = replay_state_over(vec![noop(), start_now(), cancelled_with_partial_for(2, 42)]).await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Cancelled {
            cancelled_idx,
            partial,
        } => {
            assert_eq!(cancelled_idx, OplogIndex::from_u64(3));
            match partial {
                Some(OplogPayload::Inline(response)) => match *response {
                    HostResponse::MonotonicClockTimestamp(
                        HostResponseMonotonicClockTimestamp { nanos },
                    ) => assert_eq!(nanos, 42),
                    other => panic!("unexpected partial response: {other:?}"),
                },
                other => panic!("expected an inline partial payload, got {other:?}"),
            }
        }
        other => panic!("expected Cancelled, got {other:?}"),
    }
}

fn discarded_for(start_index: u64) -> OplogEntry {
    OplogEntry::CompletionDiscarded {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
    }
}

fn start_with_parent(parent_start_index: u64) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: Some(OplogIndex::from_u64(parent_start_index)),
        function_name: HostFunctionName::MonotonicClockNow,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::ReadLocal,
    }
}

fn invocation_finished() -> OplogEntry {
    OplogEntry::AgentInvocationFinished {
        timestamp: Timestamp::now_utc(),
        result: OplogPayload::Inline(Box::new(AgentInvocationResult::AgentInitialization)),
        method_name: None,
        consumed_fuel: 0,
        component_revision: ComponentRevision::INITIAL,
    }
}

async fn read_invocation_finished(
    rs: &ReplayState,
) -> Result<Option<AgentInvocationResult>, WorkerExecutorError> {
    rs.get_oplog_entry_agent_invocation_finished().await
}

#[test]
async fn invocation_boundary_tolerates_abandoned_closed_start() {
    // [NoOp, Start(2), End(2→3), AgentInvocationFinished(4)] — the durable call was issued
    // live but the replayed guest never re-issued it (an abandoned branch). At the invocation
    // boundary the never-claimed Start and its End are drained instead of failing the
    // positional read.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        invocation_finished(),
    ])
    .await;
    let result = read_invocation_finished(&rs).await.unwrap();
    assert!(matches!(
        result,
        Some(AgentInvocationResult::AgentInitialization)
    ));
}

#[test]
async fn invocation_boundary_tolerates_abandoned_cancelled_start() {
    // Same as above but the abandoned call was closed by a `Cancelled` terminal.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        cancelled_for(2),
        invocation_finished(),
    ])
    .await;
    let result = read_invocation_finished(&rs).await.unwrap();
    assert!(matches!(
        result,
        Some(AgentInvocationResult::AgentInitialization)
    ));
}

#[test]
async fn invocation_boundary_tolerates_nested_abandoned_scope() {
    // [NoOp, Start(2), Start(3, parent=2), End(3→4), End(2→5), AgentInvocationFinished(6)] —
    // an abandoned scope root with an abandoned child, both properly closed, is tolerated as
    // a structurally valid closed tail.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_with_parent(2),
        end_for(3, 43),
        end_for(2, 42),
        invocation_finished(),
    ])
    .await;
    let result = read_invocation_finished(&rs).await.unwrap();
    assert!(matches!(
        result,
        Some(AgentInvocationResult::AgentInitialization)
    ));
}

#[test]
async fn invocation_boundary_rejects_unclosed_abandoned_start() {
    // [NoOp, Start(2), AgentInvocationFinished(3)] — a dangling abandoned Start with no
    // terminal before the finished marker stays fatal: the closed-tail structural validation
    // fails.
    let rs = replay_state_over(vec![noop(), start_now(), invocation_finished()]).await;
    let err = read_invocation_finished(&rs)
        .await
        .expect_err("unclosed abandoned Start must be fatal");
    assert!(
        err.to_string().contains("unclosed abandoned Start"),
        "unexpected error: {err}"
    );
}

#[test]
async fn invocation_boundary_rejects_duplicate_terminal() {
    // [NoOp, Start(2), End(2→3), End(2→4), AgentInvocationFinished(5)] — a second terminal
    // closing the same abandoned Start is corruption, not tolerated noise.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        end_for(2, 43),
        invocation_finished(),
    ])
    .await;
    let err = read_invocation_finished(&rs)
        .await
        .expect_err("duplicate terminal for an abandoned Start must be fatal");
    assert!(
        err.to_string().contains("already closed"),
        "unexpected error: {err}"
    );
}

#[test]
async fn invocation_boundary_rejects_terminal_without_start() {
    // [NoOp, End(7→2), AgentInvocationFinished(3)] — a terminal whose Start was never drained
    // as abandoned (and is not awaited/orphaned) is not tolerated; the positional read still
    // fails with the unexpected entry.
    let rs = replay_state_over(vec![noop(), end_for(7, 42), invocation_finished()]).await;
    let err = read_invocation_finished(&rs)
        .await
        .expect_err("terminal without a matching abandoned Start must be fatal");
    assert!(
        err.to_string().contains("AgentInvocationFinished"),
        "unexpected error: {err}"
    );
}

#[test]
async fn invocation_boundary_rejects_unrelated_entry() {
    // [NoOp, NoOp(2), AgentInvocationFinished(3)] — non-hint entries other than abandoned
    // durable-call records stay fatal on the walk to the finished marker (`NoOp` is not a
    // hint entry).
    let rs = replay_state_over(vec![noop(), noop(), invocation_finished()]).await;
    let err = read_invocation_finished(&rs)
        .await
        .expect_err("unrelated positional entry must be fatal");
    assert!(
        err.to_string().contains("AgentInvocationFinished"),
        "unexpected error: {err}"
    );
}

#[test]
async fn invocation_boundary_does_not_drain_claimed_start() {
    // [NoOp, Start(2), End(2→3), AgentInvocationFinished(4)] with the Start claimed by a
    // concurrent replay call: the claim consumes the Start, the boundary walk drains the End
    // to the claim's resolver (awaited terminal), and the finished marker is read cleanly.
    // The claimed call still resolves as Completed — it is never miscounted as abandoned.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        invocation_finished(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    let result = read_invocation_finished(&rs).await.unwrap();
    assert!(matches!(
        result,
        Some(AgentInvocationResult::AgentInitialization)
    ));

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn invocation_boundary_tolerates_abandoned_child_of_claimed_start() {
    // [NoOp, Start(2), Start(3, parent=2), End(3→4), CompletionDiscarded(3),
    // End(2→6), AgentInvocationFinished(7)] with Start(2) claimed — the exact shape a
    // discarded response-body chunk leaves behind: the parent consume-body scope is claimed
    // by the replayed guest, but the guest dropped the body reader before the persisted
    // child chunk was delivered (the child's marker records the discard) and never demands
    // it again on replay. The boundary walk drains the abandoned child records, skips the
    // hint marker, routes the parent's awaited End to its claim, and reads the finished
    // marker cleanly.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_with_parent(2),
        end_for(3, 43),
        discarded_for(3),
        end_for(2, 42),
        invocation_finished(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));

    let result = read_invocation_finished(&rs).await.unwrap();
    assert!(matches!(
        result,
        Some(AgentInvocationResult::AgentInitialization)
    ));

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => {
            assert_eq!(end_idx, OplogIndex::from_u64(6));
        }
        other => panic!("expected Completed for the claimed parent, got {other:?}"),
    }
}

#[test]
async fn invocation_boundary_tolerates_abandoned_start_with_unknown_parent() {
    // [NoOp, Start(2, parent=99), End(2→3), AgentInvocationFinished(4)] — an abandoned
    // Start whose parent lies outside the walked records (a claimed scope, or a region
    // deleted by a jump/revert) is treated as a root of the abandoned tail: the parent
    // linkage is informational, only the closed-tail structure is validated.
    let rs = replay_state_over(vec![
        noop(),
        start_with_parent(99),
        end_for(2, 42),
        invocation_finished(),
    ])
    .await;
    let result = read_invocation_finished(&rs).await.unwrap();
    assert!(matches!(
        result,
        Some(AgentInvocationResult::AgentInitialization)
    ));
}

#[test]
async fn invocation_boundary_rejects_cancelled_after_end() {
    // [NoOp, Start(2), End(2→3), Cancelled(2→4), AgentInvocationFinished(5)] — a mixed
    // duplicate terminal (a `Cancelled` closing an abandoned Start already closed by an
    // `End`) is corruption, not tolerated noise.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        cancelled_for(2),
        invocation_finished(),
    ])
    .await;
    let err = read_invocation_finished(&rs)
        .await
        .expect_err("a Cancelled closing an already-Ended abandoned Start must be fatal");
    assert!(
        err.to_string().contains("already closed"),
        "unexpected error: {err}"
    );
}

#[test]
async fn invocation_boundary_rejects_terminal_of_resolved_claimed_start() {
    // [NoOp, Start(2), End(2→3), End(2→4), AgentInvocationFinished(5)] with Start(2)
    // claimed and resolved before the boundary read: the first End resolves the claim, so
    // the second End targets a start that is neither awaited nor drained as abandoned — it
    // stays fatal on the walk to the finished marker instead of being normalized into the
    // abandoned tail.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        end_for(2, 43),
        invocation_finished(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
        }
        other => panic!("expected Completed, got {other:?}"),
    }

    let err = read_invocation_finished(&rs)
        .await
        .expect_err("a duplicate terminal of a resolved claimed Start must be fatal");
    assert!(
        err.to_string().contains("AgentInvocationFinished"),
        "unexpected error: {err}"
    );
}

#[test]
async fn invocation_boundary_rejects_unclaimed_fork_pair() {
    // [NoOp, Start(2, GolemApiFork), End(2→3, Forked), AgentInvocationFinished(4)] — an
    // unclaimed legacy fork pair is a dedicated-positional-consumer record whose committed
    // consume is not inert: committing it would record a pending fork and decode the End
    // into a `ForkReplayed` event the replayed guest never requested. It must stay fatal
    // at the invocation boundary, and neither its commit-side state nor its replay event
    // may be applied.
    let rs = replay_state_over(vec![
        noop(),
        fork_start(),
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(2),
            response: Some(OplogPayload::Inline(Box::new(HostResponse::GolemApiFork(
                HostResponseGolemApiFork {
                    forked_phantom_id: Uuid::new_v4(),
                    result: Ok(ForkResult::Forked),
                },
            )))),
            forced_commit: false,
        },
        invocation_finished(),
    ])
    .await;

    let err = read_invocation_finished(&rs)
        .await
        .expect_err("an unclaimed GolemApiFork pair must stay fatal");
    assert!(
        err.to_string().contains("GolemApiFork"),
        "unexpected error: {err}"
    );

    assert!(
        rs.take_new_replay_events().is_empty(),
        "no replay event may be emitted for the rejected fork pair"
    );
    let internal = rs.cursor.state.lock().await;
    assert!(
        internal.pending_fork_starts.is_empty(),
        "the rejected fork Start's commit-side state must not be applied"
    );
}

#[test]
async fn invocation_boundary_tolerates_abandoned_consume_body_scope_shape() {
    // The actual shape a fully abandoned P3 consume-body leaves behind:
    //
    //   [NoOp,
    //    Start(2, P3HttpClientConsumeBody, WriteRemoteBatched(None)),          — parent scope
    //    Start(3, P3HttpClientConsumeBodyChunk, WriteRemoteBatched(Some(2)),
    //          parent_start_index=2),                                          — child chunk
    //    End(3→4, Data),                                                       — persisted chunk
    //    CompletionDiscarded(3),                                               — never delivered
    //    End(2→6, Trailers(None)),                                             — scope closed
    //    AgentInvocationFinished(7)]
    //
    // The replayed guest never re-issued the consume-body call, so nothing claims the
    // parent: the boundary walk drains the whole abandoned subtree (parent scope,
    // discarded child, both terminals), skips the discard hint, and reads the finished
    // marker cleanly.
    let rs = replay_state_over(vec![
        noop(),
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::P3HttpClientConsumeBody,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
        },
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(OplogIndex::from_u64(2)),
            function_name: HostFunctionName::P3HttpClientConsumeBodyChunk,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemoteBatched(Some(
                OplogIndex::from_u64(2),
            )),
        },
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(3),
            response: Some(OplogPayload::Inline(Box::new(
                HostResponse::P3HttpClientConsumeBodyChunk(
                    HostResponseP3HttpClientConsumeBodyChunk {
                        chunk: SerializableP3HttpBodyChunk::Data(vec![1, 2, 3]),
                    },
                ),
            ))),
            forced_commit: false,
        },
        discarded_for(3),
        OplogEntry::End {
            timestamp: Timestamp::now_utc(),
            start_index: OplogIndex::from_u64(2),
            response: Some(OplogPayload::Inline(Box::new(
                HostResponse::P3HttpClientConsumeBodyResult(
                    HostResponseP3HttpClientConsumeBodyResult {
                        result: SerializableP3HttpConsumeBodyResult::Trailers(None),
                    },
                ),
            ))),
            forced_commit: false,
        },
        invocation_finished(),
    ])
    .await;

    let result = read_invocation_finished(&rs).await.unwrap();
    assert!(matches!(
        result,
        Some(AgentInvocationResult::AgentInitialization)
    ));
    // Reaching the end of replay emits `ReplayFinished`; the drained subtree itself must not
    // emit any side-effecting event (`ForkReplayed` / `UpdateReplayed`).
    let events = rs.take_new_replay_events();
    assert!(
        events
            .iter()
            .all(|event| matches!(event, ReplayEvent::ReplayFinished)),
        "draining the abandoned consume-body subtree must not emit side-effecting replay \
             events, got {events:?}"
    );
}

#[test]
async fn replay_resolves_completed_but_discarded() {
    // [NoOp, Start, End, CompletionDiscarded] — the End was persisted live but its response
    // was never delivered to the guest (the marker records the discard), so replay must
    // resolve the call as CompletedButDiscarded, carrying the recorded response so deferred
    // replay can perform the recorded post-`End` continuation before parking.
    let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42), discarded_for(2)]).await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::CompletedButDiscarded {
            end_idx,
            marker_idx,
            response,
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
            assert_eq!(marker_idx, OplogIndex::from_u64(4));
            assert!(response.is_some());
        }
        other => panic!("expected CompletedButDiscarded, got {other:?}"),
    }
}

#[test]
async fn marker_in_deleted_region_delivers_end_normally() {
    // A CompletionDiscarded marker inside a deleted region belongs to an abandoned timeline:
    // the still-visible End must be delivered normally.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [noop(), start_now(), end_for(2, 42), discarded_for(2)] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped =
        golem_common::model::regions::DeletedRegionsBuilder::from_regions([OplogRegion {
            start: OplogIndex::from_u64(4),
            end: OplogIndex::from_u64(4),
        }])
        .build();
    let rs = ReplayState::new(test_agent_id(), oplog, skipped)
        .await
        .expect("failed to build replay state");
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed {
            end_idx, response, ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
            assert!(response.is_some());
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn duplicate_completion_discarded_markers_fail_construction() {
    // Two markers referencing the same Start is oplog corruption; the upfront scan rejects it.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        noop(),
        start_now(),
        end_for(2, 42),
        discarded_for(2),
        discarded_for(2),
    ] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let err = ReplayState::new(test_agent_id(), oplog, DeletedRegions::default())
        .await
        .expect_err("duplicate markers must fail replay state construction");
    assert!(
        err.to_string().contains("CompletionDiscarded"),
        "unexpected error: {err}"
    );
}

#[test]
async fn marker_recorded_at_runtime_is_visible_to_replay() {
    // record_discarded_completion feeds the same map as the upfront scan: a marker appended
    // live by this instance must park a later re-replay of its End exactly like a scanned
    // marker (e.g. after a drop-override restart). The marker is appended to the oplog and
    // the replay target grown over it, mirroring the live flow; growing over the
    // already-recorded marker must be idempotent, not a duplicate-marker error.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [noop(), start_now(), end_for(2, 42)] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let rs = ReplayState::new(test_agent_id(), oplog.clone(), DeletedRegions::default())
        .await
        .expect("failed to build replay state");
    let marker_idx = oplog.add(discarded_for(2)).await;
    rs.record_discarded_completion(OplogIndex::from_u64(2), marker_idx);
    rs.set_replay_target(marker_idx)
        .await
        .expect("growing the target over the recorded marker must be idempotent");

    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::CompletedButDiscarded {
            end_idx,
            marker_idx,
            response,
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
            assert_eq!(marker_idx, OplogIndex::from_u64(4));
            assert!(response.is_some());
        }
        other => panic!("expected CompletedButDiscarded, got {other:?}"),
    }
}

#[test]
async fn drain_parks_on_unclaimed_start() {
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — draining the awaited terminals
    // while only A is claimed must stop on the still-unclaimed Start(B): A stays pending and the
    // cursor does not advance past A's own Start. The cursor never steals a non-terminal entry a
    // positional consumer / sibling claim owns.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));

    rs.drain_awaited_terminals().await.unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(2)),
            "drain must not resolve A across the unclaimed Start(B)"
        );
    }
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(2),
        "drain must not advance past the unclaimed Start(B)"
    );

    // Once B is claimed (guest re-execution reaches its call), awaiting drains both Ends.
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn drain_parks_on_positional_marker() {
    // [NoOp, Start(A=2), BeginAtomicRegion(3), End(A=2→4)] — draining parks on the scope marker a
    // positional reader owns; A resolves once that marker has been consumed.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        begin_atomic_region(),
        end_for(2, 42),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    rs.drain_awaited_terminals().await.unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(2))
        );
    }
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(2));

    // The positional reader consumes the marker, after which awaiting A drains its End.
    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn drain_parks_on_unawaited_end() {
    // [NoOp, Start(A=2), End(scope=99→3), End(A=2→4)] — End(99) is a scope End nobody awaits (its
    // Start was consumed positionally). Draining must park on it and leave it for the positional
    // reader instead of consuming it on A's behalf.
    let rs = replay_state_over(vec![noop(), start_now(), end_for(99, 7), end_for(2, 42)]).await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    rs.drain_awaited_terminals().await.unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(2)),
            "drain must not resolve A across an unawaited End"
        );
    }
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(2),
        "drain must not consume the unawaited scope End"
    );

    // The positional scope reader consumes its own End, after which A's End resolves.
    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::End { .. }));

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn interleaved_calls_resolve_out_of_order() {
    // [NoOp, Start(A), Start(B), End(B=3), End(A=2)] — completion order (B then A) differs from
    // claim order (A then B). Each call resolves by its own start index, not by position.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(3, 43),
        end_for(2, 42),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

    // End(B) at index 4 resolves B; End(A) at index 5 resolves A.
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn await_suspends_until_sibling_claim() {
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — A is claimed and awaited *before*
    // B is claimed. With real overlap the awaiter must SUSPEND on the still-unclaimed Start(B)
    // at the cursor head (neither erroring nor resolving), and then resume once a sibling claims
    // B and advances the cursor — at which point A's End becomes a drainable awaited terminal.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));

    // Awaiting A parks: its End sits behind the unclaimed Start(B), so the first poll is Pending.
    let a_fut = rs.await_resolution(handle_a);
    tokio::pin!(a_fut);
    assert!(
        futures::poll!(a_fut.as_mut()).is_pending(),
        "awaiting A must suspend while Start(B) is unclaimed"
    );
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(2),
        "a parked awaiter must not advance the cursor past the unclaimed Start(B)"
    );

    // A sibling claims B (guest re-execution reaches B's call): this advances the cursor and
    // signals progress, waking A so its End at index 4 can be drained on the next poll.
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

    match a_fut.await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn speculative_read_does_not_publish_live_cursor() {
    // [NoOp, Start(A=2)] — replay_target = 2. A speculative read of the last entry must NOT make
    // the cursor observably reach the replay target while the read is still rollbackable: the
    // predicate (run after the read) must still see `is_live() == false`. This is the regression
    // guard for "publish only committed cursor state" — a transient live cursor would let a
    // concurrent awaiter falsely conclude end-of-replay.
    let rs = replay_state_over(vec![noop(), start_now()]).await;
    assert!(!rs.is_live());

    let observed_live = std::cell::Cell::new(None);
    let probe = rs
        .try_get_oplog_entry(|_entry| {
            observed_live.set(Some(rs.is_live()));
            false
        })
        .await
        .unwrap();

    assert!(probe.is_none());
    assert_eq!(
        observed_live.get(),
        Some(false),
        "cursor must not be observably advanced to live while the read is still speculative"
    );
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(1),
        "a rolled-back probe must leave the committed cursor unchanged"
    );
    assert!(!rs.is_live());
}

#[test]
async fn positional_reader_drains_awaited_terminal_before_marker() {
    // [NoOp, Start(A=2), Start(B=3), End(B=3→4), BeginAtomicRegion(5), End(A=2→6)] — both A and B
    // are claimed; a positional read for the atomic-region marker must first auto-drain B's End
    // (idx 4) to B's awaiter and only then return the marker (idx 5). It must never steal/return
    // End(B) positionally.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(3, 43),
        begin_atomic_region(),
        end_for(2, 42),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));

    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(5));
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(6)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn overlap_layout_with_scope_end_behind_awaited_sibling() {
    // The headline overlap layout:
    //   [NoOp, Start(A=2), Start(scope S=3), Start(B=4), End(B=4→5), End(scope S=3→6), End(A=2→7)]
    // A is claimed and awaited first, but its End sits last; in between are a positional scope
    // (S, consumed by a positional reader) and a fully overlapping sibling call B. This proves:
    // A suspends through the scope Start and B's Start; B's End is auto-drained to B; the scope's
    // End (nobody awaits it) is left for the positional reader; A resolves only once everything
    // ahead of its End has been consumed.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        start_now(),
        end_for(4, 44),
        end_for(3, 43),
        end_for(2, 42),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));

    // A awaits first; it parks on the scope Start (idx 3).
    let a_fut = rs.await_resolution(handle_a);
    tokio::pin!(a_fut);
    assert!(
        futures::poll!(a_fut.as_mut()).is_pending(),
        "A must park on the unclaimed scope Start"
    );

    // The positional scope reader consumes the scope Start (idx 3); A now parks on Start(B).
    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::Start { .. }));
    assert!(
        futures::poll!(a_fut.as_mut()).is_pending(),
        "A must park on the unclaimed Start(B)"
    );

    // The sibling call B is claimed and resolved; its End (idx 5) is auto-drained to B.
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(4));
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }

    // The scope End (idx 6) has no awaiter, so it is left for the positional scope reader.
    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(6));
    assert!(matches!(entry, OplogEntry::End { .. }));

    // Only now is A's End (idx 7) at the head; A resolves.
    match a_fut.await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(7)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn switch_to_live_wakes_parked_awaiter_as_incomplete() {
    // [NoOp, Start(A=2), Start(B=3)] — A is claimed and awaited but B is never claimed, so
    // awaiting A parks on the unclaimed Start(B). switch_to_live (end of replay) must wake the
    // parked awaiter as Incomplete instead of leaving it asleep forever, and must drop its
    // registration.
    let rs = replay_state_over(vec![noop(), start_now(), start_now()]).await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let start_idx = handle_a.start_idx();

    let a_fut = rs.await_resolution_outcome(handle_a);
    tokio::pin!(a_fut);
    assert!(
        futures::poll!(a_fut.as_mut()).is_pending(),
        "A must park on the unclaimed Start(B)"
    );

    rs.switch_to_live().await;

    match a_fut.await.unwrap() {
        ResolutionOutcome::Incomplete => {}
        other => panic!("expected Incomplete, got {other:?}"),
    }
    let internal = rs.cursor.state.lock().await;
    assert!(
        !internal.concurrent_resolver.is_pending(start_idx),
        "switch_to_live must unregister the parked awaiter"
    );
}

fn change_persistence_nothing() -> OplogEntry {
    OplogEntry::ChangePersistenceLevel {
        timestamp: Timestamp::now_utc(),
        persistence_level: PersistenceLevel::PersistNothing,
    }
}

fn log_entry() -> OplogEntry {
    OplogEntry::Log {
        timestamp: Timestamp::now_utc(),
        level: LogLevel::Info,
        context: "ctx".to_string(),
        message: "msg".to_string(),
    }
}

/// When replay reaches the target via a persist-nothing-zone jump (the zone extends to the end of
/// the oplog and is never closed) rather than by consuming the target entry, the transition to
/// live must still synthesize `ReplayFinished` — otherwise a pending automatic update would never
/// be finalized. (Regression: the synthesis used to be gated on the *consumed* entry index
/// equalling `replay_target`, which this jump never satisfies.)
#[test]
async fn replay_finished_emitted_when_persist_nothing_zone_reaches_target() {
    // [NoOp(1), ChangePersistenceLevel(PersistNothing)(2), Log(3), Log(4)] — the persist-nothing
    // zone opened at 2 is never closed, so replay jumps straight to the target (4) without
    // consuming it.
    let rs = replay_state_over(vec![
        noop(),
        change_persistence_nothing(),
        log_entry(),
        log_entry(),
    ])
    .await;
    assert!(rs.is_live(), "replay should be complete after construction");
    let events = rs.take_new_replay_events();
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ReplayEvent::ReplayFinished)),
        "a persist-nothing-zone jump to the target must still emit ReplayFinished, got {events:?}"
    );
}

/// When replay reaches the target via a skipped-region jump (`get_out_of_skipped_region` jumps
/// the cursor to the region end, which is the target) rather than by consuming the target entry,
/// the transition to live must still synthesize `ReplayFinished`.
#[test]
async fn replay_finished_emitted_when_skipped_region_reaches_target() {
    // [NoOp(1), Start(2), Log(3), Log(4)] with deleted region [3, 4]: consuming the Start at 2
    // jumps the cursor over the deleted tail straight to the target (4).
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [noop(), start_now(), log_entry(), log_entry()] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(3),
        end: OplogIndex::from_u64(4),
    }]);
    let rs = ReplayState::new(test_agent_id(), oplog, skipped)
        .await
        .expect("failed to build replay state");

    assert!(!rs.is_live(), "Start at 2 is not yet consumed");
    let (idx, _) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(2));

    assert!(
        rs.is_live(),
        "consuming the Start must jump over the deleted tail to the target"
    );
    let events = rs.take_new_replay_events();
    let finished = events
        .iter()
        .filter(|e| matches!(e, ReplayEvent::ReplayFinished))
        .count();
    assert_eq!(
        finished, 1,
        "a skipped-region jump to the target must emit exactly one ReplayFinished, got {events:?}"
    );
}

/// Regression guard for the moved transition detection: consuming the target entry directly
/// (the common path) still emits exactly one `ReplayFinished`.
#[test]
async fn replay_finished_emitted_when_target_entry_consumed() {
    // [NoOp(1), Start(2), End(3)] — replay becomes live by consuming the End at the target (3).
    let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
    // Nothing has crossed into live yet (the Start is still pending a claim).
    assert!(rs.take_new_replay_events().is_empty());

    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(handle).await.unwrap();

    assert!(rs.is_live());
    let events = rs.take_new_replay_events();
    let finished = events
        .iter()
        .filter(|e| matches!(e, ReplayEvent::ReplayFinished))
        .count();
    assert_eq!(
        finished, 1,
        "consuming the target entry must emit exactly one ReplayFinished, got {events:?}"
    );
}

/// How a generated concurrent call terminates in the fabricated oplog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CallKind {
    /// Recorded an `End` (successful completion).
    Completed,
    /// Recorded a `Cancelled` (dropped before completion).
    Cancelled,
    /// No terminal at all: a committed `Start` whose `End`/`Cancelled` never made it to disk
    /// (the forced-commit / crash window). Replay must report this as `Incomplete`.
    Incomplete,
}

fn cancelled_for(start_index: u64) -> OplogEntry {
    OplogEntry::Cancelled {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        partial: None,
    }
}

fn cancelled_with_partial_for(start_index: u64, nanos: u64) -> OplogEntry {
    OplogEntry::Cancelled {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        partial: Some(OplogPayload::Inline(Box::new(
            HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp { nanos }),
        ))),
    }
}

fn end_atomic_region(begin_index: u64) -> OplogEntry {
    OplogEntry::EndAtomicRegion {
        timestamp: Timestamp::now_utc(),
        begin_index: OplogIndex::from_u64(begin_index),
    }
}

fn change_persistence_smart() -> OplogEntry {
    OplogEntry::ChangePersistenceLevel {
        timestamp: Timestamp::now_utc(),
        persistence_level: PersistenceLevel::Smart,
    }
}

fn pre_commit_remote_transaction(begin_index: u64) -> OplogEntry {
    OplogEntry::PreCommitRemoteTransaction {
        timestamp: Timestamp::now_utc(),
        begin_index: OplogIndex::from_u64(begin_index),
    }
}

fn committed_remote_transaction(begin_index: u64) -> OplogEntry {
    OplogEntry::CommittedRemoteTransaction {
        timestamp: Timestamp::now_utc(),
        begin_index: OplogIndex::from_u64(begin_index),
    }
}

/// A non-hint *positional* marker entry (atomic-region boundary, persistence-level switch, or an
/// rdbms-transaction internal marker). These are never claimed and never auto-drained; a
/// positional reader must consume them, and an overlapping awaiter parks on them until then.
fn random_positional_marker(rng: &mut rand::rngs::StdRng) -> OplogEntry {
    use rand::Rng;
    match rng.random_range(0..6) {
        0 => begin_atomic_region(),
        1 => end_atomic_region(1),
        2 => change_persistence_smart(),
        3 => pre_commit_remote_transaction(1),
        4 => committed_remote_transaction(1),
        // `NoOp` is non-hint, so it too must be consumed by a positional reader (unlike the
        // `Log` hint entries, which are skipped transparently).
        _ => noop(),
    }
}

/// A generated item in a fabricated overlap layout: either a concurrent call (claimed +
/// awaited) or a positional scope (a `Start`/`End` pair consumed by positional reads, standing
/// in for a durable scope / rdbms transaction span).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ItemKind {
    Call(CallKind),
    Scope,
}

/// The role of a single fabricated oplog entry, aligned by index, so the replay driver knows how
/// to consume each entry as the cursor reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Placeholder,
    CallStart(usize),
    CallTerminal(usize),
    ScopeStart,
    ScopeEnd,
    Marker,
    Hint,
}

/// Seam 1 of the concurrent-durability validation plan: a randomized generator over
/// host-call-only oplog layouts. Each case builds
/// `[<placeholder>, Start_1 .. Start_n, <terminals in a random completion order>]` with `Log`
/// hint entries optionally interleaved everywhere, where each call independently completes (`End`), is
/// cancelled (`Cancelled`), or is left incomplete (a committed `Start` with no terminal). It then
/// claims every `Start` and awaits each call's resolution in a random order, asserting that:
///
/// - the k-th positional claim returns the k-th `Start`;
/// - every call resolves to exactly its recorded terminal *by oplog index*, independent of the
///   completion order recorded in the oplog and the order the calls are awaited in (a single
///   await drains all awaited terminals at the cursor head, buffering siblings' outcomes);
/// - an incomplete `Start` reports `Incomplete` rather than erroring or stealing a sibling's
///   terminal;
/// - interleaved hint entries (`Log`) are skipped transparently, whether they land between
///   `Start`s, between a sibling's `Start` and `End`, or among the terminals;
/// - once all calls resolve, replay is live with no awaiter left registered.
///
/// This generalizes the hand-written `n = 2/3` overlap tests above to the full
/// call/completion/await permutation space. Seeds are deterministic so any failure reproduces.
#[test]
async fn concurrent_replay_call_permutation_fuzz() {
    use rand::rngs::StdRng;
    use rand::seq::SliceRandom;
    use rand::{Rng, SeedableRng};

    const CASES: u64 = 2000;

    for seed in 0..CASES {
        let mut rng = StdRng::seed_from_u64(seed);
        let n = rng.random_range(1..=5usize);

        let kinds: Vec<CallKind> = (0..n)
            .map(|_| match rng.random_range(0..3) {
                0 => CallKind::Completed,
                1 => CallKind::Cancelled,
                _ => CallKind::Incomplete,
            })
            .collect();

        // Index 1 is the mandatory placeholder consumed unconditionally at construction (it
        // stands in for the `Create` worker entry), so the first Start is at index 2.
        let mut entries = vec![noop()];
        let mut start_idx = Vec::with_capacity(n);
        for _ in 0..n {
            if rng.random_bool(0.3) {
                entries.push(log_entry());
            }
            entries.push(start_now());
            start_idx.push(entries.len() as u64);
        }

        // Terminals for the non-incomplete calls, recorded in a random completion order.
        let mut terminal_calls: Vec<usize> = (0..n)
            .filter(|&i| kinds[i] != CallKind::Incomplete)
            .collect();
        terminal_calls.shuffle(&mut rng);

        let mut terminal_oplog_idx: Vec<Option<u64>> = vec![None; n];
        let mut nanos = 0u64;
        for &i in &terminal_calls {
            if rng.random_bool(0.3) {
                entries.push(log_entry());
            }
            let entry = match kinds[i] {
                CallKind::Completed => {
                    nanos += 1;
                    end_for(start_idx[i], nanos)
                }
                CallKind::Cancelled => cancelled_for(start_idx[i]),
                CallKind::Incomplete => unreachable!("incomplete calls have no terminal"),
            };
            entries.push(entry);
            terminal_oplog_idx[i] = Some(entries.len() as u64);
        }
        if rng.random_bool(0.3) {
            entries.push(log_entry());
        }

        let rs = replay_state_over(entries).await;

        // Claim every Start positionally; the k-th claim returns the k-th Start.
        let mut handles: Vec<Option<ReplayCallHandle>> = Vec::with_capacity(n);
        for (i, expected) in start_idx.iter().enumerate() {
            let handle = rs
                .claim_concurrent_start(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                )
                .await
                .unwrap_or_else(|e| panic!("seed {seed}: claim {i} failed: {e}"));
            assert_eq!(
                handle.start_idx(),
                OplogIndex::from_u64(*expected),
                "seed {seed}: claim {i} returned the wrong Start"
            );
            handles.push(Some(handle));
        }

        // Await resolutions in a random order; out-of-order awaiting must still resolve each call
        // to its own recorded terminal.
        let mut await_order: Vec<usize> = (0..n).collect();
        await_order.shuffle(&mut rng);
        for i in await_order {
            let handle = handles[i]
                .take()
                .expect("each handle is awaited exactly once");
            let outcome = rs
                .await_resolution_outcome(handle)
                .await
                .unwrap_or_else(|e| panic!("seed {seed}: await {i} failed: {e}"));
            match (kinds[i], outcome) {
                (
                    CallKind::Completed,
                    ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }),
                ) => {
                    assert_eq!(
                        end_idx,
                        OplogIndex::from_u64(terminal_oplog_idx[i].unwrap()),
                        "seed {seed}: call {i} resolved to the wrong End index"
                    );
                }
                (
                    CallKind::Cancelled,
                    ResolutionOutcome::Resolved(Resolution::Cancelled { cancelled_idx, .. }),
                ) => {
                    assert_eq!(
                        cancelled_idx,
                        OplogIndex::from_u64(terminal_oplog_idx[i].unwrap()),
                        "seed {seed}: call {i} resolved to the wrong Cancelled index"
                    );
                }
                (CallKind::Incomplete, ResolutionOutcome::Incomplete) => {}
                (kind, other) => {
                    panic!("seed {seed}: call {i} (kind {kind:?}) resolved unexpectedly: {other:?}")
                }
            }
        }

        assert!(
            rs.is_live(),
            "seed {seed}: replay did not reach live after all calls resolved"
        );
        let internal = rs.cursor.state.lock().await;
        for (i, &si) in start_idx.iter().enumerate() {
            assert!(
                !internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(si)),
                "seed {seed}: call {i} left a registered awaiter"
            );
        }
    }
}

/// Seam 1, full layout space: a randomized generator over fabricated overlap layouts that mix
/// concurrent calls (completed / cancelled / incomplete) with **positional** scopes (`Start`/`End`
/// pairs consumed by positional reads) and non-hint positional **markers** (atomic-region
/// boundaries, persistence-level switches, rdbms-transaction internal markers), all freely
/// interleaved with `Log` hints, so that a sibling's scope `End` or a marker can land between
/// another call's `Start` and `End` — the headline overlap layout generalized.
///
/// Each call's resolution is awaited on its **own concurrently-suspended task** (`tokio::spawn`),
/// mirroring the production model where the worker drives the replay cursor (claims + positional
/// reads) while several call futures are suspended; this is what exercises the genuine
/// suspend/resume path (`await_resolution_outcome` parking on a positional blocker and resuming on
/// cursor progress), not just the auto-drain-at-head path. A single driver walks the oplog
/// left-to-right, claiming call `Start`s, positionally reading scope `Start`/`End`s and markers,
/// and leaving call terminals to be auto-drained. It asserts that:
///
/// - each positional claim / read returns exactly the entry at the expected oplog index,
///   independent of how the suspended awaiter tasks are scheduled (auto-drains only ever consume
///   awaited call terminals, never a positional entry a reader owns);
/// - every call resolves to exactly its recorded terminal (`End`/`Cancelled` by index) or, for a
///   committed `Start` with no terminal, `Incomplete`;
/// - replay ends live with no awaiter left registered.
///
/// Only final per-call outcomes and positional indices are asserted, both of which are
/// timing-independent, so the test is deterministic despite the concurrent tasks. Seeds are
/// fixed, so any failure reproduces.
#[test]
async fn concurrent_replay_overlap_with_scopes_and_markers_fuzz() {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    const CASES: u64 = 1000;

    for seed in 0..CASES {
        let mut rng = StdRng::seed_from_u64(seed);
        let num_items = rng.random_range(1..=5usize);

        let items: Vec<ItemKind> = (0..num_items)
            .map(|_| match rng.random_range(0..4) {
                0 => ItemKind::Call(CallKind::Completed),
                1 => ItemKind::Call(CallKind::Cancelled),
                2 => ItemKind::Call(CallKind::Incomplete),
                _ => ItemKind::Scope,
            })
            .collect();

        let is_incomplete = |i: usize| matches!(items[i], ItemKind::Call(CallKind::Incomplete));

        // Build a valid random interleaving: each item's Start precedes its End; incomplete
        // calls have no End; scopes and completed/cancelled calls do. Markers and hints are
        // sprinkled in from a budget so they can land between any sibling's Start and End.
        let mut entries = vec![noop()];
        let mut roles = vec![Role::Placeholder];
        let mut start_idx = vec![0u64; num_items];
        let mut terminal_idx = vec![None; num_items];
        let mut opened = vec![false; num_items];
        let mut closed = vec![false; num_items];
        let mut markers_left = rng.random_range(0..=4u32);
        let mut hints_left = rng.random_range(0..=3u32);
        let mut nanos = 0u64;

        loop {
            let can_open: Vec<usize> = (0..num_items).filter(|&i| !opened[i]).collect();
            let can_close: Vec<usize> = (0..num_items)
                .filter(|&i| opened[i] && !closed[i] && !is_incomplete(i))
                .collect();

            #[derive(Clone, Copy)]
            enum Cat {
                Open,
                Close,
                Marker,
                Hint,
            }
            let mut cats = Vec::new();
            if !can_open.is_empty() {
                cats.push(Cat::Open);
            }
            if !can_close.is_empty() {
                cats.push(Cat::Close);
            }
            if markers_left > 0 {
                cats.push(Cat::Marker);
            }
            if hints_left > 0 {
                cats.push(Cat::Hint);
            }
            if cats.is_empty() {
                break;
            }

            match cats[rng.random_range(0..cats.len())] {
                Cat::Open => {
                    let item = can_open[rng.random_range(0..can_open.len())];
                    entries.push(start_now());
                    start_idx[item] = entries.len() as u64;
                    opened[item] = true;
                    roles.push(match items[item] {
                        ItemKind::Call(_) => Role::CallStart(item),
                        ItemKind::Scope => Role::ScopeStart,
                    });
                }
                Cat::Close => {
                    let item = can_close[rng.random_range(0..can_close.len())];
                    let si = start_idx[item];
                    let (entry, role) = match items[item] {
                        ItemKind::Call(CallKind::Completed) => {
                            nanos += 1;
                            (end_for(si, nanos), Role::CallTerminal(item))
                        }
                        ItemKind::Call(CallKind::Cancelled) => {
                            (cancelled_for(si), Role::CallTerminal(item))
                        }
                        ItemKind::Scope => {
                            nanos += 1;
                            (end_for(si, nanos), Role::ScopeEnd)
                        }
                        ItemKind::Call(CallKind::Incomplete) => {
                            unreachable!("incomplete calls are never closed")
                        }
                    };
                    entries.push(entry);
                    terminal_idx[item] = Some(entries.len() as u64);
                    closed[item] = true;
                    roles.push(role);
                }
                Cat::Marker => {
                    entries.push(random_positional_marker(&mut rng));
                    roles.push(Role::Marker);
                    markers_left -= 1;
                }
                Cat::Hint => {
                    entries.push(log_entry());
                    roles.push(Role::Hint);
                    hints_left -= 1;
                }
            }
        }

        let rs = Arc::new(replay_state_over(entries).await);

        // Walk the oplog left-to-right, consuming each entry by its role. Each claimed call's
        // resolution is awaited on its own suspended task.
        let mut tasks: Vec<(usize, tokio::task::JoinHandle<_>)> = Vec::new();
        for (zero_based, role) in roles.iter().enumerate().skip(1) {
            let idx = (zero_based + 1) as u64;
            match *role {
                Role::CallStart(item) => {
                    let handle = rs
                        .claim_concurrent_start(
                            &HostFunctionName::MonotonicClockNow,
                            &DurableFunctionType::ReadLocal,
                        )
                        .await
                        .unwrap_or_else(|e| {
                            panic!("seed {seed}: claim of item {item} at {idx} failed: {e}")
                        });
                    assert_eq!(
                        handle.start_idx(),
                        OplogIndex::from_u64(idx),
                        "seed {seed}: claim of item {item} returned the wrong Start"
                    );
                    let rs2 = rs.clone();
                    tasks.push((
                        item,
                        tokio::spawn(async move { rs2.await_resolution_outcome(handle).await }),
                    ));
                }
                Role::ScopeStart | Role::ScopeEnd | Role::Marker => {
                    let (got, _) = rs.get_oplog_entry().await.unwrap_or_else(|e| {
                        panic!("seed {seed}: positional read at {idx} ({role:?}) failed: {e}")
                    });
                    assert_eq!(
                        got,
                        OplogIndex::from_u64(idx),
                        "seed {seed}: positional read ({role:?}) returned the wrong index"
                    );
                }
                // Call terminals are auto-drained to their awaiter; hints are skipped by the
                // preceding consume's skip_forward. Neither is walked explicitly.
                Role::CallTerminal(_) | Role::Hint => {}
                Role::Placeholder => unreachable!("placeholder is skipped"),
            }
        }

        // Join the suspended awaiter tasks and check each call resolved to its recorded terminal.
        for (item, task) in tasks {
            let outcome = task
                .await
                .expect("awaiter task panicked")
                .unwrap_or_else(|e| panic!("seed {seed}: await of item {item} failed: {e}"));
            let kind = match items[item] {
                ItemKind::Call(kind) => kind,
                ItemKind::Scope => unreachable!("scopes are not awaited"),
            };
            match (kind, outcome) {
                (
                    CallKind::Completed,
                    ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }),
                ) => assert_eq!(
                    end_idx,
                    OplogIndex::from_u64(terminal_idx[item].unwrap()),
                    "seed {seed}: item {item} resolved to the wrong End index"
                ),
                (
                    CallKind::Cancelled,
                    ResolutionOutcome::Resolved(Resolution::Cancelled { cancelled_idx, .. }),
                ) => assert_eq!(
                    cancelled_idx,
                    OplogIndex::from_u64(terminal_idx[item].unwrap()),
                    "seed {seed}: item {item} resolved to the wrong Cancelled index"
                ),
                (CallKind::Incomplete, ResolutionOutcome::Incomplete) => {}
                (kind, other) => panic!(
                    "seed {seed}: item {item} (kind {kind:?}) resolved unexpectedly: {other:?}"
                ),
            }
        }

        assert!(
            rs.is_live(),
            "seed {seed}: replay did not reach live after the full walk"
        );
        let internal = rs.cursor.state.lock().await;
        for (i, &si) in start_idx.iter().enumerate() {
            if matches!(items[i], ItemKind::Call(_)) {
                assert!(
                    !internal
                        .concurrent_resolver
                        .is_pending(OplogIndex::from_u64(si)),
                    "seed {seed}: item {i} left a registered awaiter"
                );
            }
        }
    }
}

/// An `End` whose `Start` lies inside a deleted region (a jump/revert cut between the pair) is
/// an *orphan terminal*: the cursor must consume it transparently instead of surfacing it to a
/// positional reader, and a later call must still claim and resolve at its true indices.
#[test]
async fn orphan_end_with_deleted_start_is_skipped() {
    // [NoOp(1), Start(2), End(2→3), Start(4), End(4→5)] with deleted region [2, 2]: the End at
    // 3 is orphaned; the kept call (4, 5) must claim and resolve normally across it.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        noop(),
        start_now(),
        end_for(2, 1),
        start_now(),
        end_for(4, 2),
    ] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(2),
        end: OplogIndex::from_u64(2),
    }]);
    let rs = ReplayState::new(test_agent_id(), oplog, skipped)
        .await
        .expect("failed to build replay state");

    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(
        handle.start_idx(),
        OplogIndex::from_u64(4),
        "the claim must skip the orphan End and land on the kept Start"
    );
    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => {
            assert_eq!(end_idx, OplogIndex::from_u64(5))
        }
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.is_live());
}

/// A `Cancelled` orphan (its `Start` deleted) is consumed transparently by a positional drain,
/// bringing replay to live instead of erroring as an unexpected entry.
#[test]
async fn orphan_cancelled_with_deleted_start_is_skipped() {
    // [NoOp(1), Start(2), Cancelled(2→3)] with deleted region [2, 2].
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [noop(), start_now(), cancelled_for(2)] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(2),
        end: OplogIndex::from_u64(2),
    }]);
    let rs = ReplayState::new(test_agent_id(), oplog, skipped)
        .await
        .expect("failed to build replay state");

    let result = rs.try_get_oplog_entry(|_| false).await.unwrap();
    assert!(result.is_none());
    assert!(
        rs.is_live(),
        "the orphan Cancelled must be drained, reaching live"
    );
}

/// A positional reader (`get_oplog_entry`) skips an orphan terminal and returns the next real
/// entry instead of surfacing the orphan as an unexpected entry.
#[test]
async fn positional_reader_skips_orphan_terminal() {
    // [NoOp(1), Start(2), End(2→3), NoOp(4)] with deleted region [2, 2]: the positional read
    // must consume the orphan End at 3 and return the NoOp at 4.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [noop(), start_now(), end_for(2, 1), noop()] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(2),
        end: OplogIndex::from_u64(2),
    }]);
    let rs = ReplayState::new(test_agent_id(), oplog, skipped)
        .await
        .expect("failed to build replay state");

    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(4));
    assert!(matches!(entry, OplogEntry::NoOp { .. }));
    assert!(rs.is_live());
}

/// The inverse partial deletion — the `Start` kept, its terminal inside a deleted region —
/// reports `Incomplete` (the caller may re-execute the call), never an error or a hang.
#[test]
async fn deleted_terminal_reports_incomplete() {
    // [NoOp(1), Start(2), End(2→3)] with deleted region [3, 3].
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [noop(), start_now(), end_for(2, 1)] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(3),
        end: OplogIndex::from_u64(3),
    }]);
    let rs = ReplayState::new(test_agent_id(), oplog, skipped)
        .await
        .expect("failed to build replay state");

    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));
    match rs.await_resolution_outcome(handle).await.unwrap() {
        ResolutionOutcome::Incomplete => {}
        other => panic!("expected Incomplete, got {other:?}"),
    }
    assert!(rs.is_live());
}

/// How a generated call pair interacts with the deleted regions in
/// [`replay_skips_deleted_regions_fuzz`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Deletion {
    /// Both the `Start` and its terminal are kept.
    Kept,
    /// The whole pair lies inside a deleted region (a clean jump/revert cut).
    Pair,
    /// Only the `Start` is deleted: its terminal survives as an *orphan terminal* the cursor
    /// must skip transparently.
    StartOnly,
    /// Only the terminal is deleted: the kept `Start` must report `Incomplete`.
    TerminalOnly,
}

/// Seam 1, deleted/jump regions: a randomized generator that records a run of contiguous
/// call pairs — each terminating in an `End` or a `Cancelled` — and then marks, per pair,
/// either the whole pair, only its `Start`, or only its terminal as belonging to deleted
/// oplog regions (as a `Jump`/revert cutting at an arbitrary point would leave behind).
/// Deleted entries must be skipped by the replay cursor entirely — never claimed, never
/// read; orphan terminals (Start deleted, terminal kept) must be consumed transparently;
/// kept `Start`s whose terminal was deleted must report `Incomplete`; and fully kept calls
/// must still claim at their true indices and resolve to their recorded terminal. Deleting
/// a leading region exercises the construction-time jump; deleting a trailing region
/// exercises the jump-to-target transition into live. Seeds are fixed, so any failure
/// reproduces.
#[test]
async fn replay_skips_deleted_regions_fuzz() {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    const CASES: u64 = 500;

    for seed in 0..CASES {
        let mut rng = StdRng::seed_from_u64(seed);
        let num_calls = rng.random_range(1..=6usize);

        // Contiguous call pairs after the placeholder: [Start, terminal, Start, terminal, ...],
        // where each terminal is independently an `End` or a `Cancelled`.
        let mut entries = vec![noop()];
        let mut start_idx = Vec::with_capacity(num_calls);
        let mut terminal_idx = Vec::with_capacity(num_calls);
        let mut is_cancelled = Vec::with_capacity(num_calls);
        let mut deletion = Vec::with_capacity(num_calls);
        let mut nanos = 0u64;
        for _ in 0..num_calls {
            entries.push(start_now());
            let si = entries.len() as u64;
            let cancelled = rng.random_bool(0.3);
            if cancelled {
                entries.push(cancelled_for(si));
            } else {
                nanos += 1;
                entries.push(end_for(si, nanos));
            }
            let ti = entries.len() as u64;
            start_idx.push(si);
            terminal_idx.push(ti);
            is_cancelled.push(cancelled);
            deletion.push(match rng.random_range(0..10u32) {
                0..=3 => Deletion::Kept,
                4..=5 => Deletion::Pair,
                6..=7 => Deletion::StartOnly,
                _ => Deletion::TerminalOnly,
            });
        }

        // Coalesce the deleted entry indices into contiguous regions.
        let mut deleted_indices: std::collections::BTreeSet<u64> =
            std::collections::BTreeSet::new();
        for i in 0..num_calls {
            match deletion[i] {
                Deletion::Kept => {}
                Deletion::Pair => {
                    deleted_indices.insert(start_idx[i]);
                    deleted_indices.insert(terminal_idx[i]);
                }
                Deletion::StartOnly => {
                    deleted_indices.insert(start_idx[i]);
                }
                Deletion::TerminalOnly => {
                    deleted_indices.insert(terminal_idx[i]);
                }
            }
        }
        let mut regions = Vec::new();
        let mut run: Option<(u64, u64)> = None;
        for &idx in &deleted_indices {
            match run {
                Some((s, e)) if idx == e + 1 => run = Some((s, idx)),
                Some((s, e)) => {
                    regions.push((s, e));
                    run = Some((idx, idx));
                }
                None => run = Some((idx, idx)),
            }
        }
        if let Some((s, e)) = run {
            regions.push((s, e));
        }

        let oplog = Arc::new(InMemoryOplog::new());
        for entry in entries {
            oplog.add(entry).await;
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let skipped = DeletedRegions::from_regions(regions.iter().map(|&(s, e)| OplogRegion {
            start: OplogIndex::from_u64(s),
            end: OplogIndex::from_u64(e),
        }));
        let rs = ReplayState::new(test_agent_id(), oplog, skipped)
            .await
            .expect("failed to build replay state");

        // Claim only the calls whose `Start` is kept, in order; the cursor must jump over
        // every deleted region and transparently consume every orphan terminal.
        let mut handles = Vec::new();
        for i in 0..num_calls {
            if matches!(deletion[i], Deletion::Pair | Deletion::StartOnly) {
                continue;
            }
            let handle = rs
                .claim_concurrent_start(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                )
                .await
                .unwrap_or_else(|e| panic!("seed {seed}: claim of kept call {i} failed: {e}"));
            assert_eq!(
                handle.start_idx(),
                OplogIndex::from_u64(start_idx[i]),
                "seed {seed}: kept call {i} claimed a wrong (possibly deleted) Start"
            );
            handles.push((i, handle));
        }

        for (i, handle) in handles {
            match deletion[i] {
                Deletion::Kept => {
                    match rs.await_resolution(handle).await.unwrap_or_else(|e| {
                        panic!("seed {seed}: await of kept call {i} failed: {e}")
                    }) {
                        Resolution::Completed { end_idx: ti, .. } if !is_cancelled[i] => {
                            assert_eq!(
                                ti,
                                OplogIndex::from_u64(terminal_idx[i]),
                                "seed {seed}: kept call {i} resolved to the wrong End"
                            )
                        }
                        Resolution::Cancelled {
                            cancelled_idx: ti, ..
                        } if is_cancelled[i] => {
                            assert_eq!(
                                ti,
                                OplogIndex::from_u64(terminal_idx[i]),
                                "seed {seed}: kept call {i} resolved to the wrong Cancelled"
                            )
                        }
                        other => panic!(
                            "seed {seed}: kept call {i} (cancelled: {}) resolved to the wrong terminal kind: {other:?}",
                            is_cancelled[i]
                        ),
                    }
                }
                Deletion::TerminalOnly => {
                    match rs
                        .await_resolution_outcome(handle)
                        .await
                        .unwrap_or_else(|e| {
                            panic!("seed {seed}: await of terminal-deleted call {i} failed: {e}")
                        }) {
                        ResolutionOutcome::Incomplete => {}
                        other => panic!(
                            "seed {seed}: terminal-deleted call {i} expected Incomplete, got {other:?}"
                        ),
                    }
                }
                Deletion::Pair | Deletion::StartOnly => unreachable!(),
            }
        }

        // Any trailing orphan terminals (a Start-deleted call at the end of the layout) are
        // only consumed when something drives the cursor: drain and expect no real entry.
        let trailing = rs
            .try_get_oplog_entry(|_| false)
            .await
            .unwrap_or_else(|e| panic!("seed {seed}: final drain failed: {e}"));
        assert!(
            trailing.is_none(),
            "seed {seed}: final drain unexpectedly returned an entry: {trailing:?}"
        );

        assert!(
            rs.is_live(),
            "seed {seed}: replay did not reach live after skipping deleted regions"
        );
    }
}

/// Seam 1, persist-nothing zones: a randomized generator that wraps a contiguous block of call
/// pairs (and a `NoOp`) in a `ChangePersistenceLevel(PersistNothing)` … `ChangePersistenceLevel`
/// zone. Everything recorded inside the zone is observability-only and must be skipped by the
/// replay cursor (never claimed, never read), while the calls outside the zone claim at their
/// true indices and resolve. A leading zone exercises the construction-time skip. Seeds are
/// fixed, so any failure reproduces.
#[test]
async fn replay_skips_persist_nothing_zones_fuzz() {
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    const CASES: u64 = 500;

    for seed in 0..CASES {
        let mut rng = StdRng::seed_from_u64(seed);
        let before = rng.random_range(0..=2usize);
        let inside = rng.random_range(0..=2usize);
        let after = rng.random_range(0..=2usize);
        // Need at least one observable call so the assertions are meaningful.
        if before + after == 0 {
            continue;
        }

        let mut entries = vec![noop()];
        let mut kept_start = Vec::new();
        let mut kept_end = Vec::new();
        let mut nanos = 0u64;

        let push_call = |entries: &mut Vec<OplogEntry>, nanos: &mut u64| -> (u64, u64) {
            entries.push(start_now());
            let si = entries.len() as u64;
            *nanos += 1;
            entries.push(end_for(si, *nanos));
            (si, entries.len() as u64)
        };

        for _ in 0..before {
            let (si, ei) = push_call(&mut entries, &mut nanos);
            kept_start.push(si);
            kept_end.push(ei);
        }

        // Open the persist-nothing zone, record skipped filler, then close it.
        entries.push(change_persistence_nothing());
        entries.push(noop());
        for _ in 0..inside {
            push_call(&mut entries, &mut nanos);
        }
        entries.push(change_persistence_smart());

        for _ in 0..after {
            let (si, ei) = push_call(&mut entries, &mut nanos);
            kept_start.push(si);
            kept_end.push(ei);
        }

        let rs = replay_state_over(entries).await;

        let mut handles = Vec::new();
        for (k, &si) in kept_start.iter().enumerate() {
            let handle = rs
                .claim_concurrent_start(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                )
                .await
                .unwrap_or_else(|e| panic!("seed {seed}: claim of kept call {k} failed: {e}"));
            assert_eq!(
                handle.start_idx(),
                OplogIndex::from_u64(si),
                "seed {seed}: kept call {k} claimed a Start inside the persist-nothing zone"
            );
            handles.push((k, handle));
        }

        for (k, handle) in handles {
            match rs
                .await_resolution(handle)
                .await
                .unwrap_or_else(|e| panic!("seed {seed}: await of kept call {k} failed: {e}"))
            {
                Resolution::Completed { end_idx: ei, .. } => assert_eq!(
                    ei,
                    OplogIndex::from_u64(kept_end[k]),
                    "seed {seed}: kept call {k} resolved to the wrong End"
                ),
                other => panic!("seed {seed}: kept call {k} expected Completed, got {other:?}"),
            }
        }

        assert!(
            rs.is_live(),
            "seed {seed}: replay did not reach live after skipping the persist-nothing zone"
        );
    }
}

fn suspend() -> OplogEntry {
    OplogEntry::Suspend {
        timestamp: Timestamp::now_utc(),
    }
}

/// A batched-write scope `Start` exactly as `begin_function` records it: request-less,
/// top-level, `WriteRemoteBatched(None)`.
fn batched_scope_start() -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::Custom("<scope:batched-write>".to_string()),
        request: None,
        durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
    }
}

/// A batched-write scope `End` exactly as `end_function` records it: response-less,
/// `forced_commit: true`.
fn batched_scope_end(start_index: u64) -> OplogEntry {
    OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        response: None,
        forced_commit: true,
    }
}

/// A host-call `Start` nested in the batched-write scope at `parent`, exactly as the
/// sequential adapter records followup batched invocations:
/// `parent_start_index: Some(scope)`, `WriteRemoteBatched(Some(scope))`.
fn batched_child_start(parent: u64) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: Some(OplogIndex::from_u64(parent)),
        function_name: HostFunctionName::MonotonicClockNow,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::WriteRemoteBatched(Some(OplogIndex::from_u64(
            parent,
        ))),
    }
}

/// A representative oplog written by the sequential adapter, where every host call is an
/// *adjacent* `Start`/`End` pair appended atomically via `Oplog::add_pair`, must replay cleanly
/// through the concurrent resolver.
///
/// The fixture is synthesized in the exact shapes the sequential writers produced
/// (`OplogOps::add_completed_host_call`, `begin_function` / `end_function`), covering:
/// - plain adjacent host-call pairs,
/// - a hint entry (`Suspend`) between calls,
/// - an adjacent pair inside an atomic region (positional `Begin`/`EndAtomicRegion` markers),
/// - a batched-write scope (request-less scope `Start`, a child call pair recorded with
///   `parent_start_index: Some(scope)` / `WriteRemoteBatched(Some(scope))`, and the
///   response-less, forced-commit scope `End`),
/// - no `Cancelled` entries and no overlapping calls anywhere.
///
/// Replay drives the same claim/await sequence the sequential durability layer performs:
/// each call is claimed then awaited immediately, scope `End`s resolve through the
/// resolver, and positional markers are consumed by `get_oplog_entry`.
#[test]
async fn pre_migration_adjacent_pair_oplog_replays_through_concurrent_resolver() {
    // [ 1: NoOp,
    //   2: Start(A), 3: End(A=2, 41),
    //   4: Suspend (hint),
    //   5: Start(B), 6: End(B=5, 42),
    //   7: BeginAtomicRegion, 8: Start(C), 9: End(C=8, 43), 10: EndAtomicRegion(7),
    //   11: Start(scope), 12: Start(D, parent=11), 13: End(D=12, 44), 14: End(scope=11) ]
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 41),
        suspend(),
        start_now(),
        end_for(5, 42),
        begin_atomic_region(),
        start_now(),
        end_for(8, 43),
        end_atomic_region(7),
        batched_scope_start(),
        batched_child_start(11),
        end_for(12, 44),
        batched_scope_end(11),
    ])
    .await;

    // Call A: claim + immediate await, the sequential replay pattern. The recorded
    // response payload must round-trip through the resolution.
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_a.start_idx(), OplogIndex::from_u64(2));
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed {
            end_idx, response, ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
            match response {
                Some(OplogPayload::Inline(boxed)) => assert_eq!(
                    *boxed,
                    HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp {
                        nanos: 41
                    })
                ),
                other => panic!("expected inline response payload, got {other:?}"),
            }
        }
        other => panic!("expected Completed for A, got {other:?}"),
    }

    // Call B: the Suspend hint between the pairs is skipped transparently by the claim.
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(5));
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(6)),
        other => panic!("expected Completed for B, got {other:?}"),
    }

    // Atomic region markers are positional; call C replays inside the region.
    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(7));
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

    let handle_c = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_c.start_idx(), OplogIndex::from_u64(8));
    match rs.await_resolution(handle_c).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(9)),
        other => panic!("expected Completed for C, got {other:?}"),
    }

    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(10));
    assert!(
        matches!(entry, OplogEntry::EndAtomicRegion { begin_index, .. } if begin_index == OplogIndex::from_u64(7))
    );

    // Batched-write scope: scope Start claims through the resolver, the child call
    // is claimed by identity (parent_start_index), and the scope End resolves response-less.
    let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());
    let (scope_idx, scope_handle) = rs
        .claim_scope_start(&scope_name, &DurableFunctionType::WriteRemoteBatched(None))
        .await
        .unwrap();
    assert_eq!(scope_idx, OplogIndex::from_u64(11));

    let handle_d = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::WriteRemoteBatched(Some(OplogIndex::from_u64(11))),
            OplogIndex::from_u64(11),
        )
        .await
        .unwrap();
    assert_eq!(handle_d.start_idx(), OplogIndex::from_u64(12));
    match rs.await_resolution(handle_d).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(13)),
        other => panic!("expected Completed for D, got {other:?}"),
    }

    match rs.await_resolution_outcome(scope_handle).await.unwrap() {
        ResolutionOutcome::Resolved(Resolution::Completed {
            end_idx, response, ..
        }) => {
            assert_eq!(end_idx, OplogIndex::from_u64(14));
            assert!(response.is_none(), "scope End must be response-less");
        }
        other => panic!("expected Completed for the scope, got {other:?}"),
    }

    // The whole sequential oplog is consumed: replay is over and nothing is left pending.
    assert!(rs.is_live(), "replay must reach live at the end");
    let internal = rs.cursor.state.lock().await;
    assert!(
        !internal
            .concurrent_resolver
            .is_pending(OplogIndex::from_u64(2))
            && !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(5))
            && !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(8))
            && !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(11))
            && !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(12)),
        "no resolver awaiter may remain pending after a full replay"
    );
}

#[test]
async fn discriminated_scope_claim_never_matches_plain_scope_start() {
    // Scope claims match the expected name exactly: a discriminated claim
    // (`<scope:batched-write:DISC>`) must NOT claim a plain `<scope:batched-write>` Start —
    // there is no plain-name fallback, so a discriminated call can never steal a concurrent
    // plain sibling's recorded scope. The failed claim must not consume or claim anything:
    // the plain scope must still be claimable by its own exact name afterwards.
    let rs = replay_state_over(vec![noop(), batched_scope_start(), batched_scope_end(2)]).await;

    let discriminated =
        HostFunctionName::Custom("<scope:batched-write:consume-body:2>".to_string());
    let err = rs
        .claim_scope_start(
            &discriminated,
            &DurableFunctionType::WriteRemoteBatched(None),
        )
        .await
        .expect_err("discriminated claim must not match the plain scope Start");
    let message = format!("{err}");
    assert!(
        message.contains("no matching Start"),
        "unexpected error: {message}"
    );

    let plain = HostFunctionName::Custom("<scope:batched-write>".to_string());
    let (scope_idx, scope_handle) = rs
        .claim_scope_start(&plain, &DurableFunctionType::WriteRemoteBatched(None))
        .await
        .unwrap();
    assert_eq!(scope_idx, OplogIndex::from_u64(2));
    match rs.await_resolution_outcome(scope_handle).await.unwrap() {
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
        }
        other => panic!("expected Completed for the plain scope, got {other:?}"),
    }
    assert!(rs.is_live(), "replay must reach live at the end");
}

#[test]
async fn plain_scope_claim_never_matches_discriminated_scope_start() {
    // The inverse direction: a plain claim must not match a discriminated scope Start.
    let discriminated_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::Custom("<scope:batched-write:req:abc123>".to_string()),
        request: None,
        durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
    };
    let rs = replay_state_over(vec![noop(), discriminated_start, batched_scope_end(2)]).await;

    let plain = HostFunctionName::Custom("<scope:batched-write>".to_string());
    let err = rs
        .claim_scope_start(&plain, &DurableFunctionType::WriteRemoteBatched(None))
        .await
        .expect_err("plain claim must not match a discriminated scope Start");
    let message = format!("{err}");
    assert!(
        message.contains("no matching Start"),
        "unexpected error: {message}"
    );

    let discriminated = HostFunctionName::Custom("<scope:batched-write:req:abc123>".to_string());
    let (scope_idx, scope_handle) = rs
        .claim_scope_start(
            &discriminated,
            &DurableFunctionType::WriteRemoteBatched(None),
        )
        .await
        .unwrap();
    assert_eq!(scope_idx, OplogIndex::from_u64(2));
    match rs.await_resolution_outcome(scope_handle).await.unwrap() {
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
        }
        other => panic!("expected Completed for the discriminated scope, got {other:?}"),
    }
    assert!(rs.is_live(), "replay must reach live at the end");
}

/// Pins the exact "expected" label each [`StartClaim`] variant renders for
/// `unexpected_oplog_entry` claim errors, so diagnostic wording does not silently drift.
#[test]
fn start_claim_expected_descriptions_are_stable() {
    use super::claims::StartClaim;

    let name = HostFunctionName::MonotonicClockNow;
    let request = HostRequest::NoInput(HostRequestNoInput {});

    assert_eq!(
        StartClaim::any_unowned_call().expected_description(),
        "Start { request: Some(..), parent_start_index: None }"
    );

    assert_eq!(
        StartClaim::scope(&name, &DurableFunctionType::WriteRemoteBatched(None))
            .expected_description(),
        format!(
            "Start {{ {name}, WriteRemoteBatched(None), request: None, parent_start_index: None }}"
        )
    );

    assert_eq!(
        StartClaim::unowned(&name, &DurableFunctionType::ReadRemote).expected_description(),
        format!("Start {{ {name}, ReadRemote, request: Some(..), parent_start_index: None }}")
    );
    assert_eq!(
        StartClaim::unowned(
            &name,
            &DurableFunctionType::WriteRemoteBatched(Some(OplogIndex::from_u64(4)))
        )
        .expected_description(),
        format!(
            "Start {{ {name}, WriteRemoteBatched(Some(OplogIndex(4))), request: Some(..), parent_start_index: Some(OplogIndex(4)) }}"
        )
    );
    assert_eq!(
        StartClaim::unowned_matching_request(&name, &DurableFunctionType::ReadRemote, &request)
            .expected_description(),
        format!(
            "Start {{ {name}, ReadRemote, request: Some(<matching payload>), parent_start_index: None }}"
        )
    );

    assert_eq!(
        StartClaim::owned(
            &name,
            &DurableFunctionType::ReadRemote,
            OplogIndex::from_u64(7)
        )
        .expected_description(),
        format!("Start {{ {name}, ReadRemote, parent_start_index: Some(7) }}")
    );
    assert_eq!(
        StartClaim::owned_matching_request(
            &name,
            &DurableFunctionType::ReadRemote,
            OplogIndex::from_u64(7),
            &request
        )
        .expected_description(),
        format!(
            "Start {{ {name}, ReadRemote, request: Some(<matching payload>), parent_start_index: Some(7) }}"
        )
    );
}
