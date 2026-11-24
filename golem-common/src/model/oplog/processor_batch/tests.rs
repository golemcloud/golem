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

use crate::base_model::caller::test_support::{account_principal, sample_caller, user_principal};
use crate::base_model::caller::{Caller, Claims, Principal, UserPrincipal};
use crate::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
use crate::base_model::tool::ToolName;
use crate::model::agent::{AgentMode, OwnerKind};
use crate::model::card::{InvocationWalletPin, WalletVersionToken};
use crate::model::component::ComponentRevision;
use crate::model::entity::{
    AgentEntity, EntityCallMode, EntityInvocationDescriptor, EntityInvocationPlanReference,
    EntityInvocationRequest, ToolInvocationDescriptor, ToolOutputContract,
};
use crate::model::environment::EnvironmentId;
use crate::model::invocation_context::{InvocationContextStack, TraceId};
use crate::model::oplog::host_functions::HostFunctionName;
use crate::model::oplog::payload::types::AgentMetadataForGuests;
use crate::model::oplog::{
    CreateParameters, CreateRequest, CreateRequestKind, DurableFunctionType, HostRequest,
    HostRequestEntityInvocation, HostStreamKind, OplogEntry, OplogPayload, PayloadId,
    UpdateDescription,
};
use crate::model::{
    AccountId, AgentFingerprint, AgentId, AgentStatus, ComponentId, InvocationId,
    InvocationRequestOperation, OplogIndex, StoredAgentInvocationPayload, Timestamp,
};
use crate::schema::graph::{SchemaGraph, TypedSchemaValue};
use crate::schema::schema_type::SchemaType;
use crate::schema::schema_value::SchemaValue;
use prost::Message;
use std::collections::BTreeMap;
use std::sync::Arc;
use test_r::test;
use uuid::Uuid;

/// Text that only the claims of `user_principal()` hold: its email.
const CLAIM_MARKER: &[u8] = b"a@b.example";

fn holds_marker(bytes: &[u8]) -> bool {
    bytes
        .windows(CLAIM_MARKER.len())
        .any(|window| window == CLAIM_MARKER)
}

/// Whether the entry holds the user's claims anywhere, at any depth: in its stored encoding, which
/// the processor's own oplog keeps, or in the gRPC form that the remote path sends.
fn holds_claims(entry: &OplogEntry) -> bool {
    let stored = crate::serialization::serialize(entry).unwrap();
    let proto: golem_api_grpc::proto::golem::worker::RawOplogEntry =
        entry.clone().try_into().unwrap();
    holds_marker(&stored) || holds_marker(&proto.encode_to_vec())
}

/// The user of `user_principal()`, with the empty claim set.
fn user_without_claims() -> Principal {
    let Principal::User(user) = user_principal() else {
        unreachable!("user_principal() is a user")
    };
    Principal::User(UserPrincipal {
        pair: user.pair,
        claims: Claims::from_json("{}".to_string()).unwrap(),
        authenticated_at: user.authenticated_at,
    })
}

/// `sample_caller()` as a processor must see it: the same roles and tokens, and the user without
/// claims.
fn sample_caller_without_claims() -> Caller {
    let caller = sample_caller();
    Caller {
        original: user_without_claims(),
        immediate: caller.immediate,
        tokens: caller.tokens,
    }
}

fn assert_same_record(actual: &Caller, expected: &Caller) {
    assert!(
        actual.original.same_record(&expected.original)
            && actual.immediate.same_record(&expected.immediate)
            && actual.tokens == expected.tokens,
        "expected {expected:?}, got {actual:?}"
    );
}

fn method_payload(caller: Caller) -> StoredAgentInvocationPayload {
    StoredAgentInvocationPayload::AgentMethod {
        input: SchemaValue::Record { fields: Vec::new() }
            .try_into()
            .unwrap(),
        allow_streams: false,
        caller,
        scope_card: None,
    }
}

fn guest_metadata() -> AgentMetadataForGuests {
    AgentMetadataForGuests {
        agent_id: AgentId::from_parts(7, 2),
        agent_type_name: None,
        fingerprint: AgentFingerprint(Uuid::nil()),
        args: Vec::new(),
        env: Vec::new(),
        config: BTreeMap::new(),
        status: AgentStatus::Running,
        component_revision: ComponentRevision::INITIAL,
        retry_count: 0,
        environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
    }
}

fn inline<T>(value: T) -> OplogPayload<T>
where
    T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq,
{
    OplogPayload::Inline(Box::new(value))
}

fn serialized_inline<T>(value: T) -> OplogPayload<T>
where
    T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq,
{
    OplogPayload::SerializedInline {
        bytes: crate::serialization::serialize(&value).unwrap(),
        cached: None,
    }
}

fn started(payload: OplogPayload<StoredAgentInvocationPayload>) -> OplogEntry {
    OplogEntry::AgentInvocationStarted {
        timestamp: Timestamp::now_utc().rounded(),
        invocation_id: Uuid::from_u128(0x1).into(),
        operation: Box::new(InvocationRequestOperation::AgentMethod {
            method_name: "report".to_string(),
        }),
        payload,
        trace_id: TraceId::generate(),
        trace_states: Vec::new(),
        invocation_context: Vec::new(),
        wallet_pin: Box::new(InvocationWalletPin {
            wallet_token: WalletVersionToken {
                wallet_id_hash: [0x42; 32],
                generation: 1,
            },
            pinned_card_ids: Vec::new(),
            scope_card_id: None,
        }),
    }
}

fn pending(payload: OplogPayload<StoredAgentInvocationPayload>) -> OplogEntry {
    OplogEntry::PendingAgentInvocation {
        timestamp: Timestamp::now_utc().rounded(),
        not_before: None,
        invocation_id: Uuid::from_u128(0x2).into(),
        operation: Box::new(InvocationRequestOperation::AgentMethod {
            method_name: "report".to_string(),
        }),
        payload,
        trace_id: TraceId::generate(),
        trace_states: Vec::new(),
        invocation_context: Vec::new(),
    }
}

/// The request of a tool call that the agent makes for `caller`, as an entity `Start` records it.
fn tool_call_request(caller: Caller) -> HostRequest {
    let metadata = EntityInvocationRequest {
        entity: AgentEntity::Tool(ToolName::try_from("grep").unwrap()),
        call_mode: EntityCallMode::Synchronous,
        operation: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
            attempt_ordinal: 1,
            command_path: vec!["grep".to_string()],
            args: Vec::new(),
            has_stdin: false,
            has_stdout: true,
            declares_stdout: true,
            has_stderr: false,
            declares_stderr: false,
            output_contract: ToolOutputContract {
                result: None,
                errors: Vec::new(),
            },
        }),
        caller,
        plan: EntityInvocationPlanReference::Descendant {
            root_start_index: OplogIndex::from_u64(3),
            position: 0,
        },
        authority_wallet: Vec::new(),
    };
    HostRequest::EntityInvocation(HostRequestEntityInvocation {
        metadata: desert_rust::serialize_to_byte_vec(&metadata).unwrap(),
        input: TypedSchemaValue::new(
            SchemaGraph::anonymous(SchemaType::tuple(Vec::new())),
            SchemaValue::Tuple {
                elements: Vec::new(),
            },
        ),
        stream_session_idempotency_key: InvocationId(Uuid::from_u128(0x3)),
    })
}

fn tool_call_start(request: OplogPayload<HostRequest>) -> OplogEntry {
    OplogEntry::Start {
        timestamp: Timestamp::now_utc().rounded(),
        parent_start_index: None,
        function_name: HostFunctionName::Custom("golem::entity::invoke".to_string()),
        invocation_id: None,
        observational_owner: None,
        driver: None,
        request: Some(request),
        durable_function_type: DurableFunctionType::WriteRemote,
        span_started: None,
        denial: None,
    }
}

fn create(caller: Caller) -> OplogEntry {
    OplogEntry::Create {
        timestamp: Timestamp::now_utc().rounded(),
        parameters: Box::new(CreateParameters {
            component_id: ComponentId(Uuid::from_u128(0xc0)),
            agent_type_name: None,
            canonical_key: None,
            constructor_parameters: None,
            agent_id: AgentId::from_parts(7, 4),
            owner_kind: OwnerKind::ComponentAgent,
            agent_mode: AgentMode::Durable,
            component_revision: ComponentRevision::INITIAL,
            env: Vec::new(),
            environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
            created_by: AccountId(Uuid::from_u128(0xacc0)),
            parent: None,
            component_size: 17,
            initial_total_linear_memory_size: 29,
            initial_active_plugins: Default::default(),
            local_agent_config: Vec::new(),
            instance_id: Uuid::from_u128(0x4),
            create_request: CreateRequest::new(
                CreateRequestKind::GetOrCreate,
                None,
                caller,
                &InvocationContextStack::fresh_rounded(),
            ),
            never_live: false,
        }),
    }
}

/// The stored payload of an invocation entry, decoded.
fn invocation_payload(entry: &OplogEntry) -> StoredAgentInvocationPayload {
    let (OplogEntry::AgentInvocationStarted { payload, .. }
    | OplogEntry::PendingAgentInvocation { payload, .. }) = entry
    else {
        panic!("expected an invocation entry, got {entry:?}")
    };
    inline_value(payload)
}

fn inline_value<T>(payload: &OplogPayload<T>) -> T
where
    T: desert_rust::BinaryCodec + std::fmt::Debug + Clone + PartialEq,
{
    match payload {
        OplogPayload::Inline(value) => (**value).clone(),
        OplogPayload::SerializedInline { bytes, .. } => {
            crate::serialization::deserialize(bytes).unwrap()
        }
        other => panic!("expected an inline payload, got {other:?}"),
    }
}

fn caller_of(payload: &StoredAgentInvocationPayload) -> &Caller {
    match payload {
        StoredAgentInvocationPayload::ManualUpdate { requester } => requester,
        StoredAgentInvocationPayload::AgentInitialization { caller, .. }
        | StoredAgentInvocationPayload::AgentMethod { caller, .. }
        | StoredAgentInvocationPayload::ExternalTool { caller, .. }
        | StoredAgentInvocationPayload::LoadSnapshot { caller, .. }
        | StoredAgentInvocationPayload::SaveSnapshot { caller }
        | StoredAgentInvocationPayload::ProcessOplogEntries { caller, .. } => caller,
    }
}

/// The caller inside the tool-call request of a `Start` or `HostStreamFrame` entry.
fn tool_call_caller(entry: &OplogEntry) -> Caller {
    let request = match entry {
        OplogEntry::Start {
            request: Some(request),
            ..
        } => request,
        OplogEntry::HostStreamFrame { payload, .. } => payload,
        other => panic!("expected an entry with a host request, got {other:?}"),
    };
    let HostRequest::EntityInvocation(request) = inline_value(request) else {
        panic!("expected a tool-call request")
    };
    desert_rust::deserialize::<EntityInvocationRequest>(&request.metadata)
        .unwrap()
        .caller
}

#[test]
fn an_invocation_reaches_a_processor_without_the_users_claims() {
    let entries = [
        started(inline(method_payload(sample_caller()))),
        started(serialized_inline(method_payload(sample_caller()))),
        pending(inline(method_payload(sample_caller()))),
        pending(serialized_inline(method_payload(sample_caller()))),
    ];
    for entry in entries {
        assert!(holds_claims(&entry), "the stored entry holds the claims");

        let received = entry.clone().for_oplog_processor().unwrap();

        assert!(!holds_claims(&received), "{received:?}");
        assert_same_record(
            caller_of(&invocation_payload(&received)),
            &sample_caller_without_claims(),
        );
        assert!(holds_claims(&entry), "the stored entry does not change");
    }
}

#[test]
fn every_kind_of_invocation_loses_the_users_claims() {
    let payloads = [
        StoredAgentInvocationPayload::ManualUpdate {
            requester: sample_caller(),
        },
        StoredAgentInvocationPayload::AgentInitialization {
            input: SchemaValue::Record { fields: Vec::new() },
            caller: sample_caller(),
        },
        method_payload(sample_caller()),
        StoredAgentInvocationPayload::SaveSnapshot {
            caller: sample_caller(),
        },
        StoredAgentInvocationPayload::ProcessOplogEntries {
            account_id: AccountId(Uuid::from_u128(0xacc0)),
            config: Vec::new(),
            metadata: guest_metadata(),
            first_entry_index: OplogIndex::INITIAL,
            entries: vec![started(serialized_inline(method_payload(sample_caller())))],
            caller: sample_caller(),
        },
    ];
    for payload in payloads {
        let entry = started(serialized_inline(payload));
        assert!(holds_claims(&entry));

        let received = entry.for_oplog_processor().unwrap();

        assert!(!holds_claims(&received), "{received:?}");
        assert_same_record(
            caller_of(&invocation_payload(&received)),
            &sample_caller_without_claims(),
        );
    }
}

#[test]
fn an_update_requester_reaches_a_processor_without_the_users_claims() {
    let entry = OplogEntry::PendingUpdate {
        timestamp: Timestamp::now_utc().rounded(),
        description: UpdateDescription::Automatic {
            target_revision: ComponentRevision::new(2).unwrap(),
        },
        update_attempt_index: Some(OplogIndex::from_u64(4)),
        requester: Box::new(sample_caller()),
    };
    assert!(holds_claims(&entry));

    let received = entry.for_oplog_processor().unwrap();

    assert!(!holds_claims(&received), "{received:?}");
    let OplogEntry::PendingUpdate { requester, .. } = &received else {
        panic!("expected a pending update, got {received:?}")
    };
    assert_same_record(requester, &sample_caller_without_claims());
}

#[test]
fn a_processor_never_receives_the_delivery_state_of_a_processor_installation() {
    let timestamp = Timestamp::now_utc().rounded();
    let checkpoint = |delivery| OplogEntry::OplogProcessorCheckpoint {
        timestamp,
        plugin_grant_id: EnvironmentPluginGrantId(Uuid::from_u128(0x9)),
        source_component_revision: ComponentRevision::INITIAL,
        target_agent_id: AgentId::from_parts(7, 5),
        confirmed_up_to: OplogIndex::from_u64(3),
        sending_up_to: OplogIndex::from_u64(5),
        last_batch_start: OplogIndex::from_u64(4),
        delivery,
    };
    // The delivery state holds the agent's metadata, and with it the caller that created the agent.
    let entry = checkpoint(Some(
        crate::serialization::serialize(&sample_caller()).unwrap(),
    ));
    assert!(holds_claims(&entry));

    let received = entry.for_oplog_processor().unwrap();

    assert_eq!(received, checkpoint(None));
}

#[test]
fn a_tool_call_request_reaches_a_processor_without_the_users_claims() {
    let entries = [
        tool_call_start(inline(tool_call_request(sample_caller()))),
        tool_call_start(serialized_inline(tool_call_request(sample_caller()))),
        OplogEntry::HostStreamFrame {
            timestamp: Timestamp::now_utc().rounded(),
            parent_start_index: OplogIndex::from_u64(3),
            kind: HostStreamKind::P3HttpRequestBody,
            payload: serialized_inline(tool_call_request(sample_caller())),
        },
    ];
    for entry in entries {
        assert!(holds_claims(&entry));

        let received = entry.for_oplog_processor().unwrap();

        assert!(!holds_claims(&received), "{received:?}");
        assert_same_record(
            &tool_call_caller(&received),
            &sample_caller_without_claims(),
        );
    }
}

#[test]
fn the_creator_reaches_a_processor_without_the_users_claims() {
    let entry = create(sample_caller());
    assert!(holds_claims(&entry));

    let received = entry.for_oplog_processor().unwrap();

    assert!(!holds_claims(&received), "{received:?}");
    let OplogEntry::Create { parameters, .. } = &received else {
        panic!("expected a create entry, got {received:?}")
    };
    assert_same_record(
        &parameters.create_request.caller,
        &sample_caller_without_claims(),
    );
}

#[test]
fn an_external_payload_keeps_its_reference() {
    let payload_id = PayloadId(Uuid::from_u128(0x5));
    let entry = started(OplogPayload::External {
        payload_id: payload_id.clone(),
        md5_hash: vec![1, 2, 3],
        cached: Some(Arc::new(method_payload(sample_caller()))),
    });

    let received = entry.for_oplog_processor().unwrap();

    let OplogEntry::AgentInvocationStarted {
        payload:
            OplogPayload::External {
                payload_id: received_id,
                md5_hash,
                cached,
            },
        ..
    } = &received
    else {
        panic!("expected an external payload, got {received:?}")
    };
    assert_eq!(received_id, &payload_id);
    assert_eq!(md5_hash, &vec![1, 2, 3]);
    assert!(cached.is_none(), "the in-memory copy with the claims goes");
}

#[test]
fn an_entry_without_claims_keeps_its_bytes() {
    let no_user = Caller::entry(account_principal());
    let user_without_claims = Caller::entry(user_without_claims());
    let entries = [
        started(serialized_inline(method_payload(no_user.clone()))),
        started(serialized_inline(method_payload(user_without_claims))),
        tool_call_start(serialized_inline(tool_call_request(no_user.clone()))),
        create(no_user),
        OplogEntry::GrowMemory {
            timestamp: Timestamp::now_utc().rounded(),
            delta: 100,
        },
    ];
    for entry in entries {
        let stored = crate::serialization::serialize(&entry).unwrap();

        let received = entry.for_oplog_processor().unwrap();

        assert_eq!(crate::serialization::serialize(&received).unwrap(), stored);
    }
}
