use super::*;
use crate::services::oplog::{CommitLevel, OplogAddReceipt, OrderedOplogStart, PendingUpload};
use async_trait::async_trait;
use golem_common::model::card::{
    AgentCardHolder, Card, CardHolder, CardId, InvocationWalletPin, StoredCard, WalletVersionToken,
};
use golem_common::model::component::ComponentId;
use golem_common::model::entity::{
    EntityCallMode, ToolInvocationClaimIdentity, ToolInvocationRejectedIdentity,
};
use golem_common::model::environment::EnvironmentId;
use golem_common::model::invocation_context::TraceId;
use golem_common::model::oplog::payload::types::{
    SerializableP3HttpBodyChunk, SerializableP3HttpConsumeBodyResult, SerializableToolRpcError,
};
use golem_common::model::oplog::{
    AgentError, DurableFunctionType, HostRequest, HostRequestGolemToolInvocationRejected,
    HostRequestNoInput, HostRequestPollCount, HostResponseMonotonicClockTimestamp,
    HostResponseP3HttpClientConsumeBodyChunk, HostResponseP3HttpClientConsumeBodyResult,
    HostStreamKind, OplogPayload, PayloadId, RawOplogPayload,
};
use golem_common::model::regions::OplogRegion;
use golem_common::model::tool::ToolName;
use golem_common::model::{AgentId, AgentInvocationPayload, IdempotencyKey, Timestamp};
use golem_common::schema::IntoTypedSchemaValue;
use std::collections::BTreeMap;
use std::time::Duration;
use test_r::test;

type StoredExternalPayload = (PayloadId, Vec<u8>, Vec<u8>);

/// Minimal in-memory `Oplog` used to drive a [`ReplayState`] over hand-built entries.
#[derive(Debug)]
struct InMemoryOplog {
    entries: std::sync::Mutex<Vec<OplogEntry>>,
    external_payloads: tokio::sync::Mutex<Vec<StoredExternalPayload>>,
}

impl InMemoryOplog {
    fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(Vec::new()),
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
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry);
        OplogIndex::from_u64(entries.len() as u64)
    }

    fn enqueue_add(&self, entry: OplogEntry) -> OplogAddReceipt {
        let mut entries = self.entries.lock().unwrap();
        entries.push(entry);
        let index = OplogIndex::from_u64(entries.len() as u64);
        Box::pin(async move { index })
    }

    async fn add_pair(
        &self,
        start: OplogEntry,
        make_second: Box<dyn FnOnce(OplogIndex) -> OplogEntry + Send>,
    ) -> (OplogIndex, OplogIndex) {
        let mut entries = self.entries.lock().unwrap();
        entries.push(start);
        let first_idx = OplogIndex::from_u64(entries.len() as u64);
        entries.push(make_second(first_idx));
        let second_idx = OplogIndex::from_u64(entries.len() as u64);
        (first_idx, second_idx)
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

    async fn add_start_with_indexed_reserved_raw_payload(
        &self,
        build_request: crate::services::oplog::IndexedReservedStartBuilder,
    ) -> Result<OrderedOplogStart, String> {
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

    async fn commit(&self, _level: CommitLevel) -> BTreeMap<OplogIndex, OplogEntry> {
        BTreeMap::new()
    }

    async fn current_oplog_index(&self) -> OplogIndex {
        OplogIndex::from_u64(self.entries.lock().unwrap().len() as u64)
    }

    async fn last_added_non_hint_entry(&self) -> Option<OplogIndex> {
        None
    }

    async fn wait_for_replicas(&self, _replicas: u8, _timeout: Duration) -> bool {
        true
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
    ReplayState::new_for_owner(
        owned_agent_id,
        oplog,
        skipped_regions,
        initial_snapshot_skip_end,
        crate::durable_host::tool::operation::OwnerToolOperations::new(),
    )
    .await
}

fn noop() -> OplogEntry {
    OplogEntry::NoOp {
        timestamp: Timestamp::now_utc(),
    }
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
        wallet_pin: Some(wallet_pin),
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
    }
}

fn rejected_tool_reconstruction_start(
    parent_start_index: OplogIndex,
) -> (OplogEntry, ToolInvocationClaimIdentity) {
    let tool_name = ToolName::try_from("reconstruction-test").unwrap();
    let identity = ToolInvocationClaimIdentity {
        accepted: None,
        rejected: ToolInvocationRejectedIdentity {
            tool_name: tool_name.clone(),
            command_path: vec!["run".to_string()],
            input: None,
            input_decode_failure: None,
            has_stdin: false,
            has_stdout: false,
            call_mode: EntityCallMode::Synchronous,
        },
    };
    let request =
        HostRequest::GolemToolInvocationRejected(HostRequestGolemToolInvocationRejected {
            tool_name: tool_name.into_inner(),
            command_path: vec!["run".to_string()],
            input: None,
            input_decode_failure: None,
            has_stdin: false,
            has_stdout: false,
            call_mode: EntityCallMode::Synchronous,
            error: SerializableToolRpcError::Denied("recorded rejection".to_string()),
        });
    (
        OplogEntry::Start {
            timestamp: Timestamp::now_utc(),
            parent_start_index: Some(parent_start_index),
            function_name: HostFunctionName::GolemToolInvocationRejected,
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(request))),
            durable_function_type: DurableFunctionType::WriteLocal,
        },
        identity,
    )
}

async fn claim_rejected_tool_reconstruction(
    replay: &ReplayState,
    parent_start_index: OplogIndex,
    identity: &ToolInvocationClaimIdentity,
) -> ReplayCallHandle {
    match replay
        .claim_start_or_replay_end(StartClaim::owned_tool_invocation(
            &HostFunctionName::GolemEntityInvoke,
            &HostFunctionName::GolemToolInvocationRejected,
            &DurableFunctionType::WriteLocal,
            parent_start_index,
            identity,
        ))
        .await
        .unwrap()
    {
        ReplayStartClaimOutcome::Claimed { handle, .. } => handle,
        ReplayStartClaimOutcome::ReplayEnded | ReplayStartClaimOutcome::DeletedRegion => {
            panic!("expected rejected tool reconstruction Start")
        }
    }
}

fn custom_request(value: i32) -> HostRequest {
    HostRequest::Custom(value.into_typed_schema_value().unwrap())
}

fn custom_start(name: &str, value: i32, parent: Option<u64>, invocation_id: u128) -> OplogEntry {
    custom_start_with_request(name, custom_request(value), parent, invocation_id)
}

fn custom_start_with_request(
    name: &str,
    request: HostRequest,
    parent: Option<u64>,
    invocation_id: u128,
) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: parent.map(OplogIndex::from_u64),
        function_name: HostFunctionName::Custom(name.to_string()),
        invocation_id: Some(uuid::Uuid::from_u128(invocation_id)),
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(request))),
        durable_function_type: DurableFunctionType::ReadRemote,
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
        noop(),
        custom_start("outer", 1, None, 1),
        custom_start("inner", 2, Some(2), 2),
        custom_end(3, 20),
        custom_end(2, 10),
    ])
    .await;

    let claimed = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
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
        noop(),
        custom_start("outer", 1, None, 1),
        custom_start("inner", 2, Some(2), 2),
        custom_end(3, 20),
    ])
    .await;

    let claimed = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
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
        noop(),
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
async fn request_matching_claim_never_claims_observational_start() {
    let request: HostRequest = HostRequestNoInput {}.into();
    let rs = replay_state_over(vec![
        noop(),
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        start_now(),
    ])
    .await;

    let handle = rs
        .claim_concurrent_start_matching_request(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            &request,
        )
        .await
        .unwrap();

    assert_eq!(handle.start_idx(), OplogIndex::from_u64(4));
}

#[test]
async fn custom_replay_skips_interleaved_observational_tree_without_stealing_sibling() {
    let rs = replay_state_over(vec![
        noop(),
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        start_now(),
        end_for(4, 42),
        end_for(3, 99),
        custom_end(2, 10),
    ])
    .await;

    let custom = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
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
        noop(),
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        owned_start_now(3),
        stream_frame(4),
        end_for(4, 42),
        end_for(3, 99),
        custom_end(2, 10),
    ])
    .await;

    let custom = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
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
        noop(),
        custom_start("outer", 1, None, 1),
        custom_start("nested", 2, Some(2), 2),
        observational_start_now(3, None),
        end_for(4, 42),
        custom_end(3, 20),
        custom_end(2, 10),
    ])
    .await;

    let outer = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
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
        noop(),
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
    ])
    .await;

    let custom = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
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
        noop(),
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        end_for(3, 42),
        delivered_for(3),
        observational_start_now(2, None),
    ])
    .await;

    let custom = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
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
        noop(),
        custom_start("outer", 1, None, 1),
        custom_end(2, 10),
        observational_start_now(2, None),
        end_for(4, 99),
        noop(),
    ])
    .await;

    let custom = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
        )
        .await
        .unwrap();
    assert!(matches!(
        rs.await_resolution_outcome(custom.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { end_idx, .. })
            if end_idx == OplogIndex::from_u64(3)
    ));

    let next = rs
        .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::NoOp { .. }))
        .await
        .unwrap();
    assert_eq!(next.map(|(idx, _)| idx), Some(OplogIndex::from_u64(6)));
}

#[test]
async fn custom_replay_skips_observational_cancellation() {
    let rs = replay_state_over(vec![
        noop(),
        custom_start("outer", 1, None, 1),
        observational_start_now(2, None),
        cancelled_for(3),
        custom_end(2, 10),
    ])
    .await;

    let custom = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("outer".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
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
        noop(),
        custom_start_with_request("generator", no_input.clone(), None, 1),
        custom_start_with_request("generator", no_input.clone(), None, 2),
        custom_end(2, 10),
        custom_end(3, 20),
    ])
    .await;
    let name = HostFunctionName::Custom("generator".to_string());
    let second = rs
        .claim_custom_start_matching_invocation_id(
            &name,
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(2),
            &no_input,
        )
        .await
        .unwrap();
    let first = rs
        .claim_custom_start_matching_invocation_id(
            &name,
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &no_input,
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
async fn custom_claim_rejects_changed_request_for_same_invocation_id() {
    let rs = replay_state_over(vec![
        noop(),
        custom_start("operation", 1, None, 1),
        custom_end(2, 10),
    ])
    .await;

    let result = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("operation".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(2),
        )
        .await;
    let Err(err) = result else {
        panic!("a replayed custom request must match its recorded payload");
    };
    assert!(format!("{err}").contains("recorded request payload differs"));

    let claimed = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("operation".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
        )
        .await
        .expect("failed validation must not leave claim state behind");
    assert_eq!(claimed.handle.start_idx(), OplogIndex::from_u64(2));
}

#[test]
async fn custom_claim_rejects_reused_invocation_id() {
    let rs = replay_state_over(vec![
        noop(),
        custom_start("operation", 1, None, 1),
        custom_start("different-operation", 2, None, 1),
        custom_end(2, 10),
        custom_end(3, 20),
    ])
    .await;

    let result = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("operation".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &custom_request(1),
        )
        .await;
    let Err(err) = result else {
        panic!("custom invocation IDs are single-use");
    };
    assert!(format!("{err}").contains("reused by Starts 2 and 3"));
}

#[test]
async fn custom_claim_rejects_start_without_invocation_id() {
    let no_input: HostRequest = HostRequestNoInput {}.into();
    let rs = replay_state_over(vec![
        noop(),
        custom_start_without_invocation_id("generator", no_input.clone(), None),
        custom_end(2, 10),
    ])
    .await;

    let result = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("generator".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(1),
            &no_input,
        )
        .await;
    let Err(err) = result else {
        panic!("custom replay must require a deterministic invocation ID");
    };
    assert!(format!("{err}").contains("no Start with the required custom invocation ID"));
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
        noop(),
        custom_start("owner", 0, None, 1),
        observational,
        custom_start_with_request("operation", request.clone(), None, 2),
    ])
    .await;

    let claimed = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("operation".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(2),
            &request,
        )
        .await
        .unwrap();

    assert_eq!(claimed.handle.start_idx(), OplogIndex::from_u64(4));
}

#[test]
async fn custom_claim_ignores_start_without_invocation_id_before_exact_match() {
    let request = custom_request(1);
    let rs = replay_state_over(vec![
        noop(),
        custom_start_without_invocation_id("operation", request.clone(), None),
        custom_start_with_request("operation", request.clone(), None, 2),
        custom_end(2, 10),
        custom_end(3, 20),
    ])
    .await;

    let claimed = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("operation".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(2),
            &request,
        )
        .await
        .unwrap();
    assert_eq!(claimed.handle.start_idx(), OplogIndex::from_u64(3));
}

#[test]
async fn custom_claim_rejects_wrong_metadata_for_exact_id() {
    let request = custom_request(1);
    let rs = replay_state_over(vec![
        noop(),
        custom_start_with_request("different-operation", request.clone(), None, 2),
    ])
    .await;

    let result = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("operation".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            uuid::Uuid::from_u128(2),
            &request,
        )
        .await;
    let Err(err) = result else {
        panic!("an exact invocation ID with divergent metadata must be rejected");
    };
    assert!(format!("{err}").contains("different-operation"));
}

#[test]
async fn custom_claim_id_can_be_reused_after_replay_restart() {
    let entries = vec![
        noop(),
        custom_start("operation", 1, None, 1),
        custom_end(2, 10),
    ];
    let rs = replay_state_over(entries.clone()).await;
    let name = HostFunctionName::Custom("operation".to_string());
    let invocation_id = uuid::Uuid::from_u128(1);

    let claimed = rs
        .claim_custom_start_matching_invocation_id(
            &name,
            &DurableFunctionType::ReadRemote,
            None,
            invocation_id,
            &custom_request(1),
        )
        .await
        .unwrap();
    assert!(matches!(
        rs.await_resolution_outcome(claimed.handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));

    drop(rs);
    let rs = replay_state_over(entries).await;
    let claimed_again = rs
        .claim_custom_start_matching_invocation_id(
            &name,
            &DurableFunctionType::ReadRemote,
            None,
            invocation_id,
            &custom_request(1),
        )
        .await
        .unwrap();
    assert_eq!(claimed_again.handle.start_idx(), OplogIndex::from_u64(2));
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
        invocation_id: None,
        observational_owner: None,
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
    test_replay_state(test_agent_id(), oplog, DeletedRegions::default(), None)
        .await
        .expect("failed to build replay state")
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
    let (start, identity) = rejected_tool_reconstruction_start(parent);
    let oplog = Arc::new(InMemoryOplog::new());
    oplog.add(noop()).await;
    oplog.add(start).await;
    oplog.add(end_for(2, 1)).await;
    let replay = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::default(),
        None,
    )
    .await
    .expect("failed to build replay state");
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
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
async fn growing_replay_target_revokes_published_live_state() {
    let oplog = Arc::new(InMemoryOplog::new());
    oplog.add(noop()).await;
    let replay = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::default(),
        None,
    )
    .await
    .expect("failed to build replay state");
    assert!(replay.is_live_published());

    let new_target = oplog.add(noop()).await;
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

    let new_target = oplog.add(noop()).await;
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

    assert_eq!(
        replay
            .switch_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live
    );
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

    assert_eq!(
        transition.await.unwrap().unwrap(),
        ReplayToLiveOutcome::Live
    );
    assert_eq!(
        replay.take_new_replay_events(),
        vec![ReplayEvent::ReplayFinished]
    );
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

    assert_eq!(first.await.unwrap().unwrap(), ReplayToLiveOutcome::Live);
    assert_eq!(second.await.unwrap().unwrap(), ReplayToLiveOutcome::Live);
    assert_eq!(first_memory.reconciliation_grant_bytes(1), 1);
    assert_eq!(second_memory.reconciliation_grant_bytes(1), 1);
    assert!(replay.is_live_published());
}

#[test]
async fn old_settler_cannot_publish_a_grown_target() {
    let (replay, oplog, reconstruction) = held_completed_reconstruction().await;
    let old_target = replay.switch_cursor_to_live().await.unwrap();
    let new_target = oplog.add(noop()).await;
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
                        Ok(tx.publish_live_if_still_settling(old_target, &stale_memory))
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
                        Ok(tx.publish_live_if_still_settling(second_target, &current_memory))
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
    oplog.add(noop()).await;
    let owner_operations = crate::durable_host::tool::operation::OwnerToolOperations::new();
    let replay = ReplayState::new_for_owner(
        test_agent_id(),
        oplog,
        DeletedRegions::default(),
        None,
        owner_operations.clone(),
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
        noop(),
        invocation_started(wallet_pin.clone()),
        OplogEntry::CardDerived {
            timestamp: Timestamp::now_utc(),
            card: derived_card.clone(),
            wallet_generation: Some(0),
        },
        start_now(),
    ] {
        oplog.add(entry).await;
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
    assert_eq!(invocation.wallet_pin, Some(wallet_pin.clone()));
    assert_eq!(
        replay_state
            .pending_card_derivation(derived_card.card_id())
            .await,
        Some((derived_card.clone(), Some(0)))
    );
    assert_eq!(
        replay_state.take_new_replay_events(),
        vec![
            ReplayEvent::InvocationWalletPinned { wallet_pin },
            ReplayEvent::CardDerived {
                card: derived_card,
                wallet_generation: Some(0),
            },
        ]
    );
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
        noop(),
        OplogEntry::CardInstalled {
            timestamp: Timestamp::now_utc(),
            queued_event_index: None,
            card: card.clone(),
            wallet_generation: Some(7),
        },
        custom_start("durable-operation", 41, None, 1),
        custom_end(3, 42),
    ])
    .await;

    let claimed = rs
        .claim_custom_start_matching_invocation_id(
            &HostFunctionName::Custom("durable-operation".to_string()),
            &DurableFunctionType::ReadRemote,
            None,
            Uuid::from_u128(1),
            &custom_request(41),
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
    assert_eq!(
        rs.switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live
    );
    assert_eq!(
        rs.take_new_replay_events(),
        vec![
            ReplayEvent::CardInstalled {
                card,
                wallet_generation: Some(7),
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
        noop(),
        OplogEntry::CardTransferred {
            timestamp: Timestamp::now_utc(),
            transfer_id,
            source_card_id: Some(source_card_id),
            installed_card_id: card.card_id(),
            target_holder: target_holder.clone(),
            card: card.clone(),
            target_wallet_generation: Some(1),
        },
        start_now(),
    ] {
        oplog.add(entry).await;
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
            source_card_id: Some(source_card_id),
            installed_card_id: card.card_id(),
            target_holder,
            card,
            target_wallet_generation: Some(1),
        }]
    );
}

#[test]
async fn snapshot_prefix_suppresses_replayed_permission_events() {
    let card = stored_test_card(CardId::new());
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        noop(),
        OplogEntry::CardInstalled {
            timestamp: Timestamp::now_utc(),
            queued_event_index: None,
            card,
            wallet_generation: Some(1),
        },
        start_now(),
    ] {
        oplog.add(entry).await;
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

fn stdout_log(message: &str) -> OplogEntry {
    OplogEntry::Log {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
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
async fn start_claim_reports_replay_ended_when_cursor_is_live() {
    let rs = replay_state_over(vec![noop()]).await;

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
async fn missing_start_claim_remains_divergence_while_replaying() {
    let rs = replay_state_over(vec![noop(), start_now()]).await;

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
    for entry in [noop(), start_now(), start_with_parent(1)] {
        oplog.add(entry).await;
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
                invocation_id: None,
                observational_owner: None,
                request: Some(payload),
                durable_function_type: DurableFunctionType::ReadLocal,
            })
            .await;
    }

    let oplog: Arc<dyn Oplog> = oplog;
    let rs = test_replay_state(test_agent_id(), oplog, DeletedRegions::default(), None)
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
            invocation_id: None,
            observational_owner: None,
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
    // response payload (the DurableCallSession replay path downloads and converts it).
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
async fn invocation_boundary_rejects_abandoned_delivered_completion() {
    // A CompletionDelivered marker proves the recorded guest received the call's result. If replay
    // reaches the invocation boundary without claiming that Start, this is divergence rather than
    // a tolerable live-only abandoned branch and must fail instead of parking forever at the marker.
    let rs = replay_state_over(vec![
        noop(),
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
            invocation_id: None,
            observational_owner: None,
            request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
                HostRequestNoInput {},
            )))),
            durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
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
async fn marked_completion_is_prefetched_without_advancing_past_intervening_entry() {
    // The call completed only after another durable operation was recorded. Replay must expose the
    // host result without consuming that intervening operation, otherwise a guest scheduler that
    // waits for the call's readiness before reproducing the operation deadlocks. Guest delivery is
    // still held at the marker after both the intervening entry and the End are consumed.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        begin_atomic_region(),
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

    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

    let barrier = rs
        .await_completion_delivery(OplogIndex::from_u64(2), OplogIndex::from_u64(5))
        .await
        .unwrap();
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(5));
    barrier.acknowledge();
}

#[test]
async fn replay_delivery_marker_holds_cursor_until_guest_boundary() {
    // A completed before B was started, but A's callback was handed to the guest only after B
    // completed. Replay returns A's response to its host-side continuation at End, lets B advance,
    // then positionally consumes A's delivery marker. Once consumed, no later entry can advance
    // until A's actual guest-facing boundary acknowledges the transferred cursor gate.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        start_now(),
        end_for(4, 43),
        delivered_for(2),
        noop(),
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
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));

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
    let next = rs.get_oplog_entry();
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

#[test]
async fn replay_delivery_marker_skips_following_hints_after_guest_boundary() {
    let rs = replay_state_over(vec![
        noop(),
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
        noop(),
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        noop(),
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

    let optional = rs.try_get_oplog_entry(|entry| matches!(entry, OplogEntry::NoOp { .. }));
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
        noop(),
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
async fn request_matching_claim_does_not_scan_past_delivery_marker() {
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        start_now(),
        end_for(5, 43),
    ])
    .await;
    let request = HostRequest::NoInput(HostRequestNoInput {});
    let first = rs
        .claim_concurrent_start_matching_request(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            &request,
        )
        .await
        .unwrap();
    rs.await_resolution(first).await.unwrap();

    let second = rs.claim_concurrent_start_matching_request(
        &HostFunctionName::MonotonicClockNow,
        &DurableFunctionType::ReadLocal,
        &request,
    );
    tokio::pin!(second);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), second.as_mut())
            .await
            .is_err(),
        "a request-matching claim must not scan ahead to a Start after the delivery marker"
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
async fn markerless_completed_end_resolves_without_delivery_marker() {
    // The recorded run crashed after the `End` became durable but before the completion crossed
    // to the guest, so no `CompletionDelivered` marker follows. The resolution must expose that
    // as `delivery_marker: None` — the deferred accessor path tail-gates such completions.
    let rs = replay_state_over(vec![noop(), start_now(), end_for(2, 42)]).await;
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
        noop(),
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

    rs.await_natural_tail_end().await.unwrap();
    assert!(rs.is_live());
}

#[test]
async fn await_natural_tail_end_waits_for_positionally_owned_entry() {
    // The crash tail contains a real entry (`BeginAtomicRegion`) owned by the replaying guest:
    // the tail-gated waiter must park until the guest's positional reader consumes it, and only
    // then observe the exhausted tail. Delivering earlier could make the replayed guest skip
    // recorded entries — the crash window this gate closes.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        begin_atomic_region(),
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

    let waiter = rs.await_natural_tail_end();
    tokio::pin!(waiter);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), waiter.as_mut())
            .await
            .is_err(),
        "the tail-gated waiter must park while a positionally-owned entry remains"
    );

    let (idx, entry) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(4));
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

    waiter.await.unwrap();
    assert!(rs.is_live());
}

#[test]
async fn await_natural_tail_end_propagates_delivery_failure() {
    // Poisoning replay (a delivery boundary fired while a completion was still tail-gated) must
    // wake and fail a parked tail waiter instead of leaving it parked forever.
    let rs = replay_state_over(vec![noop(), begin_atomic_region()]).await;

    let waiter = rs.await_natural_tail_end();
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
async fn replay_delivery_barriers_preserve_adjacent_callback_order() {
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
        delivered_for(2),
        delivered_for(3),
        noop(),
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
        rs.get_oplog_entry().await.unwrap().0,
        OplogIndex::from_u64(8)
    );
}

#[test]
async fn dropped_replay_delivery_barrier_fails_later_cursor_work() {
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        noop(),
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
        .get_oplog_entry()
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
async fn delivered_marker_with_deleted_start_is_skipped_as_orphan() {
    // The deleted Start/End belong to an abandoned timeline. Their surviving delivery marker is
    // therefore an orphan hint and must not strand positional replay before the next kept entry.
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        noop(),
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        noop(),
    ] {
        oplog.add(entry).await;
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

    let (index, entry) = tokio::time::timeout(Duration::from_millis(100), rs.get_oplog_entry())
        .await
        .expect("an orphan CompletionDelivered marker must not block replay")
        .expect("the next kept entry must remain readable");
    assert_eq!(index, OplogIndex::from_u64(5));
    assert!(matches!(entry, OplogEntry::NoOp { .. }));
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
        noop(),
        start_now(),
        end_for(2, 42),
        delivered_for(2),
        discarded_for(2),
    ] {
        oplog.add(entry).await;
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
    for entry in [noop(), start_now(), end_for(2, 42)] {
        oplog.add(entry).await;
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

    rs.switch_cursor_to_live().await.unwrap();

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

#[test]
async fn reconstruction_claim_is_barrier_visible_when_atomic_claim_returns() {
    let parent = OplogIndex::from_u64(1);
    let (start, identity) = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![noop(), start, end_for(2, 1)]).await;
    let handle = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
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
    let (start, identity) = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![noop(), start]).await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
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
    let (start, identity) = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![noop(), start, end_for(2, 1)]).await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
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
    let (start, identity) = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![noop(), start, end_for(2, 1)]).await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
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

    let (start, identity) = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![noop(), start]).await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
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
async fn completed_entity_body_detects_unconsumed_owned_start_at_cursor_head() {
    let rs = replay_state_over(vec![
        noop(),
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
async fn completed_entity_body_waits_for_active_owner_of_retried_transaction_begin() {
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_with_parent(2),
        noop(),
        OplogEntry::begin_remote_transaction(
            golem_common::model::TransactionId::new("retried".to_string()),
            Some(OplogIndex::from_u64(3)),
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

    let (index, _) = rs.get_oplog_entry().await.unwrap();
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
        noop(),
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
    let mut entries = vec![noop(), start_now()];
    entries.extend(std::iter::repeat_with(noop).take(CHUNK_SIZE as usize + 1));
    entries.push(end_for(2, 42));
    let rs = replay_state_over(entries).await;

    assert!(
        rs.has_visible_terminal(OplogIndex::from_u64(2)).await,
        "entity execution mode classification must scan through the complete replay prefix"
    );
}

#[test]
async fn visible_scope_descendant_distinguishes_owned_work_from_siblings() {
    let only_sibling =
        replay_state_over(vec![noop(), start_now(), start_now(), end_for(3, 41)]).await;
    assert!(
        !only_sibling
            .has_visible_scope_descendant(OplogIndex::from_u64(2))
            .await,
        "a later sibling must not be mistaken for historical entity-body work"
    );

    let owned_child = replay_state_over(vec![
        noop(),
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
    }
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
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
        .await
        .expect("failed to build replay state");

    assert!(!rs.is_live(), "Start at 2 is not yet consumed");
    let (idx, _) = rs.get_oplog_entry().await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(2));

    assert!(
        rs.is_live(),
        "consuming the Start must jump over the deleted tail to the target"
    );
    assert_eq!(
        rs.switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live
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
    assert_eq!(
        rs.switch_to_live(&replay_linear_memory(), ReplayToLiveRole::PrimaryAgent)
            .await
            .unwrap(),
        ReplayToLiveOutcome::Live
    );
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

/// A non-hint *positional* marker entry (atomic-region boundary or an
/// rdbms-transaction internal marker). These are never claimed and never auto-drained; a
/// positional reader must consume them, and an overlapping awaiter parks on them until then.
fn random_positional_marker(rng: &mut rand::rngs::StdRng) -> OplogEntry {
    use rand::Rng;
    match rng.random_range(0..5) {
        0 => begin_atomic_region(),
        1 => end_atomic_region(1),
        2 => pre_commit_remote_transaction(1),
        3 => committed_remote_transaction(1),
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
/// boundaries and rdbms-transaction internal markers), all freely
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
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
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
/// top-level, `WriteRemoteBatched(None)`.
fn batched_scope_start() -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::Custom("<scope:batched-write>".to_string()),
        invocation_id: None,
        observational_owner: None,
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
        invocation_id: None,
        observational_owner: None,
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
        .claim_scope_start(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            None,
        )
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
            None,
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
        .claim_scope_start(&plain, &DurableFunctionType::WriteRemoteBatched(None), None)
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
        invocation_id: None,
        observational_owner: None,
        request: None,
        durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
    };
    let rs = replay_state_over(vec![noop(), discriminated_start, batched_scope_end(2)]).await;

    let plain = HostFunctionName::Custom("<scope:batched-write>".to_string());
    let err = rs
        .claim_scope_start(&plain, &DurableFunctionType::WriteRemoteBatched(None), None)
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
            None,
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

#[test]
async fn missing_scope_recovery_switches_live_over_benign_suffix() {
    let rs = replay_state_over(vec![noop(), noop()]).await;
    let scope_name = HostFunctionName::Custom("<scope:batched-write:consume-body:7>".to_string());

    let outcome = rs
        .claim_scope_start_or_recover_missing(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            None,
        )
        .await
        .unwrap();

    assert!(matches!(
        outcome,
        ScopeStartClaimOutcome::MissingSwitchedToLive
    ));
    assert!(rs.is_live());
}

#[test]
async fn missing_scope_presence_check_does_not_switch_live() {
    let foreign_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::MonotonicClockNow,
        invocation_id: None,
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::ReadLocal,
    };
    let rs = replay_state_over(vec![noop(), foreign_start]).await;
    let scope_name = HostFunctionName::Custom("<scope:batched-write:consume-body:7>".to_string());

    let outcome = rs
        .claim_scope_start_if_present(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            None,
        )
        .await
        .unwrap();

    assert!(matches!(outcome, ScopeStartClaimOutcome::Missing));
    assert!(!rs.is_live());
}

#[test]
async fn existing_scope_claim_does_not_wait_for_missing_scope_recovery_readiness() {
    let scope_name = HostFunctionName::Custom("<scope:batched-write:consume-body:7>".to_string());
    let scope_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: scope_name.clone(),
        invocation_id: None,
        observational_owner: None,
        request: None,
        durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
    };
    let rs = replay_state_over(vec![noop(), scope_start, batched_scope_end(2)]).await;

    let outcome = rs
        .claim_scope_start_or_recover_missing_when_ready(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            None,
            async { panic!("recovery readiness must not be polled for an existing scope") },
        )
        .await
        .unwrap();

    let ScopeStartClaimOutcome::Claimed {
        begin_index,
        handle,
    } = outcome
    else {
        panic!("expected the existing scope to be claimed")
    };
    assert_eq!(begin_index, OplogIndex::from_u64(2));
    assert!(matches!(
        rs.await_resolution_outcome(handle).await.unwrap(),
        ResolutionOutcome::Resolved(Resolution::Completed { .. })
    ));
    assert!(rs.is_live());
}

#[test]
async fn missing_scope_recovery_rejects_foreign_remote_write() {
    let foreign_write = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: HostFunctionName::MonotonicClockNow,
        invocation_id: None,
        observational_owner: None,
        request: Some(OplogPayload::Inline(Box::new(HostRequest::NoInput(
            HostRequestNoInput {},
        )))),
        durable_function_type: DurableFunctionType::WriteRemote,
    };
    let rs = replay_state_over(vec![noop(), foreign_write]).await;
    let scope_name = HostFunctionName::Custom("<scope:batched-write:consume-body:7>".to_string());

    let result = rs
        .claim_scope_start_or_recover_missing(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            None,
        )
        .await;
    let error = match result {
        Ok(_) => panic!("a foreign remote write must prevent missing-scope recovery"),
        Err(error) => error,
    };

    assert!(format!("{error}").contains("unsafe concurrent side effect"));
    assert!(!rs.is_live());
}

#[test]
async fn missing_scope_recovery_rejects_discriminator_collision() {
    let scope_name = HostFunctionName::Custom("<scope:batched-write:consume-body:7>".to_string());
    let conflicting_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: scope_name.clone(),
        invocation_id: None,
        observational_owner: None,
        request: None,
        durable_function_type: DurableFunctionType::ReadLocal,
    };
    let rs = replay_state_over(vec![noop(), conflicting_start]).await;

    let result = rs
        .claim_scope_start_or_recover_missing(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            None,
        )
        .await;
    let error = match result {
        Ok(_) => panic!("a same-name scope Start must not be treated as absent"),
        Err(error) => error,
    };

    assert!(format!("{error}").contains("same discriminator exists"));
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
        durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
    };
    let rs = replay_state_over(vec![noop(), scope_start, batched_scope_end(2)]).await;
    let scope_name = HostFunctionName::Custom("<scope:batched-write>".to_string());

    rs.claim_scope_start(
        &scope_name,
        &DurableFunctionType::WriteRemoteBatched(None),
        Some(OplogIndex::from_u64(8)),
    )
    .await
    .expect_err("a scope claim must not steal a sibling entity invocation's scope");

    let (scope_index, handle) = rs
        .claim_scope_start(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            Some(parent),
        )
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
    let scope_name = HostFunctionName::Custom("<scope:batched-write:req:owned>".to_string());
    let scope_start = OplogEntry::Start {
        timestamp: Timestamp::now_utc(),
        parent_start_index: None,
        function_name: scope_name.clone(),
        invocation_id: None,
        observational_owner: Some(owner),
        request: None,
        durable_function_type: DurableFunctionType::WriteRemoteBatched(None),
    };

    assert!(
        StartClaim::scope(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            None,
        )
        .with_observational_owner(Some(owner))
        .matches_start_identity(&scope_start)
    );
    assert!(
        !StartClaim::scope(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            None,
        )
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
        StartClaim::scope(&name, &DurableFunctionType::WriteRemoteBatched(None), None)
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
