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
use crate::base_model::account::AccountId;
use crate::base_model::agent::AgentTypeName;
use crate::base_model::application::ApplicationId;
use crate::base_model::caller::test_support::{account_principal, sample_caller};
use crate::base_model::card::CardId;
use crate::base_model::environment::EnvironmentId;
use crate::base_model::security_scheme::SecuritySchemeId;
use crate::base_model::tool::ToolName;
use crate::base_model::{AgentFingerprint, AgentId, InvocationId, OplogIndex, PromiseId};
use std::collections::BTreeSet;
use std::net::IpAddr;
use test_r::test;
use uuid::Uuid;

fn fingerprint() -> AgentFingerprint {
    AgentFingerprint(Uuid::from_u128(0xf1))
}

fn agent() -> AgentTarget {
    AgentTarget {
        environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
        agent_type: AgentTypeName("Relay".to_string()),
        agent_id: AgentId::from_parts(7, 0xa1),
        fingerprint: Some(fingerprint()),
    }
}

fn tool() -> ToolTarget {
    ToolTarget {
        environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
        tool_name: ToolName::try_from("report").unwrap(),
    }
}

fn tool_invocation() -> InvocationTarget {
    InvocationTarget {
        invocation_id: InvocationId(Uuid::from_u128(0x1a)),
        owner: InvocationOwner::Tool(tool()),
    }
}

fn tool_webhook() -> EntityTarget {
    EntityTarget {
        id: EntityIdText("webhook_3xAmPlE".to_string()),
        owner: Owner::Tool(tool()),
    }
}

fn tool_promise() -> PromiseTarget {
    PromiseTarget {
        id: EntityIdText("promise_3xAmPlE".to_string()),
        owner: PromiseOwner::Invocation(tool_invocation()),
    }
}

fn every_presented_input() -> PresentedAuthority {
    PresentedAuthority::none()
        .with_token(PresentedToken::new("golem_ct_first"))
        .with_token(PresentedToken::new("golem_ct_second"))
        .with_courtesy_card(CardId(Uuid::from_u128(0xc0c0)))
        .with_source(IpAddress("2001:db8::1".parse::<IpAddr>().unwrap()))
        .with_session(SessionRef("session-1".to_string()))
}

fn round_trip_json<T>(value: &T) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
}

fn round_trip_desert<T>(value: &T) -> T
where
    T: desert_rust::BinarySerializer + desert_rust::BinaryDeserializer,
{
    desert_rust::deserialize(&desert_rust::serialize_to_byte_vec(value).unwrap()).unwrap()
}

fn class_and_verb(shape: &PartShape) -> (Class, &'static str) {
    (shape.verb.class(), shape.verb.name())
}

#[test]
fn every_catalog_operation_has_at_least_one_part_and_each_part_a_target() {
    for operation in CatalogOperation::ALL {
        assert!(
            !operation.parts().is_empty(),
            "{operation} has no (class, verb) part"
        );
        for shape in operation.parts() {
            assert!(
                !shape.targets.is_empty(),
                "a part of {operation} has no target kind"
            );
        }
    }
}

#[test]
fn catalog_operation_names_are_unique() {
    let names: BTreeSet<&str> = CatalogOperation::ALL.iter().map(|o| o.name()).collect();
    assert_eq!(names.len(), CatalogOperation::ALL.len());
}

#[test]
fn hosted_exec_checks_agent_exec_on_the_agent_first_and_tool_invoke_on_the_tool() {
    let shapes = CatalogOperation::HostedExec.parts();
    assert_eq!(
        shapes.iter().map(class_and_verb).collect::<Vec<_>>(),
        vec![(Class::Agent, "exec"), (Class::Tool, "invoke")]
    );
    assert_eq!(shapes[0].targets, &[TargetKind::Agent]);
    assert_eq!(shapes[1].targets, &[TargetKind::Tool]);

    let operation = Operation::hosted_exec(agent(), tool(), "bash".to_string()).unwrap();
    assert_eq!(
        operation.parts(),
        &[
            OperationPart::new(AgentVerb::Exec, Resource::None, Target::Agent(agent())),
            OperationPart::new(
                ToolVerb::Invoke,
                Resource::Command("bash".to_string()),
                Target::Tool(tool())
            ),
        ]
    );
}

#[test]
fn a_hosted_exec_with_only_one_of_its_parts_is_refused() {
    let agent_part = OperationPart::new(AgentVerb::Exec, Resource::None, Target::Agent(agent()));
    let tool_part = OperationPart::new(
        ToolVerb::Invoke,
        Resource::Command("bash".to_string()),
        Target::Tool(tool()),
    );

    for parts in [vec![agent_part.clone()], vec![tool_part.clone()]] {
        assert!(matches!(
            Operation::new(CatalogOperation::HostedExec, parts),
            Err(OperationShapeError::PartCount { .. })
        ));
    }
    assert!(matches!(
        Operation::new(CatalogOperation::HostedExec, vec![tool_part, agent_part]),
        Err(OperationShapeError::Verb { index: 0, .. })
    ));
}

#[test]
fn tool_invocation_controls_and_tool_webhook_closing_target_the_invocation_or_the_webhook() {
    let tool_controls = [
        ToolVerb::Interrupt,
        ToolVerb::Resume,
        ToolVerb::CloseWebhook,
    ];
    let mut seen = BTreeSet::new();
    for operation in CatalogOperation::ALL {
        for shape in operation.parts() {
            let Verb::Tool(verb) = shape.verb else {
                continue;
            };
            if !tool_controls.contains(&verb) {
                continue;
            }
            seen.insert(verb.name());
            assert!(
                shape
                    .targets
                    .iter()
                    .all(|t| matches!(t, TargetKind::Invocation | TargetKind::Webhook)),
                "{operation} checks tool : {} on {:?}, not on the invocation or the webhook",
                verb.name(),
                shape.targets
            );
        }
    }
    assert_eq!(
        seen,
        BTreeSet::from(["interrupt", "resume", "close-webhook"]),
        "the catalog must hold every tool control"
    );

    assert!(matches!(
        Operation::single(
            CatalogOperation::InterruptToolInvocation,
            Resource::None,
            Target::Tool(tool())
        ),
        Err(OperationShapeError::Target { .. })
    ));
    assert!(
        Operation::single(
            CatalogOperation::InterruptToolInvocation,
            Resource::None,
            Target::Invocation(tool_invocation())
        )
        .is_ok()
    );
    assert!(
        Operation::single(
            CatalogOperation::CloseToolWebhook,
            Resource::None,
            Target::Webhook(tool_webhook())
        )
        .is_ok()
    );
}

#[test]
fn parts_on_designated_targets_come_before_the_other_parts() {
    let designated =
        |shape: &PartShape| shape.targets.iter().all(|t| t.designated_kind().is_some());
    for operation in CatalogOperation::ALL {
        let parts = operation.parts();
        if let Some(first_other) = parts.iter().position(|shape| !designated(shape)) {
            assert!(
                parts[first_other..].iter().all(|shape| !designated(shape)),
                "{operation} checks a part on a designated target after another part"
            );
        }
    }
}

#[test]
fn requests_round_trip_through_json_and_desert() {
    let hosted_exec = AuthorizationRequest::new(
        sample_caller(),
        Operation::hosted_exec(agent(), tool(), "bash".to_string()).unwrap(),
    )
    .with_invocation(InvocationTarget {
        invocation_id: InvocationId(Uuid::from_u128(0x2b)),
        owner: InvocationOwner::Agent(agent()),
    })
    .with_presented(every_presented_input());

    let close_webhook = AuthorizationRequest::new(
        sample_caller(),
        Operation::single(
            CatalogOperation::CloseToolWebhook,
            Resource::None,
            Target::Webhook(tool_webhook()),
        )
        .unwrap(),
    )
    .on_authority_of(account_principal());

    let retry = AuthorizationRequest::new(
        sample_caller(),
        Operation::new(
            CatalogOperation::RetryToolInvocation,
            vec![
                OperationPart::new(
                    InvocationVerb::Read,
                    Resource::None,
                    Target::Invocation(tool_invocation()),
                ),
                OperationPart::new(
                    ToolVerb::Invoke,
                    Resource::Command("summary".to_string()),
                    Target::Tool(tool()),
                ),
            ],
        )
        .unwrap(),
    );

    let create = AuthorizationRequest::new(
        sample_caller(),
        Operation::single(
            CatalogOperation::CreateAgent,
            Resource::None,
            Target::Agent(AgentTarget {
                fingerprint: None,
                ..agent()
            }),
        )
        .unwrap(),
    );

    let send_to_tool_promise = AuthorizationRequest::new(
        sample_caller(),
        Operation::single(
            CatalogOperation::SendToPromise,
            Resource::None,
            Target::Promise(tool_promise()),
        )
        .unwrap(),
    );

    for request in [
        hosted_exec,
        close_webhook,
        retry,
        create,
        send_to_tool_promise,
    ] {
        assert_eq!(round_trip_json(&request), request);
        assert_eq!(round_trip_desert(&request), request);
    }
}

#[test]
fn an_operation_whose_parts_do_not_match_the_catalog_does_not_decode() {
    let hosted_exec = Operation::hosted_exec(agent(), tool(), "bash".to_string()).unwrap();
    let mut json = serde_json::to_value(&hosted_exec).unwrap();
    json["parts"].as_array_mut().unwrap().pop();

    assert!(serde_json::from_value::<Operation>(json).is_err());
}

#[test]
fn grantors_keep_every_ancestor_and_presenting_token_through_json_and_desert() {
    let long_chain: Vec<CardId> = (0..300).map(|n| CardId(Uuid::from_u128(n))).collect();
    let grantors = Grantors {
        cards: vec![
            GrantorCard {
                card_id: CardId(Uuid::from_u128(0xa1)),
                ancestors: vec![],
                presented_by: None,
            },
            GrantorCard {
                card_id: CardId(Uuid::from_u128(0xa2)),
                ancestors: vec![CardId(Uuid::from_u128(0xb1)), CardId(Uuid::from_u128(0xb2))],
                presented_by: Some(CardTokenId(Uuid::from_u128(0x70c1))),
            },
            GrantorCard {
                card_id: CardId(Uuid::from_u128(0xa3)),
                ancestors: long_chain.clone(),
                presented_by: Some(CardTokenId(Uuid::from_u128(0x70c2))),
            },
        ],
    };

    for decoded in [round_trip_json(&grantors), round_trip_desert(&grantors)] {
        assert_eq!(decoded, grantors);
        assert_eq!(decoded.cards[2].ancestors, long_chain);
        assert_eq!(
            decoded.cards[1].presented_by,
            Some(CardTokenId(Uuid::from_u128(0x70c1)))
        );
        assert_eq!(decoded.cards[0].presented_by, None);
    }
}

#[test]
fn decisions_round_trip_through_json_and_desert() {
    let decisions = [
        Decision::Allow(Grantors {
            cards: vec![GrantorCard {
                card_id: CardId(Uuid::from_u128(0xa1)),
                ancestors: vec![CardId(Uuid::from_u128(0xb1))],
                presented_by: Some(CardTokenId(Uuid::from_u128(0x70c1))),
            }],
        }),
        Decision::Deny(Denial::SignInRequired { scheme: None }),
        Decision::Deny(Denial::SignInRequired {
            scheme: Some(SecuritySchemeId(Uuid::from_u128(0x5c))),
        }),
        Decision::Deny(Denial::LooksUnknown {
            kind: DesignatedKind::Agent,
        }),
        Decision::Deny(Denial::Forbidden {
            missing_grant: Some(GrantText("tool(?env/report) @ * : invoke : *".to_string())),
        }),
        Decision::Deny(Denial::Forbidden {
            missing_grant: None,
        }),
    ];

    for decision in decisions {
        assert_eq!(round_trip_json(&decision), decision);
        assert_eq!(round_trip_desert(&decision), decision);
    }
}

#[test]
fn a_request_decides_on_the_immediate_caller_unless_another_authority_is_named() {
    let operation = Operation::single(
        CatalogOperation::ViewAgent,
        Resource::None,
        Target::Agent(agent()),
    )
    .unwrap();

    let request = AuthorizationRequest::new(sample_caller(), operation);
    assert!(request.principal.same_record(&sample_caller().immediate));

    let placement = request.on_authority_of(account_principal());
    assert!(placement.principal.same_record(&account_principal()));
    assert_eq!(placement.caller, sample_caller());
}

#[test]
fn debug_output_never_shows_a_presented_token_value() {
    let text = format!("{:?}", every_presented_input());
    assert!(!text.contains("golem_ct_first"));
    assert!(!text.contains("golem_ct_second"));
}

#[test]
fn socket_destinations_keep_the_address_the_host_flag_and_the_linked_names() {
    let socket = Resource::Destination(Destination::Socket {
        address: IpAddress("2001:db8::5".parse::<IpAddr>().unwrap()),
        port: 443,
        this_host: false,
        hostnames: vec![
            "api.example.com".to_string(),
            "edge.example.com".to_string(),
        ],
    });
    let host_port = Resource::Destination(Destination::HostPort {
        host: "api.example.com".to_string(),
        port: None,
    });

    for resource in [socket, host_port] {
        assert_eq!(resource.kind(), ResourceKind::Destination);
        assert_eq!(round_trip_json(&resource), resource);
        assert_eq!(round_trip_desert(&resource), resource);
    }
}

#[test]
fn entity_targets_must_have_the_owner_kind_that_the_catalog_names() {
    let agent_invocation = InvocationTarget {
        invocation_id: InvocationId(Uuid::from_u128(0x2b)),
        owner: InvocationOwner::Agent(agent()),
    };
    let agent_webhook = EntityTarget {
        id: EntityIdText("webhook_ag3nt".to_string()),
        owner: Owner::Agent(agent()),
    };

    // A tool invocation that runs inside an agent belongs to the agent, so the tool's controls and
    // the tool's oplog reads do not reach it.
    for operation in [
        CatalogOperation::InterruptToolInvocation,
        CatalogOperation::ResumeToolInvocation,
        CatalogOperation::ReadToolInvocationOplog,
    ] {
        assert!(
            matches!(
                Operation::single(
                    operation,
                    Resource::None,
                    Target::Invocation(agent_invocation.clone())
                ),
                Err(OperationShapeError::Owner { .. })
            ),
            "{operation} accepted an invocation that an agent owns"
        );
        assert!(
            Operation::single(
                operation,
                Resource::None,
                Target::Invocation(tool_invocation())
            )
            .is_ok(),
            "{operation} refused an invocation that a tool owns"
        );
    }

    assert!(matches!(
        Operation::single(
            CatalogOperation::CloseToolWebhook,
            Resource::None,
            Target::Webhook(agent_webhook.clone())
        ),
        Err(OperationShapeError::Owner { .. })
    ));
    assert!(matches!(
        Operation::single(
            CatalogOperation::CloseAgentWebhook,
            Resource::None,
            Target::Webhook(tool_webhook())
        ),
        Err(OperationShapeError::Owner { .. })
    ));
    assert!(
        Operation::single(
            CatalogOperation::CloseAgentWebhook,
            Resource::None,
            Target::Webhook(agent_webhook)
        )
        .is_ok()
    );

    // An entry that names no owner kinds accepts any owner.
    for invocation in [agent_invocation, tool_invocation()] {
        assert!(
            Operation::single(
                CatalogOperation::ReadInvocation,
                Resource::None,
                Target::Invocation(invocation)
            )
            .is_ok()
        );
    }
}

#[test]
fn invocation_listing_and_task_read_and_cancel_are_in_the_catalog() {
    let environment = Target::Environment(EnvironmentTarget {
        account_id: AccountId(Uuid::from_u128(0xacc0)),
        application_id: ApplicationId(Uuid::from_u128(0xa0)),
        environment_id: EnvironmentId(Uuid::from_u128(0xe0)),
    });
    for owner in [Target::Agent(agent()), Target::Tool(tool()), environment] {
        let listing =
            Operation::single(CatalogOperation::ListInvocations, Resource::None, owner).unwrap();
        assert_eq!(
            listing.parts()[0].verb,
            Verb::Invocation(InvocationVerb::Read)
        );
    }

    let task = Target::Task(EntityTarget {
        id: EntityIdText("task_7".to_string()),
        owner: Owner::Invocation(tool_invocation()),
    });
    for (operation, verb) in [
        (CatalogOperation::ReadTask, InvocationVerb::Read),
        (CatalogOperation::CancelTask, InvocationVerb::Cancel),
    ] {
        let built = Operation::single(operation, Resource::None, task.clone()).unwrap();
        assert_eq!(built.parts()[0].verb, Verb::Invocation(verb));
    }
}

#[test]
fn task_input_needs_invocation_read_on_the_task_and_promise_send_on_its_input_promise() {
    let task = Target::Task(EntityTarget {
        id: EntityIdText("task_7".to_string()),
        owner: Owner::Agent(agent()),
    });
    let input_promise = Target::Promise(agent_promise());

    assert_eq!(
        CatalogOperation::SendTaskInput
            .parts()
            .iter()
            .map(class_and_verb)
            .collect::<Vec<_>>(),
        vec![(Class::Invocation, "read"), (Class::Promise, "send")]
    );
    let operation = Operation::new(
        CatalogOperation::SendTaskInput,
        vec![
            OperationPart::new(InvocationVerb::Read, Resource::None, task.clone()),
            OperationPart::new(PromiseVerb::Send, Resource::None, input_promise),
        ],
    )
    .unwrap();
    assert_eq!(operation.parts()[0].target, task);

    // No invocation grant alone continues a task: the promise part is always there.
    assert!(matches!(
        Operation::single(
            CatalogOperation::SendTaskInput,
            Resource::None,
            task.clone()
        ),
        Err(OperationShapeError::PartCount { .. })
    ));
    assert!(matches!(
        Operation::new(
            CatalogOperation::SendTaskInput,
            vec![OperationPart::new(
                InvocationVerb::Send,
                Resource::None,
                task
            )],
        ),
        Err(OperationShapeError::PartCount { .. })
    ));
}

/// A promise that an agent created, with today's promise ID in its text form.
fn agent_promise() -> PromiseTarget {
    let promise_id = PromiseId {
        agent_id: agent().agent_id,
        fingerprint: fingerprint(),
        oplog_idx: OplogIndex::from_u64(42),
    };
    PromiseTarget {
        id: EntityIdText(promise_id.to_string()),
        owner: PromiseOwner::Agent(agent()),
    }
}

#[test]
fn a_promise_is_owned_by_the_agent_or_the_tool_invocation_that_created_it() {
    let by_agent = Target::Promise(agent_promise());
    let by_tool_invocation = Target::Promise(tool_promise());

    assert_eq!(by_agent.owner_kind(), Some(OwnerKind::Agent));
    assert_eq!(by_tool_invocation.owner_kind(), Some(OwnerKind::Invocation));

    for promise in [by_agent, by_tool_invocation] {
        let send = Operation::single(
            CatalogOperation::SendToPromise,
            Resource::None,
            promise.clone(),
        )
        .unwrap();
        assert_eq!(send.parts()[0].target, promise);
        assert_eq!(round_trip_json(&promise), promise);
        assert_eq!(round_trip_desert(&promise), promise);
    }
}

fn position_in_catalog(operation: CatalogOperation) -> usize {
    CatalogOperation::ALL
        .iter()
        .position(|each| *each == operation)
        .unwrap()
}

#[test]
fn a_methodless_upsert_asks_for_any_one_method_and_only_upsert_agent_takes_that_form() {
    // The new operation follows the last operation of the first catalog, so the encodings of the
    // others do not move.
    assert_eq!(
        position_in_catalog(CatalogOperation::UpsertAgent),
        position_in_catalog(CatalogOperation::ListCardTokens) + 1
    );
    assert_eq!(
        CatalogOperation::UpsertAgent
            .parts()
            .iter()
            .map(class_and_verb)
            .collect::<Vec<_>>(),
        vec![(Class::Agent, "invoke")]
    );
    let taking_any_one_method: Vec<CatalogOperation> = CatalogOperation::ALL
        .iter()
        .copied()
        .filter(|operation| {
            operation
                .parts()
                .iter()
                .any(|shape| shape.resource == ResourceKind::AnyOneMethod)
        })
        .collect();
    assert_eq!(taking_any_one_method, vec![CatalogOperation::UpsertAgent]);

    // An existing agent, and a keyed agent that does not exist yet.
    let new_agent = AgentTarget {
        fingerprint: None,
        ..agent()
    };
    for target in [agent(), new_agent] {
        let upsert = AuthorizationRequest::new(
            sample_caller(),
            Operation::single(
                CatalogOperation::UpsertAgent,
                Resource::AnyOneMethod,
                Target::Agent(target),
            )
            .unwrap(),
        );
        assert_eq!(round_trip_json(&upsert), upsert);
        assert_eq!(round_trip_desert(&upsert), upsert);
    }

    // invoke-agent stays exact: it refuses the weaker form.
    assert_eq!(
        Operation::single(
            CatalogOperation::InvokeAgent,
            Resource::AnyOneMethod,
            Target::Agent(agent())
        ),
        Err(OperationShapeError::Resource {
            operation: "invoke-agent",
            index: 0,
            expected: ResourceKind::Method,
            actual: ResourceKind::AnyOneMethod,
        })
    );
    // An upsert that calls a method uses invoke-agent, so upsert-agent refuses a method.
    assert!(matches!(
        Operation::single(
            CatalogOperation::UpsertAgent,
            Resource::Method("summary".to_string()),
            Target::Agent(agent())
        ),
        Err(OperationShapeError::Resource { .. })
    ));
}

type Shape = (
    Class,
    &'static str,
    ResourceKind,
    Vec<TargetKind>,
    Vec<OwnerKind>,
);

fn shapes(operation: CatalogOperation) -> Vec<Shape> {
    operation
        .parts()
        .iter()
        .map(|part| {
            (
                part.verb.class(),
                part.verb.name(),
                part.resource,
                part.targets.to_vec(),
                part.owners.to_vec(),
            )
        })
        .collect()
}

#[test]
fn input_to_a_running_tool_invocation_repeats_its_admission() {
    // Appended after upsert-agent, in this order.
    assert_eq!(
        position_in_catalog(CatalogOperation::SendToolInput),
        position_in_catalog(CatalogOperation::UpsertAgent) + 1
    );
    assert_eq!(
        position_in_catalog(CatalogOperation::SendHostedExecInput),
        position_in_catalog(CatalogOperation::UpsertAgent) + 2
    );

    let standalone: Vec<Shape> = vec![
        (
            Class::Invocation,
            "send",
            ResourceKind::None,
            vec![TargetKind::Invocation],
            vec![OwnerKind::Tool],
        ),
        (
            Class::Tool,
            "invoke",
            ResourceKind::Command,
            vec![TargetKind::Tool],
            vec![],
        ),
    ];
    assert_eq!(shapes(CatalogOperation::SendToolInput), standalone);
    let hosted: Vec<Shape> = vec![
        (
            Class::Invocation,
            "send",
            ResourceKind::None,
            vec![TargetKind::Invocation],
            vec![OwnerKind::Agent],
        ),
        (
            Class::Agent,
            "exec",
            ResourceKind::None,
            vec![TargetKind::Agent],
            vec![],
        ),
        (
            Class::Tool,
            "invoke",
            ResourceKind::Command,
            vec![TargetKind::Tool],
            vec![],
        ),
    ];
    assert_eq!(shapes(CatalogOperation::SendHostedExecInput), hosted);

    let agent_invocation = InvocationTarget {
        invocation_id: InvocationId(Uuid::from_u128(0x2b)),
        owner: InvocationOwner::Agent(agent()),
    };
    let standalone_input = |invocation: InvocationTarget, command: Resource| {
        Operation::new(
            CatalogOperation::SendToolInput,
            vec![
                OperationPart::new(
                    InvocationVerb::Send,
                    Resource::None,
                    Target::Invocation(invocation),
                ),
                OperationPart::new(ToolVerb::Invoke, command, Target::Tool(tool())),
            ],
        )
    };
    let hosted_input = |invocation: InvocationTarget| {
        Operation::new(
            CatalogOperation::SendHostedExecInput,
            vec![
                OperationPart::new(
                    InvocationVerb::Send,
                    Resource::None,
                    Target::Invocation(invocation),
                ),
                OperationPart::new(AgentVerb::Exec, Resource::None, Target::Agent(agent())),
                OperationPart::new(
                    ToolVerb::Invoke,
                    Resource::Command("bash".to_string()),
                    Target::Tool(tool()),
                ),
            ],
        )
    };

    // The tool part carries the command that the input goes to.
    let standalone_operation =
        standalone_input(tool_invocation(), Resource::Command("summary".to_string())).unwrap();
    assert_eq!(
        standalone_operation.parts()[1].resource,
        Resource::Command("summary".to_string())
    );
    let hosted_operation = hosted_input(agent_invocation.clone()).unwrap();
    assert_eq!(
        hosted_operation.parts()[2].resource,
        Resource::Command("bash".to_string())
    );
    for operation in [standalone_operation, hosted_operation] {
        let request = AuthorizationRequest::new(sample_caller(), operation);
        assert_eq!(round_trip_json(&request), request);
        assert_eq!(round_trip_desert(&request), request);
    }

    // A standalone input names a tool's invocation, and a hosted input an agent's.
    assert!(matches!(
        standalone_input(
            agent_invocation.clone(),
            Resource::Command("summary".to_string())
        ),
        Err(OperationShapeError::Owner { .. })
    ));
    assert!(matches!(
        hosted_input(tool_invocation()),
        Err(OperationShapeError::Owner { .. })
    ));
    // Tool admission is always repeated for a command, never for every command at once.
    assert!(matches!(
        standalone_input(tool_invocation(), Resource::All),
        Err(OperationShapeError::Resource { .. })
    ));
    // Hosted input never skips `agent : exec`.
    assert!(matches!(
        Operation::new(
            CatalogOperation::SendHostedExecInput,
            vec![
                OperationPart::new(
                    InvocationVerb::Send,
                    Resource::None,
                    Target::Invocation(agent_invocation),
                ),
                OperationPart::new(
                    ToolVerb::Invoke,
                    Resource::Command("bash".to_string()),
                    Target::Tool(tool()),
                ),
            ],
        ),
        Err(OperationShapeError::PartCount { .. })
    ));
}

#[test]
fn a_forbidden_denial_names_its_grant_under_a_camel_case_key() {
    let json = serde_json::to_value(Denial::Forbidden {
        missing_grant: Some(GrantText("tool(?env/report) @ * : invoke : *".to_string())),
    })
    .unwrap();

    assert_eq!(json["missingGrant"], "tool(?env/report) @ * : invoke : *");
    assert!(json.get("missing_grant").is_none());
}

/// `ApiToken` -> `api-token`, for the debug names of the kind enums.
fn kebab(name: String) -> String {
    let mut out = String::new();
    for (index, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if index > 0 {
                out.push('-');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// The classes with their verbs, then the catalog with the parts of each operation, each in
/// encoding order.
fn render_catalog() -> String {
    let mut out = String::from("# Classes and their verbs, in encoding order.\n");
    for class in Class::ALL {
        let verbs: Vec<&str> = class.verbs().iter().map(|verb| verb.name()).collect();
        out.push_str(&format!("{class}: {}\n", verbs.join(" ")));
    }
    out.push_str(
        "\n# Operations, in encoding order. A part is `class : verb [resource] @ targets`, \
         with the allowed owner kinds in parentheses.\n",
    );
    for operation in CatalogOperation::ALL {
        let parts: Vec<String> = operation
            .parts()
            .iter()
            .map(|shape| {
                let targets: Vec<String> = shape
                    .targets
                    .iter()
                    .map(|target| kebab(format!("{target:?}")))
                    .collect();
                let owners = if shape.owners.is_empty() {
                    String::new()
                } else {
                    let owners: Vec<String> = shape
                        .owners
                        .iter()
                        .map(|owner| kebab(format!("{owner:?}")))
                        .collect();
                    format!(" ({})", owners.join(" | "))
                };
                format!(
                    "{} [{}] @ {}{owners}",
                    shape.verb,
                    kebab(format!("{:?}", shape.resource)),
                    targets.join(" | ")
                )
            })
            .collect();
        out.push_str(&format!("{operation}: {}\n", parts.join(" ; ")));
    }
    out
}

#[test]
fn the_catalog_matches_its_golden_file() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/base_model/authority/catalog.golden.txt"
    );
    let rendered = render_catalog();
    if std::env::var_os("GOLEM_UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &rendered).unwrap();
    }
    let golden = std::fs::read_to_string(path).unwrap_or_default();

    pretty_assertions::assert_eq!(
        golden,
        rendered,
        "The catalog differs from {path}. The binary encoding of classes, verbs and operations is \
         their position, so add new ones at the end of their list, never in the middle. Check the \
         change against the spec, then run again with GOLEM_UPDATE_GOLDEN=1 to accept it."
    );
}
