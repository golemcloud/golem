use super::*;
use crate::services::oplog::{CommitLevel, OplogAddReceipt, OrderedOplogStart, PendingUpload};
use async_trait::async_trait;
use golem_common::model::card::{
    AgentCardHolder, Card, CardHolder, CardId, InvocationWalletPin, StoredCard, WalletVersionToken,
};
use golem_common::model::component::ComponentId;
use golem_common::model::entity::EntityCallMode;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::TraceId;
use golem_common::model::oplog::payload::types::{
    SerializableP3HttpBodyChunk, SerializableP3HttpConsumeBodyResult, SerializableToolRpcError,
};
use golem_common::model::oplog::{
    AgentError, DurableFunctionType, DurableInvocationId, FilesystemSnapshotName, HostRequest,
    HostRequestGolemToolInvocationRejected, HostRequestMonotonicClockDuration, HostRequestNoInput,
    HostResponseMonotonicClockTimestamp, HostResponseP3HttpClientConsumeBodyChunk,
    HostResponseP3HttpClientConsumeBodyResult, HostStreamKind, OplogErrorKind, OplogPayload,
    PayloadId, RawOplogPayload,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::{AgentId, AgentInvocationPayload, IdempotencyKey, Timestamp};
use golem_common::schema::IntoTypedSchemaValue;
use golem_service_base::error::worker_executor::InterruptKind;
use std::collections::BTreeMap;
use std::time::Duration;
use test_r::test;

type StoredExternalPayload = (PayloadId, Vec<u8>, Vec<u8>);

/// Minimal in-memory `Oplog` used to drive a [`ReplayState`] over hand-built entries.
#[derive(Debug)]
struct InMemoryOplog {
    entries: std::sync::Mutex<Vec<OplogEntry>>,
    /// Every index served by `read_exact`, in order.
    reads: std::sync::Mutex<Vec<OplogIndex>>,
    external_payloads: tokio::sync::Mutex<Vec<StoredExternalPayload>>,
}

impl InMemoryOplog {
    fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(Vec::new()),
            reads: std::sync::Mutex::new(Vec::new()),
            external_payloads: tokio::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Oplog for InMemoryOplog {
    async fn add(
        &self,
        entry: OplogEntry,
    ) -> Result<OplogIndex, crate::services::oplog::OplogError> {
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry);
        Ok(OplogIndex::from_u64(entries.len() as u64))
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry);
        let index = OplogIndex::from_u64(entries.len() as u64);
        Box::pin(async move { Ok(index) })
    }

    fn enqueue_add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> crate::services::oplog::OplogAddPairReceipt {
        let mut entries = self.entries.lock().unwrap();
        entries.push(start);
        let first_idx = OplogIndex::from_u64(entries.len() as u64);
        entries.push(make_second(first_idx));
        let second_idx = OplogIndex::from_u64(entries.len() as u64);
        Box::pin(async move { Ok((first_idx, second_idx)) })
    }

    async fn add_start_with_reserved_raw_payload(
        &self,
        serialized_request: Vec<u8>,
        build_start: Box<dyn FnOnce(RawOplogPayload) -> Result<OplogEntry, String> + Send>,
    ) -> Result<OrderedOplogStart, crate::services::oplog::OplogError> {
        let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
        let index = self.add(entry.clone()).await?;
        Ok(OrderedOplogStart {
            index,
            entry,
            pending_upload: PendingUpload::already_durable(),
        })
    }

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: crate::services::oplog::IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, crate::services::oplog::OplogError> {
        let mut entries = self.entries.lock().unwrap();
        let index = OplogIndex::from_u64(entries.len() as u64 + 1);
        let (serialized_request, build_start) = build_request(index)?;
        let entry = build_start(RawOplogPayload::SerializedInline(serialized_request))?;
        entries.push(entry.clone());
        Ok(OrderedOplogStart {
            index,
            entry,
            pending_upload: PendingUpload::already_durable(),
        })
    }

    async fn drop_prefix(&self, _last_dropped_id: OplogIndex) -> u64 {
        0
    }

    async fn commit(
        &self,
        _level: CommitLevel,
    ) -> Result<BTreeMap<OplogIndex, OplogEntry>, crate::services::oplog::OplogError> {
        Ok(BTreeMap::new())
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        OplogIndex::from_u64(self.entries.lock().unwrap().len() as u64)
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }

    async fn wait_for_replicas(
        &self,
        _replicas: u8,
        _timeout: Duration,
    ) -> Result<bool, crate::services::oplog::OplogError> {
        Ok(true)
    }

    async fn read_exact(
        &self,
        oplog_index: OplogIndex,
        n: u64,
    ) -> BTreeMap<OplogIndex, OplogEntry> {
        let entries = self.entries.lock().unwrap();
        let start: u64 = oplog_index.into();
        let mut result = BTreeMap::new();
        for i in start..(start + n) {
            let entry = entries.get((i - 1) as usize).unwrap_or_else(|| {
                panic!(
                    "Missing oplog entry in exact range [{oplog_index}..={}]",
                    OplogIndex::from_u64(start + n - 1)
                )
            });
            result.insert(OplogIndex::from_u64(i), entry.clone());
            self.reads.lock().unwrap().push(OplogIndex::from_u64(i));
        }
        result
    }

    async fn length(&self) -> u64 {
        self.entries.lock().unwrap().len() as u64
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

async fn test_replay_state(
    owned_agent_id: OwnedAgentId,
    oplog: Arc<dyn Oplog>,
    skipped_regions: DeletedRegions,
    initial_snapshot_skip_end: Option<OplogIndex>,
) -> Result<ReplayState, WorkerExecutorError> {
    let horizon = oplog.current_oplog_index().await;
    ReplayState::new_for_owner(
        owned_agent_id,
        oplog,
        skipped_regions,
        initial_snapshot_skip_end,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        horizon,
        None,
    )
    .await
}

fn stored_test_card(card_id: CardId) -> StoredCard {
    StoredCard::Concrete(Card {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: Vec::new(),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::Utc::now(),
        expires_at: None,
        system_card: false,
        managed_by: None,
    })
}

fn invocation_started(wallet_pin: InvocationWalletPin) -> OplogEntry {
    OplogEntry::AgentInvocationStarted {
        timestamp: Timestamp::now_utc(),
        idempotency_key: IdempotencyKey::new("wallet-pin-replay".to_string()),
        payload: OplogPayload::Inline(Box::new(AgentInvocationPayload::SaveSnapshot)),
        trace_id: TraceId::generate(),
        trace_states: Vec::new(),
        invocation_context: Vec::new(),
        wallet_pin: Box::new(wallet_pin),
    }
}

fn start_now() -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::MonotonicClockNow,
        invocation_id: None,
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::ReadLocal,
        span_started: None,
    }
}

fn start_resolution() -> OplogEntry {
    start_named(HostFunctionName::MonotonicClockResolution)
}

/// A `ReadLocal` `Start` of `name` with a no-input request; the recorded kind is what matters
/// to the tests using it, not the durable function type the real host call would record.
fn start_named(name: HostFunctionName) -> OplogEntry {
    let mut entry = start_now();
    let OplogEntry::Start { function_name, .. } = &mut entry else {
        unreachable!();
    };
    *function_name = name;
    entry
}

fn rejected_tool_reconstruction_start(parent_start_index: OplogIndex) -> OplogEntry {
    let request =
        HostRequest::GolemToolInvocationRejected(HostRequestGolemToolInvocationRejected {
            attempt_ordinal: 0,
            tool_name: "reconstruction-test".to_string(),
            command_path: vec!["run".to_string()],
            input: None,
            input_decode_failure: None,
            has_stdin: false,
            has_stdout: false,
            has_stderr: false,
            call_mode: EntityCallMode::Synchronous,
            error: SerializableToolRpcError::Denied("recorded rejection".to_string()),
        });
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: Some(parent_start_index),
        function_name: HostFunctionName::GolemToolInvocationRejected,
        invocation_id: None,
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(request))),
        durable_function_type: DurableFunctionType::WriteLocal,
        span_started: None,
    }
}

async fn claim_rejected_tool_reconstruction(
    replay: &ReplayState,
    parent_start_index: OplogIndex,
) -> ReplayCallHandle {
    match replay
        .claim_start_or_replay_end(StartClaim::tool_invocation(
            &HostFunctionName::GolemEntityInvoke,
            &HostFunctionName::GolemToolInvocationRejected,
            &DurableFunctionType::WriteLocal,
            parent_start_index,
        ))
        .await
        .unwrap()
    {
        ReplayStartClaimOutcome::Claimed { handle, .. } => handle,
        ReplayStartClaimOutcome::ReplayEnded
        | ReplayStartClaimOutcome::DeletedRegion
        | ReplayStartClaimOutcome::StoreAlreadyLive => {
            panic!("expected rejected tool reconstruction Start")
        }
    }
}

/// Strict custom-root claim on behalf of a replaying Store, through the production
/// [`ReplayState::claim_custom_start_for_store`]: the tests here never expect a replay-end or
/// live-Store continuation, so those outcomes are failures.
async fn claim_custom_start(
    rs: &ReplayState,
    expected_function_name: &HostFunctionName,
    expected_function_type: &DurableFunctionType,
    expected_parent_start_index: Option<OplogIndex>,
    expected_invocation_id: DurableInvocationId,
) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
    match rs
        .claim_custom_start_for_store(
            expected_function_name,
            expected_function_type,
            expected_parent_start_index,
            expected_invocation_id,
            false,
        )
        .await?
    {
        CustomStartClaimOutcome::Claimed(claimed) => Ok(claimed),
        CustomStartClaimOutcome::ReplayEnded => {
            panic!("strict custom claim {expected_invocation_id} reached the replay end")
        }
        CustomStartClaimOutcome::StoreAlreadyLive => {
            panic!("strict custom claim {expected_invocation_id} was issued for a live Store")
        }
    }
}

fn custom_request(value: i32) -> HostRequest {
    HostRequest::Custom(value.into_typed_schema_value().unwrap())
}

fn test_invocation_id(ordinal: u64) -> DurableInvocationId {
    DurableInvocationId::new(OplogIndex::from_u64(1), ordinal)
}

fn custom_start(name: &str, value: i32, parent: Option<u64>, invocation_id: u64) -> OplogEntry {
    custom_start_with_request(name, custom_request(value), parent, invocation_id)
}

fn custom_start_with_request(
    name: &str,
    request: HostRequest,
    parent: Option<u64>,
    invocation_id: u64,
) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: parent.map(OplogIndex::from_u64),
        function_name: HostFunctionName::Custom(name.to_string()),
        invocation_id: Some(test_invocation_id(invocation_id)),
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(request))),
        durable_function_type: DurableFunctionType::ReadRemote,
        span_started: None,
    }
}

fn custom_start_without_invocation_id(
    name: &str,
    request: HostRequest,
    parent: Option<u64>,
) -> OplogEntry {
    let mut entry = custom_start_with_request(name, request, parent, 0);
    let OplogEntry::Start { invocation_id, .. } = &mut entry else {
        unreachable!();
    };
    *invocation_id = None;
    entry
}

fn custom_end(start_index: u64, value: i32) -> OplogEntry {
    OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        response: Some(OplogPayload::Inline(Box::new(HostResponse::Custom(
            value.into_typed_schema_value().unwrap(),
        )))),
        forced_commit: false,
        span_finished: None,
        span_attributes: None,
    }
}

fn observational_start_now(owner: u64, parent: Option<u64>) -> OplogEntry {
    let mut entry = start_now();
    let OplogEntry::Start {
        observational_owner,
        parent_start_index,
        ..
    } = &mut entry
    else {
        unreachable!();
    };
    *observational_owner = Some(OplogIndex::from_u64(owner));
    *parent_start_index = parent.map(OplogIndex::from_u64);
    entry
}

fn owned_start_now(parent: u64) -> OplogEntry {
    let mut entry = start_now();
    let OplogEntry::Start {
        parent_start_index, ..
    } = &mut entry
    else {
        unreachable!();
    };
    *parent_start_index = Some(OplogIndex::from_u64(parent));
    entry
}

fn stream_frame(parent: u64) -> OplogEntry {
    OplogEntry::HostStreamFrame {
        timestamp: Timestamp::now_utc(),
        parent_start_index: OplogIndex::from_u64(parent),
        kind: HostStreamKind::P3HttpRequestBody,
        payload: OplogPayload::Inline(Box::new(HostRequest::NoInput(HostRequestNoInput {}))),
    }
}

#[test]
async fn completed_custom_invocation_drains_nested_custom_subtree() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        custom_start("inner", 2, Some(2), 2),
        custom_end(3, 20),
        custom_end(2, 10),
    ])
    .await;

    let claimed = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();
    match rs.await_resolution_outcome(claimed.handle).await.unwrap() {
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. }) => {
            assert_eq!(end_idx, OplogIndex::from_u64(5));
        }
        other => panic!("expected completed custom subtree, got {other:?}"),
    }
    assert!(rs.is_live());
}

#[test]
async fn incomplete_custom_invocation_drains_completed_descendants_then_reexecutes_root() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        custom_start("inner", 2, Some(2), 2),
        custom_end(3, 20),
    ])
    .await;

    let claimed = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();
    assert_eq!(claimed.handle.start_idx(), OplogIndex::from_u64(2));
    assert!(matches!(
        rs.await_resolution_outcome(claimed.handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));
}

#[test]
async fn ordinary_claim_never_claims_observational_start() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        start_now(),
    ])
    .await;

    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();

    assert_eq!(handle.start_idx(), OplogIndex::from_u64(4));
}

#[test]
async fn custom_replay_skips_interleaved_observational_tree_without_stealing_sibling() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        start_now(),
        end_for(4, 42),
        end_for(3, 99),
        custom_end(2, 10),
    ])
    .await;

    let custom = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();
    let sibling = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(sibling.start_idx(), OplogIndex::from_u64(4));

    assert!(matches!(
        rs.await_resolution_outcome(sibling).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(5)
    ));
    assert!(matches!(
        rs.await_resolution_outcome(custom.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(7)
    ));
}

#[test]
async fn custom_replay_skips_nested_observational_calls_and_stream_frames_by_identity() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        owned_start_now(3),
        stream_frame(4),
        end_for(4, 42),
        end_for(3, 99),
        custom_end(2, 10),
    ])
    .await;

    let custom = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();

    assert!(matches!(
        rs.await_resolution_outcome(custom.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(8)
    ));
    assert!(rs.is_live());
}

#[test]
async fn outer_custom_replay_skips_observational_calls_owned_by_nested_custom_invocation() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        custom_start("nested", 2, Some(2), 2),
        observational_start_now(3, None),
        end_for(4, 42),
        custom_end(3, 20),
        custom_end(2, 10),
    ])
    .await;

    let outer = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();

    assert!(matches!(
        rs.await_resolution_outcome(outer.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(7)
    ));
    assert!(rs.is_live());
}

#[test]
async fn incomplete_observational_tree_does_not_block_custom_live_fallback() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
    ])
    .await;

    let custom = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();

    assert!(matches!(
        rs.await_resolution_outcome(custom.handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));
    assert!(rs.is_live());
}

#[test]
async fn delivered_observational_completion_does_not_block_custom_live_fallback() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        end_for(3, 42),
        delivered_for(3),
        observational_start_now(2, None),
    ])
    .await;

    let custom = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();

    assert!(matches!(
        rs.await_resolution_outcome(custom.handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));
    assert!(rs.is_live());
}

#[test]
async fn observational_call_finishing_after_custom_terminal_is_still_skipped() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        custom_end(2, 10),
        observational_start_now(2, None),
        end_for(4, 99),
        remove_retry_policy(),
    ])
    .await;

    let custom = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();
    assert!(matches!(
        rs.await_resolution_outcome(custom.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(3)
    ));

    let next = rs
        .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::RemoveRetryPolicy { .. }))
        .await
        .unwrap();
    assert_eq!(next.map(|(idx, _)| idx), Some(OplogIndex::from_u64(6)));
}

#[test]
async fn custom_replay_skips_observational_cancellation() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        cancelled_for(3),
        custom_end(2, 10),
    ])
    .await;

    let custom = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();

    assert!(matches!(
        rs.await_resolution_outcome(custom.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(5)
    ));
    assert!(rs.is_live());
}

#[test]
async fn custom_claim_matches_identical_generators_by_invocation_id_in_reverse_order() {
    let no_input: HostRequest = HostRequestNoInput {}.into();
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start_with_request("generator", no_input.clone(), None, 1),
        custom_start_with_request("generator", no_input.clone(), None, 2),
        custom_end(2, 10),
        custom_end(3, 20),
    ])
    .await;
    let name = HostFunctionName::Custom("generator".to_string());
    let second = claim_custom_start(
        &rs,
        &name,
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(2),
    )
    .await
    .unwrap();
    let first = claim_custom_start(
        &rs,
        &name,
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();
    assert_eq!(first.handle.start_idx(), OplogIndex::from_u64(2));
    assert_eq!(second.handle.start_idx(), OplogIndex::from_u64(3));
    assert!(matches!(
        rs.await_resolution_outcome(first.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(4)
    ));
    assert!(matches!(
        rs.await_resolution_outcome(second.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(5)
    ));
}

#[test]
async fn custom_claim_rejects_start_without_invocation_id() {
    let no_input: HostRequest = HostRequestNoInput {}.into();
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start_without_invocation_id("generator", no_input.clone(), None),
        custom_end(2, 10),
    ])
    .await;

    let result = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("generator".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await;
    let Err(err) = result else {
        panic!("custom replay must require a deterministic invocation ID");
    };
    assert!(format!("{err}").contains("no matching Start"));
}

#[test]
async fn custom_claim_never_claims_observational_start_with_same_invocation_id() {
    let request = custom_request(1);
    let mut observational = custom_start_with_request("operation", request.clone(), None, 2);
    let OplogEntry::Start {
        observational_owner,
        ..
    } = &mut observational
    else {
        unreachable!();
    };
    *observational_owner = Some(OplogIndex::from_u64(2));

    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("owner", 0, None, 1),
        observational,
        custom_start_with_request("operation", request.clone(), None, 2),
    ])
    .await;

    let claimed = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("operation".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(2),
    )
    .await
    .unwrap();

    assert_eq!(claimed.handle.start_idx(), OplogIndex::from_u64(4));
}

#[test]
async fn custom_claim_ignores_start_without_invocation_id_before_exact_match() {
    let request = custom_request(1);
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start_without_invocation_id("operation", request.clone(), None),
        custom_start_with_request("operation", request.clone(), None, 2),
        custom_end(2, 10),
        custom_end(3, 20),
    ])
    .await;

    let claimed = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("operation".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(2),
    )
    .await
    .unwrap();
    assert_eq!(claimed.handle.start_idx(), OplogIndex::from_u64(3));
}

#[test]
async fn custom_claim_rejects_wrong_metadata_for_exact_id() {
    let request = custom_request(1);
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start_with_request("different-operation", request.clone(), None, 2),
    ])
    .await;

    let result = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("operation".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(2),
    )
    .await;
    let Err(err) = result else {
        panic!("an exact invocation ID with divergent metadata must be rejected");
    };
    assert!(format!("{err}").contains("no matching Start"));
}

#[test]
async fn custom_claim_id_can_be_reused_after_replay_restart() {
    let entries = vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        custom_start("operation", 1, None, 1),
        custom_end(2, 10),
    ];
    let rs = replay_state_over(entries.clone()).await;
    let name = HostFunctionName::Custom("operation".to_string());
    let invocation_id = test_invocation_id(1);

    let claimed = claim_custom_start(
        &rs,
        &name,
        &DurableFunctionType::ReadRemote,
        None,
        invocation_id,
    )
    .await
    .unwrap();
    assert!(matches!(
        rs.await_resolution_outcome(claimed.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));

    drop(rs);
    let rs = replay_state_over(entries).await;
    let claimed_again = claim_custom_start(
        &rs,
        &name,
        &DurableFunctionType::ReadRemote,
        None,
        invocation_id,
    )
    .await
    .unwrap();
    assert_eq!(claimed_again.handle.start_idx(), OplogIndex::from_u64(2));
}

fn remove_retry_policy() -> OplogEntry {
    OplogEntry::RemoveRetryPolicy {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        name: "replay-policy".to_string(),
    }
}

fn anchored_remove_retry_policy(parent_start_index: u64) -> OplogEntry {
    OplogEntry::RemoveRetryPolicy {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: Some(OplogIndex::from_u64(parent_start_index)),
        name: "replay-policy".to_string(),
    }
}

fn anchored_error(entity_parent_start_index: u64, retry_from: u64) -> OplogEntry {
    OplogEntry::error(
        Some(OplogIndex::from_u64(entity_parent_start_index)),
        OplogErrorKind::Invocation,
        AgentError::TransientError("retry".to_string()),
        OplogIndex::from_u64(retry_from),
        None,
    )
}

fn end_for(start_index: u64, nanos: u64) -> OplogEntry {
    OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        response: Some(OplogPayload::Inline(Box::new(
            HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp { nanos }),
        ))),
        forced_commit: false,
        span_finished: None,
        span_attributes: None,
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
        invocation_id: None,
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: None,
    }
}

async fn replay_state_over(entries: Vec<OplogEntry>) -> ReplayState {
    replay_state_and_oplog_over(entries).await.0
}

async fn replay_state_and_oplog_over(
    entries: Vec<OplogEntry>,
) -> (ReplayState, Arc<InMemoryOplog>) {
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in entries {
        oplog.add(entry).await.unwrap();
    }
    let rs = test_replay_state(
        test_agent_id(),
        oplog.clone() as Arc<dyn Oplog>,
        DeletedRegions::default(),
        None,
    )
    .await
    .expect("failed to build replay state");
    (rs, oplog)
}

fn replay_linear_memory() -> crate::services::linear_memory::LinearMemoryTracker {
    crate::services::linear_memory::LinearMemoryTracker::new(
        2,
        2,
        golem_common::model::agent::AgentMode::Durable,
        true,
        Arc::new(crate::services::resource_limits::AtomicResourceEntry::new(
            0, 10, 0, 0, 0,
        )),
        Arc::new(std::sync::Mutex::new(
            crate::services::active_agents::MemoryGrant::inert(2),
        )),
        std::time::Instant::now(),
    )
}

async fn held_completed_reconstruction() -> (
    ReplayState,
    Arc<InMemoryOplog>,
    crate::durable_host::concurrent::HistoricalReconstruction,
) {
    let parent = OplogIndex::from_u64(1);
    let start = rejected_tool_reconstruction_start(parent);
    let oplog = Arc::new(InMemoryOplog::new());
    oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    oplog.add(start).await.unwrap();
    oplog.add(end_for(2, 1)).await.unwrap();
    let replay = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::default(),
        None,
    )
    .await
    .expect("failed to build replay state");
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let mut reconstruction = handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    assert!(matches!(
        replay.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    reconstruction.body_settled();
    (replay, oplog, reconstruction)
}

#[test]
async fn primary_publication_requires_completed_reconstruction_validation() {
    let (replay, _oplog, reconstruction) = held_completed_reconstruction().await;
    let target = replay.switch_cursor_to_live().await.unwrap();
    let memory = replay_linear_memory();

    let publication = replay
        .run_owned_cursor_op({
            let memory = memory.clone();
            move |state| async move {
                state
                    .with_tx(async |tx| Ok(tx.finish_primary_settling(target, &memory).await))
                    .await
            }
        })
        .await
        .unwrap();
    assert_eq!(
        publication,
        LivePublicationOutcome::ReconstructionClaimsActive
    );
    assert!(!replay.is_live_published());
    assert_eq!(memory.reconciliation_grant_bytes(1), 2);

    drop(reconstruction);
    assert!(matches!(
        replay
            .finish_settling_to_live(&memory, ReplayToLiveRole::PrimaryAgent, target)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
    assert!(replay.is_live_published());
    assert_eq!(memory.reconciliation_grant_bytes(1), 1);
}

#[test]
async fn growing_replay_target_revokes_published_live_state() {
    let oplog = Arc::new(InMemoryOplog::new());
    oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    let replay = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::default(),
        None,
    )
    .await
    .expect("failed to build replay state");
    assert!(replay.is_live_published());

    let new_target = oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    replay
        .set_replay_target(new_target)
        .await
        .expect("failed to grow replay target");

    assert!(replay.is_replay());
    assert!(
        !replay.is_live_published(),
        "resuming replay must revoke owner live publication"
    );
}

#[test]
async fn growing_replay_target_revokes_an_active_settling_transition() {
    let (replay, oplog, reconstruction) = held_completed_reconstruction().await;
    let linear_memory = replay_linear_memory();
    let transition = tokio::spawn({
        let replay = replay.clone();
        let linear_memory = linear_memory.clone();
        async move {
            replay
                .switch_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while replay.cursor.transition_phase.load(Ordering::Acquire)
            != ReplayTransitionPhase::Settling as u8
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("primary transition did not enter settling");

    let new_target = oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    replay
        .set_replay_target(new_target)
        .await
        .expect("failed to grow replay target while settling");
    drop(reconstruction);

    assert_eq!(
        transition.await.unwrap().unwrap(),
        ReplayToLiveOutcome::ReplayResumed
    );
    assert!(replay.is_replay());
    assert!(!replay.is_live_published());
    assert_eq!(
        linear_memory.reconciliation_grant_bytes(1),
        2,
        "a revoked settling waiter must not switch linear memory to live"
    );

    let (index, entry) = replay.get_oplog_entry(None).await.unwrap();
    assert_eq!(index, new_target);
    assert!(matches!(entry, OplogEntry::RecoverySucceeded { .. }));
    assert!(matches!(
        replay
            .switch_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
    assert_eq!(
        replay.take_new_replay_events(),
        vec![ReplayEvent::ReplayFinished],
        "target growth must discard the stale ReplayFinished event"
    );
}

#[test]
async fn replay_finished_is_withheld_while_reconstruction_settles() {
    let (replay, _oplog, reconstruction) = held_completed_reconstruction().await;
    let linear_memory = replay_linear_memory();
    let transition = tokio::spawn({
        let replay = replay.clone();
        let linear_memory = linear_memory.clone();
        async move {
            replay
                .switch_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while replay.cursor.transition_phase.load(Ordering::Acquire)
            != ReplayTransitionPhase::Settling as u8
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("primary transition did not enter settling");

    assert!(
        replay.take_new_replay_events().is_empty(),
        "ReplayFinished must not be consumable before reconstruction validation"
    );
    drop(reconstruction);

    assert!(matches!(
        transition.await.unwrap().unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
    assert!(
        replay.take_new_replay_events_for(false).is_empty(),
        "an entity must leave the owner's finalization marker for the primary Store"
    );
    assert_eq!(
        replay.take_new_replay_events(),
        vec![ReplayEvent::ReplayFinished]
    );
}

#[test]
async fn historical_revision_updates_remain_for_root_after_entity_drain() {
    let replay = replay_state_over(vec![remove_retry_policy(), remove_retry_policy()]).await;
    assert!(replay.is_replay());
    assert!(!replay.is_live_published());
    let update = ReplayEvent::UpdateReplayed {
        new_revision: golem_common::model::component::ComponentRevision::new(2).unwrap(),
    };
    let fork = ReplayEvent::ForkReplayed {
        new_phantom_id: uuid::Uuid::new_v4(),
    };
    replay
        .cursor
        .pending_replay_events
        .lock()
        .unwrap()
        .extend([update.clone(), fork.clone()]);

    assert_eq!(replay.take_new_replay_events_for(false), vec![fork]);
    assert!(replay.take_new_replay_events_for(false).is_empty());
    assert_eq!(replay.take_new_replay_events_for(true), vec![update]);
    assert!(replay.take_new_replay_events_for(true).is_empty());
    assert!(replay.is_replay());
}

#[test]
async fn primary_transition_releases_an_incomplete_reconstruction_fence() {
    let parent = OplogIndex::from_u64(1);
    let start = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
    ])
    .await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let start_index = handle.start_idx();
    let mut reconstruction = handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    let claims = replay.cursor.reconstruction_claims.clone();

    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        replay.switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent),
    )
    .await
    .expect("incomplete reconstruction deadlocked the primary transition")
    .unwrap();
    assert!(matches!(outcome, ReplayToLiveOutcome::Live { .. }));
    assert!(claims.active_fences().is_empty());
    assert_eq!(claims.active_bodies(), HashSet::from([start_index]));
    assert!(matches!(
        replay.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));

    reconstruction.body_settled();
    drop(reconstruction);
    assert!(claims.active_bodies().is_empty());
}

#[test]
async fn target_growth_does_not_misclassify_a_reconstruction_as_incomplete() {
    let parent = OplogIndex::from_u64(1);
    let first_start = rejected_tool_reconstruction_start(parent);
    let second_start = rejected_tool_reconstruction_start(parent);
    let oplog = Arc::new(InMemoryOplog::new());
    oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    oplog.add(first_start).await.unwrap();
    oplog.add(second_start).await.unwrap();
    oplog.add(end_for(3, 2)).await.unwrap();
    let replay = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::default(),
        None,
    )
    .await
    .expect("failed to build replay state");
    let mut first = claim_rejected_tool_reconstruction(&replay, parent).await;
    let mut first_reconstruction = first
        .take_historical_reconstruction()
        .expect("first reconstruction guard");
    let mut second = claim_rejected_tool_reconstruction(&replay, parent).await;
    let mut second_reconstruction = second
        .take_historical_reconstruction()
        .expect("second reconstruction guard");
    assert!(matches!(
        replay.await_resolution_outcome(second).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    second_reconstruction.body_settled();

    let transition = tokio::spawn({
        let replay = replay.clone();
        async move {
            replay
                .switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while replay.cursor.transition_phase.load(Ordering::Acquire)
            != ReplayTransitionPhase::Settling as u8
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("primary transition did not enter settling");
    tokio::task::yield_now().await;
    assert!(
        !transition.is_finished(),
        "the incomplete candidate bypassed the completed reconstruction fence"
    );

    let new_target = oplog.add(end_for(2, 1)).await.unwrap();
    replay
        .set_replay_target(new_target)
        .await
        .expect("failed to grow replay target with the missing terminal");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(1), transition)
            .await
            .expect("stale transition did not observe target growth")
            .unwrap()
            .unwrap(),
        ReplayToLiveOutcome::ReplayResumed
    );
    assert!(matches!(
        replay.await_resolution_outcome(first).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));

    first_reconstruction.body_settled();
    drop(first_reconstruction);
    drop(second_reconstruction);
}

#[test]
async fn concurrent_same_target_transitions_are_idempotent() {
    let (replay, _oplog, reconstruction) = held_completed_reconstruction().await;
    let first_memory = replay_linear_memory();
    let second_memory = replay_linear_memory();
    let first = tokio::spawn({
        let replay = replay.clone();
        let linear_memory = first_memory.clone();
        async move {
            replay
                .switch_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(1), async {
        while replay.cursor.transition_phase.load(Ordering::Acquire)
            != ReplayTransitionPhase::Settling as u8
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("first primary transition did not enter settling");
    let second = tokio::spawn({
        let replay = replay.clone();
        let linear_memory = second_memory.clone();
        async move {
            replay
                .switch_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
                .await
        }
    });

    drop(reconstruction);

    assert!(matches!(
        first.await.unwrap().unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
    assert!(matches!(
        second.await.unwrap().unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
    assert_eq!(first_memory.reconciliation_grant_bytes(1), 1);
    assert_eq!(second_memory.reconciliation_grant_bytes(1), 1);
    assert!(replay.is_live_published());
}

#[test]
async fn old_settler_cannot_publish_a_grown_target() {
    let (replay, oplog, reconstruction) = held_completed_reconstruction().await;
    let old_target = replay.switch_cursor_to_live().await.unwrap();
    let new_target = oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    replay
        .set_replay_target(new_target)
        .await
        .expect("failed to grow replay target while settling");
    let second_target = replay.switch_cursor_to_live().await.unwrap();
    assert_eq!(second_target, new_target);
    drop(reconstruction);

    let stale_memory = replay_linear_memory();
    let stale_publication = replay
        .run_owned_cursor_op({
            let stale_memory = stale_memory.clone();
            move |state| async move {
                state
                    .with_tx(async |tx| {
                        Ok(tx.finish_primary_settling(old_target, &stale_memory).await)
                    })
                    .await
            }
        })
        .await
        .unwrap();
    assert_eq!(stale_publication, LivePublicationOutcome::ReplayResumed);
    assert_eq!(stale_memory.reconciliation_grant_bytes(1), 2);

    let current_memory = replay_linear_memory();
    let current_publication = replay
        .run_owned_cursor_op({
            let current_memory = current_memory.clone();
            move |state| async move {
                state
                    .with_tx(async |tx| {
                        Ok(tx
                            .finish_primary_settling(second_target, &current_memory)
                            .await)
                    })
                    .await
            }
        })
        .await
        .unwrap();
    assert_eq!(current_publication, LivePublicationOutcome::Published);
    assert_eq!(current_memory.reconciliation_grant_bytes(1), 1);
    assert!(replay.is_live_published());
}

#[test]
async fn owner_failure_wins_when_reconstruction_barrier_is_already_empty() {
    let oplog = Arc::new(InMemoryOplog::new());
    oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    let owner_operations = crate::durable_host::tool::operation::OwnerToolOperations::new();
    let horizon = oplog.current_oplog_index().await;
    let replay = ReplayState::new_for_owner(
        test_agent_id(),
        oplog,
        DeletedRegions::default(),
        None,
        owner_operations.clone(),
        horizon,
        None,
    )
    .await
    .expect("failed to build replay state");
    owner_operations
        .select_owner_failure(
            crate::durable_host::tool::operation::OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("ready owner failure"),
            ),
        )
        .await;

    let error = replay
        .test_wait_for_reconstruction_fences()
        .await
        .expect_err("biased barrier must prefer a ready owner failure");
    assert!(error.to_string().contains("ready owner failure"));
}

#[test]
async fn owner_lifecycle_change_during_reconstruction_remains_an_interrupt() {
    let oplog = Arc::new(InMemoryOplog::new());
    oplog
        .add(OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .await
        .unwrap();
    let owner_operations = crate::durable_host::tool::operation::OwnerToolOperations::new();
    let horizon = oplog.current_oplog_index().await;
    let replay = ReplayState::new_for_owner(
        test_agent_id(),
        oplog,
        DeletedRegions::default(),
        None,
        owner_operations.clone(),
        horizon,
        None,
    )
    .await
    .expect("failed to build replay state");
    owner_operations
        .select_owner_failure(
            crate::durable_host::tool::operation::OwnerFailureWinner::Lifecycle(
                InterruptKind::Restart,
            ),
        )
        .await;

    let error = replay
        .test_wait_for_reconstruction_fences()
        .await
        .expect_err("owner lifecycle change must interrupt reconstruction");
    assert_eq!(
        error,
        WorkerExecutorError::Interrupted {
            kind: InterruptKind::Restart
        }
    );
}

#[test]
#[test_r::timeout("10s")]
async fn owner_restart_interrupts_claim_and_delivery_behind_an_unconsumed_marker() {
    use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};

    for already_failed in [false, true] {
        let oplog = Arc::new(InMemoryOplog::new());
        *oplog.entries.lock().unwrap() = vec![
            OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            },
            start_now(),
            start_now(),
            end_for(2, 42),
            end_for(3, 43),
            delivered_for(2),
            delivered_for(3),
            start_now(),
            end_for(8, 44),
        ];
        let owner = OwnerToolOperations::new();
        let horizon = oplog.current_oplog_index().await;
        let replay = ReplayState::new_for_owner(
            test_agent_id(),
            oplog,
            DeletedRegions::default(),
            None,
            owner.clone(),
            horizon,
            None,
        )
        .await
        .unwrap();
        for _ in 0..2 {
            let call = replay
                .claim_concurrent_start(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                )
                .await
                .unwrap();
            replay.await_resolution(call).await.unwrap();
        }
        if already_failed {
            owner
                .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
                .await;
        }
        let mut claim = Box::pin(replay.claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ));
        let mut delivery = Box::pin(
            replay.await_completion_delivery(OplogIndex::from_u64(3), OplogIndex::from_u64(7)),
        );
        if !already_failed {
            assert_pending(&mut claim, "claim beyond another call's delivery marker").await;
            assert!(
                tokio::time::timeout(Duration::from_millis(50), delivery.as_mut())
                    .await
                    .is_err(),
                "delivery must wait for the earlier call's marker"
            );
            owner
                .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
                .await;
        }
        assert!(matches!(
            claim.await,
            Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Restart
            })
        ));
        assert!(matches!(
            delivery.await,
            Err(WorkerExecutorError::Interrupted {
                kind: InterruptKind::Restart
            })
        ));
        assert_eq!(replay.last_replayed_index(), OplogIndex::from_u64(5));
    }
}

#[test]
#[test_r::timeout("10s")]
async fn owner_lifecycle_interrupts_scope_resolution_without_crossing_delivery() {
    use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};

    for kind in [
        InterruptKind::Restart,
        InterruptKind::Jump,
        InterruptKind::ShardLost,
    ] {
        for hold_gate in [false, true] {
            let oplog = Arc::new(InMemoryOplog::new());
            *oplog.entries.lock().unwrap() = vec![
                OplogEntry::RecoverySucceeded {
                    timestamp: Timestamp::now_utc(),
                },
                batched_scope_start(),
                start_now(),
                end_for(3, 43),
                delivered_for(3),
                batched_scope_end(2),
            ];
            let owner = OwnerToolOperations::new();
            let horizon = oplog.current_oplog_index().await;
            let replay = ReplayState::new_for_owner(
                test_agent_id(),
                oplog,
                DeletedRegions::default(),
                None,
                owner.clone(),
                horizon,
                None,
            )
            .await
            .unwrap();
            let (_, scope) = replay
                .claim_scope_start(
                    &HostFunctionName::Custom("<scope:batched-write>".to_string()),
                    &DurableFunctionType::WriteRemote,
                    None,
                )
                .await
                .unwrap();
            let sibling = replay
                .claim_concurrent_start(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                )
                .await
                .unwrap();
            replay.await_resolution(sibling).await.unwrap();
            let gate = if hold_gate {
                Some(replay.cursor.advance_gate.clone().lock_owned().await)
            } else {
                None
            };
            let mut resolution = Box::pin(replay.await_resolution_outcome(scope));
            assert_pending(&mut resolution, "scope End behind another call's delivery").await;
            owner
                .select_owner_failure(OwnerFailureWinner::Lifecycle(kind))
                .await;
            assert_eq!(
                resolution.await.unwrap_err(),
                WorkerExecutorError::Interrupted { kind }
            );
            assert_eq!(replay.last_replayed_index(), OplogIndex::from_u64(4));
            assert!(!replay.is_live());
            drop(gate);
        }
    }
}

#[test]
#[test_r::timeout("10s")]
async fn cancelled_owner_failure_unregister_keeps_historical_fence() {
    use crate::durable_host::tool::operation::{OwnerFailureWinner, OwnerToolOperations};

    let parent = OplogIndex::from_u64(1);
    let start = rejected_tool_reconstruction_start(parent);
    let oplog = Arc::new(InMemoryOplog::new());
    *oplog.entries.lock().unwrap() = vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
        remove_retry_policy(),
        end_for(2, 1),
        remove_retry_policy(),
    ];
    let owner = OwnerToolOperations::new();
    let horizon = oplog.current_oplog_index().await;
    let replay = ReplayState::new_for_owner(
        test_agent_id(),
        oplog,
        DeletedRegions::default(),
        None,
        owner.clone(),
        horizon,
        None,
    )
    .await
    .unwrap();
    let handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let start_index = handle.start_idx();
    let claims = replay.cursor.reconstruction_claims.clone();
    let mut resolution = Box::pin(replay.await_resolution_outcome(handle));
    assert_pending(
        &mut resolution,
        "historical resolution at positional blocker",
    )
    .await;
    let state = replay.cursor.state.lock().await;
    assert!(state.concurrent_resolver.is_awaited(start_index));
    owner
        .select_owner_failure(OwnerFailureWinner::Lifecycle(InterruptKind::Restart))
        .await;
    assert!(futures::poll!(resolution.as_mut()).is_pending());
    drop(resolution);
    assert_eq!(claims.active_bodies(), HashSet::from([start_index]));
    assert_eq!(claims.active_fences(), HashSet::from([start_index]));
    drop(state);
    replay.fence_owned_cursor_ops().await.unwrap();
    claims.wait_for_fences().await;
    replay.ensure_reconstruction_claims_empty().unwrap();
    assert!(
        !replay
            .cursor
            .state
            .lock()
            .await
            .concurrent_resolver
            .is_pending(start_index)
    );
    assert_eq!(replay.last_replayed_index(), start_index);
}

#[test]
async fn owner_failure_during_final_classification_prevents_live_publication() {
    let (replay, _oplog, reconstruction) = held_completed_reconstruction().await;
    let linear_memory = replay_linear_memory();
    let entered = Arc::new(tokio::sync::Barrier::new(2));
    let release = Arc::new(tokio::sync::Barrier::new(2));
    *replay.cursor.primary_publication_gate.lock().unwrap() =
        Some((entered.clone(), release.clone()));

    let transition = tokio::spawn({
        let replay = replay.clone();
        let linear_memory = linear_memory.clone();
        async move {
            replay
                .switch_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
                .await
        }
    });
    drop(reconstruction);
    entered.wait().await;

    replay
        .cursor
        .owner_tool_operations
        .select_owner_failure(
            crate::durable_host::tool::operation::OwnerFailureWinner::Infrastructure(
                WorkerExecutorError::runtime("owner failed before live publication"),
            ),
        )
        .await;
    release.wait().await;

    let error = transition
        .await
        .unwrap()
        .expect_err("owner failure must defeat final live publication");
    assert!(
        error
            .to_string()
            .contains("owner failed before live publication")
    );
    assert!(!replay.is_live_published());
    assert!(replay.take_new_replay_events().is_empty());
    assert_eq!(linear_memory.reconciliation_grant_bytes(1), 2);
}

#[test]
async fn permission_events_replay_after_invocation_wallet_pin() {
    let owned_agent_id = test_agent_id();
    let derived_card = stored_test_card(CardId::new());
    let wallet_pin = InvocationWalletPin {
        wallet_token: WalletVersionToken {
            wallet_id_hash: CardHolder::Agent(AgentCardHolder {
                agent_id: owned_agent_id.agent_id.clone(),
            })
            .wallet_id_hash(),
            generation: 0,
        },
        pinned_card_ids: Vec::new(),
        scope_card_id: Some(CardId::new()),
    };
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        invocation_started(wallet_pin.clone()),
        OplogEntry::CardDerived {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            card: Box::new(derived_card.clone()),
            wallet_generation: 0,
        },
        start_now(),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let replay_state = test_replay_state(owned_agent_id, oplog, DeletedRegions::default(), None)
        .await
        .expect("failed to build replay state");

    let invocation = replay_state
        .get_oplog_entry_agent_invocation_started()
        .await
        .expect("failed to replay invocation start")
        .expect("expected invocation start");
    assert_eq!(invocation.wallet_pin, wallet_pin.clone());
    assert_eq!(
        replay_state
            .pending_card_derivation(derived_card.card_id())
            .await,
        Some((derived_card.clone(), 0))
    );
    assert_eq!(
        replay_state.take_new_replay_events(),
        vec![
            ReplayEvent::InvocationWalletPinned {
                oplog_index: OplogIndex::from_u64(2),
                wallet_pin,
            },
            ReplayEvent::CardDerived {
                card: derived_card,
                wallet_generation: 0,
            },
        ]
    );
}

#[test]
async fn selected_snapshot_leaves_only_post_snapshot_invocation_pin_for_root() {
    let pin = InvocationWalletPin {
        wallet_token: WalletVersionToken {
            wallet_id_hash: [3; 32],
            generation: 7,
        },
        pinned_card_ids: Vec::new(),
        scope_card_id: None,
    };
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        invocation_started(pin.clone()),
        OplogEntry::Snapshot {
            timestamp: Timestamp::now_utc(),
            data: OplogPayload::Inline(Box::new(Vec::new())),
            mime_type: "application/octet-stream".to_string(),
            active_cards: Vec::new(),
            wallet_generation: 7,
            filesystem_snapshot: None,
        },
        invocation_started(pin.clone()),
        start_now(),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let replay = test_replay_state(
        test_agent_id(),
        oplog,
        DeletedRegions::from_regions([OplogRegion::from_range(2..=3)]),
        Some(OplogIndex::from_u64(3)),
    )
    .await
    .unwrap();
    let invocation = replay
        .get_oplog_entry_agent_invocation_started()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(invocation.wallet_pin, pin);
    assert!(replay.take_new_replay_events_for(false).is_empty());
    assert_eq!(
        replay.take_new_replay_events_for(true),
        vec![ReplayEvent::InvocationWalletPinned {
            oplog_index: OplogIndex::from_u64(4),
            wallet_pin: pin,
        }]
    );
    assert!(replay.take_new_replay_events_for(true).is_empty());
}

#[test]
async fn recorded_success_replays_without_live_expiry_or_authority_inputs() {
    let card_id = CardId::new();
    let mut card = stored_test_card(card_id);
    let StoredCard::Concrete(card_data) = &mut card else {
        unreachable!("the test fixture always creates a concrete card");
    };
    card_data.expires_at = Some(chrono::DateTime::UNIX_EPOCH);

    // ReplayState's complete input is this oplog. In particular, it has no clock, card service,
    // effective surface, or authorization callback. An already-expired card therefore remains a
    // recorded installation until a recorded removal is encountered, and cannot invalidate the
    // recorded successful operation that follows it.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        OplogEntry::CardInstalled {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            queued_event_index: None,
            card: Box::new(card.clone()),
            wallet_generation: 7,
        },
        custom_start("durable-operation", 41, None, 1),
        custom_end(3, 42),
    ])
    .await;

    let claimed = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("durable-operation".to_string()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .expect("recorded durable operation must be claimable");
    match rs
        .await_resolution_outcome(claimed.handle)
        .await
        .expect("recorded durable operation must resolve")
    {
        ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
            assert_eq!(
                response,
                Some(OplogPayload::Inline(Box::new(HostResponse::Custom(
                    42.into_typed_schema_value().unwrap(),
                ))))
            );
        }
        other => panic!("expected the recorded successful result, got {other:?}"),
    }

    assert!(rs.is_live());
    assert!(matches!(
        rs.switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
    assert_eq!(
        rs.take_new_replay_events(),
        vec![
            ReplayEvent::CardInstalled {
                card,
                wallet_generation: 7,
            },
            ReplayEvent::ReplayFinished,
        ],
        "replay must preserve recorded admission state and must not synthesize expiry"
    );
}

#[test]
async fn permission_events_are_recovered_from_skipped_regions() {
    let transfer_id = Uuid::new_v4();
    let source_card_id = CardId::new();
    let card = stored_test_card(CardId::new());
    let target_holder = CardHolder::Agent(AgentCardHolder {
        agent_id: test_agent_id().agent_id,
    });
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        OplogEntry::CardTransferred {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            transfer_id,
            source_card_id,
            installed_card_id: card.card_id(),
            target_holder: target_holder.clone(),
            card: Box::new(card.clone()),
            target_wallet_generation: 1,
        },
        start_now(),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion::from_range(2..=2)]);
    let replay_state = test_replay_state(test_agent_id(), oplog, skipped, None)
        .await
        .expect("failed to build replay state");

    assert_eq!(
        replay_state.take_new_replay_events(),
        vec![ReplayEvent::CardTransferred {
            transfer_id,
            source_card_id,
            installed_card_id: card.card_id(),
            target_holder,
            card,
            target_wallet_generation: 1,
        }]
    );
}

#[test]
async fn snapshot_prefix_suppresses_replayed_permission_events() {
    let card = stored_test_card(CardId::new());
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        OplogEntry::CardInstalled {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            queued_event_index: None,
            card: Box::new(card),
            wallet_generation: 1,
        },
        start_now(),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion::from_range(2..=2)]);
    let replay_state = test_replay_state(
        test_agent_id(),
        oplog,
        skipped,
        Some(OplogIndex::from_u64(2)),
    )
    .await
    .expect("failed to build replay state");

    assert!(replay_state.take_new_replay_events().is_empty());
}

#[test]
async fn snapshot_start_scans_recovery_metadata_only_after_the_snapshot() {
    // 1..=4: an attachment write and its observation, 5..=8: a delivered call and a guest
    // stream observation, 9: the snapshot. 10..=13 repeat the call and the observation.
    let mut entries = attachment_index_fixture(OplogPayload::Inline(Box::new(b"abc".to_vec())));
    entries.extend([
        start_now(),
        end_for(5, 40),
        delivered_for(5),
        consumer_observation(0, true, false),
        remove_retry_policy(),
        start_now(),
        end_for(10, 41),
        delivered_for(10),
        consumer_observation(1, true, false),
    ]);
    let snapshot = OplogIndex::from_u64(9);
    let skipped_prefix =
        |prefix_end: u64| DeletedRegions::from_regions([OplogRegion::from_range(2..=prefix_end)]);
    let build = async |skipped: DeletedRegions, snapshot_end: Option<OplogIndex>| {
        let oplog = Arc::new(InMemoryOplog::new());
        *oplog.entries.lock().unwrap() = entries.clone();
        let replay = test_replay_state(test_agent_id(), oplog.clone(), skipped, snapshot_end)
            .await
            .unwrap();
        let reads = oplog.reads.lock().unwrap().clone();
        (replay, reads)
    };
    let reads_prefix = |reads: &[OplogIndex]| {
        reads
            .iter()
            .any(|index| (2..=snapshot.as_u64()).contains(&index.as_u64()))
    };

    let (from_snapshot, reads) = build(skipped_prefix(9), Some(snapshot)).await;
    assert!(
        !reads_prefix(&reads),
        "a snapshot start read the replaced prefix: {reads:?}"
    );
    // Without a snapshot, and with a snapshot whose prefix is not fully skipped, the history
    // before it is replayed, so its metadata is scanned.
    let (without_snapshot, reads) = build(skipped_prefix(9), None).await;
    assert!(reads_prefix(&reads));
    let (uncovered, reads) = build(skipped_prefix(4), Some(snapshot)).await;
    assert!(reads_prefix(&reads));
    assert_eq!(
        *uncovered.cursor.completion_markers.lock().unwrap(),
        HashMap::from([
            (
                OplogIndex::from_u64(5),
                CompletionMarker::Delivered(OplogIndex::from_u64(7))
            ),
            (
                OplogIndex::from_u64(10),
                CompletionMarker::Delivered(OplogIndex::from_u64(12))
            ),
        ])
    );

    for replay in [&from_snapshot, &without_snapshot] {
        assert_eq!(
            *replay.cursor.completion_markers.lock().unwrap(),
            HashMap::from([(
                OplogIndex::from_u64(10),
                CompletionMarker::Delivered(OplogIndex::from_u64(12))
            )])
        );
        assert_eq!(
            *replay.cursor.stream_observations.lock().unwrap(),
            HashMap::from([(OplogIndex::from_u64(13), None)])
        );
        let attachments = replay.cursor.tool_attachment_reconstruction.lock().unwrap();
        assert!(attachments.accepted.is_empty());
        assert!(attachments.logical_observations.is_empty());
        assert!(attachments.pending_writes.is_empty());
    }
    {
        let attachments = from_snapshot
            .cursor
            .tool_attachment_reconstruction
            .lock()
            .unwrap();
        assert!(attachments.history.writes.is_empty());
        assert!(attachments.history.observations.is_empty());
        assert_eq!(
            attachments.starts.keys().copied().collect::<Vec<_>>(),
            vec![OplogIndex::from_u64(10)]
        );
    }

    let call = from_snapshot
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    match from_snapshot.await_resolution(call).await.unwrap() {
        Resolution::Completed {
            end_idx,
            delivery_marker,
            ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(11));
            assert_eq!(delivery_marker, Some(OplogIndex::from_u64(12)));
        }
        other => panic!("expected a completion delivered at its marker, got {other:?}"),
    }
}

fn stdout_log(message: &str) -> OplogEntry {
    OplogEntry::Log {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        level: LogLevel::Stdout,
        context: "stdout".to_string(),
        message: message.to_string(),
        trace_context: None,
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
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
async fn resolution_readiness_preserves_positional_entries_and_retained_tails() {
    for with_positional_entry in [false, true] {
        for completed in [false, true] {
            let mut entries = vec![
                OplogEntry::RecoverySucceeded {
                    timestamp: Timestamp::now_utc(),
                },
                start_now(),
            ];
            if with_positional_entry {
                entries.push(remove_retry_policy());
            }
            if completed {
                entries.push(end_for(2, 37));
                entries.push(remove_retry_policy()); // Ready terminal need not be the replay tail.
            }
            let rs = replay_state_over(entries).await;
            let handle = rs
                .claim_concurrent_start(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                )
                .await
                .unwrap();
            assert_eq!(
                rs.resolution_ready(&handle).await.unwrap(),
                !with_positional_entry
            );
            if with_positional_entry {
                assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(2));
                assert!(!rs.resolution_ready(&handle).await.unwrap());
                let (index, entry) = rs.get_oplog_entry(None).await.unwrap();
                assert_eq!(index, OplogIndex::from_u64(3));
                assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
                assert!(rs.resolution_ready(&handle).await.unwrap());
            }
            let outcome = rs.await_resolution_outcome(handle).await.unwrap();
            match outcome {
                ResolutionOutcome::Incomplete => assert!(!completed),
                ResolutionOutcome::Resolved(Resolution::Completed { response, .. }) => {
                    assert!(completed);
                    assert!(matches!(response, Some(OplogPayload::Inline(payload))
                        if matches!(*payload, HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp { nanos: 37 }))));
                    assert!(matches!(
                        rs.get_oplog_entry(None).await.unwrap().1,
                        OplogEntry::RemoveRetryPolicy { .. }
                    ));
                }
                other => panic!("unexpected resolution: {other:?}"),
            }
        }
    }
}

#[test]
async fn claim_and_await_resolves_completed() {
    // [RecoverySucceeded, Start, End]
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
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
async fn start_claim_reports_replay_ended_when_cursor_is_live() {
    let rs = replay_state_over(vec![OplogEntry::RecoverySucceeded {
        timestamp: Timestamp::now_utc(),
    }])
    .await;

    let outcome = rs
        .claim_start_or_replay_end(StartClaim::unowned(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ))
        .await
        .unwrap();

    assert!(matches!(outcome, ReplayStartClaimOutcome::ReplayEnded));
}

#[test]
#[test_r::timeout("30s")]
async fn accepted_start_notification_survives_cancelled_claim_waiter() {
    let entry = start_now();
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        entry.clone(),
        end_for(2, 42),
    ])
    .await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let lock = rs.cursor.state.lock().await;
    let mut claim = Box::pin(rs.claim_start_for_store_observed(
        StartClaim::unowned(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ),
        false,
        move |index, entry| {
            tx.send((index, entry.clone())).unwrap();
        },
    ));
    assert!(futures::poll!(claim.as_mut()).is_pending());
    drop(claim);
    assert!(rx.try_recv().is_err());
    drop(lock);
    let (index, recorded) = rx.recv().await.unwrap();
    assert_eq!(index, OplogIndex::from_u64(2));
    assert_eq!(recorded, entry);
    rs.fence_owned_cursor_ops().await.unwrap();
    assert!(rx.try_recv().is_err());
}

#[test]
async fn missing_start_claim_remains_divergence_while_replaying() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
    ])
    .await;

    let result = rs
        .claim_start_or_replay_end(StartClaim::unowned(
            &HostFunctionName::Custom("missing".to_string()),
            &DurableFunctionType::ReadLocal,
        ))
        .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("missing Start must not be accepted while replay remains active"),
    };

    assert!(
        format!("{error}").contains("missing"),
        "missing replay claim must remain strict divergence: {error}"
    );
}

#[test]
async fn start_claim_reports_matching_deleted_region_while_replay_continues() {
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(1),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion::from_range(2..=2)]);
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
        .await
        .unwrap();

    let outcome = rs
        .claim_start_or_replay_end(StartClaim::unowned(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ))
        .await
        .unwrap();

    assert!(matches!(outcome, ReplayStartClaimOutcome::DeletedRegion));
}

#[test]
async fn identity_claim_includes_replay_target_after_full_scan_chunk() {
    let mut entries = Vec::with_capacity(CHUNK_SIZE as usize + 2);
    entries.push(OplogEntry::RecoverySucceeded {
        timestamp: Timestamp::now_utc(),
    });
    entries.extend((0..CHUNK_SIZE).map(|_| OplogEntry::RecoverySucceeded {
        timestamp: Timestamp::now_utc(),
    }));
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
async fn typed_claim_mismatch_does_not_leak_pending() {
    // A typed claim whose expected type does not match the recorded Start must fail AND drop the
    // resolver receiver that the claim registered, so no stale awaiter leaks.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
    ])
    .await;
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
async fn predicate_probe_retains_unclaimed_start_and_drains_awaited_terminals() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — after claiming A, the cursor head
    // is the still-unclaimed, non-terminal Start(B). A read whose predicate fails must not steal
    // Start(B) (only B's owner may claim it) and must not park on it either (B's owner may need
    // the Store the reader holds): it retains B, keeps draining, routes End(A) to A's awaiter and
    // keeps End(B) with the retained Start for B's later claim.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
        OplogIndex::from_u64(5),
        "the probe must drain past the retained Start(B) and both terminals"
    );
    assert!(rs.has_unclaimed_retained_starts());
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            !internal.concurrent_resolver.is_pending(start_idx),
            "End(A) drained by the probe must resolve A's awaiter"
        );
        assert!(
            !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(3)),
            "the probe must not claim Start(B) on behalf of its owner"
        );
    }
    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }

    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn speculative_rollback_does_not_apply_side_effects() {
    // A speculative read whose predicate fails rolls the cursor back AND applies none of the
    // entry's commit-only side effects. A GolemApiFork `Start` records its index in
    // `pending_fork_starts` only when permanently consumed; a rolled-back read must not.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        fork_start(),
    ])
    .await;

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
    // [RecoverySucceeded, Start, Error{retry_from: Start}, End] — Error is a hint, skipped transparently.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        OplogEntry::error(
            None,
            OplogErrorKind::Invocation,
            AgentError::TransientError("boom".to_string()),
            OplogIndex::from_u64(2),
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
async fn snapshot_confirmed_hint_between_start_and_end_resolves() {
    // [RecoverySucceeded, Start, SnapshotConfirmed, End] — SnapshotConfirmed is a hint, skipped transparently.
    // A non-hint entry in that position would park the resolution, so the wait is bounded.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        OplogEntry::snapshot_confirmed(FilesystemSnapshotName::periodic()).rounded(),
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

    let resolution = tokio::time::timeout(Duration::from_secs(5), rs.await_resolution(handle))
        .await
        .expect("resolution must not block on a SnapshotConfirmed hint entry")
        .unwrap();
    match resolution {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn dangling_start_without_end_errors() {
    // [RecoverySucceeded, Start] — eager Start with no matching End/Cancelled (crash window).
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
    ])
    .await;
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
    // [RecoverySucceeded, Start] — same crash window as above, but via the outcome-returning API: the lone
    // committed Start (no End) must be reported as Incomplete (not an error), and the stale
    // resolver registration must be dropped so it cannot leak.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
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
    // [RecoverySucceeded, Start(2), End(2→3), RemoveRetryPolicy(4)]
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        remove_retry_policy(),
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
    assert_eq!(start_idx, OplogIndex::from_u64(2));
    drop(handle);

    // A later positional read must drain the abandoned call's End on the way to RemoveRetryPolicy(4).
    let consumed = rs
        .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::RemoveRetryPolicy { .. }))
        .await
        .unwrap();
    assert_eq!(
        consumed.map(|(idx, _)| idx),
        Some(OplogIndex::from_u64(4)),
        "the positional reader must see RemoveRetryPolicy(4), not the abandoned call's End"
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
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::ReadLocal,
            span_started: None,
        }
    }

    // [RecoverySucceeded, Start(A=2), Start(B=3, parent=2), End(B=3→4), End(A=2→5)]
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(B=3→4)] — a worker interrupted mid-call commits A's
    // `Start` but never its terminal, while a concurrent sibling B completed before the
    // interrupt. Replay must resolve B normally and report A as Incomplete (so A can be
    // re-executed live), not error out or misroute B's End to A.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(3, 42),
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
    // [RecoverySucceeded, Start, Cancelled { partial: None }] — a call dropped mid-flight live and
    // recorded as `Cancelled` with no partial result replays to a `Cancelled` resolution
    // carrying no payload. (The caller decides how to surface it; the accessor replay path
    // rejects it as an unexpected entry when a response is required.)
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        cancelled_for(2),
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
    // [RecoverySucceeded, Start, Cancelled { partial: Some(..) }] — a call cancelled live with a partial
    // result replays to a `Cancelled` resolution that preserves the recorded partial
    // response payload (the DurableCallSession replay path downloads and converts it).
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        cancelled_with_partial_for(2, 42),
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

fn delivered_for(start_index: u64) -> OplogEntry {
    OplogEntry::CompletionDelivered {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
    }
}

fn start_with_parent(parent_start_index: u64) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: Some(OplogIndex::from_u64(parent_start_index)),
        function_name: HostFunctionName::MonotonicClockNow,
        invocation_id: None,
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::ReadLocal,
        span_started: None,
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
    // [RecoverySucceeded, Start(2), End(2→3), AgentInvocationFinished(4)] — the durable call was issued
    // live but the replayed guest never re-issued it (an abandoned branch). At the invocation
    // boundary the never-claimed Start and its End are drained instead of failing the
    // positional read.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
async fn invocation_boundary_rejects_abandoned_delivered_completion() {
    // A CompletionDelivered marker proves the recorded guest received the call's result. If replay
    // reaches the invocation boundary without claiming that Start, this is divergence rather than
    // a tolerable live-only abandoned branch and must fail instead of parking forever at the marker.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        invocation_finished(),
    ])
    .await;

    let error = read_invocation_finished(&rs)
        .await
        .expect_err("an unclaimed delivered completion must be fatal");
    assert!(
        error.to_string().contains("recorded guest received"),
        "unexpected error: {error}"
    );
}

#[test]
async fn invocation_boundary_tolerates_abandoned_cancelled_start() {
    // Same as above but the abandoned call was closed by a `Cancelled` terminal.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, Start(2), Start(3, parent=2), End(3→4), End(2→5), AgentInvocationFinished(6)] —
    // an abandoned scope root with an abandoned child, both properly closed, is tolerated as
    // a structurally valid closed tail.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, Start(2), AgentInvocationFinished(3)] — a dangling abandoned Start with no
    // terminal before the finished marker stays fatal: the closed-tail structural validation
    // fails.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        invocation_finished(),
    ])
    .await;
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
    // [RecoverySucceeded, Start(2), End(2→3), End(2→4), AgentInvocationFinished(5)] — a second terminal
    // closing the same abandoned Start is corruption, not tolerated noise.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, End(7→2), AgentInvocationFinished(3)] — a terminal whose Start was never drained
    // as abandoned (and is not awaited/orphaned) is not tolerated; the positional read still
    // fails with the unexpected entry.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        end_for(7, 42),
        invocation_finished(),
    ])
    .await;
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
    // [RecoverySucceeded, RemoveRetryPolicy(2), AgentInvocationFinished(3)] — non-hint entries other than abandoned
    // durable-call records stay fatal on the walk to the finished marker (`RemoveRetryPolicy` is not a
    // hint entry).
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        remove_retry_policy(),
        invocation_finished(),
    ])
    .await;
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
    // [RecoverySucceeded, Start(2), End(2→3), AgentInvocationFinished(4)] with the Start claimed by a
    // concurrent replay call: the claim consumes the Start, the boundary walk drains the End
    // to the claim's resolver (awaited terminal), and the finished marker is read cleanly.
    // The claimed call still resolves as Completed — it is never miscounted as abandoned.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, Start(2), Start(3, parent=2), End(3→4), CompletionDiscarded(3),
    // End(2→6), AgentInvocationFinished(7)] with Start(2) claimed — the exact shape a
    // discarded response-body chunk leaves behind: the parent consume-body scope is claimed
    // by the replayed guest, but the guest dropped the body reader before the persisted
    // child chunk was delivered (the child's marker records the discard) and never demands
    // it again on replay. The boundary walk drains the abandoned child records, skips the
    // hint marker, routes the parent's awaited End to its claim, and reads the finished
    // marker cleanly.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, Start(2, parent=99), End(2→3), AgentInvocationFinished(4)] — an abandoned
    // Start whose parent lies outside the walked records (a claimed scope, or a region
    // deleted by a jump/revert) is treated as a root of the abandoned tail: the parent
    // linkage is informational, only the closed-tail structure is validated.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, Start(2), End(2→3), Cancelled(2→4), AgentInvocationFinished(5)] — a mixed
    // duplicate terminal (a `Cancelled` closing an abandoned Start already closed by an
    // `End`) is corruption, not tolerated noise.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, Start(2), End(2→3), End(2→4), AgentInvocationFinished(5)] with Start(2)
    // claimed and resolved before the boundary read: the first End resolves the claim, so
    // the second End targets a start that is neither awaited nor drained as abandoned — it
    // stays fatal on the walk to the finished marker instead of being normalized into the
    // abandoned tail.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    // [RecoverySucceeded, Start(2, GolemApiFork), End(2→3, Forked), AgentInvocationFinished(4)] — an
    // unclaimed legacy fork pair is a dedicated-positional-consumer record whose committed
    // consume is not inert: committing it would record a pending fork and decode the End
    // into a `ForkReplayed` event the replayed guest never requested. It must stay fatal
    // at the invocation boundary, and neither its commit-side state nor its replay event
    // may be applied.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
            span_finished: None,
            span_attributes: None,
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
    //   [RecoverySucceeded,
    //    Start(2, P3HttpClientConsumeBody, WriteRemote),                         — parent scope
    //    Start(3, P3HttpClientConsumeBodyChunk, WriteRemote,
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
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: None,
            function_name: HostFunctionName::P3HttpClientConsumeBody,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
        },
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(OplogIndex::from_u64(2)),
            function_name: HostFunctionName::P3HttpClientConsumeBodyChunk,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemote,
            span_started: None,
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
            span_finished: None,
            span_attributes: None,
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
            span_finished: None,
            span_attributes: None,
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
    // [RecoverySucceeded, Start, End, CompletionDiscarded] — the End was persisted live but its response
    // was never delivered to the guest (the marker records the discard), so replay must
    // resolve the call as CompletedButDiscarded, carrying the recorded response so deferred
    // replay can perform the recorded post-`End` continuation before parking.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        discarded_for(2),
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
async fn marked_completion_is_prefetched_without_advancing_past_intervening_entry() {
    // The call completed only after another durable operation was recorded. Replay must expose the
    // host result without consuming that intervening operation, otherwise a guest scheduler that
    // waits for the call's readiness before reproducing the operation deadlocks. Guest delivery is
    // still held at the marker after both the intervening entry and the End are consumed.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        remove_retry_policy(),
        end_for(2, 42),
        delivered_for(2),
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
        Resolution::Completed {
            end_idx,
            delivery_marker,
            ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(4));
            assert_eq!(delivery_marker, Some(OplogIndex::from_u64(5)));
        }
        other => panic!("expected marked completion, got {other:?}"),
    }
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(2),
        "prefetching the End must not advance the positional cursor"
    );

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(5))
        .await
        .unwrap();
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    barrier.acknowledge();
}

#[test]
#[test_r::timeout("10s")]
async fn host_internal_end_does_not_wait_for_accessor_continuation() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 41),
        start_now(),
        end_for(4, 73),
        remove_retry_policy(),
    ])
    .await;
    let internal = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let (resolved, resolution_ready) = tokio::sync::oneshot::channel();
    let continuation = rs.run_owned_cursor_op(move |state| async move {
        let result = state.await_resolution_outcome(internal).await;
        resolved.send(()).unwrap();
        result
    });
    tokio::pin!(continuation);
    let continuation_result = futures::poll!(continuation.as_mut());
    resolution_ready.await.unwrap();

    // Model a direct host call holding the Store. The accessor continuation remains unpolled
    // until this reader finishes; nothing may require that continuation to acknowledge End.
    let direct = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert!(matches!(
        rs.await_resolution(direct).await.unwrap(),
        Resolution::Completed { end_idx, delivery_marker: None, .. }
            if end_idx == OplogIndex::from_u64(5)
    ));
    let continuation_result = match continuation_result {
        std::task::Poll::Ready(result) => result,
        std::task::Poll::Pending => continuation.await,
    };
    assert!(matches!(
        continuation_result.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed {
            end_idx, delivery_marker: None, ..
        }) if end_idx == OplogIndex::from_u64(3)
    ));
}

#[test]
async fn replay_delivery_marker_holds_cursor_until_guest_boundary() {
    // A completed before B was started, but A's callback was handed to the guest only after B
    // completed. Replay returns A's response to its host-side continuation at End, lets B advance,
    // then positionally consumes A's delivery marker. Once consumed, no later entry can advance
    // until A's actual guest-facing boundary acknowledges the transferred cursor gate.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        start_now(),
        end_for(4, 43),
        delivered_for(2),
        remove_retry_policy(),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed {
            end_idx,
            delivery_marker,
            ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
            assert_eq!(delivery_marker, Some(OplogIndex::from_u64(6)));
        }
        other => panic!("expected A to complete for host-side continuation, got {other:?}"),
    }
    // A's awaiter drained past B's still-unclaimed Start (retained for B's owner together with
    // its End) and parked at A's delivery marker.
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    assert!(rs.has_unclaimed_retained_starts());

    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected B to complete, got {other:?}"),
    }
    let next = rs.get_oplog_entry(None);
    tokio::pin!(next);
    assert!(
        futures::poll!(next.as_mut()).is_pending(),
        "an unrelated positional reader must not steal A's delivery marker"
    );
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));

    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(6))
        .await
        .unwrap();
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(6));
    assert_eq!(
        rs.last_replayed_non_hint_index(),
        OplogIndex::from_u64(5),
        "the delivery scheduling barrier remains a hint"
    );

    assert!(
        futures::poll!(next.as_mut()).is_pending(),
        "later cursor work must wait until the recorded guest boundary"
    );
    barrier.acknowledge();
    let (idx, _) = next.await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(7));
}

fn consumer_observation(reader_slot: u32, guest: bool, terminal: bool) -> OplogEntry {
    use golem_common::model::durable_stream::{
        LocalStreamReaderId, STREAM_CONSUMER_RECORD_FORMAT_VERSION, StreamConsumerItemValueRecord,
        StreamConsumerObservation, StreamConsumerTerminal, StreamConsumerTerminalRecord,
        StreamEndResult, StreamOffset, StreamRegistrationInvocation, StreamSessionRecord,
    };
    let observation = if guest {
        StreamConsumerObservation::Guest {
            parent_start_index: None,
        }
    } else {
        StreamConsumerObservation::Host
    };
    let session_key =
        StreamRegistrationInvocation::Local(IdempotencyKey::new("observations".to_string()));
    let reader_id = LocalStreamReaderId {
        introducing_oplog_index: OplogIndex::from_u64(1),
        binding_slot: reader_slot,
    };
    let source_offset = StreamOffset::new(OplogIndex::from_u64(10), 0);
    let record = if terminal {
        StreamSessionRecord::ConsumerTerminal(StreamConsumerTerminalRecord {
            reobserves: None,
            format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
            observation,
            session_key,
            reader_id,
            source_offset,
            consumer_read_ordinal: 1,
            terminal: StreamConsumerTerminal::End(StreamEndResult::Ok),
        })
    } else {
        StreamSessionRecord::ConsumerItemValue(StreamConsumerItemValueRecord {
            reobserves: None,
            format_version: STREAM_CONSUMER_RECORD_FORMAT_VERSION,
            observation,
            session_key,
            reader_id,
            source_offset,
            consumer_read_ordinal: 0,
            value: vec![42],
            packed_u8: true,
            recursive_mappings: vec![],
        })
    };
    let summary = golem_common::model::oplog::DurableStreamEventSummary::session(&record);
    OplogEntry::stream_session(None, OplogPayload::Inline(Box::new(record)), summary)
}

fn attachment_terminal_observation(guest: bool, parent: Option<OplogIndex>) -> OplogEntry {
    use golem_common::model::durable_stream::StreamConsumerObservation;
    use golem_common::model::oplog::attachment::{
        ToolAttachmentId, ToolAttachmentReadObservation, ToolAttachmentReadOutcome,
        ToolAttachmentSlot,
    };
    OplogEntry::ToolAttachmentRead {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: parent,
        observation: Box::new(ToolAttachmentReadObservation {
            attachment: ToolAttachmentId {
                creation_start: OplogIndex::INITIAL,
                slot: ToolAttachmentSlot::Stdout,
            },
            logical_invocation: IdempotencyKey::new("attachment-observation".into()),
            observation: if guest {
                StreamConsumerObservation::Guest {
                    parent_start_index: parent,
                }
            } else {
                StreamConsumerObservation::Host
            },
            ordinal: 0,
            read_attempt_ordinal: 2,
            outcome: ToolAttachmentReadOutcome::Eof {
                terminal_index: OplogIndex::from_u64(3),
            },
            reobserves: None,
        }),
        payload: None,
    }
}

fn attachment_index_fixture(payload: OplogPayload<Vec<u8>>) -> Vec<OplogEntry> {
    use golem_common::model::oplog::attachment::{
        AcceptedAttachmentWriteEnd, ToolAttachmentReadOutcome,
    };
    use golem_common::model::oplog::payload::tool_attachment::{
        ToolAttachmentWriteOutcome, ToolAttachmentWriteRequest,
    };
    use golem_common::model::oplog::{
        HostRequestToolAttachmentWrite, HostResponseToolAttachmentWrite,
    };
    let mut read = attachment_terminal_observation(true, None);
    let OplogEntry::ToolAttachmentRead {
        observation,
        payload: body,
        ..
    } = &mut read
    else {
        unreachable!()
    };
    observation.outcome =
        ToolAttachmentReadOutcome::TypedChunk(AcceptedAttachmentWriteEnd(OplogIndex::from_u64(3)));
    *body = Some(payload);
    let request = HostRequestToolAttachmentWrite {
        request: ToolAttachmentWriteRequest::new(observation.attachment, 0, b"abc"),
    };
    let mut start = start_named(HostFunctionName::GolemToolOutputWrite);
    let OplogEntry::Start {
        request: recorded, ..
    } = &mut start
    else {
        unreachable!()
    };
    *recorded = Some(OplogPayload::Inline(Box::new(request.into())));
    let mut end = end_for(2, 0);
    let OplogEntry::End { response, .. } = &mut end else {
        unreachable!()
    };
    *response = Some(OplogPayload::Inline(Box::new(
        HostResponseToolAttachmentWrite {
            outcome: ToolAttachmentWriteOutcome::Accepted,
        }
        .into(),
    )));
    vec![start_now(), start, end, read]
}

#[test]
async fn attachment_index_excludes_custom_owned_effects_but_preserves_ordinary_reconstruction() {
    use golem_common::model::oplog::HostRequestToolAttachmentWrite;
    use golem_common::model::oplog::attachment::{
        AcceptedAttachmentWriteEnd, ToolAttachmentId, ToolAttachmentReadOutcome, ToolAttachmentSlot,
    };
    use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentWriteRequest;

    for completed_custom in [false, true] {
        for nested_tool in [false, true] {
            let fixture = attachment_index_fixture(OplogPayload::Inline(Box::new(b"abc".to_vec())));
            let attachment = ToolAttachmentId {
                creation_start: OplogIndex::INITIAL,
                slot: ToolAttachmentSlot::Stdout,
            };
            let custom_request = ToolAttachmentWriteRequest::new(attachment, 0, b"abc");
            let ordinary_request = ToolAttachmentWriteRequest::new(attachment, 1, b"abc");
            let mut tool = start_now();
            let OplogEntry::Start {
                parent_start_index,
                observational_owner,
                ..
            } = &mut tool
            else {
                unreachable!()
            };
            *parent_start_index = Some(OplogIndex::INITIAL);
            *observational_owner = nested_tool.then(|| OplogIndex::from_u64(2));
            let mut custom_write = fixture[1].clone();
            let OplogEntry::Start {
                parent_start_index,
                observational_owner,
                ..
            } = &mut custom_write
            else {
                unreachable!()
            };
            *parent_start_index = Some(OplogIndex::from_u64(3));
            *observational_owner = (!nested_tool).then(|| OplogIndex::from_u64(2));
            let mut custom_accept = fixture[2].clone();
            let OplogEntry::End { start_index, .. } = &mut custom_accept else {
                unreachable!()
            };
            *start_index = OplogIndex::from_u64(4);
            let mut read = fixture[3].clone();
            let OplogEntry::ToolAttachmentRead { observation, .. } = &mut read else {
                unreachable!()
            };
            observation.outcome = ToolAttachmentReadOutcome::TypedChunk(
                AcceptedAttachmentWriteEnd(OplogIndex::from_u64(5)),
            );
            let mut ordinary_write = fixture[1].clone();
            let OplogEntry::Start {
                parent_start_index,
                request,
                ..
            } = &mut ordinary_write
            else {
                unreachable!()
            };
            *parent_start_index = Some(OplogIndex::from_u64(8));
            *request = Some(OplogPayload::Inline(Box::new(
                HostRequestToolAttachmentWrite {
                    request: ordinary_request.clone(),
                }
                .into(),
            )));
            let mut ordinary_accept = fixture[2].clone();
            let OplogEntry::End { start_index, .. } = &mut ordinary_accept else {
                unreachable!()
            };
            *start_index = OplogIndex::from_u64(9);
            let replay = replay_state_over(vec![
                OplogEntry::RecoverySucceeded {
                    timestamp: Timestamp::now_utc(),
                },
                custom_start("custom", 0, Some(1), 1),
                tool,
                custom_write,
                custom_accept,
                read,
                if completed_custom {
                    custom_end(2, 0)
                } else {
                    remove_retry_policy()
                },
                custom_start_without_invocation_id(
                    "ordinary-scope",
                    HostRequest::NoInput(HostRequestNoInput {}),
                    Some(1),
                ),
                ordinary_write,
                ordinary_accept,
            ])
            .await;

            // Rebuilding an extended target must preserve the same ownership classification.
            replay
                .set_replay_target(OplogIndex::from_u64(3))
                .await
                .unwrap();
            replay
                .set_replay_target(OplogIndex::from_u64(10))
                .await
                .unwrap();
            assert!(
                replay
                    .historical_tool_attachment_write(&custom_request)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(
                replay
                    .historical_tool_attachment_write(&ordinary_request)
                    .unwrap()
                    .unwrap()
                    .start,
                OplogIndex::from_u64(9)
            );
            assert_eq!(
                replay
                    .hydrate_tool_attachment_observation(OplogIndex::from_u64(6), 3)
                    .await
                    .unwrap()
                    .2,
                Some(b"abc".to_vec())
            );
            assert!(replay.has_open_tool_attachments());
            let fence = replay.await_tool_attachment_reconstruction(OplogIndex::INITIAL);
            tokio::pin!(fence);
            assert!(futures::poll!(fence.as_mut()).is_pending());
            replay
                .complete_tool_attachment_reconstruction(OplogIndex::from_u64(9))
                .unwrap();
            assert!(matches!(
                futures::poll!(fence.as_mut()),
                std::task::Poll::Ready(Ok(()))
            ));
            assert!(!replay.has_open_tool_attachments());
        }
    }
}

fn attachment_terminal_prefix() -> Vec<OplogEntry> {
    use golem_common::model::oplog::attachment::{
        ByteStreamCloseCause, ToolAttachmentId, ToolAttachmentSlot, ToolAttachmentTerminal,
    };
    use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentTerminalOutcome;
    use golem_common::model::oplog::{
        HostRequestToolAttachmentFinish, HostResponseToolAttachmentTerminalResult,
    };
    let mut start = start_named(HostFunctionName::GolemToolOutputFinish);
    let OplogEntry::Start { request, .. } = &mut start else {
        unreachable!()
    };
    *request = Some(OplogPayload::Inline(Box::new(
        HostRequestToolAttachmentFinish {
            attachment: ToolAttachmentId {
                creation_start: OplogIndex::INITIAL,
                slot: ToolAttachmentSlot::Stdout,
            },
            operation_ordinal: 1,
        }
        .into(),
    )));
    let mut end = end_for(2, 0);
    let OplogEntry::End { response, .. } = &mut end else {
        unreachable!()
    };
    *response = Some(OplogPayload::Inline(Box::new(
        HostResponseToolAttachmentTerminalResult {
            outcome: ToolAttachmentTerminalOutcome::Selected(ToolAttachmentTerminal::Application(
                ByteStreamCloseCause::Finished,
            )),
        }
        .into(),
    )));
    vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
        end,
    ]
}

#[test]
async fn attachment_index_hydrates_external_body_only_on_demand() {
    use golem_common::model::oplog::attachment::{ToolAttachmentId, ToolAttachmentSlot};
    use golem_common::model::oplog::payload::tool_attachment::ToolAttachmentWriteRequest;

    let oplog = Arc::new(InMemoryOplog::new());
    let id = PayloadId::new();
    let hash = vec![1];
    *oplog.entries.lock().unwrap() = attachment_index_fixture(OplogPayload::External {
        payload_id: id.clone(),
        md5_hash: hash.clone(),
        cached: None,
    });
    // The body is deliberately unavailable during indexing.
    let replay = test_replay_state(test_agent_id(), oplog.clone(), DeletedRegions::new(), None)
        .await
        .unwrap();
    let request = ToolAttachmentWriteRequest::new(
        ToolAttachmentId {
            creation_start: OplogIndex::INITIAL,
            slot: ToolAttachmentSlot::Stdout,
        },
        0,
        b"abc",
    );
    let backing = replay
        .historical_tool_attachment_write(&request)
        .unwrap()
        .unwrap();
    assert_eq!(backing.start, OplogIndex::from_u64(2));
    assert_eq!(backing.end.0, OplogIndex::from_u64(3));
    assert_eq!(backing.original_typed_read, Some(OplogIndex::from_u64(4)));
    assert!(replay.last_replayed_index() < backing.end.0);
    assert!(
        replay
            .historical_tool_attachment_write(&ToolAttachmentWriteRequest::new(
                request.attachment,
                0,
                b"abd"
            ))
            .is_err()
    );
    replay
        .validate_tool_attachment_regenerated_bytes(
            golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd(
                OplogIndex::from_u64(3),
            ),
            b"abc",
        )
        .await
        .expect("typed regeneration must not download a consumer payload");
    assert!(
        replay
            .hydrate_tool_attachment_observation(OplogIndex::from_u64(4), 3)
            .await
            .is_err()
    );
    oplog.external_payloads.lock().await.push((
        id.clone(),
        hash.clone(),
        golem_common::serialization::serialize(&b"abd".to_vec()).unwrap(),
    ));
    assert!(
        replay
            .hydrate_tool_attachment_observation(OplogIndex::from_u64(4), 3)
            .await
            .is_err(),
        "same-length corrupted bytes must fail before consumer delivery"
    );
    oplog.external_payloads.lock().await.clear();
    oplog.external_payloads.lock().await.push((
        id,
        hash,
        golem_common::serialization::serialize(&b"abc".to_vec()).unwrap(),
    ));
    {
        let _storage_blocked = oplog.external_payloads.lock().await;
        let hydration = replay.hydrate_tool_attachment_observation(OplogIndex::from_u64(4), 2);
        tokio::pin!(hydration);
        assert!(
            matches!(
                futures::poll!(hydration.as_mut()),
                std::task::Poll::Ready(Err(_))
            ),
            "oversized history must fail before waiting for payload storage"
        );
    }
    let (source, read, bytes) = replay
        .hydrate_tool_attachment_observation(OplogIndex::from_u64(4), 3)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref().unwrap().capacity(), 3);
    assert_eq!(bytes, Some(b"abc".to_vec()));
    assert_eq!(
        replay.tool_attachment_observation(read.attachment, &read.logical_invocation, 0),
        Some((source, Some((source, 2))))
    );
    assert_eq!(
        replay.historical_tool_attachment_write(&request).unwrap(),
        Some(backing)
    );
    let reconstructed = test_replay_state(
        test_agent_id(),
        oplog,
        DeletedRegions::from_regions([OplogRegion::from_range(2..=4)]),
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        reconstructed.tool_attachment_observation(read.attachment, &read.logical_invocation, 0),
        None
    );
    replay
        .set_replay_target(OplogIndex::from_u64(3))
        .await
        .unwrap();
    assert_eq!(
        replay.tool_attachment_observation(read.attachment, &read.logical_invocation, 0),
        None
    );
}

#[test]
async fn attachment_index_validates_raw_slice_against_regenerated_argument() {
    use golem_common::model::oplog::attachment::{
        AcceptedAttachmentWriteEnd, ToolAttachmentReadOutcome,
    };
    let accepted = AcceptedAttachmentWriteEnd(OplogIndex::from_u64(3));
    for (body, valid) in [(b"b", true), (b"x", false)] {
        let oplog = Arc::new(InMemoryOplog::new());
        let mut entries = attachment_index_fixture(OplogPayload::Inline(Box::new(body.to_vec())));
        let OplogEntry::ToolAttachmentRead { observation, .. } = &mut entries[3] else {
            unreachable!()
        };
        observation.outcome = ToolAttachmentReadOutcome::RawBytes {
            end: accepted,
            offset: 1,
            length: 1,
        };
        *oplog.entries.lock().unwrap() = entries;
        let replay = test_replay_state(test_agent_id(), oplog, DeletedRegions::new(), None)
            .await
            .unwrap();
        assert_eq!(
            replay
                .validate_tool_attachment_regenerated_bytes(accepted, b"abc")
                .await
                .is_ok(),
            valid
        );
    }
}

#[test]
async fn attachment_index_crossed_acceptance_does_not_wait_for_cursor() {
    use golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd;

    let oplog = Arc::new(InMemoryOplog::new());
    *oplog.entries.lock().unwrap() =
        attachment_index_fixture(OplogPayload::Inline(Box::new(b"abc".to_vec())));
    let replay = test_replay_state(test_agent_id(), oplog, DeletedRegions::new(), None)
        .await
        .unwrap();
    let start = OplogIndex::from_u64(2);
    let end = AcceptedAttachmentWriteEnd(OplogIndex::from_u64(3));
    assert!(replay.last_replayed_index() < end.0);
    replay
        .await_tool_attachment_acceptance(start)
        .await
        .unwrap();
    let _gate = replay.cursor.advance_gate.clone().lock_owned().await;
    let mut acceptance = Box::pin(replay.await_tool_attachment_acceptance(start));
    assert!(matches!(
        futures::poll!(acceptance.as_mut()),
        std::task::Poll::Ready(Ok(actual)) if actual == end
    ));
}

#[test]
async fn attachment_index_retains_deleted_original_for_reobservation() {
    let oplog = Arc::new(InMemoryOplog::new());
    let mut entries = attachment_index_fixture(OplogPayload::Inline(Box::new(b"abc".to_vec())));
    let mut reobservation = entries[3].clone();
    let OplogEntry::ToolAttachmentRead {
        observation,
        payload,
        ..
    } = &mut reobservation
    else {
        unreachable!()
    };
    observation.reobserves = Some(OplogIndex::from_u64(4));
    observation.read_attempt_ordinal = 5;
    *payload = None;
    entries.push(reobservation);
    *oplog.entries.lock().unwrap() = entries;
    let replay = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::from_regions([OplogRegion::from_range(4..=4)]),
        None,
    )
    .await
    .unwrap();
    let (source, read, bytes) = replay
        .hydrate_tool_attachment_observation(OplogIndex::from_u64(5), 3)
        .await
        .unwrap();
    assert_eq!(source, OplogIndex::from_u64(4));
    assert_eq!(bytes, Some(b"abc".to_vec()));
    assert_eq!(read.read_attempt_ordinal, 5);
    assert_eq!(read.reobserves, Some(source));
    assert_eq!(
        replay.tool_attachment_observation(read.attachment, &read.logical_invocation, 0),
        Some((source, Some((OplogIndex::from_u64(5), 5))))
    );
    replay
        .set_replay_target(OplogIndex::from_u64(3))
        .await
        .unwrap();
    replay
        .set_replay_target(OplogIndex::from_u64(5))
        .await
        .unwrap();
    let accepted =
        golem_common::model::oplog::attachment::AcceptedAttachmentWriteEnd(OplogIndex::from_u64(3));
    replay
        .validate_tool_attachment_regenerated_bytes(accepted, b"abc")
        .await
        .unwrap();
    assert!(
        replay
            .validate_tool_attachment_regenerated_bytes(accepted, b"abd")
            .await
            .is_err(),
        "growing the replay horizon must restore producer-request validation"
    );
    assert!(
        replay
            .hydrate_tool_attachment_observation(OplogIndex::from_u64(5), 3)
            .await
            .is_ok()
    );
    assert!(
        test_replay_state(
            test_agent_id(),
            oplog,
            DeletedRegions::from_regions([OplogRegion::from_range(3..=4)]),
            None
        )
        .await
        .is_err()
    );
}

#[test]
async fn attachment_terminal_index_preserves_source_across_target_changes() {
    let oplog = Arc::new(InMemoryOplog::new());
    let mut entries = attachment_terminal_prefix();
    let original = attachment_terminal_observation(true, None);
    let mut replacement = original.clone();
    let OplogEntry::ToolAttachmentRead { observation, .. } = &mut replacement else {
        unreachable!()
    };
    observation.reobserves = Some(OplogIndex::from_u64(4));
    let read = *observation.clone();
    entries.extend([original, replacement]);
    *oplog.entries.lock().unwrap() = entries;
    let replay = test_replay_state(
        test_agent_id(),
        oplog,
        DeletedRegions::from_regions([OplogRegion::from_range(4..=4)]),
        None,
    )
    .await
    .unwrap();
    for target in [5, 4, 5] {
        replay
            .set_replay_target(OplogIndex::from_u64(target))
            .await
            .unwrap();
        assert_eq!(
            replay.tool_attachment_observation(read.attachment, &read.logical_invocation, 0),
            Some((
                OplogIndex::from_u64(4),
                (target == 5).then_some((OplogIndex::from_u64(5), 2))
            )),
        );
    }
}

#[test]
#[test_r::timeout("10s")]
async fn attachment_terminal_restores_without_a_surviving_host_waiter() {
    use crate::durable_host::tool::attachment::{
        AttachmentMemory, AttachmentRead, attachment_pair,
    };
    use golem_common::model::oplog::attachment::{ToolAttachmentId, ToolAttachmentSlot};

    let mut entries = attachment_terminal_prefix();
    entries.push(remove_retry_policy());
    let replay = replay_state_over(entries).await;
    let identity = ToolAttachmentId {
        creation_start: OplogIndex::INITIAL,
        slot: ToolAttachmentSlot::Stdout,
    };
    let (producer, consumer, _) = attachment_pair(8, AttachmentMemory::inert());
    producer.configure_live();
    producer.writer().set_identity(identity);
    replay
        .register_tool_attachment(identity, &producer.controller())
        .unwrap();
    let read = consumer.read_next();
    tokio::pin!(read);
    assert!(futures::poll!(&mut read).is_pending());
    // No host waiter claims the terminal call. Cursor crossing still restores its accepted End.
    assert_eq!(
        replay.get_oplog_entry(None).await.unwrap().0,
        OplogIndex::from_u64(4)
    );
    assert!(matches!(read.await, AttachmentRead::End));
}

#[test]
async fn attachment_index_rejects_invalid_acceptance_and_reobservation() {
    for case in 0..5 {
        let oplog = Arc::new(InMemoryOplog::new());
        let mut entries = attachment_index_fixture(OplogPayload::Inline(Box::new(b"abc".to_vec())));
        if case == 0 {
            entries[2] = remove_retry_policy();
        }
        if case == 4 {
            let OplogEntry::End { response, .. } = &mut entries[2] else {
                unreachable!()
            };
            *response = Some(OplogPayload::Inline(Box::new(golem_common::model::oplog::HostResponseToolAttachmentWrite {
                outcome: golem_common::model::oplog::payload::tool_attachment::ToolAttachmentWriteOutcome::Empty,
            }.into())));
        }
        let OplogEntry::ToolAttachmentRead {
            observation,
            payload,
            ..
        } = &mut entries[3]
        else {
            unreachable!()
        };
        match case {
            1 => {
                observation.attachment.slot =
                    golem_common::model::oplog::attachment::ToolAttachmentSlot::Stderr
            }
            2 => {
                observation.reobserves = Some(OplogIndex::from_u64(2));
                *payload = None;
            }
            3 => {
                *payload = None;
            }
            _ => {}
        }
        *oplog.entries.lock().unwrap() = entries;
        assert!(
            test_replay_state(test_agent_id(), oplog, DeletedRegions::new(), None)
                .await
                .is_err()
        );
    }
}

#[test]
#[test_r::timeout("10s")]
async fn completed_entity_body_detects_unconsumed_attachment_read() {
    for guest in [false, true] {
        let parent = OplogIndex::from_u64(4);
        let mut entries = attachment_terminal_prefix();
        entries.extend([
            start_now(),
            attachment_terminal_observation(guest, Some(parent)),
            end_for(4, 42),
        ]);
        let rs = replay_state_over(entries).await;
        let terminal = rs
            .claim_concurrent_start(
                &HostFunctionName::GolemToolOutputFinish,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        rs.await_resolution(terminal).await.unwrap();
        let handle = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        assert_eq!(
            rs.unconsumed_scope_head(parent, HashSet::new())
                .await
                .unwrap(),
            Some(OplogIndex::from_u64(5)),
        );
        let mut resolution = Box::pin(rs.await_resolution(handle));
        assert!(futures::poll!(resolution.as_mut()).is_pending());
        let barrier = rs
            .await_stream_observation(OplogIndex::from_u64(5))
            .await
            .unwrap();
        assert!(futures::poll!(resolution.as_mut()).is_pending());
        barrier.acknowledge();
        assert!(matches!(
            resolution.await.unwrap(),
            Resolution::Completed { .. }
        ));
    }
}

#[test]
#[test_r::timeout("10s")]
async fn custom_subtree_attachment_observation_does_not_block_resolution() {
    let mut entries = attachment_terminal_prefix();
    entries.extend([
        custom_start("outer", 1, None, 1),
        attachment_terminal_observation(true, Some(OplogIndex::from_u64(4))),
        custom_end(4, 10),
    ]);
    let rs = replay_state_over(entries).await;
    let terminal = rs
        .claim_concurrent_start(
            &HostFunctionName::GolemToolOutputFinish,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(terminal).await.unwrap();
    let claimed = claim_custom_start(
        &rs,
        &HostFunctionName::Custom("outer".into()),
        &DurableFunctionType::ReadRemote,
        None,
        test_invocation_id(1),
    )
    .await
    .unwrap();
    assert_eq!(
        rs.unconsumed_scope_head(OplogIndex::from_u64(4), HashSet::new())
            .await
            .unwrap(),
        None
    );
    assert!(matches!(
        rs.await_resolution_outcome(claimed.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
}

#[test]
#[test_r::timeout("10s")]
async fn custom_subtree_guest_observation_does_not_block_resolution() {
    for complete in [true, false] {
        let mut observation = consumer_observation(0, true, false);
        if let OplogEntry::StreamSession { record, .. } = &mut observation
            && let OplogPayload::Inline(record) = record
            && let golem_common::model::durable_stream::StreamSessionRecord::ConsumerItemValue(
                record,
            ) = record.as_mut()
        {
            record.observation =
                golem_common::model::durable_stream::StreamConsumerObservation::Guest {
                    parent_start_index: Some(OplogIndex::from_u64(2)),
                };
        }
        let mut entries = vec![
            OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            },
            custom_start("outer", 1, None, 1),
            observation,
        ];
        if complete {
            entries.push(custom_end(2, 10));
        }
        let rs = replay_state_over(entries).await;
        let claimed = claim_custom_start(
            &rs,
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            test_invocation_id(1),
        )
        .await
        .unwrap();
        let outcome = tokio::time::timeout(
            Duration::from_secs(2),
            rs.await_resolution_outcome(claimed.handle),
        )
        .await
        .expect("a custom subtree has no replaying guest read observer")
        .unwrap();
        assert!(matches!(
            (complete, outcome),
            (
                true,
                ResolutionOutcome::Resolved(Resolution::Completed { .. })
            ) | (false, ResolutionOutcome::Incomplete)
        ));
    }
}

#[test]
async fn replay_consumer_observations_reserve_item_and_terminal_guest_boundaries() {
    for terminal in [false, true] {
        let rs = replay_state_over(vec![
            OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            },
            consumer_observation(1, true, terminal),
            remove_retry_policy(),
            consumer_observation(0, true, terminal),
            remove_retry_policy(),
        ])
        .await;
        let mut next = Box::pin(rs.get_oplog_entry(None));
        assert!(futures::poll!(next.as_mut()).is_pending());
        let b = rs
            .await_stream_observation(OplogIndex::from_u64(2))
            .await
            .unwrap();
        assert!(
            futures::poll!(next.as_mut()).is_pending(),
            "an unrelated reader cannot acknowledge B"
        );
        b.acknowledge();
        assert_eq!(next.await.unwrap().0, OplogIndex::from_u64(3));
        let a = rs
            .await_stream_observation(OplogIndex::from_u64(4))
            .await
            .unwrap();
        let mut next = Box::pin(rs.get_oplog_entry(None));
        assert!(
            futures::poll!(next.as_mut()).is_pending(),
            "an unrelated reader cannot acknowledge A"
        );
        a.acknowledge();
        assert_eq!(next.await.unwrap().0, OplogIndex::from_u64(5));
    }
}

#[test]
async fn invocation_boundary_after_guest_observation_preserves_finished_or_incomplete_tail() {
    for finished in [false, true] {
        let mut entries = vec![
            OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            },
            consumer_observation(0, true, true),
            stdout_log("after-consumption"),
        ];
        if finished {
            entries.push(invocation_finished());
        }
        let rs = replay_state_over(entries).await;
        let barrier = rs
            .await_stream_observation(OplogIndex::from_u64(2))
            .await
            .unwrap();
        barrier.acknowledge();
        assert_eq!(
            rs.get_oplog_entry_agent_invocation_finished()
                .await
                .unwrap()
                .is_some(),
            finished,
            "deferred hint skipping must distinguish a recorded finish from a natural tail"
        );
    }
}

#[test]
async fn replay_host_projection_observations_remain_skippable_hints() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        consumer_observation(0, false, false),
        consumer_observation(0, false, true),
        remove_retry_policy(),
    ])
    .await;
    assert_eq!(
        rs.get_oplog_entry(None).await.unwrap().0,
        OplogIndex::from_u64(4)
    );
}

#[test]
async fn replay_start_claim_cannot_cross_a_guest_stream_observation() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        consumer_observation(0, true, false),
        start_now(),
        end_for(3, 42),
    ])
    .await;
    let mut claim = Box::pin(rs.claim_concurrent_start(
        &HostFunctionName::MonotonicClockNow,
        &DurableFunctionType::ReadLocal,
    ));
    assert!(futures::poll!(claim.as_mut()).is_pending());
    let barrier = rs
        .await_stream_observation(OplogIndex::from_u64(2))
        .await
        .unwrap();
    assert!(futures::poll!(claim.as_mut()).is_pending());
    barrier.acknowledge();
    let handle = claim.await.unwrap();
    assert!(
        matches!(rs.await_resolution(handle).await.unwrap(), Resolution::Completed { end_idx, .. } if end_idx == OplogIndex::from_u64(4))
    );
}

#[test]
async fn replay_delivery_marker_skips_following_hints_after_guest_boundary() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        stdout_log("after-delivery"),
        start_now(),
        end_for(6, 43),
    ])
    .await;
    let first = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(first).await.unwrap();

    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(4))
        .await
        .unwrap();
    barrier.acknowledge();

    let second = tokio::time::timeout(
        Duration::from_secs(1),
        rs.claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    )
    .await
    .expect("a hint after the delivery marker must not strand the replay cursor")
    .unwrap();
    assert_eq!(second.start_idx(), OplogIndex::from_u64(6));
    match rs.await_resolution(second).await.unwrap() {
        Resolution::Completed { end_idx, .. } => {
            assert_eq!(end_idx, OplogIndex::from_u64(7));
        }
        other => panic!("expected the second call to complete, got {other:?}"),
    }
}

#[test]
async fn optional_reader_waits_at_delivery_marker_before_testing_later_entry() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        remove_retry_policy(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(handle).await.unwrap();

    let optional =
        rs.try_get_oplog_entry(|entry| matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
    tokio::pin!(optional);
    assert!(
        futures::poll!(optional.as_mut()).is_pending(),
        "a completion marker is a replay barrier, not a predicate mismatch"
    );

    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(4))
        .await
        .unwrap();
    assert!(
        futures::poll!(optional.as_mut()).is_pending(),
        "the optional reader must remain blocked until guest delivery"
    );
    barrier.acknowledge();
    assert_eq!(optional.await.unwrap().unwrap().0, OplogIndex::from_u64(5));
}

#[test]
async fn identity_claim_does_not_scan_past_delivery_marker() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        start_now(),
        end_for(5, 43),
    ])
    .await;
    let first = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(first).await.unwrap();

    let second = rs.claim_concurrent_start(
        &HostFunctionName::MonotonicClockNow,
        &DurableFunctionType::ReadLocal,
    );
    tokio::pin!(second);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), second.as_mut())
            .await
            .is_err(),
        "an identity claim must not scan ahead to a Start after the delivery marker"
    );

    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(4))
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), second.as_mut())
            .await
            .is_err()
    );
    barrier.acknowledge();
    assert_eq!(second.await.unwrap().start_idx(), OplogIndex::from_u64(5));
}

#[test]
#[test_r::timeout("10s")]
async fn claim_adopts_retained_start_behind_its_own_delivery_marker() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5), Delivered(B=3) at 6,
    // Delivered(A=2) at 7]. Draining A's terminal retains B together with its End and parks the
    // cursor at B's delivery marker. B's claim must adopt the retained Start
    // instead of reporting itself blocked by the marker that only B's own delivery can release.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
        delivered_for(3),
        delivered_for(2),
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
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());

    let handle_b = tokio::time::timeout(
        Duration::from_millis(500),
        rs.claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    )
    .await
    .expect("the retained Start must be claimable while its own delivery marker heads the cursor")
    .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    assert!(!rs.has_unclaimed_retained_starts());
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed {
            end_idx,
            delivery_marker,
            ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(5));
            assert_eq!(delivery_marker, Some(OplogIndex::from_u64(6)));
        }
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn markerless_completed_end_resolves_without_delivery_marker() {
    // The recorded run crashed after the `End` became durable but before the completion crossed
    // to the guest, so no `CompletionDelivered` marker follows. The resolution must expose that
    // as `delivery_marker: None` — the deferred accessor path tail-gates such completions.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
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
        Resolution::Completed {
            end_idx,
            delivery_marker,
            ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(3));
            assert_eq!(delivery_marker, None);
        }
        other => panic!("expected markerless completion, got {other:?}"),
    }
}

#[test]
async fn await_natural_tail_end_returns_once_tail_drains() {
    // After the markerless call is claimed and resolved, the remaining tail holds only its own
    // awaited `End` terminal and a trailing hint — entries the drain loop consumes without a
    // positional owner — so the tail-gated waiter exhausts the tail and returns.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        stdout_log("crash tail hint"),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(handle).await.unwrap();

    rs.await_natural_tail_end(None).await.unwrap();
    assert!(rs.is_live());
}

#[test]
async fn await_natural_tail_end_waits_for_positionally_owned_entry() {
    // The crash tail contains a real entry (`RemoveRetryPolicy`) owned by the replaying guest:
    // the tail-gated waiter must park until the guest's positional reader consumes it, and only
    // then observe the exhausted tail. Delivering earlier could make the replayed guest skip
    // recorded entries — the crash window this gate closes.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        remove_retry_policy(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(handle).await.unwrap();

    let waiter = rs.await_natural_tail_end(None);
    tokio::pin!(waiter);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), waiter.as_mut())
            .await
            .is_err(),
        "the tail-gated waiter must park while a positionally-owned entry remains"
    );

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(4));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    waiter.await.unwrap();
    assert!(rs.is_live());
}

#[test]
async fn await_natural_tail_end_propagates_delivery_failure() {
    // Poisoning replay (a delivery boundary fired while a completion was still tail-gated) must
    // wake and fail a parked tail waiter instead of leaving it parked forever.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        remove_retry_policy(),
    ])
    .await;

    let waiter = rs.await_natural_tail_end(None);
    tokio::pin!(waiter);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), waiter.as_mut())
            .await
            .is_err()
    );

    rs.fail_tail_delivery(OplogIndex::from_u64(1), "test poisoning");
    let err = waiter.await.expect_err("the poisoned waiter must fail");
    assert!(
        err.to_string().contains("test poisoning"),
        "unexpected error: {err}"
    );
}

#[test]
async fn await_natural_tail_end_parks_only_after_owned_cursor_work() {
    use crate::durable_host::tail_work::TailWorkTracker;

    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        remove_retry_policy(),
    ])
    .await;
    let tracker = TailWorkTracker::new();
    let activity = tracker.activity();
    let lock = rs.cursor.state.lock().await;
    let mut waiter = Box::pin(rs.await_natural_tail_end(Some(&activity)));
    assert!(futures::poll!(waiter.as_mut()).is_pending());
    assert_eq!(
        tracker.active_count(),
        1,
        "a queued cursor operation stays active"
    );
    drop(lock);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), waiter.as_mut())
            .await
            .is_err()
    );
    assert_eq!(
        tracker.active_count(),
        0,
        "only the passive progress wait parks"
    );

    // A notification does not authorize delivery, and re-entering the cursor must be active.
    let lock = rs.cursor.state.lock().await;
    rs.cursor.progress.notify_waiters();
    assert!(futures::poll!(waiter.as_mut()).is_pending());
    assert_eq!(tracker.active_count(), 1);
    drop(lock);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), waiter.as_mut())
            .await
            .is_err()
    );
    assert_eq!(tracker.active_count(), 0);

    let (_, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
    waiter.await.unwrap();
    assert_eq!(
        tracker.active_count(),
        1,
        "continuation is active before delivery"
    );
    drop(activity);
    assert_eq!(tracker.active_count(), 0);
}

#[test]
async fn await_natural_tail_end_park_restores_activity_on_error_and_drop() {
    use crate::durable_host::tail_work::TailWorkTracker;

    for poison in [false, true] {
        let rs = replay_state_over(vec![
            OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            },
            remove_retry_policy(),
        ])
        .await;
        let tracker = TailWorkTracker::new();
        let activity = tracker.activity();
        let mut waiter = Box::pin(rs.await_natural_tail_end(Some(&activity)));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), waiter.as_mut())
                .await
                .is_err()
        );
        assert_eq!(tracker.active_count(), 0);
        if poison {
            rs.fail_tail_delivery(OplogIndex::from_u64(1), "parked failure");
            assert!(
                waiter
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("parked failure")
            );
        } else {
            drop(waiter);
        }
        assert_eq!(tracker.active_count(), 1);
        drop(activity);
        assert_eq!(tracker.active_count(), 0);
    }
}

#[test]
async fn replay_delivery_barriers_preserve_adjacent_callback_order() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
        delivered_for(2),
        delivered_for(3),
        remove_retry_policy(),
    ])
    .await;
    let a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(a).await.unwrap();
    rs.await_resolution(b).await.unwrap();

    let barrier_a = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(6))
        .await
        .unwrap();
    let barrier_b = rs.await_completion_delivery(OplogIndex::from_u64(3), OplogIndex::from_u64(7));
    tokio::pin!(barrier_b);
    assert!(
        futures::poll!(barrier_b.as_mut()).is_pending(),
        "B's delivery marker must remain blocked until A reaches the guest"
    );

    barrier_a.acknowledge();
    let barrier_b = barrier_b.await.unwrap();
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(7));
    barrier_b.acknowledge();
    assert_eq!(
        rs.get_oplog_entry(None).await.unwrap().0,
        OplogIndex::from_u64(8)
    );
}

#[test]
async fn dropped_replay_delivery_barrier_fails_later_cursor_work() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        remove_retry_policy(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(handle).await.unwrap();
    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(4))
        .await
        .unwrap();
    drop(barrier);

    let error = rs
        .get_oplog_entry(None)
        .await
        .expect_err("an unacknowledged recorded delivery must fail replay");
    assert!(
        error
            .to_string()
            .contains("dropped before the recorded guest-delivery boundary"),
        "unexpected error: {error}"
    );
}

#[test]
#[test_r::timeout("10s")]
async fn claim_rechecks_retention_after_initial_miss_without_losing_its_gate_queue_position() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        start_now(),
        end_for(4, 43),
        delivered_for(2),
        remove_retry_policy(),
    ])
    .await;
    let a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let gate = rs.cursor.advance_gate.clone().lock_owned().await;
    let (missed, observed_miss) = tokio::sync::oneshot::channel();
    *rs.cursor.retained_claim_miss.lock().unwrap() = Some(missed);
    let claim_state = rs.clone();
    let claim = tokio::spawn(async move {
        claim_state
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
    });
    observed_miss.await.unwrap();
    // The claim is already queued, but its Start has not yet become retained. Advancing with
    // the gate's current owner retains B and transfers that same gate to A's delivery barrier.
    let mut tx = rs.cursor.tx_with_gate(gate).await.unwrap();
    let gate = tx
        .consume_delivery_boundary(Some(OplogIndex::from_u64(2)), OplogIndex::from_u64(6))
        .await
        .unwrap()
        .unwrap();
    let barrier = ReplayDeliveryBarrier::new(
        rs.clone(),
        OplogIndex::from_u64(2),
        OplogIndex::from_u64(6),
        gate,
    );
    rs.cursor.finish_tx(tx);
    let b = tokio::time::timeout(Duration::from_secs(2), claim)
        .await
        .expect("progress must recheck retained state even though the gate remains held")
        .unwrap()
        .unwrap();
    assert_eq!(b.start_idx(), OplogIndex::from_u64(4));
    assert!(
        matches!(rs.await_resolution(b).await.unwrap(), Resolution::Completed { end_idx, .. } if end_idx == OplogIndex::from_u64(5))
    );
    let mut fence = Box::pin(rs.fence_owned_cursor_ops());
    assert_pending(
        &mut fence,
        "a cleanup fence cannot bypass the delivery owner",
    )
    .await;
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(6));
    barrier.acknowledge();
    fence.await.unwrap();
    rs.await_resolution(a).await.unwrap();
}

#[test]
#[test_r::timeout("10s")]
async fn completed_retained_start_resolves_while_another_delivery_holds_the_gate() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        start_now(),
        end_for(4, 43),
        delivered_for(2),
        remove_retry_policy(),
    ])
    .await;
    let a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(a).await.unwrap();
    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(6))
        .await
        .unwrap();
    assert!(rs.has_unclaimed_retained_starts());

    // Acquiring the barrier proves that the delivery owns the gate before B starts its claim.
    let b = tokio::time::timeout(
        Duration::from_secs(2),
        rs.claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    )
    .await
    .expect("a retained Start claim must not wait for another call's guest boundary")
    .unwrap();
    assert_eq!(b.start_idx(), OplogIndex::from_u64(4));
    let resolution = tokio::time::timeout(Duration::from_secs(2), rs.await_resolution(b))
        .await
        .expect("a retained terminal must not wait for another call's guest boundary")
        .unwrap();
    assert!(
        matches!(resolution, Resolution::Completed { end_idx, delivery_marker: None, .. }
        if end_idx == OplogIndex::from_u64(5))
    );
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(6));
    barrier.acknowledge();
    assert_eq!(
        rs.get_oplog_entry(None).await.unwrap().0,
        OplogIndex::from_u64(7)
    );
}

#[test]
#[test_r::timeout("10s")]
async fn open_retained_start_claims_but_its_later_terminal_waits_for_delivery_acknowledgement() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        start_now(),
        delivered_for(2),
        end_for(4, 43),
        remove_retry_policy(),
    ])
    .await;
    let a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(a).await.unwrap();
    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(5))
        .await
        .unwrap();
    assert!(rs.has_unclaimed_retained_starts());
    let b = tokio::time::timeout(
        Duration::from_secs(2),
        rs.claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    )
    .await
    .expect("an open retained Start claim must not wait for delivery acknowledgement")
    .unwrap();
    assert_eq!(b.start_idx(), OplogIndex::from_u64(4));
    let mut resolution = Box::pin(rs.await_resolution(b));
    assert_pending(&mut resolution, "the terminal after the delivery marker").await;
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    barrier.acknowledge();
    assert!(
        matches!(resolution.await.unwrap(), Resolution::Completed { end_idx, delivery_marker: None, .. }
        if end_idx == OplogIndex::from_u64(6))
    );
}

#[test]
#[test_r::timeout("10s")]
async fn unobserved_owned_delivery_barrier_result_latches_delivery_failure() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        remove_retry_policy(),
    ])
    .await;
    let call = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(call).await.unwrap();
    let gate = rs.cursor.advance_gate.clone().lock_owned().await;
    let progress = rs.cursor.progress.notified();
    tokio::pin!(progress);
    progress.as_mut().enable();
    let mut delivery =
        Box::pin(rs.await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(4)));
    assert!(futures::poll!(delivery.as_mut()).is_pending());
    // The acquired gate prevents the spawned marker operation from completing before its
    // waiter is cancelled. Progress then proves that operation consumed the marker.
    drop(delivery);
    drop(gate);
    progress.await;
    let error = rs
        .fence_owned_cursor_ops()
        .await
        .expect_err("discarding an owned delivery result must poison replay");
    assert!(
        error
            .to_string()
            .contains("dropped before the recorded guest-delivery boundary"),
        "unexpected error: {error}"
    );
    let error = rs
        .get_oplog_entry(None)
        .await
        .expect_err("delivery failure must stay latched");
    assert!(
        error
            .to_string()
            .contains("dropped before the recorded guest-delivery boundary"),
        "unexpected error: {error}"
    );
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(4));
}

#[test]
#[test_r::timeout("10s")]
async fn cancelled_owned_cursor_waiter_keeps_cleanup_fence_pending_until_transaction_finishes() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        remove_retry_policy(),
    ])
    .await;
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, released) = tokio::sync::oneshot::channel();
    let mut operation = Box::pin(rs.run_owned_cursor_op(move |state| async move {
        state
            .with_tx(async move |_| {
                entered.send(()).unwrap();
                released.await.unwrap();
                Ok(())
            })
            .await
    }));
    assert!(futures::poll!(operation.as_mut()).is_pending());
    ready.await.unwrap();
    drop(operation);
    let mut fence = Box::pin(rs.fence_owned_cursor_ops());
    assert_pending(&mut fence, "cleanup after cancelling the cursor waiter").await;
    release.send(()).unwrap();
    fence.await.unwrap();
    assert_eq!(
        rs.get_oplog_entry(None).await.unwrap().0,
        OplogIndex::from_u64(2)
    );
}

#[test]
async fn marker_in_deleted_region_delivers_end_normally() {
    // A CompletionDiscarded marker inside a deleted region belongs to an abandoned timeline:
    // the still-visible End must be delivered normally.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        discarded_for(2),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped =
        golem_common::model::regions::DeletedRegionsBuilder::from_regions([OplogRegion {
            start: OplogIndex::from_u64(4),
            end: OplogIndex::from_u64(4),
        }])
        .build();
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
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
async fn reverted_completion_marker_can_be_replaced_and_reconstructed() {
    for old_marker in [delivered_for(2), discarded_for(2)] {
        for delivered in [true, false] {
            for grow_target in [false, true] {
                let oplog: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());
                for entry in [
                    OplogEntry::RecoverySucceeded {
                        timestamp: Timestamp::now_utc(),
                    },
                    start_now(),
                    end_for(2, 42),
                ] {
                    oplog.add(entry).await.unwrap();
                }
                let dropped_region = OplogRegion {
                    start: OplogIndex::from_u64(4),
                    end: OplogIndex::from_u64(4),
                };
                let suffix = [
                    old_marker.clone(),
                    OplogEntry::revert(dropped_region.clone()),
                    if delivered {
                        delivered_for(2)
                    } else {
                        discarded_for(2)
                    },
                ];
                if !grow_target {
                    for entry in &suffix {
                        oplog.add(entry.clone()).await.unwrap();
                    }
                }
                let rs = test_replay_state(
                    test_agent_id(),
                    oplog.clone(),
                    DeletedRegions::from_regions([dropped_region]),
                    None,
                )
                .await
                .expect("a deleted marker must not conflict with its replacement");
                if grow_target {
                    for entry in suffix {
                        oplog.add(entry).await.unwrap();
                    }
                    rs.set_replay_target(OplogIndex::from_u64(6))
                        .await
                        .expect("target growth must ignore deleted completion markers");
                }
                let handle = rs
                    .claim_concurrent_start(
                        &HostFunctionName::MonotonicClockNow,
                        &DurableFunctionType::ReadLocal,
                    )
                    .await
                    .unwrap();
                match rs.await_resolution(handle).await.unwrap() {
                    Resolution::Completed {
                        end_idx,
                        delivery_marker,
                        response,
                        ..
                    } if delivered => {
                        assert_eq!(end_idx, OplogIndex::from_u64(3));
                        assert_eq!(delivery_marker, Some(OplogIndex::from_u64(6)));
                        assert!(response.is_some());
                    }
                    Resolution::CompletedButDiscarded {
                        end_idx,
                        marker_idx,
                        response,
                    } if !delivered => {
                        assert_eq!(end_idx, OplogIndex::from_u64(3));
                        assert_eq!(marker_idx, OplogIndex::from_u64(6));
                        assert!(response.is_some());
                    }
                    other => panic!("expected the replacement completion, got {other:?}"),
                }
            }
        }
    }
}

#[test]
async fn delivered_marker_with_deleted_start_is_skipped_as_orphan() {
    // The deleted Start/End belong to an abandoned timeline. Their surviving delivery marker is
    // therefore an orphan hint and must not strand positional replay before the next kept entry.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        remove_retry_policy(),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped =
        golem_common::model::regions::DeletedRegionsBuilder::from_regions([OplogRegion {
            start: OplogIndex::from_u64(2),
            end: OplogIndex::from_u64(3),
        }])
        .build();
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
        .await
        .expect("failed to build replay state");

    let (index, entry) = tokio::time::timeout(Duration::from_millis(100), rs.get_oplog_entry(None))
        .await
        .expect("an orphan CompletionDelivered marker must not block replay")
        .expect("the next kept entry must remain readable");
    assert_eq!(index, OplogIndex::from_u64(5));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
}

#[test]
async fn duplicate_completion_discarded_markers_fail_construction() {
    // Two markers referencing the same Start is oplog corruption; the upfront scan rejects it.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        start_now(),
        end_for(2, 42),
        discarded_for(2),
        discarded_for(2),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let err = test_replay_state(test_agent_id(), oplog, DeletedRegions::default(), None)
        .await
        .expect_err("duplicate markers must fail replay state construction");
    assert!(
        err.to_string().contains("CompletionDiscarded"),
        "unexpected error: {err}"
    );
}

#[test]
async fn conflicting_completion_markers_fail_construction() {
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        discarded_for(2),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let err = test_replay_state(test_agent_id(), oplog, DeletedRegions::default(), None)
        .await
        .expect_err("conflicting markers must fail replay state construction");
    assert!(
        err.to_string().contains("CompletionDelivered")
            && err.to_string().contains("CompletionDiscarded"),
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
    for entry in [
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let rs = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::default(),
        None,
    )
    .await
    .expect("failed to build replay state");
    let marker_idx = oplog.add(discarded_for(2)).await.unwrap();
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
async fn drain_retains_unclaimed_start_for_its_owner() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — draining the awaited terminals
    // while only A is claimed must step over the still-unclaimed Start(B) *without* consuming it
    // as its own: B is retained (with its End attached once reached) for the owner that claims it
    // later, and A's End is drained to A. The drain never parks on B, because B's owner may need
    // the Store the draining task holds.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    assert!(!rs.has_unclaimed_retained_starts());

    rs.drain_awaited_terminals().await.unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            !internal
                .concurrent_resolver
                .is_pending(OplogIndex::from_u64(2)),
            "drain must resolve A across the unclaimed Start(B)"
        );
        assert!(
            internal
                .retained_starts
                .contains_key(&OplogIndex::from_u64(3)),
            "Start(B) must be retained for its owner"
        );
    }
    assert!(rs.has_unclaimed_retained_starts());
    assert!(
        rs.last_replayed_index() >= OplogIndex::from_u64(4),
        "drain must advance past the retained Start(B) to A's End"
    );
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }

    // B's owner arrives later (guest re-execution reaches its call): the claim adopts the
    // retained Start instead of erroring or claiming something else, and resolves with the End
    // attached while it was retained.
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    assert!(!rs.has_unclaimed_retained_starts());
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn hints_behind_a_retained_start_are_published_when_its_owner_claims_it() {
    // [RecoverySucceeded, Start(A=2), End(A=2→3), Start(B=4), End(B=4→5), CardDerived(6), Start(C=7)] — A's
    // drain steps over B (retained, End attached) and the CardDerived hint that B's owner
    // recorded right after its End. The hint's replay event is a side effect of B's durable
    // call: publishing it while B is merely retained would let the guest's later authority
    // boundary apply the card before B's owner re-claims the Start and reads the derivation,
    // so it stays deferred on the retained Start until the owner claims it.
    let derived_card = stored_test_card(CardId::new());
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        start_now(),
        end_for(4, 43),
        OplogEntry::CardDerived {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            card: Box::new(derived_card.clone()),
            wallet_generation: 0,
        },
        start_now(),
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
        let retained_b = internal
            .retained_starts
            .get(&OplogIndex::from_u64(4))
            .expect("Start(B) must be retained for its owner");
        assert_eq!(
            retained_b
                .terminal
                .as_ref()
                .map(|(terminal_idx, _)| *terminal_idx),
            Some(OplogIndex::from_u64(5))
        );
        assert_eq!(
            retained_b
                .deferred_events
                .iter()
                .map(|(idx, _)| *idx)
                .collect::<Vec<_>>(),
            vec![OplogIndex::from_u64(6)],
            "the CardDerived hint behind B's End is deferred on the retained Start"
        );
        assert!(
            internal
                .retained_starts
                .contains_key(&OplogIndex::from_u64(7)),
            "the drain retains Start(C) after skipping the hint"
        );
    }
    assert!(
        rs.last_replayed_index() >= OplogIndex::from_u64(6),
        "the drain must have committed past the hint"
    );
    assert_eq!(
        rs.pending_card_derivation(derived_card.card_id()).await,
        None,
        "a retained Start's hint must not be observable before its owner claims it"
    );
    assert!(rs.take_new_replay_events().is_empty());
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(3)),
        other => panic!("expected Completed, got {other:?}"),
    }

    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(4));
    assert_eq!(
        rs.pending_card_derivation(derived_card.card_id()).await,
        Some((derived_card.clone(), 0)),
        "claiming B publishes the deferred hint for its owner"
    );
    assert_eq!(
        rs.take_new_replay_events(),
        vec![ReplayEvent::CardDerived {
            card: derived_card,
            wallet_generation: 0,
        }]
    );
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn hints_behind_a_released_retained_start_are_published_at_live_invocation_end() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(B=3→4), CardDerived(5), End(A=2→6)] — B and the hint
    // behind its End are retained by A's await and B is never claimed. Releasing the closed
    // retained Start at the live invocation end must still publish the deferred hint: the card
    // derivation is recorded history and the next authority boundary has to observe it.
    let derived_card = stored_test_card(CardId::new());
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(3, 43),
        OplogEntry::CardDerived {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            card: Box::new(derived_card.clone()),
            wallet_generation: 0,
        },
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
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(6)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());
    assert_eq!(
        rs.pending_card_derivation(derived_card.card_id()).await,
        None
    );
    rs.switch_cursor_to_live().await.unwrap();
    assert_eq!(
        rs.pending_card_derivation(derived_card.card_id()).await,
        None,
        "switch_to_live keeps retained Starts and their deferred hints"
    );

    rs.release_retained_starts_at_live_invocation_end()
        .await
        .unwrap();
    assert!(!rs.has_unclaimed_retained_starts());
    assert_eq!(
        rs.pending_card_derivation(derived_card.card_id()).await,
        Some((derived_card, 0)),
        "releasing the retained Start publishes its deferred hint"
    );
}

#[test]
async fn retained_start_claim_rejects_mismatched_identity() {
    // [RecoverySucceeded, Start(A=2 now), Start(B=3 now), End(A=2→4), End(B=3→5), Start(C=6 resolution)] —
    // after A's
    // drain retained B, a claim with a *different* identity must not adopt the retained Start:
    // ownership is validated at claim time, so the mismatched claim falls through to the cursor
    // head (C) exactly as if B had never been retained.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
        start_resolution(),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());

    let handle_c = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockResolution,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(
        handle_c.start_idx(),
        OplogIndex::from_u64(6),
        "a mismatched identity must not adopt the retained Start(B)"
    );
    assert!(rs.has_unclaimed_retained_starts());

    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn retained_start_counts_are_published_per_quota_class() {
    // [RecoverySucceeded, Start(A=2 now), Start(B=3 p3 http send), Start(C=4 rpc invoke),
    //  Start(D=5 resolution), End(A=2→6), End(B=3→7), End(C=4→8), End(D=5→9)] — resolving A
    // commits past B, C and D, retaining all three. Only a call charged to the same quota as a
    // retained Start may still be its late owner: B keeps HTTP-charged calls unfresh, C keeps
    // RPC-charged calls unfresh, and D (charged to no quota) suppresses neither. Adopting a
    // retained Start clears its class again while the others stay published.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_named(HostFunctionName::P3HttpClientSend),
        start_named(HostFunctionName::GolemRpcWasmRpcInvoke),
        start_resolution(),
        end_for(2, 42),
        end_for(3, 43),
        end_for(4, 44),
        end_for(5, 45),
    ])
    .await;
    assert!(!rs.retains_unclaimed_start_charged_to(QuotaClass::Http));
    assert!(!rs.retains_unclaimed_start_charged_to(QuotaClass::Rpc));

    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(6)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());
    assert!(rs.retains_unclaimed_start_charged_to(QuotaClass::Http));
    assert!(rs.retains_unclaimed_start_charged_to(QuotaClass::Rpc));

    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::P3HttpClientSend,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    assert!(
        !rs.retains_unclaimed_start_charged_to(QuotaClass::Http),
        "adopting the only HTTP-charged retained Start makes HTTP calls fresh again"
    );
    assert!(
        rs.retains_unclaimed_start_charged_to(QuotaClass::Rpc),
        "a retained RPC-charged Start is unaffected by an HTTP adoption"
    );
    assert!(rs.has_unclaimed_retained_starts());

    let handle_c = rs
        .claim_concurrent_start(
            &HostFunctionName::GolemRpcWasmRpcInvoke,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_c.start_idx(), OplogIndex::from_u64(4));
    assert!(!rs.retains_unclaimed_start_charged_to(QuotaClass::Rpc));
    assert!(
        rs.has_unclaimed_retained_starts(),
        "the retained Start charged to no quota still keeps admission on the claim path"
    );

    let handle_d = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockResolution,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_d.start_idx(), OplogIndex::from_u64(5));
    assert!(!rs.has_unclaimed_retained_starts());
    for (handle, end) in [(handle_b, 7), (handle_c, 8), (handle_d, 9)] {
        match rs.await_resolution(handle).await.unwrap() {
            Resolution::Completed { end_idx, .. } => {
                assert_eq!(end_idx, OplogIndex::from_u64(end))
            }
            other => panic!("expected Completed, got {other:?}"),
        }
    }
}

#[test]
async fn retained_start_without_terminal_resolves_incomplete_at_live_switch() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4)] — B's End was never recorded. Draining A's End
    // retains B; when B's owner claims it, the claim registers an ordinary awaiter that reaches
    // the replay tail and resolves as Incomplete instead of hanging or fabricating a result.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
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
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());

    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    assert!(!rs.has_unclaimed_retained_starts());
    match rs.await_resolution_outcome(handle_b).await.unwrap() {
        ResolutionOutcome::Incomplete => {}
        other => panic!("expected Incomplete, got {other:?}"),
    }
}

#[test]
async fn positional_marker_read_retains_unclaimed_start_instead_of_consuming_it() {
    // [RecoverySucceeded, Start(B=2), RemoveRetryPolicy(3), End(B=2→4)] — a positional reader looking for the
    // retry-policy marker reaches the unclaimed durable-call Start(B) first. It must neither be
    // handed that Start (kind/ownership mismatch) nor park on it: the Start is retained for its
    // owner and the reader receives the marker at index 3.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        remove_retry_policy(),
        end_for(2, 42),
    ])
    .await;

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
    assert!(rs.has_unclaimed_retained_starts());

    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(2));
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn positional_marker_read_still_rejects_wrong_kind_at_head() {
    // [RecoverySucceeded, RemoveRetryPolicy(2), RemoveRetryPolicy(3)] — retention only applies to durable-call
    // Starts. A conditional positional reader expecting the *end* marker while the *begin* marker
    // is at the head must still see the mismatch (no entry, cursor untouched), never skip the
    // marker; the unconditional reader then receives the begin marker itself.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        remove_retry_policy(),
        remove_other_retry_policy(),
    ])
    .await;
    let probe = rs
        .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::RemoveRetryPolicy { name, .. } if name == "other-policy"))
        .await
        .unwrap();
    assert!(
        probe.is_none(),
        "a non-Start kind mismatch must not be skipped"
    );
    assert!(!rs.has_unclaimed_retained_starts());
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(1));

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(2));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
}

#[test]
async fn store_live_claim_adopts_retained_start_or_reports_store_already_live() {
    // [RecoverySucceeded, Start(A=2 now), Start(B=3 now), End(A=2→4), End(B=3→5), RemoveRetryPolicy(6),
    // Start(C=7 resolution), End(C=7→8)] — A's awaiter retained B (and attached B's End) and
    // stopped at the positional RemoveRetryPolicy(6), so the shared cursor is still short of the target. A
    // Store that already continued live locally still takes the replay admission path while
    // unclaimed retained Starts exist: its claim for B's identity adopts the retained Start
    // (never appending a duplicate), whereas a claim for an identity nobody retained is reported
    // as `StoreAlreadyLive` — a new live call of that Store — instead of strict divergence, and
    // must not steal the unclaimed Start(C) behind the marker.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
        remove_retry_policy(),
        start_resolution(),
        end_for(7, 47),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));

    match rs
        .claim_start_for_store(
            StartClaim::unowned(
                &HostFunctionName::WallClockNow,
                &DurableFunctionType::ReadLocal,
            ),
            true,
        )
        .await
        .unwrap()
    {
        ReplayStartClaimOutcome::StoreAlreadyLive => {}
        ReplayStartClaimOutcome::Claimed { handle, .. } => {
            panic!(
                "a live Store must not claim an unrelated Start at {}",
                handle.start_idx()
            )
        }
        ReplayStartClaimOutcome::ReplayEnded | ReplayStartClaimOutcome::DeletedRegion => {
            panic!("expected StoreAlreadyLive")
        }
    }
    assert!(rs.has_unclaimed_retained_starts());

    match rs
        .claim_start_for_store(
            StartClaim::unowned(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            ),
            true,
        )
        .await
        .unwrap()
    {
        ReplayStartClaimOutcome::Claimed { handle, .. } => {
            assert_eq!(handle.start_idx(), OplogIndex::from_u64(3));
            match rs.await_resolution(handle).await.unwrap() {
                Resolution::Completed { end_idx, .. } => {
                    assert_eq!(end_idx, OplogIndex::from_u64(5))
                }
                other => panic!("expected Completed, got {other:?}"),
            }
        }
        _ => panic!("a live Store must adopt its retained Start"),
    }
    assert!(!rs.has_unclaimed_retained_starts());

    // Start(C) is not retained (the awaiter stopped at the marker before it), so a live Store's
    // claim for C's identity finds it by scan-ahead behind the unconsumed positional RemoveRetryPolicy(6)
    // rather than reporting a new live call.
    match rs
        .claim_start_for_store(
            StartClaim::unowned(
                &HostFunctionName::MonotonicClockResolution,
                &DurableFunctionType::ReadLocal,
            ),
            true,
        )
        .await
        .unwrap()
    {
        ReplayStartClaimOutcome::Claimed { handle, .. } => {
            assert_eq!(
                handle.start_idx(),
                OplogIndex::from_u64(7),
                "an unclaimed matching Start behind a positional marker is claimed by scan-ahead"
            );
        }
        ReplayStartClaimOutcome::StoreAlreadyLive => {
            panic!("a matching unclaimed Start ahead of the cursor must be claimed, not skipped")
        }
        ReplayStartClaimOutcome::ReplayEnded | ReplayStartClaimOutcome::DeletedRegion => {
            panic!("expected Claimed")
        }
    }
}

#[test]
async fn store_already_live_claim_reports_replay_ended_once_the_cursor_reaches_the_target() {
    // [RecoverySucceeded, Start(A=2 now), Start(B=3 now), End(A=2→4), End(B=3→5), Start(C=6 resolution),
    // End(C=6→7)] — nothing positional stands between A's End and the target, so A's awaiter
    // drains to the target, retaining B and C with their Ends attached. The shared cursor is then
    // exhausted while retained Starts are still unclaimed: a live Store's claim for an identity
    // nobody retained reports `ReplayEnded` (the consumer's replay-to-live is idempotent for a
    // Store that already continued live), and the retained Starts stay for their owners' claims.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
        start_resolution(),
        end_for(6, 46),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.is_live());
    assert!(rs.has_unclaimed_retained_starts());

    match rs
        .claim_start_for_store(
            StartClaim::unowned(
                &HostFunctionName::WallClockNow,
                &DurableFunctionType::ReadLocal,
            ),
            true,
        )
        .await
        .unwrap()
    {
        ReplayStartClaimOutcome::ReplayEnded => {}
        ReplayStartClaimOutcome::Claimed { handle, .. } => {
            panic!(
                "a live Store must not claim an unrelated Start at {}",
                handle.start_idx()
            )
        }
        ReplayStartClaimOutcome::StoreAlreadyLive | ReplayStartClaimOutcome::DeletedRegion => {
            panic!("expected ReplayEnded")
        }
    }
    assert!(rs.has_unclaimed_retained_starts());

    for (name, start, end) in [
        (HostFunctionName::MonotonicClockNow, 3, 5),
        (HostFunctionName::MonotonicClockResolution, 6, 7),
    ] {
        match rs
            .claim_start_for_store(
                StartClaim::unowned(&name, &DurableFunctionType::ReadLocal),
                true,
            )
            .await
            .unwrap()
        {
            ReplayStartClaimOutcome::Claimed { handle, .. } => {
                assert_eq!(handle.start_idx(), OplogIndex::from_u64(start));
                match rs.await_resolution(handle).await.unwrap() {
                    Resolution::Completed { end_idx, .. } => {
                        assert_eq!(end_idx, OplogIndex::from_u64(end))
                    }
                    other => panic!("expected Completed, got {other:?}"),
                }
            }
            _ => panic!("a live Store must adopt its retained Start at {start}"),
        }
    }
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn unclaimed_retained_descendant_reports_only_settled_subtrees() {
    // [RecoverySucceeded, Start(root=2), Start(child=3 ← 2), Start(grandchild=4 ← 3), End(2→5), End(3→6),
    // End(4→7)] — root and child are claimed; awaiting root retains the grandchild. While the
    // child's body is still active, the grandchild is that body's own pending call and is not
    // reported; once no body is active it is the earliest unclaimed descendant of the root.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        owned_start_now(2),
        owned_start_now(3),
        end_for(2, 42),
        end_for(3, 43),
        end_for(4, 44),
    ])
    .await;
    let root = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(root.start_idx(), OplogIndex::from_u64(2));
    let child = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    assert_eq!(child.start_idx(), OplogIndex::from_u64(3));
    assert_eq!(
        rs.unclaimed_retained_descendant(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        None
    );

    match rs.await_resolution(root).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());
    assert_eq!(
        rs.unclaimed_retained_descendant(
            OplogIndex::from_u64(2),
            HashSet::from([OplogIndex::from_u64(3)])
        )
        .await
        .unwrap(),
        None,
        "a retained Start owned by a still active body is not the root's divergence"
    );
    assert_eq!(
        rs.unclaimed_retained_descendant(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        Some(OplogIndex::from_u64(4))
    );
    assert_eq!(
        rs.unclaimed_retained_descendant(OplogIndex::from_u64(3), HashSet::new())
            .await
            .unwrap(),
        Some(OplogIndex::from_u64(4)),
        "the grandchild is also a descendant of the child"
    );
    assert_eq!(
        rs.unclaimed_retained_descendant(OplogIndex::from_u64(4), HashSet::new())
            .await
            .unwrap(),
        None
    );

    let grandchild = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(3),
        )
        .await
        .unwrap();
    assert_eq!(grandchild.start_idx(), OplogIndex::from_u64(4));
    match rs.await_resolution(child).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(6)),
        other => panic!("expected Completed, got {other:?}"),
    }
    match rs.await_resolution(grandchild).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(7)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn closed_retained_starts_are_released_at_live_invocation_end() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(B=3→4), End(A=2→5)] — A's await commits past B's Start
    // (retained) and End (attached to it) before resolving at 5, and B is never claimed before
    // the invocation finishes live. B is closed history, so releasing at the live invocation end
    // clears the retained set (later durable calls take the plain live path) and a late claim now
    // sees the replay tail rather than a stale retained Start.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    rs.switch_cursor_to_live().await.unwrap();
    assert!(
        rs.has_unclaimed_retained_starts(),
        "switch_to_live keeps retained Starts"
    );

    rs.release_retained_starts_at_live_invocation_end()
        .await
        .unwrap();
    assert!(!rs.has_unclaimed_retained_starts());
    let err = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("end of replay"),
        "unexpected error: {err}"
    );
}

#[test]
async fn unclosed_retained_starts_are_rejected_at_live_invocation_end() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4)] — B is retained by A's await, never claimed,
    // and has no terminal in history. Finishing the live invocation would append
    // `AgentInvocationFinished` after an unclosed Start, which the next reconstruction's
    // boundary read rejects; the release must therefore fail now instead of warning, and must
    // leave the retained Start in place.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
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
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    rs.switch_cursor_to_live().await.unwrap();
    assert!(rs.has_unclaimed_retained_starts());

    let err = rs
        .release_retained_starts_at_live_invocation_end()
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("unclosed unclaimed retained Start(s) at 3"),
        "unexpected error: {err}"
    );
    assert!(
        rs.has_unclaimed_retained_starts(),
        "a rejected release must not drop the retained Start"
    );
}

#[test]
async fn retained_terminals_the_cursor_clamped_past_are_found_at_live_invocation_end() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4), RemoveRetryPolicy=5, End(B=3→6)]: A's resolution stops the
    // drain at 4 because the RemoveRetryPolicy at 5 is neither an awaited terminal nor a retainable Start.
    // Switching to live then clamps the head to 6 without committing past B's End, so it is not
    // attached to the retained Start. The release must still recognise B as closed history by
    // reading the recorded prefix rather than rejecting it as unclosed.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(2, 42),
        remove_retry_policy(),
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
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(4));
    rs.switch_cursor_to_live().await.unwrap();
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(6));
    assert!(rs.has_unclaimed_retained_starts());

    rs.release_retained_starts_at_live_invocation_end()
        .await
        .unwrap();
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn shrinking_replay_target_prunes_hidden_retained_starts() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — after B (idx 3) is retained with
    // its End (idx 5) attached, shrinking the target to 4 must detach the now hidden End while
    // keeping the visible Start; shrinking to 2 must drop the hidden Start altogether.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    rs.drain_awaited_terminals().await.unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        let retained = internal
            .retained_starts
            .get(&OplogIndex::from_u64(3))
            .expect("Start(B) retained");
        assert_eq!(
            retained.terminal.as_ref().map(|(idx, _)| *idx),
            Some(OplogIndex::from_u64(5))
        );
    }

    rs.set_replay_target(OplogIndex::from_u64(4)).await.unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        let retained = internal
            .retained_starts
            .get(&OplogIndex::from_u64(3))
            .expect("Start(B) still visible");
        assert!(retained.terminal.is_none(), "hidden End must be detached");
    }
    assert!(rs.has_unclaimed_retained_starts());

    rs.set_replay_target(OplogIndex::from_u64(2)).await.unwrap();
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn retained_start_publishes_the_non_hint_position_only_when_claimed() {
    // [RecoverySucceeded, Start(E=2), Start(B=3 ← 2), End(B=3→4)] — after E is claimed the drain retains the
    // body's unscoped call B together with its End. Physically the cursor is at 4, but the body
    // has not consumed B yet: its `begin_function` must still observe 2, the oplog tip the live
    // run saw before appending Start(B), so that a key derived from that position is stable
    // across the restart. Claiming B publishes the withheld position.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        owned_start_now(2),
        end_for(3, 43),
    ])
    .await;
    let entity = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(entity.start_idx(), OplogIndex::from_u64(2));
    assert_eq!(rs.last_replayed_non_hint_index(), OplogIndex::from_u64(2));

    rs.drain_awaited_terminals().await.unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        let retained = internal
            .retained_starts
            .get(&OplogIndex::from_u64(3))
            .expect("Start(B) retained");
        assert_eq!(
            retained.terminal.as_ref().map(|(idx, _)| *idx),
            Some(OplogIndex::from_u64(4))
        );
    }
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(4));
    assert_eq!(
        rs.last_replayed_non_hint_index(),
        OplogIndex::from_u64(2),
        "a retained Start and its attached End must not publish the non-hint position"
    );

    let body_call = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    assert_eq!(body_call.start_idx(), OplogIndex::from_u64(3));
    assert_eq!(
        rs.last_replayed_non_hint_index(),
        OplogIndex::from_u64(4),
        "claiming the retained Start publishes it together with its attached End"
    );
    match rs.await_resolution(body_call).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected B to complete, got {other:?}"),
    }
    let _ = entity;
}

#[test]
async fn retained_incomplete_start_publishes_its_own_index_when_claimed() {
    // [RecoverySucceeded, Start(E=2), Start(B=3 ← 2)] — the incomplete tail variant: retaining B keeps the
    // position at 2; the owner's claim publishes 3 and then finds the call incomplete.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        owned_start_now(2),
    ])
    .await;
    let entity = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.drain_awaited_terminals().await.unwrap();
    assert!(rs.has_unclaimed_retained_starts());
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));
    assert_eq!(rs.last_replayed_non_hint_index(), OplogIndex::from_u64(2));

    let body_call = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    assert_eq!(body_call.start_idx(), OplogIndex::from_u64(3));
    assert_eq!(rs.last_replayed_non_hint_index(), OplogIndex::from_u64(3));
    assert!(matches!(
        rs.await_resolution_outcome(body_call).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));
    let _ = entity;
}

#[test]
async fn drain_parks_on_positional_marker() {
    // [RecoverySucceeded, Start(A=2), RemoveRetryPolicy(3), End(A=2→4)] — draining parks on the scope marker a
    // positional reader owns; A resolves once that marker has been consumed.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        remove_retry_policy(),
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
    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn drain_parks_on_unawaited_end() {
    // [RecoverySucceeded, Start(A=2), End(scope=99→3), End(A=2→4)] — End(99) is a scope End nobody awaits (its
    // Start was consumed positionally). Draining must park on it and leave it for the positional
    // reader instead of consuming it on A's behalf.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(99, 7),
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
    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::End { .. }));

    match rs.await_resolution(handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
}

#[test]
async fn interleaved_calls_resolve_out_of_order() {
    // [RecoverySucceeded, Start(A), Start(B), End(B=3), End(A=2)] — completion order (B then A) differs from
    // claim order (A then B). Each call resolves by its own start index, not by position.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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
async fn await_resolves_across_unclaimed_sibling_start() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — A is claimed and awaited *before*
    // B is claimed. The awaiter must not suspend on the still-unclaimed Start(B): B's owner may be
    // a task that needs the Store the awaiter holds, so parking would deadlock. Instead the
    // awaiter retains B for its owner, drains its own End and resolves without any other task
    // touching the cursor.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
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

    match tokio::time::timeout(Duration::from_secs(5), rs.await_resolution(handle_a))
        .await
        .expect("awaiting A must resolve across the unclaimed Start(B) instead of parking on it")
        .unwrap()
    {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(rs.has_unclaimed_retained_starts());

    // B's owner claims later and receives the End attached while B was retained.
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    match rs.await_resolution(handle_b).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected Completed, got {other:?}"),
    }
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn speculative_read_does_not_publish_live_cursor() {
    // [RecoverySucceeded, RemoveRetryPolicy(2)] — replay_target = 2. A speculative read of the last entry must
    // NOT make the cursor observably reach the replay target while the read is still
    // rollbackable: the predicate (run after the read) must still see `is_live() == false`. This
    // is the regression guard for "publish only committed cursor state" — a transient live cursor
    // would let a concurrent awaiter falsely conclude end-of-replay.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        remove_retry_policy(),
    ])
    .await;
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
async fn probe_past_lone_unclaimed_start_retains_it_and_reaches_the_tail() {
    // [RecoverySucceeded, Start(A=2)] — replay_target = 2. A predicate probe that reaches an unclaimed `Start`
    // evaluates its predicate against the not-yet-advanced cursor, then commits past the `Start`
    // and retains it: the cursor is live for positional purposes, but the retained `Start` keeps
    // the durable-call view of the Store in replay until its owner claims it (and finds it
    // incomplete at the tail).
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
    ])
    .await;

    let observed_live = std::cell::Cell::new(None);
    let probe = rs
        .try_get_oplog_entry(|_entry| {
            observed_live.set(Some(rs.is_live()));
            false
        })
        .await
        .unwrap();

    assert!(probe.is_none());
    assert_eq!(observed_live.get(), Some(false));
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(2));
    assert!(rs.is_live());
    assert!(rs.has_unclaimed_retained_starts());

    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));
    assert!(!rs.has_unclaimed_retained_starts());
    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));
}

#[test]
async fn positional_reader_drains_awaited_terminal_before_marker() {
    // [RecoverySucceeded, Start(A=2), Start(B=3), End(B=3→4), RemoveRetryPolicy(5), End(A=2→6)] — both A and B
    // are claimed; a positional read for the retry-policy marker must first auto-drain B's End
    // (idx 4) to B's awaiter and only then return the marker (idx 5). It must never steal/return
    // End(B) positionally.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(3, 43),
        remove_retry_policy(),
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

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(5));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

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
    //   [RecoverySucceeded, Start(A=2), Start(scope S=3), Start(B=4), End(B=4→5), End(scope S=3→6), End(A=2→7)]
    // A is claimed and awaited first, but its End sits last; in between are a durable scope (S,
    // claimed by its scope owner) and a fully overlapping sibling call B. This proves: A's awaiter
    // retains the scope Start and B's Start (never consuming or parking on them), attaches their
    // Ends, and resolves; the scope owner and B then adopt their retained Starts with the attached
    // terminals, in any order.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        batched_scope_start(),
        start_now(),
        end_for(4, 44),
        batched_scope_end(3),
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

    match rs.await_resolution(handle_a).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(7)),
        other => panic!("expected Completed, got {other:?}"),
    }
    {
        let internal = rs.cursor.state.lock().await;
        for idx in [3, 4] {
            let retained = internal
                .retained_starts
                .get(&OplogIndex::from_u64(idx))
                .unwrap_or_else(|| panic!("Start at {idx} must be retained"));
            assert!(
                retained.terminal.is_some(),
                "the End of retained Start {idx} must be attached"
            );
        }
    }

    // The scope owner claims S through the scope claim (retained-first), in oplog order before B
    // although B's End was recorded first.
    let (scope_idx, scope_handle) = rs
        .claim_scope_start(
            &HostFunctionName::Custom("<scope:batched-write>".to_string()),
            &DurableFunctionType::WriteRemote,
            None,
        )
        .await
        .unwrap();
    assert_eq!(scope_idx, OplogIndex::from_u64(3));
    match rs.await_resolution(scope_handle).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(6)),
        other => panic!("expected Completed, got {other:?}"),
    }

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
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn awaiter_reaching_tail_across_retained_start_is_incomplete() {
    // [RecoverySucceeded, Start(A=2), Start(B=3)] — A is claimed and awaited but B is never claimed. The
    // awaiter retains B and reaches the replay tail without A's End: it resolves Incomplete
    // (rather than sleeping on the unclaimed Start(B) until switch_to_live) and drops its
    // registration; B stays retained for a late owner.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
    ])
    .await;
    let handle_a = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let start_idx = handle_a.start_idx();

    match rs.await_resolution_outcome(handle_a).await.unwrap() {
        ResolutionOutcome::Incomplete => {}
        other => panic!("expected Incomplete, got {other:?}"),
    }
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            !internal.concurrent_resolver.is_pending(start_idx),
            "an Incomplete resolution must unregister the awaiter"
        );
        assert!(
            internal
                .retained_starts
                .contains_key(&OplogIndex::from_u64(3))
        );
    }
    assert!(rs.has_unclaimed_retained_starts());

    // switch_to_live keeps the retained Start: the late owner still adopts it and, lacking a
    // terminal, is Incomplete as well.
    rs.switch_cursor_to_live().await.unwrap();
    assert!(rs.has_unclaimed_retained_starts());
    let handle_b = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(handle_b.start_idx(), OplogIndex::from_u64(3));
    assert!(!rs.has_unclaimed_retained_starts());
    match rs.await_resolution_outcome(handle_b).await.unwrap() {
        ResolutionOutcome::Incomplete => {}
        other => panic!("expected Incomplete, got {other:?}"),
    }
}

#[test]
async fn reconstruction_claim_is_barrier_visible_when_atomic_claim_returns() {
    let parent = OplogIndex::from_u64(1);
    let start = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
        end_for(2, 1),
    ])
    .await;
    let handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let claim_state = replay.cursor.reconstruction_claims.clone();
    let start_index = handle.start_idx();

    assert_eq!(claim_state.active_fences(), HashSet::from([start_index]));
    assert_eq!(claim_state.active_bodies(), HashSet::from([start_index]));
    let wait = claim_state.wait_for_fences();
    tokio::pin!(wait);
    assert!(
        futures::poll!(wait.as_mut()).is_pending(),
        "the primary barrier must see the reconstruction before claim returns"
    );

    drop(handle);
    wait.await;
    assert!(claim_state.active_fences().is_empty());
    assert!(claim_state.active_bodies().is_empty());
}

#[test]
async fn incomplete_reconstruction_resolution_removes_only_its_fence() {
    let parent = OplogIndex::from_u64(1);
    let start = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
    ])
    .await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let start_index = handle.start_idx();
    let mut reconstruction = handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    let claim_state = replay.cursor.reconstruction_claims.clone();

    replay.switch_cursor_to_live().await.unwrap();
    assert!(claim_state.active_fences().is_empty());
    assert_eq!(claim_state.active_bodies(), HashSet::from([start_index]));
    assert!(matches!(
        replay.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));

    reconstruction.body_settled();
    drop(reconstruction);
    assert!(claim_state.active_bodies().is_empty());
}

#[test]
async fn consumed_reconstruction_terminal_blocks_until_body_validation() {
    let parent = OplogIndex::from_u64(1);
    let start = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
        end_for(2, 1),
    ])
    .await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let start_index = handle.start_idx();
    let mut reconstruction = handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    let claim_state = replay.cursor.reconstruction_claims.clone();

    assert!(matches!(
        replay.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    replay.switch_cursor_to_live().await.unwrap();
    reconstruction.body_settled();
    assert_eq!(claim_state.active_fences(), HashSet::from([start_index]));
    assert!(claim_state.active_bodies().is_empty());
    let wait = claim_state.wait_for_fences();
    tokio::pin!(wait);
    assert!(
        futures::poll!(wait.as_mut()).is_pending(),
        "terminal consumption must not release the validation fence"
    );

    drop(reconstruction);
    wait.await;
    assert!(claim_state.active_fences().is_empty());
}

#[test]
async fn replay_generation_install_check_rejects_leaked_fence_or_body() {
    let parent = OplogIndex::from_u64(1);
    let start = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
        end_for(2, 1),
    ])
    .await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let mut reconstruction = handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");

    assert!(replay.ensure_reconstruction_claims_empty().is_err());
    reconstruction.body_settled();
    assert!(
        replay.ensure_reconstruction_claims_empty().is_err(),
        "a consumed body cannot hide its still-unvalidated fence"
    );
    drop(reconstruction);
    replay.ensure_reconstruction_claims_empty().unwrap();
    drop(handle);

    let start = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
    ])
    .await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let mut reconstruction = handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    replay.switch_cursor_to_live().await.unwrap();
    assert!(
        replay.ensure_reconstruction_claims_empty().is_err(),
        "an incomplete claim's active body must still reject generation replacement"
    );
    reconstruction.body_settled();
    drop(reconstruction);
    replay.ensure_reconstruction_claims_empty().unwrap();
    drop(handle);
}

#[test]
#[test_r::timeout("10s")]
async fn cancelled_resolution_settles_reconstruction_after_owned_cursor_cleanup() {
    let parent = OplogIndex::from_u64(1);
    let start = rejected_tool_reconstruction_start(parent);
    let (replay, oplog) = replay_state_and_oplog_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start,
        remove_retry_policy(),
        end_for(2, 1),
        remove_retry_policy(),
    ])
    .await;
    let handle = claim_rejected_tool_reconstruction(&replay, parent).await;
    let start_index = handle.start_idx();
    assert!(!handle.resolution_ready());
    let claims = replay.cursor.reconstruction_claims.clone();
    let gate = replay.cursor.advance_gate.clone().lock_owned().await;
    let mut resolution = Box::pin(replay.await_resolution_outcome(handle));
    assert!(futures::poll!(resolution.as_mut()).is_pending());
    drop(resolution);

    assert_eq!(claims.active_bodies(), HashSet::from([start_index]));
    assert_eq!(claims.active_fences(), HashSet::from([start_index]));
    assert!(replay.ensure_reconstruction_claims_empty().is_err());
    let mut cleanup = Box::pin(async {
        replay.fence_owned_cursor_ops().await.unwrap();
        claims.wait_for_fences().await;
    });
    assert_pending(&mut cleanup, "cleanup while the advance gate remains held").await;
    drop(gate);
    cleanup.await;

    assert!(claims.active_bodies().is_empty());
    replay.ensure_reconstruction_claims_empty().unwrap();
    assert_eq!(replay.last_replayed_index(), start_index);
    {
        let state = replay.cursor.state.lock().await;
        assert!(!state.concurrent_resolver.is_awaited(start_index));
        assert!(
            state
                .concurrent_resolver
                .owns_terminal(start_index, OplogIndex::from_u64(4))
        );
    }
    // The same admission check used by OwnerExecution now permits a fresh replay generation.
    let replacement = test_replay_state(
        replay.cursor.owned_agent_id.clone(),
        oplog,
        DeletedRegions::default(),
        None,
    )
    .await
    .unwrap();
    let handle = claim_rejected_tool_reconstruction(&replacement, parent).await;
    assert_eq!(handle.start_idx(), start_index);
    drop(handle);
    replacement.ensure_reconstruction_claims_empty().unwrap();
    replay.ensure_reconstruction_claims_empty().unwrap();
}

#[test]
async fn completed_entity_body_detects_unconsumed_owned_start_at_cursor_head() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(2),
        end_for(3, 41),
        end_for(2, 42),
    ])
    .await;
    let outer = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(outer.start_idx(), OplogIndex::from_u64(2));

    assert_eq!(
        rs.unconsumed_scope_head(
            OplogIndex::from_u64(2),
            HashSet::from([OplogIndex::from_u64(3)]),
        )
        .await
        .unwrap(),
        None,
        "a nested reconstructed entity body remains able to claim its own Start"
    );
    assert_eq!(
        rs.unconsumed_scope_head(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        Some(OplogIndex::from_u64(3)),
        "once no nested body can consume the owned Start, it is structural divergence"
    );
}

#[test]
async fn completed_entity_body_uses_explicit_owner_for_anchored_remove_retry_policy() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(2),
        anchored_remove_retry_policy(3),
        end_for(3, 41),
        end_for(2, 42),
    ])
    .await;
    let outer = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let child = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    drop(child);
    drop(outer);

    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));
    assert_eq!(
        rs.unconsumed_scope_head(
            OplogIndex::from_u64(2),
            HashSet::from([OplogIndex::from_u64(3)]),
        )
        .await
        .unwrap(),
        None,
        "the active nested entity body can still consume its explicitly anchored RemoveRetryPolicy"
    );
    assert_eq!(
        rs.unconsumed_scope_head(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        Some(OplogIndex::from_u64(4)),
        "once the owner settles without a live resolver, the anchored RemoveRetryPolicy is divergence"
    );
    assert_eq!(
        rs.unconsumed_scope_head(
            OplogIndex::from_u64(2),
            HashSet::from([OplogIndex::from_u64(99)]),
        )
        .await
        .unwrap(),
        Some(OplogIndex::from_u64(4)),
        "an unrelated active entity body must not mask the divergence"
    );
}

#[test]
fn scope_entry_owner_prefers_error_entity_anchor_over_retry_group() {
    let error = anchored_error(2, 3);

    assert_eq!(
        cursor::scope_entry_owner(OplogIndex::from_u64(4), &error),
        Some(OplogIndex::from_u64(2)),
        "the explicit entity owner must take precedence over retry_from"
    );
}

#[test]
async fn deferred_anchored_error_is_not_structural_divergence() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        remove_retry_policy(),
        end_for(2, 42),
        delivered_for(2),
        anchored_error(2, 3),
    ])
    .await;
    let root = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    rs.await_resolution(root).await.unwrap();
    let (begin_idx, begin) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(begin_idx, OplogIndex::from_u64(3));
    assert!(matches!(begin, OplogEntry::RemoveRetryPolicy { .. }));
    rs.drain_awaited_terminals().await.unwrap();
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(4));
    assert_eq!(
        rs.unconsumed_scope_head(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        Some(OplogIndex::from_u64(5)),
        "CompletionDelivered remains a non-skippable delivery barrier"
    );

    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(5))
        .await
        .unwrap();
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(5),
        "the anchored Error must remain exposed at the head until delivery is acknowledged"
    );
    assert_eq!(
        rs.unconsumed_scope_head(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        None,
        "a deferred auto-skippable Error hint is not structural divergence"
    );
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    barrier.acknowledge();
}

#[test]
async fn deferred_root_error_ignores_live_retry_from_descendant() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(2),
        end_for(2, 42),
        delivered_for(2),
        anchored_error(2, 3),
    ])
    .await;
    let root = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let descendant = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    rs.await_resolution(root).await.unwrap();
    rs.drain_awaited_terminals().await.unwrap();
    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(5))
        .await
        .unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal
                .concurrent_resolver
                .is_awaited(OplogIndex::from_u64(3)),
            "the retry_from descendant must still have a live awaiter"
        );
    }
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    assert_eq!(
        rs.unconsumed_scope_head(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        None,
        "the deferred Error remains non-divergent after its explicit root owner settles"
    );
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    barrier.acknowledge();
    drop(descendant);
}

#[test]
async fn completed_entity_body_waits_for_active_owner_of_retried_transaction_begin() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(2),
        remove_retry_policy(),
        OplogEntry::begin_remote_transaction(
            golem_common::model::TransactionId::new("retried".to_string()),
            OplogIndex::from_u64(3),
        ),
        end_for(3, 41),
        end_for(2, 42),
    ])
    .await;
    let _outer = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let _child = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();

    let (index, _) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(index, OplogIndex::from_u64(4));
    assert_eq!(
        rs.unconsumed_scope_head(
            OplogIndex::from_u64(2),
            HashSet::from([OplogIndex::from_u64(3)]),
        )
        .await
        .unwrap(),
        None,
        "a retried transaction Begin belongs to its original scope Start even when it is not adjacent"
    );
}

#[test]
async fn completed_entity_body_does_not_reject_auto_drainable_dropped_call_terminal() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(2),
        cancelled_for(3),
        end_for(2, 42),
    ])
    .await;
    let outer = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let dropped_child = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    drop(dropped_child);

    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));
    assert_eq!(
        rs.unconsumed_scope_head(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        None,
        "a pending terminal with a dropped receiver remains auto-drainable"
    );

    rs.drain_awaited_terminals().await.unwrap();
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    match rs.await_resolution(outer).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(5)),
        other => panic!("expected completed outer call, got {other:?}"),
    }
}

#[test]
async fn visible_terminal_scan_crosses_multiple_chunks() {
    let mut entries = vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
    ];
    entries.extend(
        std::iter::repeat_with(|| OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        })
        .take(CHUNK_SIZE as usize + 1),
    );
    entries.push(end_for(2, 42));
    let rs = replay_state_over(entries).await;

    assert!(
        rs.has_visible_terminal(OplogIndex::from_u64(2)).await,
        "entity execution mode classification must scan through the complete replay prefix"
    );
}

#[test]
async fn visible_scope_descendant_distinguishes_owned_work_from_siblings() {
    let only_sibling = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_now(),
        end_for(3, 41),
    ])
    .await;
    assert!(
        !only_sibling
            .has_visible_scope_descendant(OplogIndex::from_u64(2))
            .await,
        "a later sibling must not be mistaken for historical entity-body work"
    );

    let owned_child = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(2),
        end_for(3, 41),
    ])
    .await;
    assert!(
        owned_child
            .has_visible_scope_descendant(OplogIndex::from_u64(2))
            .await,
        "a nested Start proves the historical entity body began execution"
    );
}

fn log_entry() -> OplogEntry {
    OplogEntry::Log {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        level: LogLevel::Info,
        context: "ctx".to_string(),
        message: "msg".to_string(),
        trace_context: None,
    }
}

/// Sequential positional replay reads each oplog entry from the oplog once. Committing an entry
/// peeks at the entry behind it to skip trailing hints; the peeked non-hint entry must stay in
/// the prefetch buffer, otherwise every commit discards the prefetched chunk and reads it again.
#[test]
async fn sequential_replay_reads_each_oplog_entry_once() {
    // The first entry is consumed when the replay state is built; the rest alternate hint and
    // non-hint entries so that every commit skips forward over a trailing hint.
    let mut entries = vec![OplogEntry::RecoverySucceeded {
        timestamp: Timestamp::now_utc(),
    }];
    for _ in 0..8 {
        entries.push(remove_retry_policy());
        entries.push(log_entry());
        entries.push(remove_retry_policy());
    }
    let expected: Vec<OplogIndex> = entries
        .iter()
        .enumerate()
        .skip(1)
        .filter(|(_, entry)| !entry.is_hint())
        .map(|(position, _)| OplogIndex::from_u64(position as u64 + 1))
        .collect();
    let (rs, oplog) = replay_state_and_oplog_over(entries).await;
    // Building the replay state prefetched every remaining entry in one chunk.
    oplog.reads.lock().unwrap().clear();

    let mut read_indices = Vec::new();
    while rs.is_replay() {
        let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
        assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
        read_indices.push(idx);
    }
    assert_eq!(read_indices, expected);

    let reads = oplog.reads.lock().unwrap().clone();
    assert!(
        reads.is_empty(),
        "replay re-read already prefetched entries: {reads:?}"
    );
}

/// When replay reaches the target via a skipped-region jump (`get_out_of_skipped_region` jumps
/// the cursor to the region end, which is the target) rather than by consuming the target entry,
/// the transition to live must still synthesize `ReplayFinished`.
#[test]
async fn replay_finished_emitted_when_skipped_region_reaches_target() {
    // [RecoverySucceeded(1), Start(2), Log(3), Log(4)] with deleted region [3, 4]: consuming the Start at 2
    // jumps the cursor over the deleted tail straight to the target (4).
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        log_entry(),
        log_entry(),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(3),
        end: OplogIndex::from_u64(4),
    }]);
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
        .await
        .expect("failed to build replay state");

    assert!(!rs.is_live(), "Start at 2 is not yet consumed");
    let ReplayStartClaimOutcome::Claimed { handle, .. } = rs
        .claim_start_or_replay_end(StartClaim::unowned(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ))
        .await
        .unwrap()
    else {
        panic!("Start at 2 must be claimable");
    };
    assert_eq!(handle.start_idx(), OplogIndex::from_u64(2));

    assert!(
        rs.is_live(),
        "consuming the Start must jump over the deleted tail to the target"
    );
    assert!(matches!(
        rs.switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
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
    // [RecoverySucceeded(1), Start(2), End(3)] — replay becomes live by consuming the End at the target (3).
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
    ])
    .await;
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
    assert!(matches!(
        rs.switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
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
        span_finished: None,
    }
}

fn cancelled_with_partial_for(start_index: u64, nanos: u64) -> OplogEntry {
    OplogEntry::Cancelled {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        partial: Some(OplogPayload::Inline(Box::new(
            HostResponse::MonotonicClockTimestamp(HostResponseMonotonicClockTimestamp { nanos }),
        ))),
        span_finished: None,
    }
}

fn remove_other_retry_policy() -> OplogEntry {
    OplogEntry::RemoveRetryPolicy {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: None,
        name: "other-policy".to_string(),
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

fn transaction_scope_start(parent: Option<u64>) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: parent.map(OplogIndex::from_u64),
        function_name: HostFunctionName::Custom("<scope:transaction>".to_string()),
        invocation_id: None,
        observational_owner: None,
        request: None,
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: None,
    }
}

fn transaction_terminal(
    start: u64,
    outcome: golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal,
) -> OplogEntry {
    OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start),
        response: Some(OplogPayload::Inline(Box::new(
            HostResponse::GolemRdbmsTransactionTerminal(
                golem_common::model::oplog::HostResponseGolemRdbmsTransactionTerminal { outcome },
            ),
        ))),
        forced_commit: true,
        span_finished: None,
        span_attributes: None,
    }
}

#[test]
#[test_r::timeout("30s")]
async fn transaction_failed_begin_cannot_steal_later_successful_begin() {
    use golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal as Outcome;
    let failed = Outcome::BeginFailed(Err(crate::services::rdbms::RdbmsError::ConnectionFailure(
        "begin refused".to_string(),
    )
    .into()));
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        transaction_scope_start(None),
        transaction_terminal(2, failed.clone()),
        transaction_scope_start(None),
        OplogEntry::begin_remote_transaction(
            golem_common::model::TransactionId::new("successful".to_string()),
            OplogIndex::from_u64(4),
        ),
        transaction_terminal(4, Outcome::Commit(Ok(()))),
    ])
    .await;
    let name = HostFunctionName::Custom("<scope:transaction>".to_string());
    let (first, handle) = rs
        .claim_scope_start(&name, &DurableFunctionType::WriteRemote, None)
        .await
        .unwrap();
    assert_eq!(first, OplogIndex::from_u64(2));
    assert!(
        rs.transaction_marker(first, |e| matches!(
            e,
            OplogEntry::BeginRemoteTransaction { .. }
        ))
        .is_none()
    );
    let Resolution::Completed {
        response: Some(OplogPayload::Inline(response)),
        ..
    } = rs.await_resolution(handle).await.unwrap()
    else {
        panic!("failed begin must resolve its own terminal");
    };
    let HostResponse::GolemRdbmsTransactionTerminal(response) = *response else {
        panic!("failed begin must preserve the terminal discriminator");
    };
    assert_eq!(response.outcome, failed);
    let (second, handle) = rs
        .claim_scope_start(&name, &DurableFunctionType::WriteRemote, None)
        .await
        .unwrap();
    assert_eq!(second, OplogIndex::from_u64(4));
    assert!(
        matches!(rs.transaction_marker(second, |e| matches!(e, OplogEntry::BeginRemoteTransaction { .. })), Some(OplogEntry::BeginRemoteTransaction { transaction_id, .. }) if transaction_id.to_string() == "successful")
    );
    rs.await_resolution(handle).await.unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn transaction_interleaved_markers_resolve_by_owner_without_blocking_scope_ends() {
    use golem_common::model::oplog::payload::types::SerializableRdbmsTransactionTerminal as Outcome;
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        transaction_scope_start(Some(2)),
        transaction_scope_start(Some(2)),
        OplogEntry::begin_remote_transaction(
            golem_common::model::TransactionId::new("a".to_string()),
            OplogIndex::from_u64(3),
        ),
        OplogEntry::begin_remote_transaction(
            golem_common::model::TransactionId::new("b".to_string()),
            OplogIndex::from_u64(4),
        ),
        pre_commit_remote_transaction(3),
        pre_commit_remote_transaction(4),
        committed_remote_transaction(3),
        transaction_terminal(3, Outcome::Commit(Ok(()))),
        committed_remote_transaction(4),
        transaction_terminal(4, Outcome::Commit(Ok(()))),
        end_for(2, 42),
    ])
    .await;
    let root = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let name = HostFunctionName::Custom("<scope:transaction>".to_string());
    let mut scopes = Vec::new();
    for expected in [3, 4] {
        let (owner, handle) = rs
            .claim_scope_start(
                &name,
                &DurableFunctionType::WriteRemote,
                Some(OplogIndex::from_u64(2)),
            )
            .await
            .unwrap();
        assert_eq!(owner, OplogIndex::from_u64(expected));
        assert!(
            rs.transaction_marker(owner, |e| e.is_pre_commit_remote_transaction(owner))
                .is_some()
        );
        assert!(
            rs.transaction_marker(owner, |e| e.is_committed_remote_transaction(owner))
                .is_some()
        );
        assert!(
            rs.transaction_marker(owner, |e| e.is_pre_rollback_remote_transaction(owner))
                .is_none()
        );
        scopes.push(handle);
    }
    // Await B first: A's markers and End must not need a positional reader in A's Store.
    for handle in scopes.into_iter().rev() {
        assert!(matches!(
            rs.await_resolution(handle).await.unwrap(),
            Resolution::Completed { .. }
        ));
    }
    rs.await_resolution(root).await.unwrap();
    assert!(!rs.has_unclaimed_retained_starts());
    // Cutting the replay target hides B's confirmation without hiding A's confirmation.
    rs.set_replay_target(OplogIndex::from_u64(10))
        .await
        .unwrap();
    assert!(
        rs.transaction_marker(OplogIndex::from_u64(4), |e| e
            .is_committed_remote_transaction(OplogIndex::from_u64(4)))
            .is_none()
    );
    rs.set_replay_target(OplogIndex::from_u64(13))
        .await
        .unwrap();
    assert!(
        rs.transaction_marker(OplogIndex::from_u64(3), |e| e
            .is_committed_remote_transaction(OplogIndex::from_u64(3)))
            .is_some()
    );
    assert!(
        rs.transaction_marker(OplogIndex::from_u64(4), |e| e
            .is_committed_remote_transaction(OplogIndex::from_u64(4)))
            .is_some()
    );
}

#[test]
#[test_r::timeout("30s")]
async fn rdbms_incomplete_stream_repair_reconstructs_typed_loss_at_original_start() {
    let scope = OplogIndex::from_u64(2);
    let original = OplogIndex::from_u64(3);
    let function = HostFunctionName::RdbmsMysqlDbConnectionQueryStream;
    let function_type = DurableFunctionType::WriteRemote;
    let (rs, oplog) = replay_state_and_oplog_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        batched_scope_start(),
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(scope),
            function_name: function.clone(),
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: function_type.clone(),
            span_started: None,
        },
    ])
    .await;
    let name = HostFunctionName::Custom("<scope:batched-write>".to_string());
    let (_, scope_handle) = rs
        .claim_scope_start(&name, &DurableFunctionType::WriteRemote, None)
        .await
        .unwrap();
    let handle = rs
        .claim_owned_concurrent_start(&function, &function_type, scope)
        .await
        .unwrap();
    assert_eq!(handle.start_idx(), original);
    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));
    assert!(matches!(
        rs.await_resolution_outcome(scope_handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));
    let loss = golem_common::model::oplog::HostResponseGolemRdbmsRequest {
        request: Err(crate::services::rdbms::RdbmsError::ConnectionFailure(
            "Database result stream was lost during reconstruction".to_string(),
        )
        .into()),
    };
    let mut terminal = end_for(3, 0);
    let OplogEntry::End {
        response,
        forced_commit,
        ..
    } = &mut terminal
    else {
        unreachable!()
    };
    *response = Some(OplogPayload::Inline(Box::new(
        HostResponse::GolemRdbmsRequest(loss.clone()),
    )));
    *forced_commit = true;
    oplog.add(terminal).await.unwrap();
    oplog.add(batched_scope_end(2)).await.unwrap();
    let recovered = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::default(),
        None,
    )
    .await
    .unwrap();
    let (_, scope_handle) = recovered
        .claim_scope_start(&name, &DurableFunctionType::WriteRemote, None)
        .await
        .unwrap();
    let handle = recovered
        .claim_owned_concurrent_start(&function, &function_type, scope)
        .await
        .unwrap();
    assert_eq!(handle.start_idx(), original);
    let Resolution::Completed {
        response: Some(OplogPayload::Inline(response)),
        ..
    } = recovered.await_resolution(handle).await.unwrap()
    else {
        panic!("stream repair must persist its typed terminal");
    };
    assert_eq!(*response, HostResponse::GolemRdbmsRequest(loss));
    recovered.await_resolution(scope_handle).await.unwrap();
    let entries = oplog.entries.lock().unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, OplogEntry::Start { .. }))
            .count(),
        2
    );
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, OplogEntry::BeginRemoteTransaction { .. }))
    );
}

/// A non-hint positional retry-policy marker. These are never claimed and never auto-drained; a
/// positional reader must consume them, and an overlapping awaiter parks on them until then.
fn random_positional_marker(rng: &mut rand::rngs::StdRng) -> OplogEntry {
    use rand::Rng;
    match rng.random_range(0..2) {
        0 => remove_retry_policy(),
        _ => remove_other_retry_policy(),
    }
}

/// A generated item in a fabricated overlap layout: either a concurrent call (claimed +
/// awaited) or a durable scope (a request-less `<scope:batched-write>` `Start`/`End` pair claimed
/// by its scope owner through the scope claim and awaited like a call, standing in for a durable
/// scope / rdbms transaction span).
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
    ScopeStart(usize),
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
        let mut entries = vec![OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        }];
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
/// concurrent calls (completed / cancelled / incomplete) with durable **scopes** (request-less
/// `Start`/`End` pairs claimed by their scope owner and awaited like calls) and non-hint
/// positional **markers** (retry-policy boundaries), all
/// freely interleaved with `Log` hints, so that a sibling's scope `End` or a marker can land
/// between another call's `Start` and `End` — the headline overlap layout generalized.
///
/// Each call's and scope's resolution is awaited on its **own concurrently-suspended task**
/// (`tokio::spawn`), mirroring the production model where the worker drives the replay cursor
/// (claims + positional reads) while several call futures are suspended; this is what exercises
/// the genuine suspend/resume path (`await_resolution_outcome` parking on a positional blocker
/// and resuming on cursor progress), not just the auto-drain-at-head path. A single driver walks
/// the oplog left-to-right, claiming call and scope `Start`s, positionally reading markers, and
/// leaving terminals to be auto-drained. It asserts that:
///
/// - each claim / positional read returns exactly the entry at the expected oplog index,
///   independent of how the suspended awaiter tasks are scheduled (auto-drains only ever consume
///   awaited terminals, never a positional entry a reader owns; an awaiter that walks past a
///   sibling's not-yet-claimed `Start` retains it for that sibling's later claim);
/// - every call and scope resolves to exactly its recorded terminal (`End`/`Cancelled` by
///   index) or, for a committed `Start` with no terminal, `Incomplete`;
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
        let mut entries = vec![OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        }];
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
                    let (entry, role) = match items[item] {
                        ItemKind::Call(_) => (start_now(), Role::CallStart(item)),
                        ItemKind::Scope => (batched_scope_start(), Role::ScopeStart(item)),
                    };
                    entries.push(entry);
                    start_idx[item] = entries.len() as u64;
                    opened[item] = true;
                    roles.push(role);
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
                        ItemKind::Scope => (batched_scope_end(si), Role::ScopeEnd),
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
                Role::ScopeStart(item) => {
                    let (got, handle) = rs
                        .claim_scope_start(
                            &HostFunctionName::Custom("<scope:batched-write>".to_string()),
                            &DurableFunctionType::WriteRemote,
                            None,
                        )
                        .await
                        .unwrap_or_else(|e| {
                            panic!("seed {seed}: scope claim of item {item} at {idx} failed: {e}")
                        });
                    assert_eq!(
                        got,
                        OplogIndex::from_u64(idx),
                        "seed {seed}: scope claim of item {item} returned the wrong Start"
                    );
                    let rs2 = rs.clone();
                    tasks.push((
                        item,
                        tokio::spawn(async move { rs2.await_resolution_outcome(handle).await }),
                    ));
                }
                Role::Marker => {
                    let (got, _) = rs.get_oplog_entry(None).await.unwrap_or_else(|e| {
                        panic!("seed {seed}: positional read at {idx} ({role:?}) failed: {e}")
                    });
                    assert_eq!(
                        got,
                        OplogIndex::from_u64(idx),
                        "seed {seed}: positional read ({role:?}) returned the wrong index"
                    );
                }
                // Call and scope terminals are auto-drained to their awaiter; hints are skipped
                // by the preceding consume's skip_forward. Neither is walked explicitly.
                Role::CallTerminal(_) | Role::ScopeEnd | Role::Hint => {}
                Role::Placeholder => unreachable!("placeholder is skipped"),
            }
        }

        // Join the suspended awaiter tasks and check each call and scope resolved to its
        // recorded terminal.
        for (item, task) in tasks {
            let outcome = task
                .await
                .expect("awaiter task panicked")
                .unwrap_or_else(|e| panic!("seed {seed}: await of item {item} failed: {e}"));
            let kind = match items[item] {
                ItemKind::Call(kind) => kind,
                ItemKind::Scope => CallKind::Completed,
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
        assert!(
            !rs.has_unclaimed_retained_starts(),
            "seed {seed}: the full walk left an unclaimed retained Start"
        );
        let internal = rs.cursor.state.lock().await;
        for (i, &si) in start_idx.iter().enumerate() {
            assert!(
                !internal
                    .concurrent_resolver
                    .is_pending(OplogIndex::from_u64(si)),
                "seed {seed}: item {i} left a registered awaiter"
            );
        }
    }
}

/// An `End` whose `Start` lies inside a deleted region (a jump/revert cut between the pair) is
/// an *orphan terminal*: the cursor must consume it transparently instead of surfacing it to a
/// positional reader, and a later call must still claim and resolve at its true indices.
#[test]
async fn orphan_end_with_deleted_start_is_skipped() {
    // [RecoverySucceeded(1), Start(2), End(2→3), Start(4), End(4→5)] with deleted region [2, 2]: the End at
    // 3 is orphaned; the kept call (4, 5) must claim and resolve normally across it.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        start_now(),
        end_for(2, 1),
        start_now(),
        end_for(4, 2),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(2),
        end: OplogIndex::from_u64(2),
    }]);
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
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
    // [RecoverySucceeded(1), Start(2), Cancelled(2→3)] with deleted region [2, 2].
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        cancelled_for(2),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(2),
        end: OplogIndex::from_u64(2),
    }]);
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
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
    // [RecoverySucceeded(1), Start(2), End(2→3), RemoveRetryPolicy(4)] with deleted region [2, 2]: the positional read
    // must consume the orphan End at 3 and return the RemoveRetryPolicy at 4.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 1),
        remove_retry_policy(),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(2),
        end: OplogIndex::from_u64(2),
    }]);
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
        .await
        .expect("failed to build replay state");

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(4));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
    assert!(rs.is_live());
}

/// The inverse partial deletion — the `Start` kept, its terminal inside a deleted region —
/// reports `Incomplete` (the caller may re-execute the call), never an error or a hang.
#[test]
async fn deleted_terminal_reports_incomplete() {
    // [RecoverySucceeded(1), Start(2), End(2→3)] with deleted region [3, 3].
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 1),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion {
        start: OplogIndex::from_u64(3),
        end: OplogIndex::from_u64(3),
    }]);
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
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
        let mut entries = vec![OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        }];
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
            oplog.add(entry).await.unwrap();
        }
        let oplog: Arc<dyn Oplog> = oplog;
        let skipped = DeletedRegions::from_regions(regions.iter().map(|&(s, e)| OplogRegion {
            start: OplogIndex::from_u64(s),
            end: OplogIndex::from_u64(e),
        }));
        let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
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

fn suspend() -> OplogEntry {
    OplogEntry::Suspend {
        timestamp: Timestamp::now_utc(),
    }
}

/// A batched-write scope `Start` exactly as `begin_function` records it: request-less,
/// top-level, `WriteRemote`.
fn batched_scope_start() -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::Custom("<scope:batched-write>".to_string()),
        invocation_id: None,
        observational_owner: None,
        request: None,
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: None,
    }
}

/// A batched-write scope `Start` recorded by an entity body at `parent`.
fn nested_batched_scope_start(parent: u64) -> OplogEntry {
    let mut entry = batched_scope_start();
    let OplogEntry::Start {
        parent_start_index,
        function_name,
        ..
    } = &mut entry
    else {
        unreachable!();
    };
    *parent_start_index = Some(OplogIndex::from_u64(parent));
    *function_name = HostFunctionName::Custom("<scope:batched-write>".to_string());
    entry
}

/// A batched-write scope `End` exactly as `end_function` records it: response-less,
/// `forced_commit: true`.
fn batched_scope_end(start_index: u64) -> OplogEntry {
    OplogEntry::End {
        timestamp: Timestamp::now_utc(),
        start_index: OplogIndex::from_u64(start_index),
        response: None,
        forced_commit: true,
        span_finished: None,
        span_attributes: None,
    }
}

/// A host-call `Start` nested in the batched-write scope at `parent`, exactly as the
/// sequential adapter records followup batched invocations:
/// `parent_start_index: Some(scope)`, `WriteRemote`.
fn batched_child_start(parent: u64) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: Some(OplogIndex::from_u64(parent)),
        function_name: HostFunctionName::MonotonicClockNow,
        invocation_id: None,
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: None,
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
/// - an adjacent pair between positional retry-policy removals,
/// - a batched-write scope (request-less scope `Start`, a child call pair recorded with
///   `parent_start_index: Some(scope)` / `WriteRemote`, and the
///   response-less, forced-commit scope `End`),
/// - no `Cancelled` entries and no overlapping calls anywhere.
///
/// Replay drives the same claim/await sequence the sequential durability layer performs:
/// each call is claimed then awaited immediately, scope `End`s resolve through the
/// resolver, and positional markers are consumed by `get_oplog_entry`.
#[test]
async fn pre_migration_adjacent_pair_oplog_replays_through_concurrent_resolver() {
    // [ 1: RemoveRetryPolicy,
    //   2: Start(A), 3: End(A=2, 41),
    //   4: Suspend (hint),
    //   5: Start(B), 6: End(B=5, 42),
    //   7: RemoveRetryPolicy, 8: Start(C), 9: End(C=8, 43), 10: RemoveRetryPolicy(7),
    //   11: Start(scope), 12: Start(D, parent=11), 13: End(D=12, 44), 14: End(scope=11) ]
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 41),
        suspend(),
        start_now(),
        end_for(5, 42),
        remove_retry_policy(),
        start_now(),
        end_for(8, 43),
        remove_other_retry_policy(),
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

    // Retry-policy removals are positional; call C replays between them.
    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(7));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

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

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(10));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    // Batched-write scope: scope Start claims through the resolver, the child call
    // is claimed by identity (parent_start_index), and the scope End resolves response-less.
    let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());
    let (scope_idx, scope_handle) = rs
        .claim_scope_start(&scope_name, &DurableFunctionType::WriteRemote, None)
        .await
        .unwrap();
    assert_eq!(scope_idx, OplogIndex::from_u64(11));

    let handle_d = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::WriteRemote,
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

async fn wallet_replay_fixture(
    entries: Vec<OplogEntry>,
    target: u64,
    boundary: Arc<tokio::sync::Mutex<()>>,
) -> (
    ReplayState,
    Arc<crate::worker::owner_wallet::OwnerWallet>,
    Arc<InMemoryOplog>,
    Arc<crate::services::card_interest::CardInterestIndex>,
) {
    use crate::worker::owner_wallet::{OwnerWallet, WalletBaseline};
    use crate::worker::tasks::{RuntimeAppendTasks, WorkerTasks};
    use golem_common::model::agent::ResolvedOwnerContext;

    let metadata = wallet_test_component(ComponentId::new(), EnvironmentId::new());
    let owner_id = OwnedAgentId::new(
        metadata.environment_id,
        &AgentId {
            component_id: metadata.id,
            agent_id: "wallet-replay-contract".to_string(),
        },
    );
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in entries {
        oplog.add(entry).await.unwrap();
    }
    let baseline = WalletBaseline::new(
        owner_id.clone(),
        ResolvedOwnerContext::ComponentWorker,
        Arc::new(WalletComponentService {
            metadata: metadata.clone(),
            revisions: Default::default(),
        }),
        &metadata,
        oplog.as_ref(),
        None,
        Vec::new(),
    )
    .await
    .unwrap();
    let interest_index = Arc::new(crate::services::card_interest::CardInterestIndex::new());
    let wallet = OwnerWallet::new(
        baseline,
        Arc::new(RuntimeAppendTasks::default()),
        WorkerTasks::default(),
        boundary,
        interest_index.clone(),
    )
    .await
    .unwrap();
    wallet.activate_interest().await;
    let replay = ReplayState::new_for_owner(
        owner_id,
        oplog.clone(),
        DeletedRegions::default(),
        None,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        OplogIndex::from_u64(target),
        Some(wallet.clone()),
    )
    .await
    .unwrap();
    (replay, wallet, oplog, interest_index)
}

#[test]
#[test_r::timeout("10s")]
async fn entity_wallet_prefix_is_bound_to_consumption_not_store_construction() {
    for (claim_early, body_before_start) in [(true, false), (false, false), (true, true)] {
        let parent = OplogIndex::INITIAL;
        let start = OplogIndex::from_u64(4);
        let first = stored_test_card(CardId::new());
        let second = stored_test_card(CardId::new());
        let (rs, wallet, _oplog, _interest) = wallet_replay_fixture(
            vec![
                remove_retry_policy(),
                if body_before_start {
                    anchored_remove_retry_policy(4)
                } else {
                    remove_retry_policy()
                },
                OplogEntry::card_installed(None, None, Box::new(first), 1),
                rejected_tool_reconstruction_start(parent),
                OplogEntry::card_installed(None, None, Box::new(second), 2),
                end_for(4, 1),
                remove_retry_policy(),
            ],
            7,
            Arc::new(tokio::sync::Mutex::new(())),
        )
        .await;
        let mut handle = if claim_early {
            Some(claim_rejected_tool_reconstruction(&rs, parent).await)
        } else {
            None
        };
        if claim_early {
            // A pre-Start body can observe its claim prefix before its positional policy call.
            assert_eq!(
                wallet
                    .entity_replay_wallet(start)
                    .unwrap()
                    .receipt()
                    .result
                    .await
                    .unwrap()
                    .wallet
                    .generation,
                0
            );
        }
        let (index, _) = rs
            .get_oplog_entry(body_before_start.then_some(start))
            .await
            .unwrap();
        assert_eq!(index, OplogIndex::from_u64(2));
        // Ordinary draining passes the entity Start and its later wallet mutation. A late claim
        // and delayed Store construction must still use the earlier entity prefix.
        let mut tx = rs.cursor.tx().await.unwrap();
        tx.try_get_oplog_entry(|_| false).await.unwrap();
        rs.cursor.finish_tx(tx);
        if handle.is_none() {
            handle = Some(claim_rejected_tool_reconstruction(&rs, parent).await);
        }
        let mut handle = handle.unwrap();
        let mut reconstruction = handle.take_historical_reconstruction().unwrap();
        let entity = wallet.entity_replay_wallet(start).unwrap();
        assert_eq!(
            entity.receipt().result.await.unwrap().wallet.generation,
            if body_before_start { 0 } else { 1 }
        );
        assert_eq!(
            wallet
                .latest_replay_receipt()
                .result
                .await
                .unwrap()
                .wallet
                .generation,
            2
        );
        rs.await_resolution(handle).await.unwrap();
        reconstruction.body_settled();
        drop(reconstruction);
        assert!(wallet.entity_replay_wallet(start).is_err());
        // A retained Store can keep its frozen evidence after the reconstruction guard releases.
        assert_eq!(
            entity.receipt().result.await.unwrap().wallet.generation,
            if body_before_start { 0 } else { 1 }
        );
    }
}

#[test]
#[test_r::timeout("10s")]
async fn entity_wallet_prefix_retains_preclaim_hints_and_prunes_future_starts() {
    let start = OplogIndex::from_u64(4);
    for prune in [false, true] {
        let (rs, wallet, _oplog, _interest) = wallet_replay_fixture(
            vec![
                remove_retry_policy(),
                OplogEntry::card_installed(
                    Some(start),
                    None,
                    Box::new(stored_test_card(CardId::new())),
                    1,
                ),
                OplogEntry::card_installed(
                    None,
                    None,
                    Box::new(stored_test_card(CardId::new())),
                    2,
                ),
                rejected_tool_reconstruction_start(OplogIndex::INITIAL),
                end_for(4, 1),
                remove_retry_policy(),
            ],
            6,
            Arc::new(tokio::sync::Mutex::new(())),
        )
        .await;
        // Construction skips the hints before any entity claim exists.
        assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));
        let entity = wallet.entity_replay_wallet(start).unwrap();
        assert_eq!(entity.receipt().result.await.unwrap().wallet.generation, 0);
        assert_eq!(
            wallet
                .latest_replay_receipt()
                .result
                .await
                .unwrap()
                .wallet
                .generation,
            2
        );
        if prune {
            rs.set_replay_target(OplogIndex::from_u64(3)).await.unwrap();
        } else {
            let mut handle = claim_rejected_tool_reconstruction(&rs, OplogIndex::INITIAL).await;
            let mut reconstruction = handle.take_historical_reconstruction().unwrap();
            assert!(Arc::ptr_eq(
                &entity,
                &wallet.entity_replay_wallet(start).unwrap()
            ));
            rs.await_resolution(handle).await.unwrap();
            assert_eq!(entity.receipt().result.await.unwrap().wallet.generation, 0);
            reconstruction.body_settled();
            drop(reconstruction);
        }
        assert!(wallet.entity_replay_wallet(start).is_err());
    }
}

#[test]
#[test_r::timeout("10s")]
async fn benign_card_suffix_reconstructs_before_wallet_boundary() {
    let card = stored_test_card(CardId::new());
    let (rs, wallet, oplog, _interest_index) = wallet_replay_fixture(
        vec![
            remove_retry_policy(),
            OplogEntry::card_installed(None, None, Box::new(card.clone()), 1),
            remove_retry_policy(),
        ],
        3,
        Arc::new(tokio::sync::Mutex::new(())),
    )
    .await;
    let canonical = wallet.snapshot();
    rs.get_oplog_entry(None).await.unwrap();
    assert!(!rs.is_live_published());
    assert!(matches!(
        rs.switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
    let reads = oplog.reads.lock().unwrap().clone();
    let receipt = wallet.latest_replay_receipt().result.await.unwrap();
    assert_eq!(receipt.wallet.cards[&card.card_id()], card);
    assert!(Arc::ptr_eq(&canonical, &wallet.snapshot()));
    drop(wallet.lock_boundary().await.unwrap());
    assert_eq!(wallet.snapshot().cards[&card.card_id()], card);
    assert_eq!(wallet.snapshot().generation, 1);
    assert_eq!(*oplog.reads.lock().unwrap(), reads);
}

#[test]
#[test_r::timeout("10s")]
async fn wallet_boundary_rechecks_grown_target_and_refuses_growth_after_live_commit() {
    use crate::worker::owner_wallet::WalletCommit;

    let historical = stored_test_card(CardId::new());
    let live = stored_test_card(CardId::new());
    let boundary = Arc::new(tokio::sync::Mutex::new(()));
    let contention = boundary.clone().lock_owned().await;
    let (rs, wallet, oplog, _interest_index) = wallet_replay_fixture(
        vec![
            remove_retry_policy(),
            start_now(),
            OplogEntry::card_installed(None, None, Box::new(historical.clone()), 1),
        ],
        2,
        boundary,
    )
    .await;
    let canonical = wallet.snapshot();
    let acquisition = wallet.lock_boundary();
    tokio::pin!(acquisition);
    assert!(futures::poll!(acquisition.as_mut()).is_pending());
    rs.try_get_oplog_entry(|_| true).await.unwrap().unwrap();
    assert!(rs.is_live());
    assert!(futures::poll!(acquisition.as_mut()).is_pending());
    rs.set_replay_target(OplogIndex::from_u64(3)).await.unwrap();
    drop(contention);
    assert!(futures::poll!(acquisition.as_mut()).is_pending());
    assert!(Arc::ptr_eq(&canonical, &wallet.snapshot()));
    assert!(
        rs.get_oplog_entry_agent_invocation_started()
            .await
            .unwrap()
            .is_none()
    );
    let acquired = acquisition.await.unwrap();
    assert_eq!(wallet.snapshot().cards[&historical.card_id()], historical);
    let reads = oplog.reads.lock().unwrap().clone();
    let commit = WalletCommit::prepare(
        acquired,
        OplogEntry::card_installed(None, None, Box::new(live.clone()), 0),
    )
    .await
    .unwrap();
    commit.validate_before_append().unwrap();
    let index = oplog.add(commit.entry()).await.unwrap();
    oplog.commit(CommitLevel::Always).await.unwrap();
    commit.publish().unwrap();
    drop(commit);
    let published = wallet.snapshot();
    assert_eq!(published.cards[&live.card_id()], live);
    assert_eq!(published.cards[&historical.card_id()], historical);
    assert_eq!(published.generation, 2);
    assert_eq!(*oplog.reads.lock().unwrap(), reads);
    assert!(rs.set_replay_target(index).await.is_err());
    assert!(wallet.validate_current().is_err());
    assert!(wallet.lock_boundary().await.is_err());
    assert!(Arc::ptr_eq(&published, &wallet.snapshot()));
    assert_eq!(*oplog.reads.lock().unwrap(), reads);
}

#[test]
#[test_r::timeout("10s")]
async fn card_audit_wait_drains_unclaimed_same_store_accessor_without_rescan() {
    let card = stored_test_card(CardId::new());
    let (rs, wallet, oplog, _interest_index) = wallet_replay_fixture(
        vec![
            remove_retry_policy(),
            start_now(),
            end_for(2, 42),
            start_now(),
            end_for(4, 43),
            OplogEntry::CardDerived {
                timestamp: Timestamp::now_utc(),
                entity_parent_start_index: None,
                card: Box::new(card.clone()),
                wallet_generation: 0,
            },
            remove_retry_policy(),
        ],
        7,
        Arc::new(tokio::sync::Mutex::new(())),
    )
    .await;
    let first = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert!(!wallet.has_recorded_derivation(card.card_id()));
    assert!(!rs.has_unclaimed_retained_starts());
    rs.await_card_derivation(card.card_id(), None)
        .await
        .unwrap();
    assert!(rs.has_unclaimed_retained_starts());
    assert!(wallet.has_recorded_derivation(card.card_id()));
    assert!(matches!(
        rs.await_resolution(first).await.unwrap(),
        Resolution::Completed { end_idx, .. } if end_idx == OplogIndex::from_u64(3)
    ));
    let reads = oplog.reads.lock().unwrap().clone();
    rs.await_card_derivation(card.card_id(), None)
        .await
        .unwrap();
    assert!(
        wallet
            .recorded_derivation(OplogIndex::from_u64(4), None, &card)
            .await
            .unwrap()
    );
    wallet.latest_replay_receipt().result.await.unwrap();
    assert_eq!(*oplog.reads.lock().unwrap(), reads);
    let retained = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert_eq!(retained.start_idx(), OplogIndex::from_u64(4));
    assert!(matches!(
        rs.await_resolution(retained).await.unwrap(),
        Resolution::Completed { end_idx, .. } if end_idx == OplogIndex::from_u64(5)
    ));
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn custom_claim_rejects_invocation_id_collision() {
    let scope_name = HostFunctionName::Custom("invocation-id-collision".to_string());
    let conflicting_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: scope_name.clone(),
        invocation_id: Some(test_invocation_id(7)),
        observational_owner: None,
        request: None,
        durable_function_type: DurableFunctionType::ReadLocal,
        span_started: None,
    };
    let rs = replay_state_over(vec![remove_retry_policy(), conflicting_start]).await;

    let result = rs
        .claim_custom_start_for_store(
            &scope_name,
            &DurableFunctionType::WriteRemote,
            None,
            test_invocation_id(7),
            false,
        )
        .await;
    let error = match result {
        Ok(_) => panic!("a Start with the same invocation id must not be treated as absent"),
        Err(error) => error,
    };

    assert!(format!("{error}").contains("no matching Start"));
    assert!(!rs.is_live());
}

#[test]
async fn entity_owned_scope_claim_requires_the_recorded_parent() {
    let parent = OplogIndex::from_u64(7);
    let scope_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: Some(parent),
        function_name: HostFunctionName::Custom("<scope:batched-write>".to_string()),
        invocation_id: None,
        observational_owner: None,
        request: None,
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: None,
    };
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        scope_start,
        batched_scope_end(2),
    ])
    .await;
    let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());

    rs.claim_scope_start(
        &scope_name,
        &DurableFunctionType::WriteRemote,
        Some(OplogIndex::from_u64(8)),
    )
    .await
    .expect_err("a scope claim must not steal a sibling entity invocation's scope");

    let (scope_index, handle) = rs
        .claim_scope_start(&scope_name, &DurableFunctionType::WriteRemote, Some(parent))
        .await
        .unwrap();
    assert_eq!(scope_index, OplogIndex::from_u64(2));
    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
}

#[test]
fn start_claim_requires_the_recorded_observational_owner() {
    let owner = OplogIndex::from_u64(7);
    let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());
    let scope_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: scope_name.clone(),
        invocation_id: None,
        observational_owner: Some(owner),
        request: None,
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: None,
    };

    assert!(
        StartClaim::scope(&scope_name, &DurableFunctionType::WriteRemote, None,)
            .with_observational_owner(Some(owner))
            .matches_start_identity(&scope_start)
    );
    assert!(
        !StartClaim::scope(&scope_name, &DurableFunctionType::WriteRemote, None,)
            .matches_start_identity(&scope_start),
        "an unowned replay claim must not steal an observationally owned scope"
    );

    let call_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::MonotonicClockNow,
        invocation_id: None,
        observational_owner: Some(owner),
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::ReadLocal,
        span_started: None,
    };
    assert!(
        StartClaim::unowned(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .with_observational_owner(Some(owner))
        .matches_start_identity(&call_start)
    );
}

/// Pins the exact "expected" label a [`StartClaim`] renders for `unexpected_oplog_entry` claim
/// errors, so diagnostic wording does not silently drift.
#[test]
fn start_claim_expected_descriptions_are_stable() {
    use super::claims::StartClaim;

    let name = HostFunctionName::MonotonicClockNow;

    assert_eq!(
        StartClaim::scope(&name, &DurableFunctionType::WriteRemote, None).expected_description(),
        format!("Start {{ {name}, WriteRemote, request: None, parent_start_index: None }}")
    );

    assert_eq!(
        StartClaim::unowned(&name, &DurableFunctionType::ReadRemote).expected_description(),
        format!("Start {{ {name}, ReadRemote, request: Some(..), parent_start_index: None }}")
    );
    assert_eq!(
        StartClaim::owned(
            &name,
            &DurableFunctionType::WriteRemote,
            OplogIndex::from_u64(4),
        )
        .expected_description(),
        format!(
            "Start {{ {name}, WriteRemote, request: Some(..), parent_start_index: Some(OplogIndex(4)) }}"
        )
    );
    assert_eq!(
        StartClaim::owned(
            &name,
            &DurableFunctionType::ReadRemote,
            OplogIndex::from_u64(7)
        )
        .with_invocation_id(Some(test_invocation_id(3)))
        .expected_description(),
        format!(
            "Start {{ {name}, ReadRemote, request: Some(..), parent_start_index: Some(OplogIndex(7)) }} with invocation id 1#3"
        )
    );
    assert_eq!(
        StartClaim::tool_invocation(
            &HostFunctionName::GolemEntityInvoke,
            &HostFunctionName::GolemToolInvocationRejected,
            &DurableFunctionType::WriteLocal,
            OplogIndex::from_u64(7)
        )
        .expected_description(),
        format!(
            "Start {{ {} or {}, WriteLocal, request: Some(..), parent_start_index: Some(OplogIndex(7)) }}",
            HostFunctionName::GolemEntityInvoke,
            HostFunctionName::GolemToolInvocationRejected
        )
    );
}

fn entity_remove_retry_policy(entity: u64) -> OplogEntry {
    OplogEntry::RemoveRetryPolicy {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: Some(OplogIndex::from_u64(entity)),
        name: "replay-policy".to_string(),
    }
}

fn entity_remove_other_retry_policy(entity: u64) -> OplogEntry {
    OplogEntry::RemoveRetryPolicy {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: Some(OplogIndex::from_u64(entity)),
        name: "other-policy".to_string(),
    }
}

async fn assert_pending<T: std::fmt::Debug>(
    future: &mut std::pin::Pin<Box<impl Future<Output = T>>>,
    what: &str,
) {
    assert!(
        tokio::time::timeout(Duration::from_millis(50), future.as_mut())
            .await
            .is_err(),
        "{what} must stay parked"
    );
}

#[test]
async fn interleaved_positional_markers_are_consumed_only_by_the_recording_store() {
    // An entity body (root Start 2) and its owner both remove retry policies while
    // sharing the cursor:
    //   [RecoverySucceeded(1), Start(entity=2), RemoveRetryPolicy(3, entity 2), Start(4, parent 2), End(4→5),
    //    CompletionDelivered(4→6), RemoveRetryPolicy(7), RemoveRetryPolicy(8, entity 2),
    //    RemoveRetryPolicy(9), End(2→10)]
    // The owner asks for its `RemoveRetryPolicy` first. It must not take the entity's marker at 3
    // (which would leave the entity reader to retain its own Start(4) and park forever at that
    // call's delivery marker): it parks until the entity consumed 3, retains 4 for the entity,
    // parks at 6 until the entity delivers 4, and only then receives 7. The same holds in the
    // other direction for the entity's `RemoveRetryPolicy` read behind the owner's marker at 7.
    let parent = OplogIndex::from_u64(1);
    let entity_start = rejected_tool_reconstruction_start(parent);
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        entity_start,
        entity_remove_retry_policy(2),
        start_with_parent(2),
        end_for(4, 44),
        delivered_for(4),
        remove_retry_policy(),
        entity_remove_other_retry_policy(2),
        remove_other_retry_policy(),
        end_for(2, 1),
    ])
    .await;
    let mut entity_handle = claim_rejected_tool_reconstruction(&rs, parent).await;
    let mut reconstruction = entity_handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");

    let mut owner_begin = Box::pin(rs.get_oplog_entry(None));
    assert_pending(&mut owner_begin, "the owner's RemoveRetryPolicy read").await;
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(2),
        "a parked positional reader must not advance past the other Store's marker"
    );

    let (idx, entry) = rs
        .get_oplog_entry(Some(OplogIndex::from_u64(2)))
        .await
        .unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    assert_pending(&mut owner_begin, "the owner's RemoveRetryPolicy read").await;
    assert_eq!(
        rs.last_replayed_index(),
        OplogIndex::from_u64(5),
        "the owner's reader retains the entity's Start(4), attaches End(5) and parks at the marker"
    );
    assert!(rs.has_unclaimed_retained_starts());

    let call = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    assert_eq!(call.start_idx(), OplogIndex::from_u64(4));
    match rs.await_resolution(call).await.unwrap() {
        Resolution::Completed {
            end_idx,
            delivery_marker,
            ..
        } => {
            assert_eq!(end_idx, OplogIndex::from_u64(5));
            assert_eq!(delivery_marker, Some(OplogIndex::from_u64(6)));
        }
        other => panic!("expected the entity call to complete, got {other:?}"),
    }
    rs.await_completion_delivery(OplogIndex::from_u64(4), OplogIndex::from_u64(6))
        .await
        .unwrap()
        .acknowledge();

    let mut entity_end = Box::pin(rs.get_oplog_entry(Some(OplogIndex::from_u64(2))));
    assert_pending(&mut entity_end, "the entity's RemoveRetryPolicy read").await;

    let (idx, entry) = owner_begin.await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(7));
    assert!(matches!(
        entry,
        OplogEntry::RemoveRetryPolicy {
            entity_parent_start_index: None,
            ..
        }
    ));

    let (idx, entry) = entity_end.await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(8));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(9));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    assert!(matches!(
        rs.await_resolution_outcome(entity_handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(10)
    ));
    reconstruction.body_settled();
    assert!(!rs.has_unclaimed_retained_starts());
}

#[test]
async fn positional_reader_rejects_marker_of_an_entity_body_nobody_reconstructs() {
    // [RecoverySucceeded(1), Start(2), End(2→3), RemoveRetryPolicy(4, entity 2), RemoveRetryPolicy(5)] — the entry at 4
    // claims to belong to an entity body rooted at 2, but 2 was claimed and resolved as an
    // ordinary call: no reconstruction will ever consume 4. Parking would hang replay, so the
    // owner's positional reader reports the divergence instead.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        end_for(2, 42),
        entity_remove_retry_policy(2),
        remove_retry_policy(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    assert!(matches!(
        rs.await_resolution(handle).await.unwrap(),
        Resolution::Completed { .. }
    ));

    let error = rs
        .get_oplog_entry(None)
        .await
        .expect_err("a marker of an unreconstructed entity body must be fatal");
    assert!(
        error
            .to_string()
            .contains("neither retained, claimed nor replaying"),
        "unexpected error: {error}"
    );
}

#[test]
async fn positional_reader_waits_for_a_retained_entity_start_to_be_claimed() {
    // [RecoverySucceeded(1), Start(2), Start(entity=3), RemoveRetryPolicy(4, entity 3), End(2→5), End(3→6), RemoveRetryPolicy(7)] —
    // call 2's awaiter drained past the unclaimed entity Start(3) and retained it. The owner's
    // next positional read reaches the body entry at 4 while 3 is still unclaimed: the owner is
    // going to claim 3, so the reader waits rather than failing, and receives 7 once the entity
    // body consumed 4.
    let parent = OplogIndex::from_u64(1);
    let entity_start = rejected_tool_reconstruction_start(parent);
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        entity_start,
        anchored_remove_retry_policy(3),
        end_for(2, 42),
        end_for(3, 1),
        remove_retry_policy(),
    ])
    .await;
    let handle = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let mut resolution = Box::pin(rs.await_resolution(handle));
    assert_pending(&mut resolution, "call 2's awaiter").await;
    assert!(rs.has_unclaimed_retained_starts());
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));

    let mut owner_read = Box::pin(rs.get_oplog_entry(None));
    assert_pending(&mut owner_read, "the owner's positional read").await;
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));

    let mut entity_handle = claim_rejected_tool_reconstruction(&rs, parent).await;
    let mut reconstruction = entity_handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    let (idx, entry) = rs
        .get_oplog_entry(Some(OplogIndex::from_u64(3)))
        .await
        .unwrap();
    assert_eq!(idx, OplogIndex::from_u64(4));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    let (idx, entry) = owner_read.await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(7));
    assert!(matches!(
        entry,
        OplogEntry::RemoveRetryPolicy {
            entity_parent_start_index: None,
            ..
        }
    ));
    assert!(matches!(
        resolution.await.unwrap(),
        Resolution::Completed { end_idx, .. } if end_idx == OplogIndex::from_u64(5)
    ));
    assert!(matches!(
        rs.await_resolution_outcome(entity_handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(6)
    ));
    reconstruction.body_settled();
}

#[test]
async fn positional_reader_waits_for_entity_entry_recorded_before_its_start() {
    // The entity body reserved Start(3), then appended its RemoveRetryPolicy(2) before the asynchronous Start
    // write completed. The owner's reader must leave 2 for the body while its reconstruction
    // claim scans ahead to 3, rather than rejecting the forward attribution as orphaned history.
    let parent = OplogIndex::from_u64(1);
    let entity_start = rejected_tool_reconstruction_start(parent);
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        anchored_remove_retry_policy(3),
        entity_start,
        end_for(3, 1),
        remove_retry_policy(),
    ])
    .await;

    let mut owner_read = Box::pin(rs.get_oplog_entry(None));
    assert_pending(&mut owner_read, "the owner's positional read").await;
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(1));

    let mut entity_handle = claim_rejected_tool_reconstruction(&rs, parent).await;
    let mut reconstruction = entity_handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    let (idx, entry) = rs
        .get_oplog_entry(Some(OplogIndex::from_u64(3)))
        .await
        .unwrap();
    assert_eq!(idx, OplogIndex::from_u64(2));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));

    assert!(matches!(
        rs.await_resolution_outcome(entity_handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(4)
    ));
    reconstruction.body_settled();
    let (idx, entry) = owner_read.await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(5));
    assert!(matches!(entry, OplogEntry::RemoveRetryPolicy { .. }));
}

#[test]
async fn positional_reader_rejects_forward_body_owner_that_is_not_an_entity_start() {
    // The future Start at 3 is an ordinary monotonic-clock call, so no entity body can ever claim
    // it and consume the RemoveRetryPolicy attributed to it at 2. Treating every retainable future Start as an
    // entity owner would park this read forever instead of rejecting the orphaned attribution.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        anchored_remove_retry_policy(3),
        start_now(),
        end_for(3, 42),
        remove_retry_policy(),
    ])
    .await;

    let result = tokio::time::timeout(Duration::from_millis(100), rs.get_oplog_entry(None)).await;
    let error = result
        .expect("an ordinary future Start must not leave the positional reader parked")
        .expect_err("an entry attributed to a non-entity Start must be rejected");
    assert!(
        error
            .to_string()
            .contains("neither retained, claimed nor replaying"),
        "unexpected error: {error}"
    );
}

#[test]
async fn invocation_boundary_rejects_unconsumed_entity_body_entry() {
    // [RecoverySucceeded(1), Start(entity=2), End(2→3), RemoveRetryPolicy(4, entity 2),
    //  AgentInvocationFinished(5)] — the entity body settled without consuming its own marker at
    // 4. Every entity body of the invocation has terminated by the time the boundary reader runs,
    // so nobody can consume 4 anymore: the walk must fail instead of parking at it.
    let parent = OplogIndex::from_u64(1);
    let entity_start = rejected_tool_reconstruction_start(parent);
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        entity_start,
        end_for(2, 1),
        entity_remove_retry_policy(2),
        invocation_finished(),
    ])
    .await;
    let mut handle = claim_rejected_tool_reconstruction(&rs, parent).await;
    let mut reconstruction = handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    reconstruction.body_settled();

    let error = read_invocation_finished(&rs)
        .await
        .expect_err("an unconsumed entity body entry at the boundary must be fatal");
    assert!(
        error.to_string().contains("without reconstructing"),
        "unexpected error: {error}"
    );
}

#[test]
fn head_owner_names_the_store_that_consumes_the_head() {
    use super::cursor::{HeadOwner, head_owner};

    let body = OplogIndex::from_u64(2);
    assert_eq!(head_owner(&start_now()), HeadOwner::Unattributed);
    assert_eq!(
        head_owner(&start_with_parent(2)),
        HeadOwner::ParentStart(body)
    );
    assert_eq!(head_owner(&remove_retry_policy()), HeadOwner::Agent);
    assert_eq!(
        head_owner(&anchored_remove_retry_policy(2)),
        HeadOwner::EntityBody(body)
    );
    assert_eq!(head_owner(&end_for(2, 1)), HeadOwner::Unattributed);
    assert_eq!(head_owner(&delivered_for(2)), HeadOwner::Unattributed);
}

/// Replays `[RecoverySucceeded(1), entity Start(2), body entries..]` up to the state of a tool call whose
/// completed reconstruction is claimed while its supervisor has not drained the cursor yet: the
/// entity `Start` is claimed ahead of the cursor, nothing drained it yet, and the body is active.
async fn replay_with_undrained_reconstruction(
    body_entries: Vec<OplogEntry>,
) -> (
    ReplayState,
    ReplayCallHandle,
    crate::durable_host::concurrent::HistoricalReconstruction,
    Arc<InMemoryOplog>,
) {
    let parent = OplogIndex::from_u64(1);
    let entity_start = rejected_tool_reconstruction_start(parent);
    let mut entries = vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        entity_start,
    ];
    entries.extend(body_entries);
    let (rs, oplog) = replay_state_and_oplog_over(entries).await;
    let mut handle = claim_rejected_tool_reconstruction(&rs, parent).await;
    let reconstruction = handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(1));
    (rs, handle, reconstruction, oplog)
}

fn spawn_start_claim(
    rs: &ReplayState,
    claim: StartClaim,
) -> tokio::task::JoinHandle<Result<ReplayStartClaimOutcome, WorkerExecutorError>> {
    let rs = rs.clone();
    tokio::spawn(async move { rs.claim_start_or_replay_end(claim).await })
}

/// Waits until claims of `rs` have started to wait for an active entity body `waits` times, and
/// fails if `claim` finishes first. The timeout only bounds a failure.
async fn wait_until_claim_parked<T>(
    rs: &ReplayState,
    claim: &mut tokio::task::JoinHandle<Result<T, WorkerExecutorError>>,
    waits: u64,
) {
    let parked = async {
        tokio::select! {
            outcome = &mut *claim => match outcome.unwrap() {
                Err(error) => panic!("the missing Start was decided too early: {error}"),
                Ok(_) => panic!("the missing Start was decided too early"),
            },
            () = rs.test_wait_for_claims_blocked_on_active_body(waits) => {}
        }
    };
    tokio::time::timeout(Duration::from_secs(5), parked)
        .await
        .expect("the claim did not wait for the active body");
}

async fn finished_claim<T>(
    claim: tokio::task::JoinHandle<Result<T, WorkerExecutorError>>,
) -> Result<T, WorkerExecutorError> {
    tokio::time::timeout(Duration::from_secs(5), claim)
        .await
        .expect("the blocked claim was not woken")
        .unwrap()
}

#[test]
async fn missing_start_claim_waits_while_an_active_body_owns_the_head_start() {
    // [RecoverySucceeded(1), Start(entity=2), Start(3, parent 2), End(3→4), End(2→5)] — the primary claims
    // a clock call that was never recorded. Its head path consumes the claimed entity Start and
    // stops at the body's Start(3). The body is active, so the claim waits, and after the
    // reconstruction drained the cursor it reports the end of replay.
    let (rs, handle, mut reconstruction, _oplog) = replay_with_undrained_reconstruction(vec![
        start_with_parent(2),
        end_for(3, 1),
        end_for(2, 2),
    ])
    .await;
    let mut claim = spawn_start_claim(
        &rs,
        StartClaim::unowned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    );
    wait_until_claim_parked(&rs, &mut claim, 1).await;

    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(5)
    ));
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    reconstruction.body_settled();
}

#[test]
async fn missing_numbered_claim_waits_while_an_active_body_owns_the_head_entry() {
    // [RecoverySucceeded(1), Start(entity=2), RemoveRetryPolicy(3, entity 2), End(2→4)] — the head is a positional entry
    // of the active body. The claim of a numbered call waits until the body consumed it, then
    // the entity terminal drains and the claim reports the end of replay.
    let (rs, handle, mut reconstruction, _oplog) =
        replay_with_undrained_reconstruction(vec![anchored_remove_retry_policy(2), end_for(2, 2)])
            .await;
    let mut claim = spawn_start_claim(
        &rs,
        StartClaim::unowned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .with_invocation_id(Some(test_invocation_id(4))),
    );
    wait_until_claim_parked(&rs, &mut claim, 1).await;

    let (idx, _) = rs
        .get_oplog_entry(Some(OplogIndex::from_u64(2)))
        .await
        .unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    reconstruction.body_settled();
}

#[test]
async fn missing_start_claim_is_divergence_when_the_owning_body_settles_without_consuming_it() {
    let (rs, _handle, mut reconstruction, _oplog) = replay_with_undrained_reconstruction(vec![
        start_with_parent(2),
        end_for(3, 1),
        end_for(2, 2),
    ])
    .await;
    let mut claim = spawn_start_claim(
        &rs,
        StartClaim::unowned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    );
    wait_until_claim_parked(&rs, &mut claim, 1).await;

    reconstruction.body_settled();
    let error = match finished_claim(claim).await {
        Err(error) => error,
        Ok(_) => panic!("a settled body's unconsumed head must not hide the missing Start"),
    };
    assert!(
        format!("{error}").contains("no matching Start"),
        "missing replay claim must be strict divergence: {error}"
    );
}

#[test]
async fn missing_start_claim_retains_a_top_level_sibling_start_while_a_body_is_active() {
    // [RecoverySucceeded(1), Start(entity=2), Start(3, top level), End(3), End(2)] — an active body exists,
    // and the head is another top-level call of the primary. The claim retains that sibling for
    // its owner and reaches the replay target.
    let (rs, _handle, mut reconstruction, _oplog) = replay_with_undrained_reconstruction(vec![
        wait_for_start(300_000_000, Some(2)),
        end_for(3, 1),
        end_for(2, 2),
    ])
    .await;
    let claim = spawn_start_claim(
        &rs,
        StartClaim::unowned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    );
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    assert!(rs.has_unclaimed_retained_starts());
    reconstruction.body_settled();
}

#[test]
async fn missing_start_claim_of_the_owning_body_does_not_wait_on_its_own_head() {
    // The body itself claims a call that it never recorded while its own entry is at the head.
    // Nothing but the body can consume that entry, so the claim is decided at once.
    let (rs, _handle, mut reconstruction, _oplog) =
        replay_with_undrained_reconstruction(vec![anchored_remove_retry_policy(2), end_for(2, 2)])
            .await;
    let claim = spawn_start_claim(
        &rs,
        StartClaim::owned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        ),
    );
    assert!(
        finished_claim(claim).await.is_err(),
        "a body must not wait for itself to consume its own head"
    );
    reconstruction.body_settled();
}

#[test]
fn nearest_active_body_prefers_the_innermost_active_body() {
    use super::cursor::nearest_active_body;

    let i = OplogIndex::from_u64;
    // Body 2 encloses scope 3 and the nested entity 6; entity 6 encloses scope 7; scope 4
    // belongs to body 5, which already settled. Each record names its parent.
    let parents = HashMap::from([
        (i(3), i(2)),
        (i(6), i(3)),
        (i(7), i(6)),
        (i(8), i(7)),
        (i(4), i(5)),
        (i(5), i(1)),
    ]);
    let parent_of = |index: OplogIndex| parents.get(&index).copied();

    let active = HashSet::from([i(2), i(6)]);
    let nearest = |index| nearest_active_body(i(index), &active, parent_of);
    assert_eq!(nearest(2), Some(i(2)));
    assert_eq!(nearest(3), Some(i(2)));
    assert_eq!(nearest(6), Some(i(6)));
    assert_eq!(nearest(7), Some(i(6)));
    assert_eq!(nearest(8), Some(i(6)));
    assert_eq!(nearest(4), None);
    assert_eq!(nearest(1), None);

    // After the outer body settles, the nested entity still encloses its own scopes.
    let active = HashSet::from([i(6)]);
    assert_eq!(nearest_active_body(i(8), &active, parent_of), Some(i(6)));
    assert_eq!(nearest_active_body(i(3), &active, parent_of), None);
}

#[test]
fn missing_start_waits_for_the_body_enclosing_the_head_unless_it_issued_the_claim() {
    use super::cursor::missing_start_waits_for;

    let i = OplogIndex::from_u64;
    assert_eq!(missing_start_waits_for(Some(i(2)), None, false), Some(i(2)));
    assert_eq!(
        missing_start_waits_for(Some(i(2)), Some(i(6)), false),
        Some(i(2))
    );
    assert_eq!(missing_start_waits_for(Some(i(2)), Some(i(2)), false), None);
    assert_eq!(missing_start_waits_for(Some(i(2)), None, true), None);
    assert_eq!(missing_start_waits_for(None, None, false), None);
}

#[test]
async fn missing_start_claim_waits_while_a_scope_of_an_active_body_owns_the_head() {
    // [RecoverySucceeded(1), Start(entity=2), Start(scope 3, parent 2), Start(4, parent 3), End(4→5),
    //  End(3→6), End(2→7)] — the body claimed its scope Start(3), so the head is the child
    // Start(4). It names the scope, not the entity, as its parent; the claim still waits for the
    // body that encloses the scope.
    let (rs, handle, mut reconstruction, oplog) = replay_with_undrained_reconstruction(vec![
        start_with_parent(2),
        start_with_parent(3),
        end_for(4, 1),
        end_for(3, 2),
        end_for(2, 3),
    ])
    .await;
    let scope = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    assert_eq!(scope.start_idx(), OplogIndex::from_u64(3));
    oplog.reads.lock().unwrap().clear();

    let mut claim = spawn_start_claim(
        &rs,
        StartClaim::unowned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    );
    wait_until_claim_parked(&rs, &mut claim, 1).await;
    // The owner of the head is resolved from the claimed scope, not by reading the scope Start
    // or the entity Start again.
    let reads = oplog.reads.lock().unwrap().clone();
    assert!(
        reads.iter().all(|index| *index > OplogIndex::from_u64(3)),
        "the parked claim read already consumed ancestors: {reads:?}"
    );

    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(7)
    ));
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    drop(scope);
    reconstruction.body_settled();
}

#[test]
async fn missing_start_claim_from_a_scope_of_the_owning_body_does_not_wait_on_its_own_head() {
    // As above, but the body itself claims a call under its scope that it never recorded.
    let (rs, _handle, mut reconstruction, _oplog) = replay_with_undrained_reconstruction(vec![
        start_with_parent(2),
        start_with_parent(3),
        end_for(4, 1),
        end_for(3, 2),
        end_for(2, 3),
    ])
    .await;
    let _scope = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    let claim = spawn_start_claim(
        &rs,
        StartClaim::owned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(3),
        ),
    );
    assert!(
        finished_claim(claim).await.is_err(),
        "a body must not wait for itself to consume the head under its own scope"
    );
    reconstruction.body_settled();
}

#[test]
async fn missing_custom_invocation_claim_waits_while_an_active_body_owns_the_head() {
    // The guest starts a custom durable invocation whose Start was never recorded while the
    // completed reconstruction has not drained the body's Start(3). Nothing is buffered at the
    // head, as after a positional read: the claim classifies the head from its own scan and
    // waits for the body.
    let (rs, handle, mut reconstruction, _oplog) = replay_with_undrained_reconstruction(vec![
        start_with_parent(2),
        end_for(3, 1),
        end_for(2, 2),
    ])
    .await;
    rs.cursor.state.lock().await.replay_buffer.clear();
    let mut claim = tokio::spawn({
        let rs = rs.clone();
        async move {
            rs.claim_custom_start_for_store(
                &HostFunctionName::Custom("missing-custom".to_string()),
                &DurableFunctionType::WriteRemote,
                None,
                test_invocation_id(99),
                false,
            )
            .await
        }
    });
    wait_until_claim_parked(&rs, &mut claim, 1).await;

    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        CustomStartClaimOutcome::ReplayEnded
    ));
    reconstruction.body_settled();
}

#[test]
async fn missing_start_claim_of_a_nested_entity_waits_while_the_enclosing_body_owns_the_head() {
    // [RecoverySucceeded(1), Start(entity A=2), Start(3, parent 2), Start(entity C=4, parent 2), End(3→5),
    //  End(4→6), End(2→7)] — body A invoked the nested entity C. Both reconstructions are
    // claimed, and nothing drained A's Start(3) at the head. C claims a call that was never recorded while A's Start(3) is
    // at the head. C is another Store than A, so the claim waits until A's reconstruction drained
    // the cursor, and then reports the end of replay.
    let parent = OplogIndex::from_u64(1);
    let outer_start = rejected_tool_reconstruction_start(parent);
    let nested_parent = OplogIndex::from_u64(2);
    let nested_start = rejected_tool_reconstruction_start(nested_parent);
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        outer_start,
        start_with_parent(2),
        nested_start,
        end_for(3, 1),
        end_for(4, 2),
        end_for(2, 3),
    ])
    .await;
    let mut outer = claim_rejected_tool_reconstruction(&rs, parent).await;
    let mut outer_reconstruction = outer
        .take_historical_reconstruction()
        .expect("outer reconstruction guard");
    let mut nested = claim_rejected_tool_reconstruction(&rs, nested_parent).await;
    assert_eq!(nested.start_idx(), OplogIndex::from_u64(4));
    let mut nested_reconstruction = nested
        .take_historical_reconstruction()
        .expect("nested reconstruction guard");
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(2));

    let mut claim = spawn_start_claim(
        &rs,
        StartClaim::owned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(4),
        ),
    );
    wait_until_claim_parked(&rs, &mut claim, 1).await;

    assert!(matches!(
        rs.await_resolution_outcome(outer).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(7)
    ));
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    assert!(matches!(
        rs.await_resolution_outcome(nested).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    nested_reconstruction.body_settled();
    outer_reconstruction.body_settled();
}

#[test]
async fn missing_start_claim_waits_for_a_nested_entity_after_the_outer_body_settles() {
    // [RecoverySucceeded(1), Start(entity A=2), Start(entity C=3, parent 2), Start(scope S=4, parent 3),
    //  Start(5, parent 4), End(5→6), End(4→7), End(3→8), End(2→9)] — C claimed its scope S, so
    // the head is the child Start(5). A's body settled, C's body is still active. The claim
    // waits for C, which encloses the head through S, and resolves C from the claims the cursor
    // holds, without reading S or C again.
    let outer_parent = OplogIndex::from_u64(1);
    let outer_start = rejected_tool_reconstruction_start(outer_parent);
    let nested_parent = OplogIndex::from_u64(2);
    let nested_start = rejected_tool_reconstruction_start(nested_parent);
    let (rs, oplog) = replay_state_and_oplog_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        outer_start,
        nested_start,
        start_with_parent(3),
        start_with_parent(4),
        end_for(5, 1),
        end_for(4, 2),
        end_for(3, 3),
        end_for(2, 4),
    ])
    .await;
    let mut outer = claim_rejected_tool_reconstruction(&rs, outer_parent).await;
    let mut outer_reconstruction = outer
        .take_historical_reconstruction()
        .expect("outer reconstruction guard");
    let mut nested = claim_rejected_tool_reconstruction(&rs, nested_parent).await;
    assert_eq!(nested.start_idx(), OplogIndex::from_u64(3));
    let mut nested_reconstruction = nested
        .take_historical_reconstruction()
        .expect("nested reconstruction guard");
    let scope = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(3),
        )
        .await
        .unwrap();
    assert_eq!(scope.start_idx(), OplogIndex::from_u64(4));
    outer_reconstruction.body_settled();
    oplog.reads.lock().unwrap().clear();

    let mut claim = spawn_start_claim(
        &rs,
        StartClaim::unowned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    );
    wait_until_claim_parked(&rs, &mut claim, 1).await;
    let reads = oplog.reads.lock().unwrap().clone();
    assert!(
        reads.iter().all(|index| *index > OplogIndex::from_u64(4)),
        "the parked claim read already consumed ancestors: {reads:?}"
    );

    assert!(matches!(
        rs.await_resolution_outcome(outer).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(9)
    ));
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    drop(scope);
    assert!(matches!(
        rs.await_resolution_outcome(nested).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    nested_reconstruction.body_settled();
}

/// `[RecoverySucceeded(1), Start(entity=2), RemoveRetryPolicy(3, entity 2), custom root Start(4, parent), End(4→5),
/// End(2→6)]`: the tool body records a positional entry and then a root custom invocation.
async fn replay_with_body_entry_before_custom_root(
    custom_root_parent: Option<u64>,
) -> (
    ReplayState,
    ReplayCallHandle,
    crate::durable_host::concurrent::HistoricalReconstruction,
    tokio::task::JoinHandle<Result<ReplayStartClaimOutcome, WorkerExecutorError>>,
) {
    let (rs, handle, reconstruction, _oplog) = replay_with_undrained_reconstruction(vec![
        anchored_remove_retry_policy(2),
        custom_start("entity-custom-root", 1, custom_root_parent, 7),
        custom_end(4, 1),
        end_for(2, 2),
    ])
    .await;
    let mut claim = spawn_start_claim(
        &rs,
        StartClaim::unowned(
            &HostFunctionName::WallClockNow,
            &DurableFunctionType::ReadLocal,
        ),
    );
    wait_until_claim_parked(&rs, &mut claim, 1).await;

    // The body consumes its positional entry, which wakes the parked claim.
    let (idx, _) = rs
        .get_oplog_entry(Some(OplogIndex::from_u64(2)))
        .await
        .unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    (rs, handle, reconstruction, claim)
}

#[test]
async fn missing_start_claim_keeps_waiting_on_a_custom_root_of_the_active_body() {
    // The body's root custom invocation records the entity Start as its parent. When the woken
    // claim runs again, that custom Start is at the head and still belongs to the active body,
    // so the claim waits a second time instead of reporting a divergence.
    let (rs, handle, mut reconstruction, mut claim) =
        replay_with_body_entry_before_custom_root(Some(2)).await;
    wait_until_claim_parked(&rs, &mut claim, 2).await;

    let custom = rs
        .claim_custom_start_for_store(
            &HostFunctionName::Custom("entity-custom-root".to_string()),
            &DurableFunctionType::ReadRemote,
            Some(OplogIndex::from_u64(2)),
            test_invocation_id(7),
            false,
        )
        .await
        .unwrap();
    assert!(matches!(custom, CustomStartClaimOutcome::Claimed(_)));
    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(6)
    ));
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    reconstruction.body_settled();
}

#[test]
async fn missing_start_claim_retains_a_top_level_custom_root_after_the_body_entry() {
    // The same oplog with a custom root that records no parent is a top-level invocation of the
    // primary: when the woken claim runs again, it retains that sibling for its owner and reaches
    // the replay target.
    let (rs, _handle, mut reconstruction, claim) =
        replay_with_body_entry_before_custom_root(None).await;
    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    assert!(rs.has_unclaimed_retained_starts());
    reconstruction.body_settled();
}

#[test]
async fn admission_refusal_probe_preserves_repeated_calls_across_delivery() {
    let numbered_start = || {
        let mut entry = start_with_parent(2);
        let OplogEntry::Start { invocation_id, .. } = &mut entry else {
            unreachable!()
        };
        *invocation_id = Some(test_invocation_id(7));
        entry
    };
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        numbered_start(),
        end_for(3, 41),
        delivered_for(3),
        numbered_start(),
        end_for(6, 42),
        delivered_for(6),
        end_for(2, 43),
    ])
    .await;
    let root = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let claim = StartClaim::owned(
        &HostFunctionName::MonotonicClockNow,
        &DurableFunctionType::ReadLocal,
        OplogIndex::from_u64(2),
    )
    .with_invocation_id(Some(test_invocation_id(7)));
    let ReplayStartClaimOutcome::Claimed { handle: first, .. } = rs
        .claim_start_for_store(claim.clone(), false)
        .await
        .unwrap()
    else {
        panic!("first recorded occurrence must be claimed");
    };
    rs.await_resolution(first).await.unwrap();
    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(3), OplogIndex::from_u64(5))
        .await
        .unwrap();
    let position = rs.last_replayed_index();
    assert!(
        !rs.has_unclaimed_start_before(
            claim
                .clone()
                .with_invocation_id(Some(test_invocation_id(8))),
            OplogIndex::from_u64(9),
        )
        .await
        .unwrap()
    );
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            rs.has_unclaimed_start_before(claim.clone(), OplogIndex::from_u64(9),)
        )
        .await
        .expect("visibility must not wait on the delivery gate")
        .unwrap()
    );
    assert!(
        !rs.has_unclaimed_start_before(claim.clone(), OplogIndex::from_u64(6))
            .await
            .unwrap()
    );
    assert_eq!(rs.last_replayed_index(), position);
    barrier.acknowledge();
    let ReplayStartClaimOutcome::Claimed { handle: second, .. } = rs
        .claim_start_for_store(claim.clone(), false)
        .await
        .unwrap()
    else {
        panic!("second recorded occurrence must be claimed");
    };
    assert_eq!(second.start_idx(), OplogIndex::from_u64(6));
    assert!(
        !rs.has_unclaimed_start_before(claim, OplogIndex::from_u64(9))
            .await
            .unwrap()
    );
    rs.await_resolution(second).await.unwrap();
    rs.await_completion_delivery(OplogIndex::from_u64(6), OplogIndex::from_u64(8))
        .await
        .unwrap()
        .acknowledge();
    rs.await_resolution(root).await.unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn positional_admission_refusal_preserves_records_and_delivery_gates() {
    use golem_common::model::oplog::payload::types::{
        SerializableToolAdmissionRefusal, SerializableToolPositionalRequest,
    };
    let owner = OplogIndex::from_u64(2);
    for records in [
        vec![],
        vec![anchored_remove_retry_policy(2)],
        vec![
            anchored_remove_retry_policy(2),
            anchored_remove_retry_policy(2),
        ],
    ] {
        let mut entries = vec![
            OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            },
            start_now(),
            start_with_parent(2),
            end_for(3, 41),
            delivered_for(3),
        ];
        entries.extend(records.clone());
        let terminal = OplogIndex::from_u64(entries.len() as u64 + 1);
        entries.push(end_for(2, 43));
        let positional_records = records
            .iter()
            .enumerate()
            .filter(|(_, entry)| !matches!(entry, OplogEntry::Start { .. }))
            .collect::<Vec<_>>();
        let rs = replay_state_over(entries).await;
        let root = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let child = rs
            .claim_owned_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
                owner,
            )
            .await
            .unwrap();
        rs.await_resolution(child).await.unwrap();
        let barrier = rs
            .await_completion_delivery(OplogIndex::from_u64(3), OplogIndex::from_u64(5))
            .await
            .unwrap();
        let position = rs.last_replayed_index();
        let operation = crate::durable_host::tool::operation::tests::replay_operation(2);
        let refusal = SerializableToolAdmissionRefusal::BeforePositional {
            request: SerializableToolPositionalRequest::RemoveRetryPolicy,
        };
        operation.install_recorded_admission_refusal(refusal.clone(), terminal);
        if records.is_empty() {
            let error = tokio::time::timeout(
                Duration::from_secs(1),
                rs.get_oplog_entry_or_replay_end_with_admission(
                    Some(owner),
                    Some((
                        operation.clone(),
                        SerializableToolPositionalRequest::RemoveRetryPolicy,
                    )),
                ),
            )
            .await
            .expect("refusal must not wait for the delivery held by its caller")
            .unwrap_err();
            assert!(crate::durable_host::is_tool_attachment_live_admission_rejection(&error));
            assert_eq!(rs.last_replayed_index(), position);
            barrier.acknowledge();
        } else {
            let replay = rs.clone();
            let pending_operation = operation.clone();
            let mut pending = tokio::spawn(async move {
                replay
                    .get_oplog_entry_or_replay_end_with_admission(
                        Some(owner),
                        Some((
                            pending_operation,
                            SerializableToolPositionalRequest::RemoveRetryPolicy,
                        )),
                    )
                    .await
            });
            assert!(
                tokio::time::timeout(Duration::from_millis(25), &mut pending)
                    .await
                    .is_err()
            );
            assert_eq!(operation.observed_admission_refusal(), None);
            barrier.acknowledge();
            let crate::durable_host::PositionalRead::Entry(index, entry) =
                pending.await.unwrap().unwrap()
            else {
                panic!("recorded positional event must win");
            };
            assert_eq!(
                index,
                OplogIndex::from_u64(6 + positional_records[0].0 as u64)
            );
            assert_eq!(&entry, positional_records[0].1);
            for (ordinal, expected) in positional_records.iter().skip(1) {
                let crate::durable_host::PositionalRead::Entry(index, entry) = rs
                    .get_oplog_entry_or_replay_end_with_admission(
                        Some(owner),
                        Some((
                            operation.clone(),
                            SerializableToolPositionalRequest::RemoveRetryPolicy,
                        )),
                    )
                    .await
                    .unwrap()
                else {
                    panic!("repeated positional event must win");
                };
                assert_eq!(index, OplogIndex::from_u64(6 + *ordinal as u64));
                assert_eq!(&entry, *expected);
            }
            // Only a later, absent demand can reproduce the refusal.
            assert_eq!(operation.observed_admission_refusal(), None);
            let error = rs
                .get_oplog_entry_or_replay_end_with_admission(
                    Some(owner),
                    Some((
                        operation.clone(),
                        SerializableToolPositionalRequest::RemoveRetryPolicy,
                    )),
                )
                .await
                .unwrap_err();
            assert!(crate::durable_host::is_tool_attachment_live_admission_rejection(&error));
        }
        assert_eq!(operation.observed_admission_refusal(), Some(refusal));
        rs.await_resolution(root).await.unwrap();
    }
}

#[test]
#[test_r::timeout("30s")]
async fn admission_refusal_rechecks_competing_claims_behind_delivery() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(2),
        end_for(3, 41),
        delivered_for(3),
        start_with_parent(2),
        end_for(6, 42),
        delivered_for(6),
        end_for(2, 43),
    ])
    .await;
    let root = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let first = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    rs.await_resolution(first).await.unwrap();
    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(3), OplogIndex::from_u64(5))
        .await
        .unwrap();
    let claim = StartClaim::owned(
        &HostFunctionName::MonotonicClockNow,
        &DurableFunctionType::ReadLocal,
        OplogIndex::from_u64(2),
    );
    let refusal =
        golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal::BeforeStart {
            initiating_function: "clock".to_string(),
            claim: desert_rust::serialize(&claim, Vec::new()).unwrap(),
        };
    let operation = crate::durable_host::tool::operation::tests::replay_operation(2);
    operation.install_recorded_admission_refusal(refusal.clone(), OplogIndex::from_u64(9));
    assert_eq!(operation.observed_admission_refusal(), None);
    let mut contenders = Vec::new();
    for _ in 0..2 {
        let (missed, miss) = oneshot::channel();
        *rs.cursor.retained_claim_miss.lock().unwrap() = Some(missed);
        let replay = rs.clone();
        let operation = operation.clone();
        let claim = claim.clone();
        contenders.push(tokio::spawn(async move {
            replay
                .claim_start_for_store_with_admission(
                    claim,
                    false,
                    |_, _| {},
                    Some((operation, "clock".to_string())),
                )
                .await
        }));
        miss.await.unwrap();
    }
    barrier.acknowledge();
    let mut claimed = None;
    let mut refused = 0;
    for contender in contenders {
        match contender.await.unwrap() {
            Ok(ReplayStartClaimOutcome::Claimed { handle, .. }) => {
                assert_eq!(handle.start_idx(), OplogIndex::from_u64(6));
                assert!(claimed.replace(handle).is_none());
            }
            Err(error) => {
                assert!(
                    crate::durable_host::is_tool_attachment_live_admission_rejection(&error),
                    "{error}"
                );
                refused += 1;
            }
            _ => panic!("recorded call or refusal expected"),
        }
    }
    assert_eq!(refused, 1);
    assert_eq!(operation.observed_admission_refusal(), Some(refusal));
    rs.await_resolution(claimed.unwrap()).await.unwrap();
    rs.await_completion_delivery(OplogIndex::from_u64(6), OplogIndex::from_u64(8))
        .await
        .unwrap()
        .acknowledge();
    rs.await_resolution(root).await.unwrap();
}

#[test]
async fn admission_refusal_retires_only_terminal_less_claims() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        start_now(),
        start_with_parent(2),
        start_with_parent(2),
        end_for(4, 42),
        end_for(2, 43),
    ])
    .await;
    let root = rs
        .claim_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        )
        .await
        .unwrap();
    let unfinished = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    let finished = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    assert!(
        !rs.retire_incomplete_admission_claim(finished.start_idx())
            .await
            .unwrap()
    );
    assert!(
        rs.retire_incomplete_admission_claim(unfinished.start_idx())
            .await
            .unwrap()
    );
    assert!(
        !rs.cursor
            .state
            .lock()
            .await
            .concurrent_resolver
            .has_claim(unfinished.start_idx())
    );
    assert!(
        rs.retire_incomplete_admission_claim(OplogIndex::from_u64(99))
            .await
            .is_err()
    );
    rs.await_resolution(finished).await.unwrap();
    rs.await_resolution(root).await.unwrap();
}

#[test]
#[test_r::timeout("30s")]
async fn admission_refusal_reproduces_incomplete_scope_without_hiding_completed_scope() {
    use golem_common::model::oplog::payload::types::SerializableToolAdmissionRefusal;
    for completed in [false, true] {
        let mut entries = vec![
            OplogEntry::RecoverySucceeded {
                timestamp: Timestamp::now_utc(),
            },
            start_now(),
            nested_batched_scope_start(2),
        ];
        if completed {
            entries.push(batched_scope_end(3));
        }
        entries.push(end_for(2, 43));
        let terminal = OplogIndex::from_u64(entries.len() as u64);
        let rs = replay_state_over(entries).await;
        let root = rs
            .claim_concurrent_start(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            )
            .await
            .unwrap();
        let claim = StartClaim::scope(
            &HostFunctionName::Custom("<scope:batched-write>".into()),
            &DurableFunctionType::WriteRemote,
            Some(OplogIndex::from_u64(2)),
        );
        let ReplayStartClaimOutcome::Claimed { handle, .. } =
            rs.claim_start_for_store(claim, false).await.unwrap()
        else {
            panic!("scope must be claimed");
        };
        let operation = crate::durable_host::tool::operation::tests::replay_operation(2);
        let refusal = SerializableToolAdmissionRefusal::AtRecordedOplogEntry { index: 3 };
        operation.install_recorded_admission_refusal(refusal.clone(), terminal);
        let result = crate::durable_host::concurrent::reproduce_incomplete_start_refusal(
            Some(&operation),
            &rs,
            handle.start_idx(),
        )
        .await;
        if completed {
            result.unwrap();
            assert_eq!(operation.observed_admission_refusal(), None);
            rs.await_resolution(handle).await.unwrap();
        } else {
            assert!(
                crate::durable_host::is_tool_attachment_live_admission_rejection(
                    &result.unwrap_err()
                )
            );
            assert_eq!(operation.observed_admission_refusal(), Some(refusal));
        }
        rs.await_resolution(root).await.unwrap();
    }
}

fn wait_for_start(duration_in_nanos: u64, invocation_id: Option<u64>) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::P3MonotonicClockWaitFor,
        invocation_id: invocation_id.map(test_invocation_id),
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(
            HostRequestMonotonicClockDuration { duration_in_nanos }.into(),
        ))),
        durable_function_type: DurableFunctionType::ReadLocal,
        span_started: None,
    }
}

async fn claimed(rs: &ReplayState, claim: StartClaim) -> ReplayCallHandle {
    match rs.claim_start_for_store(claim, false).await.unwrap() {
        ReplayStartClaimOutcome::Claimed { handle, .. } => handle,
        _ => panic!("the claim must find its Start"),
    }
}

fn wait_for_claim(invocation_id: Option<u64>) -> StartClaim {
    StartClaim::unowned(
        &HostFunctionName::P3MonotonicClockWaitFor,
        &DurableFunctionType::ReadLocal,
    )
    .with_invocation_id(invocation_id.map(test_invocation_id))
}

#[test]
async fn missing_start_claim_retains_the_start_of_a_sibling_recorded_before_a_crash() {
    // The guest made wait-for 1 and wait-for 2 concurrently, and only wait-for 2 recorded its
    // Start before the crash, with and without its End. Wait-for 1 claims first on replay.
    for sibling_end in [false, true] {
        let mut entries = vec![remove_retry_policy(), wait_for_start(300_000_000, Some(2))];
        if sibling_end {
            entries.push(end_for(2, 0));
        }
        let rs = replay_state_over(entries).await;

        let outcome = rs.claim_start_or_replay_end(wait_for_claim(Some(1))).await;
        assert!(
            matches!(outcome, Ok(ReplayStartClaimOutcome::ReplayEnded)),
            "the missing Start of a crash prefix must end replay"
        );
        assert!(rs.has_unclaimed_retained_starts());

        let sibling = claimed(&rs, wait_for_claim(Some(2))).await;
        assert_eq!(sibling.start_idx(), OplogIndex::from_u64(2));
        assert!(!rs.has_unclaimed_retained_starts());
        if sibling_end {
            assert!(matches!(
                rs.await_resolution(sibling).await.unwrap(),
                Resolution::Completed { end_idx, .. } if end_idx == OplogIndex::from_u64(3)
            ));
        }
    }
}

#[test]
async fn missing_start_claim_does_not_depend_on_how_far_the_sibling_claimed() {
    // The sibling guest call (id 2) recorded a wait-for and a child under it with its own id
    // before the crash; guest call 1 recorded nothing. Call 1 ends replay whether or not the
    // sibling has already claimed its outer Start, and the sibling still claims both.
    let nested_wait_for = |invocation_id: DurableInvocationId| {
        let mut entry = wait_for_start(1, None);
        let OplogEntry::Start {
            parent_start_index,
            invocation_id: id,
            ..
        } = &mut entry
        else {
            unreachable!();
        };
        *parent_start_index = Some(OplogIndex::from_u64(2));
        *id = Some(invocation_id);
        entry
    };
    let nested_claim = || {
        StartClaim::owned(
            &HostFunctionName::P3MonotonicClockWaitFor,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .with_invocation_id(Some(test_invocation_id(2)))
    };
    for sibling_claimed_first in [false, true] {
        let rs = replay_state_over(vec![
            remove_retry_policy(),
            wait_for_start(300_000_000, Some(2)),
            nested_wait_for(test_invocation_id(2)),
            end_for(3, 0),
            end_for(2, 0),
        ])
        .await;
        let sibling = if sibling_claimed_first {
            Some(claimed(&rs, wait_for_claim(Some(2))).await)
        } else {
            None
        };

        assert!(
            matches!(
                rs.claim_start_or_replay_end(wait_for_claim(Some(1))).await,
                Ok(ReplayStartClaimOutcome::ReplayEnded)
            ),
            "sibling_claimed_first = {sibling_claimed_first}"
        );

        let sibling = match sibling {
            Some(sibling) => sibling,
            None => claimed(&rs, wait_for_claim(Some(2))).await,
        };
        assert_eq!(sibling.start_idx(), OplogIndex::from_u64(2));
        let nested = claimed(&rs, nested_claim()).await;
        assert_eq!(nested.start_idx(), OplogIndex::from_u64(3));
        assert!(!rs.has_unclaimed_retained_starts());
    }

    // A child that carries an invocation id of its own under an ordinary call is not the
    // sibling guest call's to claim, whether its parent is still retained or already claimed.
    for sibling_claimed_first in [false, true] {
        let rs = replay_state_over(vec![
            remove_retry_policy(),
            wait_for_start(300_000_000, Some(2)),
            nested_wait_for(DurableInvocationId::new(OplogIndex::from_u64(2), 0)),
            end_for(3, 0),
            end_for(2, 0),
        ])
        .await;
        let _sibling = if sibling_claimed_first {
            Some(claimed(&rs, wait_for_claim(Some(2))).await)
        } else {
            None
        };
        assert!(
            matches!(
                rs.claim_start_or_replay_end(wait_for_claim(Some(1))).await,
                Err(error) if error.to_string().contains("no matching Start")
            ),
            "sibling_claimed_first = {sibling_claimed_first}"
        );
    }
}

#[test]
async fn missing_start_claim_retains_the_observed_calls_of_a_sibling_custom_invocation() {
    // The sibling guest call (id 2) ran a custom durable invocation that observed a physical call,
    // which made a nested call, before the crash; guest call 1 recorded nothing.
    for sibling_claimed_first in [false, true] {
        let rs = replay_state_over(vec![
            remove_retry_policy(),
            custom_start("outer", 1, None, 2),
            observational_start_now(2, None),
            owned_start_now(3),
            end_for(4, 0),
            end_for(3, 0),
            custom_end(2, 10),
        ])
        .await;
        let outer = HostFunctionName::Custom("outer".to_string());
        let claim_custom = || {
            claim_custom_start(
                &rs,
                &outer,
                &DurableFunctionType::ReadRemote,
                None,
                test_invocation_id(2),
            )
        };
        let sibling = if sibling_claimed_first {
            Some(claim_custom().await.unwrap())
        } else {
            None
        };

        assert!(
            matches!(
                rs.claim_start_or_replay_end(wait_for_claim(Some(1))).await,
                Ok(ReplayStartClaimOutcome::ReplayEnded)
            ),
            "sibling_claimed_first = {sibling_claimed_first}"
        );

        if sibling.is_none() {
            claim_custom().await.unwrap();
        }
        assert!(!rs.has_unclaimed_retained_starts());
    }
}

#[test]
async fn missing_start_claim_waits_for_the_body_of_a_retained_entity_invocation() {
    // The sibling guest call (id 2) dispatched a tool whose body recorded a positional entry
    // before the crash; guest call 1 recorded nothing. The body consumes its entry only after
    // its invocation is claimed, so the missing claim waits instead of reporting divergence.
    let mut entity_start = rejected_tool_reconstruction_start(OplogIndex::from_u64(2));
    let OplogEntry::Start { invocation_id, .. } = &mut entity_start else {
        unreachable!();
    };
    *invocation_id = Some(test_invocation_id(2));
    let rs = replay_state_over(vec![
        remove_retry_policy(),
        wait_for_start(300_000_000, Some(2)),
        entity_start,
        anchored_remove_retry_policy(3),
        end_for(3, 0),
        end_for(2, 0),
    ])
    .await;

    let claim = spawn_start_claim(&rs, wait_for_claim(Some(1)));
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !claim.is_finished(),
        "the body entry of a retained entity invocation must not be divergence"
    );

    let _dispatch = claimed(&rs, wait_for_claim(Some(2))).await;
    let mut entity = claimed(
        &rs,
        StartClaim::tool_invocation(
            &HostFunctionName::GolemEntityInvoke,
            &HostFunctionName::GolemToolInvocationRejected,
            &DurableFunctionType::WriteLocal,
            OplogIndex::from_u64(2),
        )
        .with_invocation_id(Some(test_invocation_id(2))),
    )
    .await;
    assert_eq!(entity.start_idx(), OplogIndex::from_u64(3));
    let mut reconstruction = entity
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    let (idx, _) = rs
        .get_oplog_entry(Some(OplogIndex::from_u64(3)))
        .await
        .unwrap();
    assert_eq!(idx, OplogIndex::from_u64(4));

    assert!(matches!(
        finished_claim(claim).await.unwrap(),
        ReplayStartClaimOutcome::ReplayEnded
    ));
    reconstruction.body_settled();
}

#[test]
async fn missing_start_claim_with_the_invocation_id_of_an_unclaimed_start_is_divergence() {
    // The Start that the claiming guest call recorded under its own invocation id names another
    // function: the guest call itself diverged.
    for retained in [false, true] {
        let rs = replay_state_over(vec![
            remove_retry_policy(),
            custom_start("recorded", 1, None, 1),
            wait_for_start(300_000_000, Some(2)),
            remove_retry_policy(),
        ])
        .await;
        if retained {
            assert!(matches!(
                rs.claim_start_or_replay_end(wait_for_claim(Some(3))).await,
                Err(error) if error.to_string().contains("no matching Start")
            ));
            assert!(rs.has_unclaimed_retained_starts());
        }

        let error = match rs
            .claim_start_or_replay_end(
                StartClaim::unowned(
                    &HostFunctionName::Custom("claimed".to_string()),
                    &DurableFunctionType::ReadRemote,
                )
                .with_invocation_id(Some(test_invocation_id(1))),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("a Start with the claim's own invocation id must not be retained"),
        };
        assert!(
            error.to_string().contains("same invocation id"),
            "unexpected error: {error}"
        );
    }
}

#[test]
async fn missing_start_claim_is_divergence_at_a_positional_entry_after_a_sibling_start() {
    let rs = replay_state_over(vec![
        remove_retry_policy(),
        wait_for_start(300_000_000, Some(2)),
        remove_retry_policy(),
    ])
    .await;

    let error = match rs.claim_start_or_replay_end(wait_for_claim(Some(1))).await {
        Err(error) => error,
        Ok(_) => panic!("a positional entry of the claiming Store must not end replay"),
    };
    assert!(
        error.to_string().contains("no matching Start"),
        "unexpected error: {error}"
    );

    let sibling = claimed(&rs, wait_for_claim(Some(2))).await;
    assert_eq!(sibling.start_idx(), OplogIndex::from_u64(2));
}

#[test]
async fn concurrent_calls_claim_their_own_start_by_invocation_id_in_any_order() {
    // A completed 300 ms wait and a pending 600 s watchdog, recorded in guest call order.
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        wait_for_start(300_000_000, Some(1)),
        wait_for_start(600_000_000_000, Some(2)),
        end_for(2, 0),
    ])
    .await;

    // The watchdog's host task reaches its claim first.
    let watchdog = claimed(&rs, wait_for_claim(Some(2))).await;
    let nap = claimed(&rs, wait_for_claim(Some(1))).await;

    assert_eq!(watchdog.start_idx(), OplogIndex::from_u64(3));
    assert_eq!(nap.start_idx(), OplogIndex::from_u64(2));
}

#[test]
async fn calls_with_equal_requests_claim_their_own_start_by_invocation_id() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        wait_for_start(1_000_000_000, Some(1)),
        wait_for_start(1_000_000_000, Some(2)),
        end_for(2, 0),
    ])
    .await;

    let second = claimed(&rs, wait_for_claim(Some(2))).await;
    let first = claimed(&rs, wait_for_claim(Some(1))).await;

    assert_eq!(second.start_idx(), OplogIndex::from_u64(3));
    assert_eq!(first.start_idx(), OplogIndex::from_u64(2));
}

#[test]
async fn a_call_without_invocation_id_never_claims_a_numbered_start() {
    let rs = replay_state_over(vec![
        OplogEntry::RecoverySucceeded {
            timestamp: Timestamp::now_utc(),
        },
        wait_for_start(300_000_000, Some(1)),
        wait_for_start(300_000_000, None),
    ])
    .await;

    let unnumbered = claimed(&rs, wait_for_claim(None)).await;

    assert_eq!(unnumbered.start_idx(), OplogIndex::from_u64(3));
}

#[test]
fn scope_claim_requires_the_recorded_invocation_id() {
    let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());
    let scope_start = |invocation_id: Option<u64>| OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: scope_name.clone(),
        invocation_id: invocation_id.map(test_invocation_id),
        observational_owner: None,
        request: None,
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: None,
    };
    let claim = |invocation_id: Option<u64>| {
        StartClaim::scope(&scope_name, &DurableFunctionType::WriteRemote, None)
            .with_invocation_id(invocation_id.map(test_invocation_id))
    };

    assert!(claim(Some(1)).matches_start_identity(&scope_start(Some(1))));
    assert!(!claim(Some(1)).matches_start_identity(&scope_start(Some(2))));
    assert!(!claim(Some(1)).matches_start_identity(&scope_start(None)));
    assert!(!claim(None).matches_start_identity(&scope_start(Some(1))));
}

struct WalletComponentService {
    metadata: golem_service_base::model::component::Component,
    revisions: std::sync::Mutex<Vec<golem_common::model::component::ComponentRevision>>,
}

#[async_trait]
impl crate::services::component::ComponentService for WalletComponentService {
    async fn get(
        &self,
        _engine: &wasmtime::Engine,
        _component_id: ComponentId,
        _component_revision: golem_common::model::component::ComponentRevision,
    ) -> Result<
        (
            wasmtime::component::Component,
            golem_service_base::model::component::Component,
        ),
        WorkerExecutorError,
    > {
        unreachable!("wallet folding loads metadata, not a guest component")
    }

    async fn get_metadata(
        &self,
        component_id: ComponentId,
        forced_revision: Option<golem_common::model::component::ComponentRevision>,
    ) -> Result<golem_service_base::model::component::Component, WorkerExecutorError> {
        assert_eq!(component_id, self.metadata.id);
        let revision = forced_revision.expect("wallet folding must load the exact revision");
        self.revisions.lock().unwrap().push(revision);
        let mut metadata = self.metadata.clone();
        metadata.revision = revision;
        Ok(metadata)
    }

    async fn resolve_component(
        &self,
        _component_reference: String,
        _resolving_environment: EnvironmentId,
        _resolving_application: golem_common::model::application::ApplicationId,
        _resolving_account: golem_common::model::account::AccountId,
    ) -> Result<Option<ComponentId>, WorkerExecutorError> {
        unreachable!("wallet folding does not resolve a component name")
    }

    async fn all_cached_metadata(&self) -> Vec<golem_service_base::model::component::Component> {
        Vec::new()
    }

    async fn invalidate_all_metadata_for_environment(&self, _environment_id: EnvironmentId) {}
}

fn wallet_test_component(
    component_id: ComponentId,
    environment_id: EnvironmentId,
) -> golem_service_base::model::component::Component {
    golem_service_base::model::component::Component {
        id: component_id,
        revision: golem_common::model::component::ComponentRevision::INITIAL,
        environment_id,
        component_name: golem_common::model::component::ComponentName(
            "wallet-prefixes".to_string(),
        ),
        hash: golem_common::model::diff::Hash::empty(),
        application_id: golem_common::model::application::ApplicationId::new(),
        account_id: golem_common::model::account::AccountId::new(),
        account_email: golem_common::model::account::AccountEmail::new("test@golem"),
        application_name: golem_common::model::application::ApplicationName::try_from(
            "test-app".to_string(),
        )
        .unwrap(),
        environment_name: golem_common::model::environment::EnvironmentName::try_from("test-env")
            .unwrap(),
        component_size: 1,
        metadata: golem_common::model::component_metadata::ComponentMetadata::default(),
        created_at: chrono::Utc::now(),
        wasm_hash: golem_common::model::diff::Hash::empty(),
        object_store_key: "wallet-prefixes".to_string(),
    }
}

#[test]
async fn owner_wallet_reconstructs_physical_prefixes_without_rewinding_live_state() {
    use crate::worker::owner_wallet::{OwnerWallet, WalletBaseline};
    use crate::worker::tasks::RuntimeAppendTasks;
    use golem_common::model::agent::ResolvedOwnerContext;
    use golem_common::model::component::ComponentRevision;

    let component_id = ComponentId::new();
    let environment_id = EnvironmentId::new();
    let owner = OwnedAgentId::new(
        environment_id,
        &AgentId {
            component_id,
            agent_id: "wallet-prefixes".to_string(),
        },
    );
    let metadata = wallet_test_component(component_id, environment_id);
    let components = Arc::new(WalletComponentService {
        metadata: metadata.clone(),
        revisions: std::sync::Mutex::new(Vec::new()),
    });
    let oplog = Arc::new(InMemoryOplog::new());
    let installed = stored_test_card(CardId::new());
    let derived = stored_test_card(CardId::new());
    let later = stored_test_card(CardId::new());
    for entry in [
        remove_retry_policy(),
        OplogEntry::card_installed(None, None, Box::new(installed.clone()), 1),
        start_now(),
        OplogEntry::card_expired(None, installed.card_id(), 2),
        OplogEntry::CardDerived {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            card: Box::new(derived.clone()),
            wallet_generation: 2,
        },
        OplogEntry::successful_update(
            ComponentRevision::new(1).unwrap(),
            1,
            None,
            Default::default(),
            None,
        ),
        OplogEntry::successful_update(
            ComponentRevision::new(2).unwrap(),
            1,
            None,
            Default::default(),
            None,
        ),
        start_now(),
        OplogEntry::card_installed(None, None, Box::new(later.clone()), 3),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let baseline = WalletBaseline::new(
        owner.clone(),
        ResolvedOwnerContext::ComponentWorker,
        components.clone(),
        &metadata,
        oplog.as_ref(),
        None,
        Vec::new(),
    )
    .await
    .unwrap();
    let interest_index = Arc::new(crate::services::card_interest::CardInterestIndex::new());
    let wallet = OwnerWallet::new(
        baseline,
        Arc::new(RuntimeAppendTasks::default()),
        crate::worker::tasks::WorkerTasks::default(),
        Arc::new(tokio::sync::Mutex::new(())),
        interest_index.clone(),
    )
    .await
    .unwrap();
    wallet.activate_interest().await;
    assert!(oplog.reads.lock().unwrap().is_empty());
    let canonical = wallet.snapshot();
    let replay = ReplayState::new_for_owner(
        owner,
        oplog.clone(),
        DeletedRegions::from_regions([OplogRegion::from_range(4..=6)]),
        None,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        OplogIndex::from_u64(8),
        Some(wallet.clone()),
    )
    .await
    .unwrap();
    let constructor_reads = oplog.reads.lock().unwrap().clone();
    let initial = wallet.latest_replay_receipt().result.await.unwrap();
    assert_eq!(initial.wallet.generation, 1);
    assert_eq!(*oplog.reads.lock().unwrap(), constructor_reads);
    let mut tx = replay.cursor.tx().await.unwrap();
    assert!(
        tx.try_get_oplog_entry_leaving_unclaimed_starts(|_| false)
            .await
            .unwrap()
            .is_none()
    );
    replay.cursor.finish_tx(tx);
    assert_eq!(
        wallet
            .latest_replay_receipt()
            .result
            .await
            .unwrap()
            .wallet
            .generation,
        1
    );
    let earlier_prefix = initial;
    assert_eq!(
        replay
            .try_get_oplog_entry(|_| true)
            .await
            .unwrap()
            .unwrap()
            .0,
        OplogIndex::from_u64(3)
    );
    let reads = oplog.reads.lock().unwrap().clone();
    let later_prefix = wallet.latest_replay_receipt().result.await.unwrap();
    assert_eq!(later_prefix.wallet.generation, 2);
    assert!(!later_prefix.wallet.cards.contains_key(&installed.card_id()));
    assert!(!later_prefix.wallet.cards.contains_key(&derived.card_id()));
    assert!(!later_prefix.wallet.cards.contains_key(&later.card_id()));
    assert_eq!(
        later_prefix.wallet.metadata.revision,
        ComponentRevision::new(2).unwrap()
    );
    assert_eq!(
        *components.revisions.lock().unwrap(),
        [ComponentRevision::new(2).unwrap()]
    );
    assert_eq!(earlier_prefix.wallet.generation, 1);
    assert_eq!(earlier_prefix.wallet.cards[&installed.card_id()], installed);
    assert_eq!(
        earlier_prefix.wallet.metadata.revision,
        ComponentRevision::INITIAL
    );
    assert!(Arc::ptr_eq(&canonical, &wallet.snapshot()));
    assert_eq!(*oplog.reads.lock().unwrap(), reads);
    assert!(!reads.contains(&OplogIndex::from_u64(9)));
    let events = replay.take_new_replay_events();
    let projected = events
        .iter()
        .filter_map(|event| match event {
            ReplayEvent::Projected { receipt, .. } => Some(receipt.index),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(projected, [2, 4, 5, 7].map(OplogIndex::from_u64));
}

#[test]
async fn owner_wallet_selected_snapshot_excludes_earlier_effects_and_preserves_exact_derivation() {
    use crate::worker::owner_wallet::{OwnerWallet, WalletBaseline};
    use crate::worker::tasks::{RuntimeAppendTasks, WorkerTasks};
    use golem_common::model::agent::ResolvedOwnerContext;

    let metadata = wallet_test_component(ComponentId::new(), EnvironmentId::new());
    let owner_id = OwnedAgentId::new(
        metadata.environment_id,
        &AgentId {
            component_id: metadata.id,
            agent_id: "snapshot-wallet".to_string(),
        },
    );
    let before = stored_test_card(CardId::new());
    let selected = stored_test_card(CardId::new());
    let audit = stored_test_card(CardId::new());
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        remove_retry_policy(),
        OplogEntry::card_installed(None, None, Box::new(before.clone()), 1),
        OplogEntry::Snapshot {
            timestamp: Timestamp::now_utc(),
            data: OplogPayload::Inline(Box::new(Vec::new())),
            mime_type: "application/octet-stream".to_string(),
            active_cards: vec![selected.clone()],
            wallet_generation: 7,
            filesystem_snapshot: None,
        },
        start_now(),
        OplogEntry::CardDerived {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            card: Box::new(audit.clone()),
            wallet_generation: 7,
        },
        remove_retry_policy(),
    ] {
        oplog.add(entry).await.unwrap();
    }
    let baseline = WalletBaseline::new(
        owner_id.clone(),
        ResolvedOwnerContext::ComponentWorker,
        Arc::new(WalletComponentService {
            metadata: metadata.clone(),
            revisions: Default::default(),
        }),
        &metadata,
        oplog.as_ref(),
        Some(OplogIndex::from_u64(3)),
        Vec::new(),
    )
    .await
    .unwrap();
    assert_eq!(*oplog.reads.lock().unwrap(), [OplogIndex::from_u64(3)]);
    let interest_index = Arc::new(crate::services::card_interest::CardInterestIndex::new());
    let wallet = OwnerWallet::new(
        baseline,
        Arc::new(RuntimeAppendTasks::default()),
        WorkerTasks::default(),
        Arc::new(tokio::sync::Mutex::new(())),
        interest_index.clone(),
    )
    .await
    .unwrap();
    assert_eq!(*oplog.reads.lock().unwrap(), [OplogIndex::from_u64(3)]);
    wallet.activate_interest().await;
    let replay = ReplayState::new_for_owner(
        owner_id,
        oplog.clone(),
        DeletedRegions::from_regions([OplogRegion::from_range(2..=3)]),
        Some(OplogIndex::from_u64(3)),
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
        OplogIndex::from_u64(6),
        Some(wallet.clone()),
    )
    .await
    .unwrap();
    assert_eq!(
        replay
            .try_get_oplog_entry(|_| true)
            .await
            .unwrap()
            .unwrap()
            .0,
        OplogIndex::from_u64(4)
    );
    let projection = wallet.latest_replay_receipt().result.await.unwrap();
    assert_eq!(projection.wallet.generation, 7);
    assert_eq!(projection.wallet.cards.len(), 1);
    assert_eq!(projection.wallet.cards[&selected.card_id()], selected);
    assert!(!projection.wallet.cards.contains_key(&before.card_id()));
    replay.take_new_replay_events();
    let reads = oplog.reads.lock().unwrap().clone();
    assert!(
        wallet
            .recorded_derivation(OplogIndex::from_u64(4), None, &audit)
            .await
            .unwrap()
    );
    assert_eq!(*oplog.reads.lock().unwrap(), reads);
}

#[test]
async fn runtime_card_audits_use_physical_predecessors_independent_of_lookup_order_and_event_drain()
{
    use crate::worker::owner_wallet::{OwnerWallet, WalletBaseline};
    use crate::worker::tasks::RuntimeAppendTasks;
    use golem_common::model::agent::ResolvedOwnerContext;

    for conflict in -3..5 {
        let metadata = wallet_test_component(ComponentId::new(), EnvironmentId::new());
        let owner_id = OwnedAgentId::new(
            metadata.environment_id,
            &AgentId {
                component_id: metadata.id,
                agent_id: "audit-prefixes".to_string(),
            },
        );
        let installed = stored_test_card(CardId::new());
        let first = stored_test_card(CardId::new());
        let second = stored_test_card(CardId::new());
        let parent = (conflict == -3).then_some(OplogIndex::from_u64(3));
        let audit = |card: &StoredCard| OplogEntry::CardDerived {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: parent,
            card: Box::new(card.clone()),
            wallet_generation: 1,
        };
        let mut entries = vec![remove_retry_policy(); 23];
        entries[10] = start_now();
        entries[11] = OplogEntry::card_installed(None, None, Box::new(installed.clone()), 1);
        entries[12] = end_for(11, 42);
        entries[13] = audit(&first);
        entries[16] = start_now();
        entries[18] = end_for(17, 43);
        entries[19] = audit(&second);
        entries[20] = OplogEntry::card_expired(None, installed.card_id(), 2);
        entries[22] = invocation_finished();
        match conflict {
            0 => entries[14] = entries[13].clone(),
            1 => {
                if let OplogEntry::CardDerived {
                    entity_parent_start_index,
                    ..
                } = &mut entries[13]
                {
                    *entity_parent_start_index = Some(OplogIndex::from_u64(3));
                }
            }
            2 => {
                if let OplogEntry::CardDerived { card, .. } = &mut entries[13]
                    && let StoredCard::Concrete(card) = card.as_mut()
                {
                    card.expires_at = Some(chrono::DateTime::UNIX_EPOCH);
                }
            }
            3 => {
                if let OplogEntry::CardDerived {
                    wallet_generation, ..
                } = &mut entries[13]
                {
                    *wallet_generation = 0;
                }
            }
            _ => {}
        }
        let oplog = Arc::new(InMemoryOplog::new());
        for entry in entries {
            oplog.add(entry).await.unwrap();
        }
        let baseline = WalletBaseline::new(
            owner_id.clone(),
            ResolvedOwnerContext::ComponentWorker,
            Arc::new(WalletComponentService {
                metadata: metadata.clone(),
                revisions: Default::default(),
            }),
            &metadata,
            oplog.as_ref(),
            None,
            Vec::new(),
        )
        .await
        .unwrap();
        let interest_index = Arc::new(crate::services::card_interest::CardInterestIndex::new());
        let wallet = OwnerWallet::new(
            baseline,
            Arc::new(RuntimeAppendTasks::default()),
            crate::worker::tasks::WorkerTasks::default(),
            Arc::new(tokio::sync::Mutex::new(())),
            interest_index.clone(),
        )
        .await
        .unwrap();
        wallet.activate_interest().await;
        let canonical = wallet.snapshot();
        let replay = ReplayState::new_for_owner(
            owner_id,
            oplog.clone(),
            DeletedRegions::default(),
            None,
            crate::durable_host::tool::operation::OwnerToolOperations::new(),
            OplogIndex::from_u64(23),
            Some(wallet.clone()),
        )
        .await
        .unwrap();
        let mut consumption_error = None;
        let mut reconstruction = parent.map(|parent| {
            ConcurrentReplayResolver::default()
                .register_reconstruction(parent, Some(Arc::downgrade(&wallet)))
        });
        let reconstruction_hold = reconstruction.clone();
        while replay.last_replayed_index() < OplogIndex::from_u64(22) {
            let mut tx = replay.cursor.tx().await.unwrap();
            let (index, entry) = tx.raw_read_next_oplog_entry().await.unwrap();
            let result = tx.commit_consumed_entry(index, &entry).await;
            replay.cursor.finish_tx(tx);
            if let Err(error) = result {
                consumption_error = Some(error);
                break;
            }
        }
        replay.take_new_replay_events_for(false);
        replay.take_new_replay_events_for(true);
        let reads = oplog.reads.lock().unwrap().clone();
        if conflict >= 0 {
            assert!(
                consumption_error.is_some()
                    || wallet
                        .recorded_derivation(
                            OplogIndex::from_u64(if conflict == 4 { 14 } else { 11 }),
                            None,
                            &first
                        )
                        .await
                        .is_err(),
                "conflict {conflict} was accepted"
            );
            assert_eq!(*oplog.reads.lock().unwrap(), reads);
            continue;
        }
        assert!(consumption_error.is_none());
        assert_eq!(
            wallet
                .latest_replay_receipt()
                .result
                .await
                .unwrap()
                .wallet
                .generation,
            2
        );
        for (start, card) in [(17, &second), (11, &first)] {
            if conflict <= -2 && start == 17 {
                continue;
            }
            assert!(
                wallet
                    .recorded_derivation(OplogIndex::from_u64(start), parent, card)
                    .await
                    .unwrap()
            );
            assert!(
                wallet
                    .recorded_derivation(OplogIndex::from_u64(start), parent, card)
                    .await
                    .is_err()
            );
            assert!(Arc::ptr_eq(&canonical, &wallet.snapshot()));
        }
        assert_eq!(*oplog.reads.lock().unwrap(), reads);
        let mut tx = replay.cursor.tx().await.unwrap();
        let (index, entry) = tx.raw_read_next_oplog_entry().await.unwrap();
        assert!(matches!(entry, OplogEntry::AgentInvocationFinished { .. }));
        tx.commit_consumed_entry(index, &entry).await.unwrap();
        replay.cursor.finish_tx(tx);
        if let Some(reconstruction) = &mut reconstruction {
            reconstruction.body_settled();
            assert!(wallet.has_recorded_derivation(first.card_id()));
            assert!(wallet.has_recorded_derivation(second.card_id()));
        }
        drop(reconstruction);
        if parent.is_some() {
            assert!(wallet.has_recorded_derivation(first.card_id()));
            assert!(wallet.has_recorded_derivation(second.card_id()));
        }
        drop(reconstruction_hold);
        assert!(
            !wallet
                .recorded_derivation(OplogIndex::from_u64(11), None, &first)
                .await
                .unwrap()
        );
        assert!(
            !wallet
                .recorded_derivation(OplogIndex::from_u64(17), None, &second)
                .await
                .unwrap()
        );
    }
}

#[test]
async fn failed_wallet_publication_poison_is_local_and_rebuild_retains_the_entry() {
    use crate::worker::owner_wallet::{
        OwnerWallet, WalletBaseline, WalletCommit, WalletPublicationFailure,
    };
    use crate::worker::tasks::RuntimeAppendTasks;
    use golem_common::model::agent::ResolvedOwnerContext;

    for retire in [false, true] {
        let metadata = wallet_test_component(ComponentId::new(), EnvironmentId::new());
        let owner_id = OwnedAgentId::new(
            metadata.environment_id,
            &AgentId {
                component_id: metadata.id,
                agent_id: "wallet-publication".to_string(),
            },
        );
        let oplog = Arc::new(InMemoryOplog::new());
        oplog.add(remove_retry_policy()).await.unwrap();
        let baseline = WalletBaseline::new(
            owner_id.clone(),
            ResolvedOwnerContext::ComponentWorker,
            Arc::new(WalletComponentService {
                metadata: metadata.clone(),
                revisions: Default::default(),
            }),
            &metadata,
            oplog.as_ref(),
            None,
            Vec::new(),
        )
        .await
        .unwrap();
        let interest_index = Arc::new(crate::services::card_interest::CardInterestIndex::new());
        let owner = OwnerWallet::new(
            baseline.clone(),
            Arc::new(RuntimeAppendTasks::default()),
            crate::worker::tasks::WorkerTasks::default(),
            Arc::new(tokio::sync::Mutex::new(())),
            interest_index.clone(),
        )
        .await
        .unwrap();
        owner.activate_interest().await;
        let card = stored_test_card(CardId::new());
        owner.finish_replay_prefix();
        let dropped = WalletCommit::prepare(
            owner.lock_boundary().await.unwrap(),
            OplogEntry::card_installed(None, None, Box::new(card.clone()), 0),
        )
        .await
        .unwrap();
        let before_drop = owner.snapshot();
        assert!(!before_drop.cards.contains_key(&card.card_id()));
        dropped.validate_before_append().unwrap();
        drop(dropped);
        assert!(Arc::ptr_eq(&before_drop, &owner.snapshot()));
        assert_eq!(oplog.current_oplog_index().await, OplogIndex::INITIAL);
        let commit = WalletCommit::prepare(
            owner.lock_boundary().await.unwrap(),
            OplogEntry::card_installed(None, None, Box::new(card.clone()), 0),
        )
        .await
        .unwrap();
        commit.validate_before_append().unwrap();
        let index = oplog.add(commit.entry()).await.unwrap();
        oplog.commit(CommitLevel::Always).await.unwrap();

        let before_fault = owner.snapshot();
        if retire {
            owner.retire();
        } else {
            owner
                .publish(&before_fault, Arc::new(before_fault.as_ref().clone()))
                .unwrap();
        }
        let preserved = owner.snapshot();
        assert_eq!(
            commit.publish(),
            Err(if retire {
                WalletPublicationFailure::RetiredGeneration
            } else {
                WalletPublicationFailure::PredecessorChanged
            })
        );
        assert!(owner.validate_current().is_err());
        assert!(Arc::ptr_eq(&preserved, &owner.snapshot()));
        assert!(!owner.snapshot().cards.contains_key(&card.card_id()));
        drop(commit);

        let replacement = OwnerWallet::new(
            baseline,
            Arc::new(RuntimeAppendTasks::default()),
            crate::worker::tasks::WorkerTasks::default(),
            Arc::new(tokio::sync::Mutex::new(())),
            interest_index.clone(),
        )
        .await
        .unwrap();
        replacement.activate_interest().await;
        let replay = ReplayState::new_for_owner(
            owner_id,
            oplog.clone(),
            DeletedRegions::default(),
            None,
            crate::durable_host::tool::operation::OwnerToolOperations::new(),
            index,
            Some(replacement.clone()),
        )
        .await
        .unwrap();
        assert!(replay.is_live());
        drop(replacement.lock_boundary().await.unwrap());
        replacement.validate_current().unwrap();
        assert_eq!(replacement.snapshot().cards[&card.card_id()], card);
        assert_eq!(replacement.snapshot().generation, 1);
        assert_eq!(oplog.current_oplog_index().await, index);
        assert!(owner.publish(&preserved, preserved.clone()).is_err());
        replacement.validate_current().unwrap();
    }
}

#[test]
#[test_r::timeout("10s")]
async fn owner_boundary_admission_precedes_poll_and_drains_cancelled_acquisition() {
    use crate::worker::owner_wallet::{OwnerWallet, WalletBaseline};
    use crate::worker::tasks::RuntimeAppendTasks;
    use golem_common::model::agent::ResolvedOwnerContext;

    let metadata = wallet_test_component(ComponentId::new(), EnvironmentId::new());
    let owner_id = OwnedAgentId::new(
        metadata.environment_id,
        &AgentId {
            component_id: metadata.id,
            agent_id: "wallet-boundary".to_string(),
        },
    );
    let oplog = InMemoryOplog::new();
    oplog.add(remove_retry_policy()).await.unwrap();
    let baseline = WalletBaseline::new(
        owner_id,
        ResolvedOwnerContext::ComponentWorker,
        Arc::new(WalletComponentService {
            metadata: metadata.clone(),
            revisions: Default::default(),
        }),
        &metadata,
        &oplog,
        None,
        Vec::new(),
    )
    .await
    .unwrap();
    for phase in 0..3 {
        let append_tasks = Arc::new(RuntimeAppendTasks::default());
        let boundary_lock = Arc::new(tokio::sync::Mutex::new(()));
        let interest_index = Arc::new(crate::services::card_interest::CardInterestIndex::new());
        let owner = OwnerWallet::new(
            baseline.clone(),
            append_tasks.clone(),
            crate::worker::tasks::WorkerTasks::default(),
            boundary_lock.clone(),
            interest_index.clone(),
        )
        .await
        .unwrap();
        owner.activate_interest().await;
        owner.finish_replay_prefix();
        let contention = if phase == 2 {
            Some(boundary_lock.lock_owned().await)
        } else {
            None
        };
        let mut captured = Box::pin({
            let transient = owner.clone();
            transient.lock_boundary()
        });
        let mut drain = Box::pin(append_tasks.seal_and_wait());
        assert!(futures::poll!(drain.as_mut()).is_pending());
        assert!(owner.lock_boundary().await.is_err());
        match phase {
            0 => drop(captured),
            1 => {
                let boundary = captured.await.unwrap();
                assert!(futures::poll!(drain.as_mut()).is_pending());
                drop(boundary);
            }
            2 => {
                assert!(futures::poll!(captured.as_mut()).is_pending());
                drop(captured);
            }
            _ => unreachable!(),
        }
        drain.await;
        drop(contention);
    }
}
