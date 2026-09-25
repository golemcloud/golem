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
    HostStreamKind, OplogErrorKind, OplogPayload, PayloadId, RawOplogPayload,
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
        entity_parent_start_index: None,
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
    }
}

fn start_now_with_request_payload(payload: OplogPayload<HostRequest>) -> OplogEntry {
    let mut entry = start_now();
    let OplogEntry::Start { request, .. } = &mut entry else {
        unreachable!();
    };
    *request = Some(payload);
    entry
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

fn rejected_tool_reconstruction_start(
    parent_start_index: OplogIndex,
) -> (OplogEntry, ToolInvocationClaimIdentity) {
    let tool_name = ToolName::try_from("reconstruction-test").unwrap();
    let identity = ToolInvocationClaimIdentity {
        accepted: None,
        rejected: ToolInvocationRejectedIdentity {
            attempt_ordinal: 0,
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
            attempt_ordinal: 0,
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
    expected_invocation_id: uuid::Uuid,
    expected_request: &HostRequest,
) -> Result<ClaimedConcurrentStart, WorkerExecutorError> {
    match rs
        .claim_custom_start_for_store(
            expected_function_name,
            expected_function_type,
            expected_parent_start_index,
            expected_invocation_id,
            expected_request,
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

    let claimed = claim_custom_start(
        &rs,
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

    let claimed = claim_custom_start(
        &rs,
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

    let custom = claim_custom_start(
        &rs,
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

    let custom = claim_custom_start(
        &rs,
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

    let outer = claim_custom_start(
        &rs,
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

    let custom = claim_custom_start(
        &rs,
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

    let custom = claim_custom_start(
        &rs,
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

    let custom = claim_custom_start(
        &rs,
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

    let custom = claim_custom_start(
        &rs,
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
    let second = claim_custom_start(
        &rs,
        &name,
        &DurableFunctionType::ReadRemote,
        None,
        uuid::Uuid::from_u128(2),
        &no_input,
    )
    .await
    .unwrap();
    let first = claim_custom_start(
        &rs,
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

    let result = claim_custom_start(
        &rs,
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

    let claimed = claim_custom_start(
        &rs,
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

    let result = claim_custom_start(
        &rs,
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

    let result = claim_custom_start(
        &rs,
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

    let claimed = claim_custom_start(
        &rs,
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

    let claimed = claim_custom_start(
        &rs,
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

    let result = claim_custom_start(
        &rs,
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

    let claimed = claim_custom_start(
        &rs,
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
    let claimed_again = claim_custom_start(
        &rs,
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
        entity_parent_start_index: None,
    }
}

fn anchored_noop(parent_start_index: u64) -> OplogEntry {
    OplogEntry::NoOp {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: Some(OplogIndex::from_u64(parent_start_index)),
    }
}

fn anchored_error(entity_parent_start_index: u64, retry_from: u64) -> OplogEntry {
    OplogEntry::error(
        Some(OplogIndex::from_u64(entity_parent_start_index)),
        OplogErrorKind::Invocation,
        AgentError::TransientError("retry".to_string()),
        OplogIndex::from_u64(retry_from),
        false,
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
    assert_eq!(
        replay.take_new_replay_events(),
        vec![ReplayEvent::ReplayFinished]
    );
}

#[test]
async fn prepared_primary_transition_withholds_tail_delivery_until_publication() {
    let replay = replay_state_over(vec![noop(), noop()]).await;
    let linear_memory = replay_linear_memory();

    let outcome = replay
        .prepare_replay_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
        .await
        .unwrap();
    let ReplayToLiveOutcome::Live { replay_target } = outcome else {
        panic!("fixed replay target unexpectedly resumed");
    };
    assert!(replay.is_live());
    assert!(!replay.is_live_published());

    let tail_delivery = replay.await_live_publication(None);
    tokio::pin!(tail_delivery);
    assert!(
        futures::poll!(tail_delivery.as_mut()).is_pending(),
        "markerless delivery must not resume at an unpublished replay tail"
    );

    replay.publish_primary_live(replay_target).await.unwrap();
    tail_delivery.await.unwrap();
    assert!(replay.is_live_published());
}

#[test]
async fn assisted_primary_transition_waits_for_success_before_publication() {
    let replay = replay_state_over(vec![noop(), noop()]).await;
    let linear_memory = replay_linear_memory();
    let ReplayToLiveOutcome::Live { replay_target } = replay
        .prepare_replay_to_live(&linear_memory, ReplayToLiveRole::PrimaryAgent)
        .await
        .unwrap()
    else {
        panic!("fixed replay target unexpectedly resumed");
    };
    let gate = Arc::new(crate::durable_host::SnapshotAssistedFinalizationGate::new());
    gate.begin_finalization();
    let publication = tokio::spawn({
        let replay = replay.clone();
        let gate = gate.clone();
        async move {
            crate::durable_host::PendingReplayToLive {
                replay_target,
                role: ReplayToLiveRole::PrimaryAgent,
                local_continuation: false,
                replay_state: Some(replay),
                snapshot_assisted_finalization: Some(gate),
                replaying_incomplete_entity: false,
                tool_entity: false,
                tool_operation: None,
                local_live_tail: Arc::new(AtomicBool::new(false)),
            }
            .finish()
            .await
        }
    });

    tokio::task::yield_now().await;
    assert!(!publication.is_finished());
    assert!(!replay.is_live_published());

    gate.finish_finalization();
    assert_eq!(
        publication.await.unwrap().unwrap(),
        crate::durable_host::FinishReplayToLive::Live
    );
    assert!(replay.is_live_published());
}

#[test]
async fn primary_transition_releases_an_incomplete_reconstruction_fence() {
    let parent = OplogIndex::from_u64(1);
    let (start, identity) = rejected_tool_reconstruction_start(parent);
    let replay = replay_state_over(vec![noop(), start]).await;
    let mut handle = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
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
    let (first_start, identity) = rejected_tool_reconstruction_start(parent);
    let (second_start, _) = rejected_tool_reconstruction_start(parent);
    let oplog = Arc::new(InMemoryOplog::new());
    oplog.add(noop()).await;
    oplog.add(first_start).await;
    oplog.add(second_start).await;
    oplog.add(end_for(3, 2)).await;
    let replay = test_replay_state(
        test_agent_id(),
        oplog.clone(),
        DeletedRegions::default(),
        None,
    )
    .await
    .expect("failed to build replay state");
    let mut first = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
    let mut first_reconstruction = first
        .take_historical_reconstruction()
        .expect("first reconstruction guard");
    let mut second = claim_rejected_tool_reconstruction(&replay, parent, &identity).await;
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

    let new_target = oplog.add(end_for(2, 1)).await;
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
    replay
        .publish_primary_live(second_target)
        .await
        .expect("current settler must be able to publish the grown target");
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
        noop(),
        invocation_started(wallet_pin.clone()),
        OplogEntry::CardDerived {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            card: Box::new(derived_card.clone()),
            wallet_generation: 0,
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
            ReplayEvent::InvocationWalletPinned { wallet_pin },
            ReplayEvent::CardDerived {
                card: derived_card,
                wallet_generation: 0,
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
        noop(),
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
        noop(),
        OplogEntry::CardInstalled {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: None,
            queued_event_index: None,
            card: Box::new(card),
            wallet_generation: 1,
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
async fn resolution_readiness_preserves_positional_entries_and_retained_tails() {
    for with_span in [false, true] {
        for completed in [false, true] {
            let mut entries = vec![noop(), start_now()];
            if with_span {
                entries.push(OplogEntry::StartSpan {
                    timestamp: Timestamp::now_utc(),
                    parent_start_index: None,
                    span_id: golem_common::model::invocation_context::SpanId::generate(),
                    parent: None,
                    linked_context_id: None,
                    attributes: HashMap::new().into(),
                });
            }
            if completed {
                entries.push(end_for(2, 37));
                entries.push(noop()); // Ready terminal need not be the replay tail.
            }
            let rs = replay_state_over(entries).await;
            let handle = rs
                .claim_concurrent_start(
                    &HostFunctionName::MonotonicClockNow,
                    &DurableFunctionType::ReadLocal,
                )
                .await
                .unwrap();
            assert_eq!(rs.resolution_ready(&handle).await.unwrap(), !with_span);
            if with_span {
                assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(2));
                assert!(!rs.resolution_ready(&handle).await.unwrap());
                let (index, entry) = rs.get_oplog_entry(None).await.unwrap();
                assert_eq!(index, OplogIndex::from_u64(3));
                assert!(matches!(entry, OplogEntry::StartSpan { .. }));
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
                        OplogEntry::NoOp { .. }
                    ));
                }
                other => panic!("unexpected resolution: {other:?}"),
            }
        }
    }
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

async fn assert_request_payload_failure_is_not_reclassified_as_deleted_region(
    failing_payload: OplogPayload<HostRequest>,
    expected_error: &str,
) {
    let oplog = Arc::new(InMemoryOplog::new());
    let expected_request: HostRequest = HostRequestPollCount { count: 1 }.into();
    for entry in [
        noop(),
        start_now_with_request_payload(OplogPayload::Inline(Box::new(expected_request.clone()))),
        start_now_with_request_payload(failing_payload),
    ] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion::from_range(2..=2)]);
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
        .await
        .unwrap();
    let replayed_before_claim = rs.last_replayed_index();

    let result = rs
        .claim_start_or_replay_end(StartClaim::unowned_matching_request(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            &expected_request,
        ))
        .await;
    let error = match result {
        Err(error) => error,
        Ok(_) => panic!("payload failure must not be reclassified as a deleted-region match"),
    };

    assert!(
        format!("{error}").contains(expected_error),
        "original payload failure must propagate: {error}"
    );
    assert_eq!(rs.last_replayed_index(), replayed_before_claim);
}

#[test]
async fn external_request_payload_failure_is_not_reclassified_as_deleted_region() {
    assert_request_payload_failure_is_not_reclassified_as_deleted_region(
        OplogPayload::External {
            payload_id: PayloadId::new(),
            md5_hash: vec![42],
            cached: None,
        },
        "missing test payload",
    )
    .await;
}

#[test]
async fn inline_request_payload_decode_failure_is_not_reclassified_as_deleted_region() {
    assert_request_payload_failure_is_not_reclassified_as_deleted_region(
        OplogPayload::SerializedInline {
            bytes: vec![golem_common::serialization::SERIALIZATION_VERSION_V3],
            cached: None,
        },
        "failed to deserialize inline request payload",
    )
    .await;
}

#[test]
async fn genuine_request_mismatch_still_reports_matching_deleted_region() {
    let oplog = Arc::new(InMemoryOplog::new());
    let expected_request: HostRequest = HostRequestPollCount { count: 1 }.into();
    let different_request: HostRequest = HostRequestPollCount { count: 2 }.into();
    for entry in [
        noop(),
        start_now_with_request_payload(OplogPayload::Inline(Box::new(expected_request.clone()))),
        start_now_with_request_payload(OplogPayload::Inline(Box::new(different_request))),
    ] {
        oplog.add(entry).await;
    }
    let oplog: Arc<dyn Oplog> = oplog;
    let skipped = DeletedRegions::from_regions([OplogRegion::from_range(2..=2)]);
    let rs = test_replay_state(test_agent_id(), oplog, skipped, None)
        .await
        .unwrap();

    let outcome = rs
        .claim_start_or_replay_end(StartClaim::unowned_matching_request(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            &expected_request,
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
async fn predicate_probe_retains_unclaimed_start_and_drains_awaited_terminals() {
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — after claiming A, the cursor head
    // is the still-unclaimed, non-terminal Start(B). A read whose predicate fails must not steal
    // Start(B) (only B's owner may claim it) and must not park on it either (B's owner may need
    // the Store the reader holds): it retains B, keeps draining, routes End(A) to A's awaiter and
    // keeps End(B) with the retained Start for B's later claim.
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
            None,
            OplogErrorKind::Invocation,
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

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
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
#[test_r::timeout("10s")]
async fn request_matching_claim_adopts_retained_start_behind_its_own_delivery_marker() {
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5), Delivered(B=3) at 6,
    // Delivered(A=2) at 7]. Draining A's terminal retains B together with its End and parks the
    // cursor at B's delivery marker. B's request-matching claim must adopt the retained Start
    // instead of reporting itself blocked by the marker that only B's own delivery can release.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
        delivered_for(3),
        delivered_for(2),
    ])
    .await;
    let request = HostRequest::NoInput(HostRequestNoInput {});
    let handle_a = rs
        .claim_concurrent_start_matching_request(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            &request,
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
        rs.claim_concurrent_start_matching_request(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            &request,
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

    rs.await_natural_tail_end(None).await.unwrap();
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
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

    waiter.await.unwrap();
    assert!(rs.is_live());
}

#[test]
async fn await_natural_tail_end_propagates_delivery_failure() {
    // Poisoning replay (a delivery boundary fired while a completion was still tail-gated) must
    // wake and fail a parked tail waiter instead of leaving it parked forever.
    let rs = replay_state_over(vec![noop(), begin_atomic_region()]).await;

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

    let rs = replay_state_over(vec![noop(), begin_atomic_region()]).await;
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
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));
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
        let rs = replay_state_over(vec![noop(), begin_atomic_region()]).await;
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
        rs.get_oplog_entry(None).await.unwrap().0,
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
async fn reverted_completion_marker_can_be_replaced_and_reconstructed() {
    for old_marker in [delivered_for(2), discarded_for(2)] {
        for delivered in [true, false] {
            for grow_target in [false, true] {
                let oplog: Arc<dyn Oplog> = Arc::new(InMemoryOplog::new());
                for entry in [noop(), start_now(), end_for(2, 42)] {
                    oplog.add(entry).await;
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
                        oplog.add(entry.clone()).await;
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
                        oplog.add(entry).await;
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

    let (index, entry) = tokio::time::timeout(Duration::from_millis(100), rs.get_oplog_entry(None))
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
async fn drain_retains_unclaimed_start_for_its_owner() {
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — draining the awaited terminals
    // while only A is claimed must step over the still-unclaimed Start(B) *without* consuming it
    // as its own: B is retained (with its End attached once reached) for the owner that claims it
    // later, and A's End is drained to A. The drain never parks on B, because B's owner may need
    // the Store the draining task holds.
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
    // [NoOp, Start(A=2), End(A=2→3), Start(B=4), End(B=4→5), CardDerived(6), Start(C=7)] — A's
    // drain steps over B (retained, End attached) and the CardDerived hint that B's owner
    // recorded right after its End. The hint's replay event is a side effect of B's durable
    // call: publishing it while B is merely retained would let the guest's later authority
    // boundary apply the card before B's owner re-claims the Start and reads the derivation,
    // so it stays deferred on the retained Start until the owner claims it.
    let derived_card = stored_test_card(CardId::new());
    let rs = replay_state_over(vec![
        noop(),
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
    // [NoOp, Start(A=2), Start(B=3), End(B=3→4), CardDerived(5), End(A=2→6)] — B and the hint
    // behind its End are retained by A's await and B is never claimed. Releasing the closed
    // retained Start at the live invocation end must still publish the deferred hint: the card
    // derivation is recorded history and the next authority boundary has to observe it.
    let derived_card = stored_test_card(CardId::new());
    let rs = replay_state_over(vec![
        noop(),
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
    // [NoOp, Start(A=2 now), Start(B=3 now), End(A=2→4), End(B=3→5), Start(C=6 resolution)] —
    // after A's
    // drain retained B, a claim with a *different* identity must not adopt the retained Start:
    // ownership is validated at claim time, so the mismatched claim falls through to the cursor
    // head (C) exactly as if B had never been retained.
    let rs = replay_state_over(vec![
        noop(),
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
    // [NoOp, Start(A=2 now), Start(B=3 p3 http send), Start(C=4 rpc invoke),
    //  Start(D=5 resolution), End(A=2→6), End(B=3→7), End(C=4→8), End(D=5→9)] — resolving A
    // commits past B, C and D, retaining all three. Only a call charged to the same quota as a
    // retained Start may still be its late owner: B keeps HTTP-charged calls unfresh, C keeps
    // RPC-charged calls unfresh, and D (charged to no quota) suppresses neither. Adopting a
    // retained Start clears its class again while the others stay published.
    let rs = replay_state_over(vec![
        noop(),
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
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4)] — B's End was never recorded. Draining A's End
    // retains B; when B's owner claims it, the claim registers an ordinary awaiter that reaches
    // the replay tail and resolves as Incomplete instead of hanging or fabricating a result.
    let rs = replay_state_over(vec![noop(), start_now(), start_now(), end_for(2, 42)]).await;
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
    // [NoOp, Start(B=2), BeginAtomicRegion(3), End(B=2→4)] — a positional reader looking for the
    // atomic-region marker reaches the unclaimed durable-call Start(B) first. It must neither be
    // handed that Start (kind/ownership mismatch) nor park on it: the Start is retained for its
    // owner and the reader receives the marker at index 3.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        begin_atomic_region(),
        end_for(2, 42),
    ])
    .await;

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(3));
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));
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
    // [NoOp, BeginAtomicRegion(2), EndAtomicRegion(3)] — retention only applies to durable-call
    // Starts. A conditional positional reader expecting the *end* marker while the *begin* marker
    // is at the head must still see the mismatch (no entry, cursor untouched), never skip the
    // marker; the unconditional reader then receives the begin marker itself.
    let rs = replay_state_over(vec![noop(), begin_atomic_region(), end_atomic_region(2)]).await;
    let probe = rs
        .try_get_oplog_entry(|entry| matches!(entry, OplogEntry::EndAtomicRegion { .. }))
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
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));
}

#[test]
async fn store_live_claim_adopts_retained_start_or_reports_store_already_live() {
    // [NoOp, Start(A=2 now), Start(B=3 now), End(A=2→4), End(B=3→5), NoOp(6),
    // Start(C=7 resolution), End(C=7→8)] — A's awaiter retained B (and attached B's End) and
    // stopped at the positional NoOp(6), so the shared cursor is still short of the target. A
    // Store that already continued live locally still takes the replay admission path while
    // unclaimed retained Starts exist: its claim for B's identity adopts the retained Start
    // (never appending a duplicate), whereas a claim for an identity nobody retained is reported
    // as `StoreAlreadyLive` — a new live call of that Store — instead of strict divergence, and
    // must not steal the unclaimed Start(C) behind the marker.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(2, 42),
        end_for(3, 43),
        noop(),
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
    // claim for C's identity finds it by scan-ahead behind the unconsumed positional NoOp(6)
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
    // [NoOp, Start(A=2 now), Start(B=3 now), End(A=2→4), End(B=3→5), Start(C=6 resolution),
    // End(C=6→7)] — nothing positional stands between A's End and the target, so A's awaiter
    // drains to the target, retaining B and C with their Ends attached. The shared cursor is then
    // exhausted while retained Starts are still unclaimed: a live Store's claim for an identity
    // nobody retained reports `ReplayEnded` (the consumer's replay-to-live is idempotent for a
    // Store that already continued live), and the retained Starts stay for their owners' claims.
    let rs = replay_state_over(vec![
        noop(),
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
    // [NoOp, Start(root=2), Start(child=3 ← 2), Start(grandchild=4 ← 3), End(2→5), End(3→6),
    // End(4→7)] — root and child are claimed; awaiting root retains the grandchild. While the
    // child's body is still active, the grandchild is that body's own pending call and is not
    // reported; once no body is active it is the earliest unclaimed descendant of the root.
    let rs = replay_state_over(vec![
        noop(),
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
    // [NoOp, Start(A=2), Start(B=3), End(B=3→4), End(A=2→5)] — A's await commits past B's Start
    // (retained) and End (attached to it) before resolving at 5, and B is never claimed before
    // the invocation finishes live. B is closed history, so releasing at the live invocation end
    // clears the retained set (later durable calls take the plain live path) and a late claim now
    // sees the replay tail rather than a stale retained Start.
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
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4)] — B is retained by A's await, never claimed,
    // and has no terminal in history. Finishing the live invocation would append
    // `AgentInvocationFinished` after an unclosed Start, which the next reconstruction's
    // boundary read rejects; the release must therefore fail now instead of warning, and must
    // leave the retained Start in place.
    let rs = replay_state_over(vec![noop(), start_now(), start_now(), end_for(2, 42)]).await;
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
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), NoOp=5, End(B=3→6)]: A's resolution stops the
    // drain at 4 because the NoOp at 5 is neither an awaited terminal nor a retainable Start.
    // Switching to live then clamps the head to 6 without committing past B's End, so it is not
    // attached to the retained Start. The release must still recognise B as closed history by
    // reading the recorded prefix rather than rejecting it as unclosed.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_now(),
        end_for(2, 42),
        noop(),
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
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — after B (idx 3) is retained with
    // its End (idx 5) attached, shrinking the target to 4 must detach the now hidden End while
    // keeping the visible Start; shrinking to 2 must drop the hidden Start altogether.
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
    // [NoOp, Start(E=2), Start(B=3 ← 2), End(B=3→4)] — after E is claimed the drain retains the
    // body's unscoped call B together with its End. Physically the cursor is at 4, but the body
    // has not consumed B yet: its `begin_function` must still observe 2, the oplog tip the live
    // run saw before appending Start(B), so that a key derived from that position is stable
    // across the restart. Claiming B publishes the withheld position.
    let rs = replay_state_over(vec![
        noop(),
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
    // [NoOp, Start(E=2), Start(B=3 ← 2)] — the incomplete tail variant: retaining B keeps the
    // position at 2; the owner's claim publishes 3 and then finds the call incomplete.
    let rs = replay_state_over(vec![noop(), start_now(), owned_start_now(2)]).await;
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
async fn replay_jump_prunes_retained_starts_of_the_abandoned_attempt() {
    // [NoOp, Start(E=2), Start(A=3 ← 2), End(A=3→4), Start(scope=5 ← 2), Start(C=6 ← 5)] — the
    // entity body E replays its direct call A, and the drain after End(A) retains the incomplete
    // batched scope 5 and its child 6. The body then re-issues the scope, adopts retained 5,
    // finds it incomplete and recovers by switching live and appending a Jump over 6..=7 (the
    // Jump's own index). Registering that Jump must drop retained 6: it belongs to the abandoned
    // first attempt and no live re-execution may claim it or be blamed for leaving it behind.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        owned_start_now(2),
        end_for(3, 42),
        nested_batched_scope_start(2),
        batched_child_start(5),
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
    let direct = rs
        .claim_owned_concurrent_start(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
            OplogIndex::from_u64(2),
        )
        .await
        .unwrap();
    assert_eq!(direct.start_idx(), OplogIndex::from_u64(3));
    match rs.await_resolution(direct).await.unwrap() {
        Resolution::Completed { end_idx, .. } => assert_eq!(end_idx, OplogIndex::from_u64(4)),
        other => panic!("expected Completed, got {other:?}"),
    }
    rs.drain_awaited_terminals().await.unwrap();
    {
        let internal = rs.cursor.state.lock().await;
        assert!(
            internal
                .retained_starts
                .contains_key(&OplogIndex::from_u64(5))
        );
        assert!(
            internal
                .retained_starts
                .contains_key(&OplogIndex::from_u64(6))
        );
    }

    let scope_name = HostFunctionName::Custom("<scope:batched-write:req>".to_string());
    let (scope_idx, scope_handle) = rs
        .claim_scope_start(
            &scope_name,
            &DurableFunctionType::WriteRemoteBatched(None),
            Some(OplogIndex::from_u64(2)),
        )
        .await
        .unwrap();
    assert_eq!(scope_idx, OplogIndex::from_u64(5));
    assert!(matches!(
        rs.await_resolution_outcome(scope_handle).await.unwrap(),
        ResolutionOutcome::Incomplete
    ));

    rs.switch_cursor_to_live().await.unwrap();
    assert!(
        rs.has_unclaimed_retained_starts(),
        "switching live alone keeps the abandoned child retained"
    );
    rs.register_replay_jump(vec![OplogRegion::from_range(6..=7)])
        .await
        .unwrap();

    assert!(
        rs.is_in_skipped_region(OplogIndex::from_u64(6))
            .await
            .unwrap()
    );
    assert!(
        !rs.has_unclaimed_retained_starts(),
        "the child of the abandoned attempt must not force later calls onto the claim path"
    );
    assert_eq!(
        rs.unclaimed_retained_descendant(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        None,
        "the body must not be blamed for a descendant hidden by its own recovery Jump"
    );
    let _ = entity;
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
    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
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
async fn await_resolves_across_unclaimed_sibling_start() {
    // [NoOp, Start(A=2), Start(B=3), End(A=2→4), End(B=3→5)] — A is claimed and awaited *before*
    // B is claimed. The awaiter must not suspend on the still-unclaimed Start(B): B's owner may be
    // a task that needs the Store the awaiter holds, so parking would deadlock. Instead the
    // awaiter retains B for its owner, drains its own End and resolves without any other task
    // touching the cursor.
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
    // [NoOp, BeginAtomicRegion(2)] — replay_target = 2. A speculative read of the last entry must
    // NOT make the cursor observably reach the replay target while the read is still
    // rollbackable: the predicate (run after the read) must still see `is_live() == false`. This
    // is the regression guard for "publish only committed cursor state" — a transient live cursor
    // would let a concurrent awaiter falsely conclude end-of-replay.
    let rs = replay_state_over(vec![noop(), begin_atomic_region()]).await;
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
    // [NoOp, Start(A=2)] — replay_target = 2. A predicate probe that reaches an unclaimed `Start`
    // evaluates its predicate against the not-yet-advanced cursor, then commits past the `Start`
    // and retains it: the cursor is live for positional purposes, but the retained `Start` keeps
    // the durable-call view of the Store in replay until its owner claims it (and finds it
    // incomplete at the tail).
    let rs = replay_state_over(vec![noop(), start_now()]).await;

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

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
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
    // A is claimed and awaited first, but its End sits last; in between are a durable scope (S,
    // claimed by its scope owner) and a fully overlapping sibling call B. This proves: A's awaiter
    // retains the scope Start and B's Start (never consuming or parking on them), attaches their
    // Ends, and resolves; the scope owner and B then adopt their retained Starts with the attached
    // terminals, in any order.
    let rs = replay_state_over(vec![
        noop(),
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
            &DurableFunctionType::WriteRemoteBatched(None),
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
    // [NoOp, Start(A=2), Start(B=3)] — A is claimed and awaited but B is never claimed. The
    // awaiter retains B and reaches the replay tail without A's End: it resolves Incomplete
    // (rather than sleeping on the unclaimed Start(B) until switch_to_live) and drops its
    // registration; B stays retained for a late owner.
    let rs = replay_state_over(vec![noop(), start_now(), start_now()]).await;
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
async fn completed_entity_body_uses_explicit_owner_for_anchored_noop() {
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_with_parent(2),
        anchored_noop(3),
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
        "the active nested entity body can still consume its explicitly anchored NoOp"
    );
    assert_eq!(
        rs.unconsumed_scope_head(OplogIndex::from_u64(2), HashSet::new())
            .await
            .unwrap(),
        Some(OplogIndex::from_u64(4)),
        "once the owner settles without a live resolver, the anchored NoOp is divergence"
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
    let begin = begin_atomic_region();
    assert!(matches!(begin, OplogEntry::BeginAtomicRegion { .. }));
    let error = anchored_error(2, 3);

    assert_eq!(
        cursor::scope_entry_owner(
            OplogIndex::from_u64(4),
            &error,
            Some(OplogIndex::from_u64(3)),
            None,
        ),
        Some(OplogIndex::from_u64(2)),
        "the explicit entity owner must take precedence over retry_from"
    );
}

#[test]
async fn deferred_anchored_error_is_not_structural_divergence() {
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        begin_atomic_region(),
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
    assert!(matches!(begin, OplogEntry::BeginAtomicRegion { .. }));
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
        noop(),
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

#[test]
async fn entity_atomic_rollback_projects_only_owned_interleaved_regions() {
    let mut begin = begin_atomic_region();
    let OplogEntry::BeginAtomicRegion {
        entity_parent_start_index,
        ..
    } = &mut begin
    else {
        unreachable!()
    };
    *entity_parent_start_index = Some(OplogIndex::from_u64(2));

    let replay_state = replay_state_over(vec![
        noop(),               // 1
        start_now(),          // 2: entity root
        start_with_parent(2), // 3: retained completed entity call
        start_with_parent(2), // 4: pre-atomic call whose terminal is rolled back
        end_for(3, 30),       // 5: retained entity terminal
        begin,                // 6
        start_with_parent(2), // 7: atomic entity child
        start_now(),          // 8: sibling
        end_for(8, 80),       // 9: sibling terminal
        end_for(4, 40),       // 10: owned terminal for a pre-atomic Start
        delivered_for(8),     // 11: sibling observation boundary
        end_for(7, 70),       // 12: atomic entity child terminal
        delivered_for(7),     // 13: atomic entity observation boundary
        end_for(2, 20),       // 14: entity invocation terminal, retained
    ])
    .await;

    let regions = replay_state
        .entity_atomic_rollback_regions(OplogIndex::from_u64(2))
        .await;

    assert_eq!(
        regions,
        vec![
            OplogRegion::from_range(7..=7),
            OplogRegion::from_range(10..=10),
            OplogRegion::from_range(12..=13),
        ]
    );
    assert!(
        regions
            .iter()
            .all(|region| !region.contains(OplogIndex::from_u64(9))
                && !region.contains(OplogIndex::from_u64(11))
                && !region.contains(OplogIndex::from_u64(14))),
        "sibling completion gates and the entity invocation terminal must survive"
    );
}

#[test]
async fn entity_atomic_rollback_skips_newly_deleted_cursor_head() {
    for consumed in [1, 2] {
        let rs = replay_state_over(vec![noop(), noop(), start_now(), noop()]).await;
        if consumed == 2 {
            assert_eq!(
                rs.get_oplog_entry(None).await.unwrap().0,
                OplogIndex::from_u64(2)
            );
        }
        rs.register_replay_jump(vec![OplogRegion::from_range(2..=3)])
            .await
            .unwrap();
        assert_eq!(
            rs.get_oplog_entry(None).await.unwrap().0,
            OplogIndex::from_u64(4),
            "registration must skip a deleted region starting at or containing the next cursor position"
        );
    }
}

#[test]
async fn entity_atomic_rollback_recovers_descendants_after_partial_jump_commit() {
    let oplog = Arc::new(InMemoryOplog::new());
    for entry in [
        noop(),      // 1
        start_now(), // 2: entity root
        OplogEntry::BeginAtomicRegion {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: Some(OplogIndex::from_u64(2)),
        }, // 3
        start_with_parent(2), // 4: parent deleted by the first committed Jump
        start_now(), // 5: foreign sibling
        start_with_parent(4), // 6: descendant still needs rollback
        end_for(6, 61), // 7
        delivered_for(6), // 8
        end_for(5, 53), // 9: foreign completion
        OplogEntry::jump(
            Some(OplogIndex::from_u64(2)),
            OplogRegion::from_range(4..=4),
        ),
    ] {
        oplog.add(entry).await;
    }
    let rs = test_replay_state(
        test_agent_id(),
        oplog,
        DeletedRegions::from_regions([OplogRegion::from_range(4..=4)]),
        None,
    )
    .await
    .unwrap();
    let regions = rs
        .entity_atomic_rollback_regions(OplogIndex::from_u64(2))
        .await;
    assert!(
        regions
            .iter()
            .any(|region| region.contains(OplogIndex::from_u64(6)))
    );
    assert!(
        regions
            .iter()
            .any(|region| region.contains(OplogIndex::from_u64(8)))
    );
    assert!(
        regions
            .iter()
            .all(|region| !region.contains(OplogIndex::from_u64(9)))
    );
    assert_eq!(regions, vec![OplogRegion::from_range(6..=8)]);
    rs.register_replay_jump(regions).await.unwrap();
    assert!(
        rs.entity_atomic_rollback_regions(OplogIndex::from_u64(2))
            .await
            .is_empty(),
        "a subsequent restart must not roll back the prior Jump"
    );
}

#[test]
#[test_r::timeout("10s")]
async fn entity_atomic_rollback_masks_pre_begin_completions_before_claiming() {
    for end_before_begin in [true, false] {
        let begin = OplogEntry::BeginAtomicRegion {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: Some(OplogIndex::from_u64(2)),
        };
        let mut entries = vec![noop(), start_now(), start_with_parent(2)];
        if end_before_begin {
            entries.extend([end_for(3, 37), begin]);
        } else {
            entries.extend([begin, end_for(3, 37)]);
        }
        entries.extend([delivered_for(3), noop()]);
        let rs = replay_state_over(entries).await;
        let regions = rs
            .entity_atomic_rollback_regions(OplogIndex::from_u64(2))
            .await;
        assert_eq!(
            regions,
            vec![OplogRegion::from_range(if end_before_begin {
                6..=6
            } else {
                5..=6
            })]
        );
        rs.register_replay_jump(regions).await.unwrap();
        let _entity = rs
            .claim_start_or_replay_end(StartClaim::unowned(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
            ))
            .await
            .unwrap(); // entity Start
        let ReplayStartClaimOutcome::Claimed { handle, .. } = rs
            .claim_start_or_replay_end(StartClaim::owned(
                &HostFunctionName::MonotonicClockNow,
                &DurableFunctionType::ReadLocal,
                OplogIndex::from_u64(2),
            ))
            .await
            .unwrap()
        else {
            panic!("pre-B Start must survive");
        };
        if end_before_begin {
            assert!(matches!(
                rs.await_resolution_outcome(handle).await.unwrap(),
                ResolutionOutcome::Resolved(Resolution::Completed {
                    delivery_marker: None,
                    ..
                })
            ));
        } else {
            let mut resolution = Box::pin(rs.await_resolution_outcome(handle));
            assert!(
                tokio::time::timeout(Duration::from_millis(20), resolution.as_mut())
                    .await
                    .is_err()
            );
            assert!(matches!(
                rs.get_oplog_entry(Some(OplogIndex::from_u64(2)))
                    .await
                    .unwrap()
                    .1,
                OplogEntry::BeginAtomicRegion { .. }
            ));
            rs.get_oplog_entry(None).await.unwrap(); // surviving foreign tail
            assert!(matches!(
                resolution.await.unwrap(),
                ResolutionOutcome::Incomplete
            ));
        }
    }
}

#[test]
#[test_r::timeout("10s")]
async fn entity_atomic_rollback_deleted_claim_waits_for_retained_begin() {
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        OplogEntry::BeginAtomicRegion {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: Some(OplogIndex::from_u64(2)),
        },
        start_with_parent(2),
        noop(),
    ])
    .await;
    let regions = rs
        .entity_atomic_rollback_regions(OplogIndex::from_u64(2))
        .await;
    rs.register_replay_jump(regions).await.unwrap();
    let _entity = rs
        .claim_start_or_replay_end(StartClaim::unowned(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ))
        .await
        .unwrap(); // entity Start
    let mut claim = Box::pin(rs.claim_start_or_replay_end(StartClaim::owned(
        &HostFunctionName::MonotonicClockNow,
        &DurableFunctionType::ReadLocal,
        OplogIndex::from_u64(2),
    )));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), claim.as_mut())
            .await
            .is_err(),
        "autonomous host subtasks must not flip entity liveness before Begin"
    );
    assert!(matches!(
        rs.get_oplog_entry(Some(OplogIndex::from_u64(2)))
            .await
            .unwrap()
            .1,
        OplogEntry::BeginAtomicRegion { .. }
    ));
    assert!(matches!(
        claim.await.unwrap(),
        ReplayStartClaimOutcome::DeletedRegion
    ));
    assert_eq!(
        rs.get_oplog_entry(None).await.unwrap().0,
        OplogIndex::from_u64(5)
    );
}

#[test]
#[test_r::timeout("10s")]
async fn entity_atomic_rollback_deleted_claim_uses_latest_matching_start() {
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        start_with_parent(2),
        OplogEntry::BeginAtomicRegion {
            timestamp: Timestamp::now_utc(),
            entity_parent_start_index: Some(OplogIndex::from_u64(2)),
        },
        start_with_parent(2),
        noop(),
    ])
    .await;
    rs.register_replay_jump(vec![
        OplogRegion::from_range(3..=3),
        OplogRegion::from_range(5..=5),
    ])
    .await
    .unwrap();
    let _entity = rs
        .claim_start_or_replay_end(StartClaim::unowned(
            &HostFunctionName::MonotonicClockNow,
            &DurableFunctionType::ReadLocal,
        ))
        .await
        .unwrap(); // entity Start
    let mut claim = Box::pin(rs.claim_start_or_replay_end(StartClaim::owned(
        &HostFunctionName::MonotonicClockNow,
        &DurableFunctionType::ReadLocal,
        OplogIndex::from_u64(2),
    )));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), claim.as_mut())
            .await
            .is_err(),
        "the deleted Start behind the cursor must not hide the matching Start after Begin"
    );
    assert!(matches!(
        rs.get_oplog_entry(Some(OplogIndex::from_u64(2)))
            .await
            .unwrap()
            .1,
        OplogEntry::BeginAtomicRegion { .. }
    ));
    assert!(matches!(
        claim.await.unwrap(),
        ReplayStartClaimOutcome::DeletedRegion
    ));
    assert_eq!(
        rs.get_oplog_entry(None).await.unwrap().0,
        OplogIndex::from_u64(6)
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
        entity_parent_start_index: None,
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
/// concurrent calls (completed / cancelled / incomplete) with durable **scopes** (request-less
/// `Start`/`End` pairs claimed by their scope owner and awaited like calls) and non-hint
/// positional **markers** (atomic-region boundaries and rdbms-transaction internal markers), all
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
                            &DurableFunctionType::WriteRemoteBatched(None),
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

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
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

/// A discriminated batched-write scope `Start` recorded by an entity body at `parent`.
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
    *function_name = HostFunctionName::Custom("<scope:batched-write:req>".to_string());
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
    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
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

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
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
async fn missing_scope_recovery_settles_then_switches_live_over_benign_suffix() {
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

    let ScopeStartClaimOutcome::MissingSettling { replay_target } = outcome else {
        panic!("missing scope did not enter replay settlement");
    };
    assert!(rs.test_is_settling());
    assert!(!rs.is_live());
    assert!(matches!(
        rs.finish_settling_to_live(
            &replay_linear_memory(),
            ReplayToLiveRole::PrimaryAgent,
            replay_target,
        )
        .await
        .unwrap(),
        ReplayToLiveOutcome::Live { .. }
    ));
    assert!(rs.is_live());
    rs.publish_primary_live(replay_target)
        .await
        .expect("settled missing-scope recovery must publish live");
    assert!(rs.is_live_published());
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

fn entity_begin_atomic_region(entity: u64) -> OplogEntry {
    OplogEntry::BeginAtomicRegion {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: Some(OplogIndex::from_u64(entity)),
    }
}

fn entity_end_atomic_region(entity: u64, begin_index: u64) -> OplogEntry {
    OplogEntry::EndAtomicRegion {
        timestamp: Timestamp::now_utc(),
        entity_parent_start_index: Some(OplogIndex::from_u64(entity)),
        begin_index: OplogIndex::from_u64(begin_index),
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
    // An entity body (root Start 2) and its owner both open and close an atomic region while
    // sharing the cursor:
    //   [NoOp(1), Start(entity=2), BeginAtomicRegion(3, entity 2), Start(4, parent 2), End(4→5),
    //    CompletionDelivered(4→6), BeginAtomicRegion(7), EndAtomicRegion(8, entity 2, begin 3),
    //    EndAtomicRegion(9, begin 7), End(2→10)]
    // The owner asks for its `BeginAtomicRegion` first. It must not take the entity's marker at 3
    // (which would leave the entity reader to retain its own Start(4) and park forever at that
    // call's delivery marker): it parks until the entity consumed 3, retains 4 for the entity,
    // parks at 6 until the entity delivers 4, and only then receives 7. The same holds in the
    // other direction for the entity's `EndAtomicRegion` read behind the owner's marker at 7.
    let parent = OplogIndex::from_u64(1);
    let (entity_start, identity) = rejected_tool_reconstruction_start(parent);
    let rs = replay_state_over(vec![
        noop(),
        entity_start,
        entity_begin_atomic_region(2),
        start_with_parent(2),
        end_for(4, 44),
        delivered_for(4),
        begin_atomic_region(),
        entity_end_atomic_region(2, 3),
        end_atomic_region(7),
        end_for(2, 1),
    ])
    .await;
    let mut entity_handle = claim_rejected_tool_reconstruction(&rs, parent, &identity).await;
    let mut reconstruction = entity_handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");

    let mut owner_begin = Box::pin(rs.get_oplog_entry(None));
    assert_pending(&mut owner_begin, "the owner's BeginAtomicRegion read").await;
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
    assert!(matches!(entry, OplogEntry::BeginAtomicRegion { .. }));

    assert_pending(&mut owner_begin, "the owner's BeginAtomicRegion read").await;
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
    assert_pending(&mut entity_end, "the entity's EndAtomicRegion read").await;

    let (idx, entry) = owner_begin.await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(7));
    assert!(matches!(
        entry,
        OplogEntry::BeginAtomicRegion {
            entity_parent_start_index: None,
            ..
        }
    ));

    let (idx, entry) = entity_end.await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(8));
    assert!(matches!(
        entry,
        OplogEntry::EndAtomicRegion { begin_index, .. } if begin_index == OplogIndex::from_u64(3)
    ));

    let (idx, entry) = rs.get_oplog_entry(None).await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(9));
    assert!(matches!(
        entry,
        OplogEntry::EndAtomicRegion { begin_index, .. } if begin_index == OplogIndex::from_u64(7)
    ));

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
    // [NoOp(1), Start(2), End(2→3), BeginAtomicRegion(4, entity 2), NoOp(5)] — the entry at 4
    // claims to belong to an entity body rooted at 2, but 2 was claimed and resolved as an
    // ordinary call: no reconstruction will ever consume 4. Parking would hang replay, so the
    // owner's positional reader reports the divergence instead.
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        end_for(2, 42),
        entity_begin_atomic_region(2),
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
    // [NoOp(1), Start(2), Start(entity=3), NoOp(4, entity 3), End(2→5), End(3→6), NoOp(7)] —
    // call 2's awaiter drained past the unclaimed entity Start(3) and retained it. The owner's
    // next positional read reaches the body entry at 4 while 3 is still unclaimed: the owner is
    // going to claim 3, so the reader waits rather than failing, and receives 7 once the entity
    // body consumed 4.
    let parent = OplogIndex::from_u64(1);
    let (entity_start, identity) = rejected_tool_reconstruction_start(parent);
    let rs = replay_state_over(vec![
        noop(),
        start_now(),
        entity_start,
        anchored_noop(3),
        end_for(2, 42),
        end_for(3, 1),
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
    let mut resolution = Box::pin(rs.await_resolution(handle));
    assert_pending(&mut resolution, "call 2's awaiter").await;
    assert!(rs.has_unclaimed_retained_starts());
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));

    let mut owner_read = Box::pin(rs.get_oplog_entry(None));
    assert_pending(&mut owner_read, "the owner's positional read").await;
    assert_eq!(rs.last_replayed_index(), OplogIndex::from_u64(3));

    let mut entity_handle = claim_rejected_tool_reconstruction(&rs, parent, &identity).await;
    let mut reconstruction = entity_handle
        .take_historical_reconstruction()
        .expect("reconstruction guard");
    let (idx, entry) = rs
        .get_oplog_entry(Some(OplogIndex::from_u64(3)))
        .await
        .unwrap();
    assert_eq!(idx, OplogIndex::from_u64(4));
    assert!(matches!(entry, OplogEntry::NoOp { .. }));

    let (idx, entry) = owner_read.await.unwrap();
    assert_eq!(idx, OplogIndex::from_u64(7));
    assert!(matches!(
        entry,
        OplogEntry::NoOp {
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
async fn invocation_boundary_rejects_unconsumed_entity_body_entry() {
    // [NoOp(1), Start(entity=2), End(2→3), BeginAtomicRegion(4, entity 2),
    //  AgentInvocationFinished(5)] — the entity body settled without consuming its own marker at
    // 4. Every entity body of the invocation has terminated by the time the boundary reader runs,
    // so nobody can consume 4 anymore: the walk must fail instead of parking at it.
    let parent = OplogIndex::from_u64(1);
    let (entity_start, identity) = rejected_tool_reconstruction_start(parent);
    let rs = replay_state_over(vec![
        noop(),
        entity_start,
        end_for(2, 1),
        entity_begin_atomic_region(2),
        invocation_finished(),
    ])
    .await;
    let mut handle = claim_rejected_tool_reconstruction(&rs, parent, &identity).await;
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
