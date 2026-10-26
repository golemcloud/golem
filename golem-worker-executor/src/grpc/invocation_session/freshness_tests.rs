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

use super::{
    AcceptanceRace, TransportStreamId, build_durable_streaming_request,
    decode_invocation_freshness_disposition, decode_invocation_input, effective_session_identity,
    is_attachment_termination, pre_acceptance_rejection_reason, race_invocation_acceptance,
    require_expected_callee_fingerprint,
};
use crate::durable_host::durable_session::durable_stream_mapping_to_proto;
use crate::durable_host::stream_session::{
    decode_recursive_stream_value, decode_recursive_stream_value_with_schema,
    encode_recursive_stream_value_with_schema, remap_recursive_stream_references,
};
use futures::future;
use golem_api_grpc::proto::golem::account::PlanId;
use golem_api_grpc::proto::golem::auth::{AuthCtx, AuthEffectiveSurface, UserAuthCtx, auth_ctx};
use golem_api_grpc::proto::golem::common::AccountId;
use golem_api_grpc::proto::golem::worker::{InvocationRejectionReason, InvocationStart};
use golem_common::base_model::Empty;
use golem_common::base_model::agent::Snapshotting;
use golem_common::base_model::component::{ComponentId, ComponentRevision};
use golem_common::base_model::component_metadata::{ComponentMetadata, KnownExports};
use golem_common::base_model::durable_stream::{
    DURABLE_STREAM_FORMAT_VERSION, DurableStreamHandle, MAX_DURABLE_STREAM_ITEM_SIZE,
    SessionStreamRole, StreamId, StreamInvocationId, StreamMapSide, StreamSessionMappingRecord,
    StreamValuePathStep,
};
use golem_common::base_model::environment::EnvironmentId;
use golem_common::model::agent::{
    AgentMode, AgentTypeName, InvocationFreshnessDisposition, Principal,
};
use golem_common::model::invocation_context::InvocationContextStack;
use golem_common::model::{AgentFingerprint, AgentId, AgentInvocation, IdempotencyKey, OplogIndex};
use golem_common::schema::SchemaValue;
use golem_common::schema::agent::{
    AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField,
    OutputSchema,
};
use golem_schema::proto::golem::common::Uuid;
use golem_schema::proto::golem::schema::{
    BinaryValue, SchemaValue as ProtoSchemaValue, schema_value as proto_schema_value,
};
use golem_schema::schema::schema_value::{ResultValuePayload, UnionValuePayload};
use golem_schema::schema::{
    DiscriminatorRule, FieldDiscriminator, NamedFieldType, ResultSpec, SchemaFingerprintV1,
    SchemaGraph, SchemaType, SchemaTypeDef, SchemaValueStream, TypeId, UnionBranch, UnionSpec,
    VariantCaseType, schema_fingerprint_v1,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use prost::Message;
use std::collections::BTreeMap;
use std::task::Poll;
use test_r::test;

#[test]
async fn response_retention_survives_queued_frames_until_drain_or_drop() {
    use futures::StreamExt;
    for drain in [true, false] {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let retention = super::ResponseLease::default();
        let weak = std::sync::Arc::downgrade(&retention);
        let mut response = super::retained_response_stream(receiver, retention);
        sender.send(Default::default()).await.unwrap();
        drop(sender);
        assert!(weak.upgrade().is_some());
        if drain {
            assert!(response.next().await.is_some());
            assert!(weak.upgrade().is_some());
            assert!(response.next().await.is_none());
            assert!(weak.upgrade().is_none());
        }
        drop(response);
        assert!(weak.upgrade().is_none());
    }
}

#[test]
async fn closed_response_cancels_parked_work_before_detach() {
    for close_transport in [true, false] {
        let (outward, receiver) = tokio::sync::mpsc::channel(1);
        let stopped = tokio_util::sync::CancellationToken::new();
        let work_dropped = tokio_util::sync::CancellationToken::new();
        let guard = work_dropped.clone().drop_guard();
        let mut operation = Box::pin(super::until_response_closed(
            &outward,
            &stopped,
            async move {
                let _guard = guard;
                future::pending::<()>().await;
            },
        ));
        assert!(futures::poll!(operation.as_mut()).is_pending());
        if close_transport {
            drop(receiver);
        } else {
            stopped.cancel();
        }
        assert!(operation.await.is_none());
        assert!(work_dropped.is_cancelled());
    }
}

#[test]
fn invocation_input_decode_moves_binary_payload() {
    let bytes = vec![7; 1024];
    let bytes_ptr = bytes.as_ptr();
    let input = ProtoSchemaValue {
        value: Some(proto_schema_value::Value::BinaryValue(BinaryValue {
            bytes,
            mime_type: None,
        })),
    };

    let decoded = decode_invocation_input(input).unwrap();
    let SchemaValue::Binary(binary) = decoded else {
        panic!("expected binary input")
    };

    assert_eq!(binary.bytes.as_ptr(), bytes_ptr);
}

#[test]
fn one_pass_input_canonicalization_matches_two_pass_traversal() {
    fn field(name: &str, body: SchemaType) -> NamedFieldType {
        NamedFieldType {
            name: name.to_string(),
            body,
            metadata: Default::default(),
        }
    }

    let map_type_id = TypeId::new("stream-map");
    let union = SchemaType::union(UnionSpec {
        branches: vec![
            UnionBranch {
                tag: "plain".to_string(),
                body: SchemaType::record(vec![field("kind", SchemaType::string())]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("plain".to_string()),
                }),
                metadata: Default::default(),
            },
            UnionBranch {
                tag: "stream".to_string(),
                body: SchemaType::record(vec![
                    field("kind", SchemaType::string()),
                    field("values", SchemaType::stream(Some(SchemaType::u64()))),
                ]),
                discriminator: DiscriminatorRule::FieldEquals(FieldDiscriminator {
                    field_name: "kind".to_string(),
                    literal: Some("stream".to_string()),
                }),
                metadata: Default::default(),
            },
        ],
    });
    let root = SchemaType::record(vec![
        field("mapping", SchemaType::ref_to(map_type_id.clone())),
        field(
            "variant",
            SchemaType::variant(vec![
                VariantCaseType {
                    name: "none".to_string(),
                    payload: None,
                    metadata: Default::default(),
                },
                VariantCaseType {
                    name: "some".to_string(),
                    payload: Some(SchemaType::option(SchemaType::stream(Some(
                        SchemaType::string(),
                    )))),
                    metadata: Default::default(),
                },
            ]),
        ),
        field(
            "result",
            SchemaType::result(ResultSpec {
                ok: Some(Box::new(SchemaType::u64())),
                err: Some(Box::new(SchemaType::stream(Some(SchemaType::string())))),
            }),
        ),
        field("union", union),
    ]);
    let graph = SchemaGraph {
        defs: vec![SchemaTypeDef {
            id: map_type_id,
            name: None,
            body: SchemaType::map(
                SchemaType::stream(Some(SchemaType::u64())),
                SchemaType::stream(Some(SchemaType::string())),
            ),
        }],
        root: root.clone(),
    };
    let input = SchemaValue::Record {
        fields: vec![
            SchemaValue::Map {
                entries: vec![(
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(101_u64)),
                    SchemaValue::Stream(SchemaValueStream::from_host_endpoint(7_u64)),
                )],
            },
            SchemaValue::Variant(golem_schema::schema::schema_value::VariantValuePayload {
                case: 1,
                payload: Some(Box::new(SchemaValue::Option {
                    inner: Some(Box::new(SchemaValue::Stream(
                        SchemaValueStream::from_host_endpoint(55_u64),
                    ))),
                })),
            }),
            SchemaValue::Result(ResultValuePayload::Err {
                value: Some(Box::new(SchemaValue::Stream(
                    SchemaValueStream::from_host_endpoint(300_u64),
                ))),
            }),
            SchemaValue::Union(UnionValuePayload {
                tag: "stream".to_string(),
                body: Box::new(SchemaValue::Record {
                    fields: vec![
                        SchemaValue::String("stream".to_string()),
                        SchemaValue::Stream(SchemaValueStream::from_host_endpoint(2_u64)),
                    ],
                }),
            }),
        ],
    };
    let original = encode_recursive_stream_value_with_schema(&input, &graph, &root, |stream, _| {
        stream.with_host_endpoint::<u64, _>(|stream_id| *stream_id)
    })
    .unwrap();

    let mut old_observations = Vec::new();
    decode_recursive_stream_value_with_schema(
        original.clone(),
        &graph,
        &root,
        |stream_id, path| {
            old_observations.push((stream_id, path.to_vec()));
            Ok(SchemaValueStream::from_host_endpoint(()))
        },
    )
    .unwrap();
    let mut old_index = 0_u64;
    let old_canonical = remap_recursive_stream_references(original.clone(), |_, _| {
        let index = old_index;
        old_index += 1;
        Ok(index)
    })
    .unwrap();

    let decoded = decode_invocation_input(original).unwrap();
    let canonicalize = |decoded: &SchemaValue| {
        let mut observations = Vec::new();
        let mut index = 0_u64;
        let canonical =
            encode_recursive_stream_value_with_schema(decoded, &graph, &root, |stream, path| {
                let stream_id =
                    stream.with_host_endpoint::<TransportStreamId, _>(|stream_id| stream_id.0)?;
                observations.push((stream_id, path.to_vec()));
                let canonical = index;
                index += 1;
                Ok(canonical)
            })
            .unwrap();
        (canonical, observations)
    };
    let (new_canonical, new_observations) = canonicalize(&decoded);

    assert_eq!(new_canonical, old_canonical);
    assert_eq!(new_observations, old_observations);
    assert_eq!(
        new_observations,
        vec![
            (
                101,
                vec![
                    StreamValuePathStep::RecordField(0),
                    StreamValuePathStep::MapEntry {
                        index: 0,
                        side: StreamMapSide::Key,
                    },
                ],
            ),
            (
                7,
                vec![
                    StreamValuePathStep::RecordField(0),
                    StreamValuePathStep::MapEntry {
                        index: 0,
                        side: StreamMapSide::Value,
                    },
                ],
            ),
            (
                55,
                vec![
                    StreamValuePathStep::RecordField(1),
                    StreamValuePathStep::VariantCasePayload(1),
                    StreamValuePathStep::OptionSome,
                ],
            ),
            (
                300,
                vec![
                    StreamValuePathStep::RecordField(2),
                    StreamValuePathStep::ResultErr,
                ],
            ),
            (
                2,
                vec![
                    StreamValuePathStep::RecordField(3),
                    StreamValuePathStep::UnionBranch(1),
                    StreamValuePathStep::RecordField(1),
                ],
            ),
        ]
    );
    assert_eq!(canonicalize(&decoded).0, old_canonical);
}

fn builder_fixture() -> (InvocationStart, ComponentMetadata, AgentInvocation) {
    let mapping_type = SchemaType::map(
        SchemaType::stream(Some(SchemaType::u64())),
        SchemaType::stream(Some(SchemaType::string())),
    );
    let tail_type = SchemaType::stream(Some(SchemaType::bool()));
    let method = AgentMethodSchema {
        name: "run".to_string(),
        description: String::new(),
        prompt_hint: None,
        input_schema: InputSchema::Parameters(vec![
            NamedField::user_supplied("mapping", mapping_type),
            NamedField::user_supplied("tail", tail_type),
        ]),
        output_schema: OutputSchema::Unit,
        http_endpoint: Vec::new(),
        read_only: None,
    };
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        Vec::new(),
        vec![AgentTypeSchema {
            kind: golem_common::schema::agent::AgentTypeKind::Regular,
            type_name: AgentTypeName("test-agent".to_string()),
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(Vec::new()),
            },
            methods: vec![method],
            dependencies: Vec::new(),
            mode: AgentMode::Durable,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: Vec::new(),
        }],
        BTreeMap::new(),
    );
    let component_id = ComponentId(uuid::Uuid::from_u128(1));
    let environment_id = EnvironmentId(uuid::Uuid::from_u128(2));
    let callee = AgentId {
        component_id,
        agent_id: "test-agent()".to_string(),
    };
    let idempotency_key = IdempotencyKey::new("test-key".to_string());
    let fingerprint = AgentFingerprint(uuid::Uuid::from_u128(3));
    let request = InvocationStart {
        agent_id: Some(callee.into()),
        method_name: Some("run".to_string()),
        idempotency_key: Some(idempotency_key.clone().into()),
        environment_id: Some(environment_id.into()),
        attempt_id: Some(uuid::Uuid::new_v4().into()),
        expected_callee_fingerprint: Some(fingerprint.0.into()),
        ..Default::default()
    };
    let invocation = AgentInvocation::AgentMethod {
        idempotency_key,
        method_name: "run".to_string(),
        input: SchemaValue::Record {
            fields: vec![
                SchemaValue::Map {
                    entries: vec![(
                        SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                            TransportStreamId(101),
                        )),
                        SchemaValue::Stream(SchemaValueStream::from_host_endpoint(
                            TransportStreamId(7),
                        )),
                    )],
                },
                SchemaValue::Stream(SchemaValueStream::from_host_endpoint(TransportStreamId(55))),
            ],
        },
        invocation_context: InvocationContextStack::fresh(),
        principal: Principal::anonymous(),
        scope_card: None,
    };
    (request, metadata, invocation)
}

fn foreign_mapping(
    transport_stream_id: u64,
    element_schema_fingerprint: SchemaFingerprintV1,
    request: &InvocationStart,
) -> StreamSessionMappingRecord {
    let environment_id: EnvironmentId = request.environment_id.unwrap().try_into().unwrap();
    let callee: AgentId = request.agent_id.clone().unwrap().try_into().unwrap();
    let callee_fingerprint = AgentFingerprint(request.expected_callee_fingerprint.unwrap().into());
    let idempotency_key = request.idempotency_key.clone().unwrap().into();
    StreamSessionMappingRecord {
        transport_stream_id,
        handle: DurableStreamHandle {
            format_version: DURABLE_STREAM_FORMAT_VERSION,
            stream_id: StreamId(uuid::Uuid::from_u128(
                1_000 + u128::from(transport_stream_id),
            )),
            producer_environment_id: environment_id,
            producer: callee.clone(),
            expected_producer_fingerprint: callee_fingerprint,
            producer_generation: OplogIndex::NONE,
            source_invocation: StreamInvocationId {
                callee_environment_id: environment_id,
                callee,
                callee_fingerprint,
                idempotency_key,
            },
            component_revision: ComponentRevision::INITIAL,
            element_schema_fingerprint,
        },
        role: SessionStreamRole::Input,
    }
}

#[test]
fn durable_request_identity_ignores_tracing_but_preserves_execution_config() {
    let (mut request, metadata, invocation) = builder_fixture();
    request.context = Some(golem_api_grpc::proto::golem::worker::InvocationContext {
        parent: request.agent_id.clone(),
        env: (0..16)
            .map(|n| (format!("key-{n}"), format!("value-{n}")))
            .collect(),
        tracing: Some(InvocationContextStack::fresh().into()),
    });
    let descriptor = |request: &InvocationStart| {
        let (accepted, _) = tokio::sync::oneshot::channel();
        build_durable_streaming_request(
            request,
            &metadata,
            ComponentRevision::INITIAL,
            AgentFingerprint(uuid::Uuid::from_u128(3)),
            invocation.clone(),
            1,
            accepted,
            8,
        )
        .unwrap()
        .attempt
        .invocation
    };
    let original = descriptor(&request);
    for _ in 0..16 {
        let mut retry = request.clone();
        let context = retry.context.as_mut().unwrap();
        context.tracing = Some(InvocationContextStack::fresh().into());
        context.env = (0..16)
            .rev()
            .map(|n| (format!("key-{n}"), format!("value-{n}")))
            .collect();
        assert_eq!(descriptor(&retry), original);
    }
    assert!(request.context.as_ref().unwrap().tracing.is_some());
    for change in 0..5 {
        let mut changed = request.clone();
        match change {
            0 => changed.context.as_mut().unwrap().parent = None,
            1 => {
                changed
                    .context
                    .as_mut()
                    .unwrap()
                    .env
                    .insert("key-3".into(), "different".into());
            }
            2 => changed.mode = 1,
            3 => changed.config.push(Default::default()),
            4 => changed.scope_card = Some(Default::default()),
            _ => unreachable!(),
        }
        assert_ne!(descriptor(&changed), original, "execution field {change}");
    }
}

#[test]
fn durable_rpc_origin_is_persisted_and_requires_an_agent_caller() {
    let (mut request, metadata, invocation) = builder_fixture();
    let caller: AgentId = request.agent_id.clone().unwrap().try_into().unwrap();
    request.principal = Some(
        Principal::Agent(golem_common::model::agent::AgentPrincipal {
            agent_id: caller.clone(),
        })
        .into(),
    );
    request.context = Some(golem_api_grpc::proto::golem::worker::InvocationContext {
        parent: Some(caller.into()),
        ..Default::default()
    });
    let origin = golem_api_grpc::proto::golem::worker::StreamInvocationIdentity {
        callee_environment_id: request.environment_id,
        callee: request.agent_id.clone(),
        callee_fingerprint: Some(uuid::Uuid::from_u128(91).into()),
        idempotency_key: Some(IdempotencyKey::new("authoring-invocation".into()).into()),
    };
    request.origin_invocation = Some(origin.clone());
    let build = |request: &InvocationStart| {
        let (accepted, _) = tokio::sync::oneshot::channel();
        build_durable_streaming_request(
            request,
            &metadata,
            ComponentRevision::INITIAL,
            AgentFingerprint(uuid::Uuid::from_u128(3)),
            invocation.clone(),
            1,
            accepted,
            8,
        )
    };
    let persisted = build(&request).unwrap().attempt.invocation;
    let (bytes, _): (Vec<u8>, Option<Vec<(String, String)>>) =
        golem_common::serialization::deserialize(&persisted.execution_config).unwrap();
    assert_eq!(
        InvocationStart::decode(bytes.as_slice())
            .unwrap()
            .origin_invocation,
        Some(origin)
    );
    let mut other = request.clone();
    other.origin_invocation.as_mut().unwrap().idempotency_key =
        Some(IdempotencyKey::new("different-invocation".into()).into());
    assert_ne!(build(&other).unwrap().attempt.invocation, persisted);
    for mutation in 0..9 {
        let mut invalid = request.clone();
        match mutation {
            0 => invalid.principal = None,
            1 => invalid.principal = Some(Principal::anonymous().into()),
            2 => invalid.context.as_mut().unwrap().parent = None,
            3 => {
                invalid
                    .origin_invocation
                    .as_mut()
                    .unwrap()
                    .callee_environment_id = None
            }
            4 => invalid.origin_invocation.as_mut().unwrap().callee = None,
            5 => {
                invalid
                    .origin_invocation
                    .as_mut()
                    .unwrap()
                    .callee_fingerprint = None
            }
            6 => {
                invalid
                    .origin_invocation
                    .as_mut()
                    .unwrap()
                    .callee_fingerprint = Some(uuid::Uuid::nil().into())
            }
            7 => invalid.origin_invocation.as_mut().unwrap().idempotency_key = None,
            8 => invalid
                .context
                .as_mut()
                .unwrap()
                .parent
                .as_mut()
                .unwrap()
                .name
                .push_str("-different"),
            _ => unreachable!(),
        }
        assert!(
            build(&invalid).is_err(),
            "invalid origin mutation {mutation}"
        );
    }
}

#[test]
fn same_origin_stream_join_preserves_execution_and_authority_checks() {
    use crate::worker::same_origin_stream_invocation;
    use golem_common::model::card::{
        AgentPermissionMonomorphizationContext, CardId, PolymorphicCard, StoredCard,
        agent_delegation_surface_from_wallet, agent_effective_surface_from_wallet,
        default_agent_initial_permission_grants, recipient::RecipientPattern,
    };
    use golem_service_base::model::auth::AuthCtx;

    let authority_for = |name: &str| {
        let context = AgentPermissionMonomorphizationContext {
            account: golem_common::model::account::AccountEmail::new("owner@example.com"),
            application: "application".parse().unwrap(),
            environment: "environment".parse().unwrap(),
            component: golem_common::model::component::ComponentName("component".into()),
            agent_name: name.into(),
            owner: golem_common::model::card::recipient::RecipientOwnerContext::AgentType(
                golem_common::model::agent::AgentTypeName("Agent".into()),
            ),
        };
        let card = StoredCard::Polymorphic(PolymorphicCard {
            card_id: CardId(uuid::Uuid::from_u128(100)),
            parent_ids: vec![],
            lower_positive: default_agent_initial_permission_grants(RecipientPattern::Any),
            lower_negative: vec![],
            upper_positive: vec![],
            upper_negative: vec![],
            created_at: chrono::Utc::now(),
            expires_at: None,
            system_card: true,
        });
        AuthCtx::agent_with_permission_surfaces(
            golem_common::model::account::AccountId(uuid::Uuid::from_u128(101)),
            context.account.clone(),
            agent_effective_surface_from_wallet(&context, [&card]),
            agent_delegation_surface_from_wallet(&context, [&card]),
        )
    };
    let (mut request, metadata, invocation) = builder_fixture();
    let caller: AgentId = request.agent_id.clone().unwrap().try_into().unwrap();
    request.principal = Some(
        Principal::Agent(golem_common::model::agent::AgentPrincipal {
            agent_id: caller.clone(),
        })
        .into(),
    );
    request.auth_ctx = Some(authority_for(&caller.agent_id).into());
    request.context = Some(golem_api_grpc::proto::golem::worker::InvocationContext {
        parent: Some(caller.into()),
        ..Default::default()
    });
    request.origin_invocation = Some(
        golem_api_grpc::proto::golem::worker::StreamInvocationIdentity {
            callee_environment_id: request.environment_id,
            callee: request.agent_id.clone(),
            callee_fingerprint: Some(uuid::Uuid::from_u128(91).into()),
            idempotency_key: Some(IdempotencyKey::new("authoring-invocation".into()).into()),
        },
    );
    request.durable_input_mappings = [
        (101, SchemaType::u64()),
        (7, SchemaType::string()),
        (55, SchemaType::bool()),
    ]
    .into_iter()
    .map(|(id, ty)| {
        durable_stream_mapping_to_proto(
            &foreign_mapping(
                id,
                schema_fingerprint_v1(&SchemaGraph::empty(), Some(&ty)).unwrap(),
                &request,
            ),
            None,
        )
    })
    .collect();
    let build = |request: &InvocationStart| {
        let (accepted, _) = tokio::sync::oneshot::channel();
        let mut built = build_durable_streaming_request(
            request,
            &metadata,
            ComponentRevision::INITIAL,
            AgentFingerprint(uuid::Uuid::from_u128(3)),
            invocation.clone(),
            1,
            accepted,
            8,
        )
        .unwrap();
        built.attempt.invocation.stream_handles = built
            .foreign_mappings
            .iter()
            .map(|mapping| mapping.handle.clone())
            .collect();
        built.attempt
    };
    let original = build(&request);
    let mut fork = request.clone();
    let mut parent = fork.context.as_ref().unwrap().parent.clone().unwrap();
    parent.name.push_str("-fork");
    fork.context.as_mut().unwrap().parent = Some(parent.clone());
    fork.auth_ctx = Some(authority_for(&parent.name).into());
    fork.principal = Some(
        Principal::Agent(golem_common::model::agent::AgentPrincipal {
            agent_id: parent.clone().try_into().unwrap(),
        })
        .into(),
    );
    fork.attempt_id = Some(uuid::Uuid::new_v4().into());
    for mapping in &mut fork.durable_input_mappings {
        let handle = mapping.handle.as_mut().unwrap();
        handle.producer = Some(parent.clone());
        handle.expected_producer_fingerprint = Some(uuid::Uuid::from_u128(92).into());
        handle.stream_id = Some(uuid::Uuid::new_v4().into());
    }
    let fork_attempt = build(&fork);
    assert_ne!(original, fork_attempt);
    assert!(same_origin_stream_invocation(&original, &fork_attempt));
    assert!(same_origin_stream_invocation(&fork_attempt, &original));
    assert!(same_origin_stream_invocation(&original, &original));
    for change in 0..4 {
        let mut changed = fork.clone();
        let AuthCtx::Agent(mut authority) = authority_for(&parent.name) else {
            unreachable!()
        };
        match change {
            0 => {
                authority.account_id =
                    golem_common::model::account::AccountId(uuid::Uuid::from_u128(102))
            }
            1 => {
                authority.effective_surface.lower[0].positive.pop();
            }
            2 => {
                authority.delegation_surface.as_mut().unwrap().cards[0]
                    .lower_positive
                    .pop();
            }
            3 => {
                authority.effective_surface.source_card_ids.clear();
            }
            _ => unreachable!(),
        }
        changed.auth_ctx = Some(AuthCtx::Agent(authority).into());
        assert!(
            !same_origin_stream_invocation(&original, &build(&changed)),
            "changed authority field {change}"
        );
    }
    for change in 0..7 {
        let mut changed = fork.clone();
        match change {
            0 => changed.origin_invocation = None,
            1 => {
                changed.origin_invocation.as_mut().unwrap().idempotency_key =
                    Some(IdempotencyKey::new("other".into()).into())
            }
            2 => {
                changed
                    .context
                    .as_mut()
                    .unwrap()
                    .env
                    .insert("changed".into(), "value".into());
            }
            3 => changed.config.push(Default::default()),
            4 => changed.scope_card = Some(Default::default()),
            5 => changed.auth_ctx = None,
            6 => {
                changed
                    .origin_invocation
                    .as_mut()
                    .unwrap()
                    .callee_fingerprint = Some(uuid::Uuid::from_u128(93).into())
            }
            _ => unreachable!(),
        }
        assert!(
            !same_origin_stream_invocation(&original, &build(&changed)),
            "changed execution field {change}"
        );
    }
    for change in 0..4 {
        let mut changed = fork_attempt.clone();
        match change {
            0 => changed.invocation.invocation_value.push(1),
            1 => {
                changed.invocation.stream_handles.pop();
            }
            2 => {
                changed.invocation.stream_handles[0].element_schema_fingerprint =
                    changed.invocation.stream_handles[1].element_schema_fingerprint
            }
            3 => {
                changed.invocation.target_component_revision =
                    ComponentRevision::INITIAL.next().unwrap()
            }
            _ => unreachable!(),
        }
        assert!(
            !same_origin_stream_invocation(&original, &changed),
            "changed descriptor field {change}"
        );
    }
}

#[test]
fn durable_request_builder_preserves_size_precedence_and_stream_metadata_order() {
    let (mut request, metadata, invocation) = builder_fixture();
    request.durable_input_mappings.push(Default::default());
    let (acceptance_committed, _) = tokio::sync::oneshot::channel();
    let error = match build_durable_streaming_request(
        &request,
        &metadata,
        ComponentRevision::INITIAL,
        AgentFingerprint(uuid::Uuid::from_u128(3)),
        invocation,
        MAX_DURABLE_STREAM_ITEM_SIZE + 1,
        acceptance_committed,
        8,
    ) {
        Ok(_) => panic!("oversized input must be rejected"),
        Err(error) => error,
    };
    assert!(
        error
            .to_string()
            .contains("input exceeds the 16 MiB logical value limit")
    );

    let (request, metadata, invocation) = builder_fixture();
    let (acceptance_committed, _) = tokio::sync::oneshot::channel();
    let built = build_durable_streaming_request(
        &request,
        &metadata,
        ComponentRevision::INITIAL,
        AgentFingerprint(uuid::Uuid::from_u128(3)),
        invocation,
        1,
        acceptance_committed,
        8,
    )
    .unwrap();
    let expected_paths = [
        vec![
            StreamValuePathStep::RecordField(0),
            StreamValuePathStep::MapEntry {
                index: 0,
                side: StreamMapSide::Key,
            },
        ],
        vec![
            StreamValuePathStep::RecordField(0),
            StreamValuePathStep::MapEntry {
                index: 0,
                side: StreamMapSide::Value,
            },
        ],
        vec![StreamValuePathStep::RecordField(1)],
    ];
    assert_eq!(
        built
            .registrations
            .iter()
            .map(|(transport_stream_id, registration)| {
                let path = match &registration.coordinate {
                    super::StreamRegistrationCoordinate::Root {
                        recursive_value_path,
                        ..
                    } => recursive_value_path.clone(),
                    _ => panic!("expected a root registration"),
                };
                (
                    *transport_stream_id,
                    path,
                    registration.element_schema_fingerprint,
                )
            })
            .collect::<Vec<_>>(),
        vec![
            (
                101,
                expected_paths[0].clone(),
                schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::u64())).unwrap(),
            ),
            (
                7,
                expected_paths[1].clone(),
                schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::string())).unwrap(),
            ),
            (
                55,
                expected_paths[2].clone(),
                schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::bool())).unwrap(),
            ),
        ]
    );
    assert_eq!(
        built.input_element_types,
        vec![
            (101, SchemaType::u64()),
            (7, SchemaType::string()),
            (55, SchemaType::bool()),
        ]
    );
    let canonical = golem_schema::proto::golem::schema::TypedSchemaValue::decode(
        built.attempt.invocation.invocation_value.as_slice(),
    )
    .unwrap()
    .value
    .unwrap();
    let mut canonical_ids = Vec::new();
    decode_recursive_stream_value(canonical, |stream_id, _| {
        canonical_ids.push(stream_id);
        Ok(SchemaValueStream::from_host_endpoint(()))
    })
    .unwrap();
    assert_eq!(canonical_ids, vec![0, 1, 2]);

    let (mut request, metadata, invocation) = builder_fixture();
    let fingerprints = [
        schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::u64())).unwrap(),
        schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::string())).unwrap(),
        schema_fingerprint_v1(&SchemaGraph::empty(), Some(&SchemaType::bool())).unwrap(),
    ];
    let mappings = [
        foreign_mapping(55, fingerprints[2], &request),
        foreign_mapping(101, fingerprints[0], &request),
        foreign_mapping(7, fingerprints[1], &request),
    ];
    request.durable_input_mappings = mappings
        .iter()
        .map(|mapping| durable_stream_mapping_to_proto(mapping, None))
        .collect();
    let (acceptance_committed, _) = tokio::sync::oneshot::channel();
    let built = build_durable_streaming_request(
        &request,
        &metadata,
        ComponentRevision::INITIAL,
        AgentFingerprint(uuid::Uuid::from_u128(3)),
        invocation,
        1,
        acceptance_committed,
        8,
    )
    .unwrap();

    assert!(built.registrations.is_empty());
    assert_eq!(
        built
            .foreign_mappings
            .iter()
            .map(|mapping| mapping.transport_stream_id)
            .collect::<Vec<_>>(),
        vec![101, 7, 55]
    );
    assert_eq!(
        built.input_element_types,
        vec![
            (101, SchemaType::u64()),
            (7, SchemaType::string()),
            (55, SchemaType::bool()),
        ]
    );
}

#[test]
fn output_pump_distinguishes_attachment_termination_from_protocol_failure() {
    assert!(is_attachment_termination(
        &crate::durable_host::durable_stream::SessionError::from(
            "invocation response stream closed"
        )
    ));
    assert!(is_attachment_termination(
        &crate::durable_host::durable_stream::SessionError::from("StaleEpoch: attachment fenced")
    ));
    assert!(!is_attachment_termination(
        &crate::durable_host::durable_stream::SessionError::from(
            "durable output journal is corrupt"
        )
    ));
}

#[test]
fn invocation_freshness_defaults_unknown_values_to_may_exist() {
    assert_eq!(
        decode_invocation_freshness_disposition(0),
        InvocationFreshnessDisposition::MayExist
    );
    assert_eq!(
        decode_invocation_freshness_disposition(i32::MAX),
        InvocationFreshnessDisposition::MayExist
    );
}

#[test]
fn invocation_freshness_decodes_known_fresh_explicitly() {
    assert_eq!(
        decode_invocation_freshness_disposition(
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh as i32
        ),
        InvocationFreshnessDisposition::KnownFresh
    );
}

#[test]
fn durable_streaming_requires_matching_callee_fingerprint() {
    let actual = AgentFingerprint::new();
    let missing = require_expected_callee_fingerprint(None, actual).unwrap_err();
    assert!(
        missing
            .to_string()
            .contains("require the expected callee fingerprint")
    );

    let mismatch =
        require_expected_callee_fingerprint(Some(AgentFingerprint::new().0.into()), actual)
            .unwrap_err();
    assert!(mismatch.to_string().contains("does not match"));

    require_expected_callee_fingerprint(Some(actual.0.into()), actual).unwrap();
}

#[test]
fn durable_stream_identity_ignores_non_authoritative_account_email() {
    let auth_ctx = |account_email: &str| {
        Some(AuthCtx {
            value: Some(auth_ctx::Value::User(UserAuthCtx {
                account_id: Some(AccountId {
                    value: Some(Uuid {
                        high_bits: 1,
                        low_bits: 2,
                    }),
                }),
                plan_id: Some(PlanId {
                    value: Some(Uuid {
                        high_bits: 3,
                        low_bits: 4,
                    }),
                }),
                effective_surface: Some(AuthEffectiveSurface::default()),
                account_email: account_email.to_string(),
                ..Default::default()
            })),
        })
    };

    assert_eq!(
        effective_session_identity(&auth_ctx("old@example.com"), &None).unwrap(),
        effective_session_identity(&auth_ctx("new@example.com"), &None).unwrap(),
        "resume identity must pin the effective principal/grant, not mutable account metadata"
    );
}

#[test]
fn durable_stream_identity_ignores_account_role_wire_order() {
    let auth_ctx = |account_roles| {
        Some(AuthCtx {
            value: Some(auth_ctx::Value::User(UserAuthCtx {
                account_id: Some(AccountId {
                    value: Some(Uuid {
                        high_bits: 1,
                        low_bits: 2,
                    }),
                }),
                plan_id: Some(PlanId {
                    value: Some(Uuid {
                        high_bits: 3,
                        low_bits: 4,
                    }),
                }),
                account_roles,
                effective_surface: Some(AuthEffectiveSurface::default()),
                ..Default::default()
            })),
        })
    };
    let admin = golem_api_grpc::proto::golem::auth::AccountRole::Admin as i32;
    let marketing = golem_api_grpc::proto::golem::auth::AccountRole::MarketingAdmin as i32;
    let ordered = auth_ctx(vec![admin, marketing]);
    let reversed = auth_ctx(vec![marketing, admin]);
    let ordered_auth: golem_service_base::model::auth::AuthCtx =
        ordered.clone().unwrap().try_into().unwrap();
    let reversed_auth: golem_service_base::model::auth::AuthCtx =
        reversed.clone().unwrap().try_into().unwrap();
    assert_eq!(ordered_auth, reversed_auth);

    assert_eq!(
        effective_session_identity(&ordered, &None).unwrap(),
        effective_session_identity(&reversed, &None).unwrap(),
        "account roles are a set after auth validation, so protobuf ordering must not change the pinned resume identity"
    );
}

#[test]
fn durable_start_failures_use_their_explicit_protocol_reasons() {
    for (details, expected) in [
        (
            "IdempotencyConflict: descriptor changed",
            InvocationRejectionReason::IdempotencyConflict,
        ),
        (
            "AttemptConflict: attempt changed",
            InvocationRejectionReason::AttemptConflict,
        ),
        (
            "ResourceExhausted: too many streams",
            InvocationRejectionReason::ResourceExhausted,
        ),
        (
            "StaleEpoch: old transport",
            InvocationRejectionReason::StaleEpoch,
        ),
        (
            "InvalidEpoch: future epoch",
            InvocationRejectionReason::InvalidEpoch,
        ),
        (
            "InvalidAttachmentState: already attached",
            InvocationRejectionReason::InvalidAttachmentState,
        ),
        (
            "Unauthorized: principal changed",
            InvocationRejectionReason::Unauthorized,
        ),
        (
            "NotFound: durable session is absent",
            InvocationRejectionReason::NotFound,
        ),
    ] {
        assert_eq!(
            pre_acceptance_rejection_reason(&WorkerExecutorError::invalid_request(details)),
            expected
        );
    }
}

#[test]
async fn inflight_committed_acceptance_wins_after_inbound_becomes_ready() {
    let (acceptance_committed_tx, mut acceptance_committed_rx) = tokio::sync::oneshot::channel();
    let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel();
    let (commit_started_tx, commit_started_rx) = tokio::sync::oneshot::channel();
    let (commit_finished_tx, commit_finished_rx) = tokio::sync::oneshot::channel();
    let mut commit_started_tx = Some(commit_started_tx);
    let actor = tokio::spawn(async move {
        commit_started_rx.await.unwrap();
        acceptance_committed_tx.send(()).unwrap();
        commit_finished_tx.send(()).unwrap();
    });
    let invocation = async move {
        commit_finished_rx.await.unwrap();
        accepted_tx.send(42_u64).unwrap();
        future::pending::<()>().await
    };
    let inbound = future::poll_fn(move |_| {
        commit_started_tx.take().unwrap().send(()).unwrap();
        Poll::Ready("transport closed")
    });
    tokio::pin!(invocation);

    let race = race_invocation_acceptance(
        &mut accepted_rx,
        &mut acceptance_committed_rx,
        invocation.as_mut(),
        inbound,
    )
    .await;
    actor.await.unwrap();

    assert!(matches!(
        race,
        AcceptanceRace::Accepted {
            acceptance: 42,
            early_output: None,
            early_inbound: Some("transport closed"),
        }
    ));
}

#[test]
async fn dropped_acceptance_sender_preserves_invocation_output() {
    let (acceptance_committed_tx, mut acceptance_committed_rx) = tokio::sync::oneshot::channel();
    let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel::<()>();
    let invocation = async move {
        drop(accepted_tx);
        tokio::task::yield_now().await;
        drop(acceptance_committed_tx);
        "invocation error"
    };
    tokio::pin!(invocation);

    let race = race_invocation_acceptance(
        &mut accepted_rx,
        &mut acceptance_committed_rx,
        invocation.as_mut(),
        future::pending::<()>(),
    )
    .await;

    assert!(matches!(
        race,
        AcceptanceRace::InvocationFinished("invocation error")
    ));
}

#[test]
async fn dropped_acceptance_sender_does_not_repoll_cached_invocation_output() {
    let (acceptance_committed_tx, mut acceptance_committed_rx) = tokio::sync::oneshot::channel();
    let (accepted_tx, mut accepted_rx) = tokio::sync::oneshot::channel::<()>();
    let actor = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        drop(accepted_tx);
    });
    let invocation = async move {
        acceptance_committed_tx.send(()).unwrap();
        "invocation output"
    };
    tokio::pin!(invocation);

    let race = race_invocation_acceptance(
        &mut accepted_rx,
        &mut acceptance_committed_rx,
        invocation.as_mut(),
        future::pending::<()>(),
    )
    .await;
    actor.await.unwrap();

    assert!(matches!(
        race,
        AcceptanceRace::InvocationFinished("invocation output")
    ));
}
