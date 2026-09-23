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
use crate::model::AgentId;
use crate::model::account::{AccountEmail, AccountId};
use crate::model::agent::{AgentPrincipal, AgentTypeName, GolemUserPrincipal};
use crate::model::component::ComponentName;
use crate::model::environment::EnvironmentId;
use crate::model::json::NormalizedJsonValue;
use crate::model::tool::{SecretKeyScope, ToolSource};
use crate::schema::tool::CommandTree;
use test_r::test;

fn tool_definition() -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        commands: CommandTree { nodes: Vec::new() },
        schema: crate::schema::SchemaGraph::empty(),
    }
}

fn owner() -> OwnedAgentId {
    OwnedAgentId::new(
        EnvironmentId::new(),
        &AgentId {
            component_id: ComponentId::new(),
            agent_id: "Example(\"owner\")".to_string(),
        },
    )
}

fn activation() -> EntityActivation {
    let component_id = ComponentId::new();
    let component_revision = ComponentRevision::try_from(7_u64).unwrap();
    let deployment_revision = DeploymentRevision::try_from(11_u64).unwrap();
    let source = ToolSource::Component {
        component_id,
        component_revision,
        component_name: ComponentName("tools:search".to_string()),
    };
    let binding = CompiledToolBinding {
        deployment_revision,
        release_id: None,
        owner: crate::model::tool::ToolBindingOwner::AgentType {
            agent_type_name: AgentTypeName("Example".to_string()),
        },
        tool_name: ToolName::try_from("search").unwrap(),
        version: "1.0.0".to_string(),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
        account_id: AccountId::new(),
        account_email: AccountEmail::new("owner@example.com"),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        config_keys_readable: crate::model::tool::ConfigKeyScope::All,
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        filesystem_access: crate::model::tool::ToolFilesystemAccess::Unset,
        source,
    };

    EntityActivation::new(
        ExecutableTarget::new(component_id, component_revision),
        deployment_revision,
        EntityActivationPolicy::Tool {
            provision: ToolProvisionConfig::default(),
            binding: Box::new(binding),
        },
        FilesystemCapability::Incapable,
    )
    .unwrap()
}

fn middleware_activation() -> EntityActivation {
    EntityActivation::new(
        ExecutableTarget::new(
            ComponentId::new(),
            ComponentRevision::try_from(9_u64).unwrap(),
        ),
        DeploymentRevision::try_from(12_u64).unwrap(),
        EntityActivationPolicy::ToolMiddleware {
            middleware_name: ToolMiddlewareName::try_from("audit").unwrap(),
            provision: ToolProvisionConfig::default(),
            config_keys_readable: crate::model::tool::ConfigKeyScope::All,
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: ToolFilesystemAccess::Unset,
        },
        FilesystemCapability::Incapable,
    )
    .unwrap()
}

fn host_activation() -> EntityActivation {
    let component_activation = activation();
    let deployment_revision = component_activation.deployment_revision;
    let mut policy = component_activation.policy;
    let host_tool_id = HostToolId::try_from("native-search".to_string()).unwrap();
    let implementation_version = "1.2.3".to_string();
    let EntityActivationPolicy::Tool { binding, .. } = &mut policy else {
        unreachable!()
    };
    binding.source = ToolSource::Host {
        host_tool_id: host_tool_id.clone(),
        implementation_version: implementation_version.clone(),
    };

    EntityActivation::new_host(
        host_tool_id,
        implementation_version,
        deployment_revision,
        policy,
        FilesystemCapability::Incapable,
    )
    .unwrap()
}

#[test]
fn equal_tool_and_middleware_names_are_distinct_selectors() {
    let tool = AgentEntity::Tool(ToolName::try_from("search").unwrap());
    let middleware = AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("search").unwrap());

    assert_ne!(tool, middleware);
}

#[test]
fn entity_ids_project_to_the_unchanged_owner() {
    let owner = owner();
    let entity_id = OwnedAgentEntityId {
        owner: owner.clone(),
        entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
    };
    let invocation_id =
        EntityInvocationId::new(entity_id.clone(), OplogIndex::from_u64(42)).unwrap();

    assert_eq!(entity_id.owner_id(), &owner);
    assert_eq!(invocation_id.owner_id(), &owner);
    assert_eq!(invocation_id.start_index(), OplogIndex::from_u64(42));
}

#[test]
fn entity_invocation_id_json_roundtrip_is_structured() {
    let invocation_id = EntityInvocationId::new(
        OwnedAgentEntityId {
            owner: owner(),
            entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
        },
        OplogIndex::from_u64(42),
    )
    .unwrap();

    let json = serde_json::to_value(&invocation_id).unwrap();
    let decoded: EntityInvocationId = serde_json::from_value(json.clone()).unwrap();

    assert_eq!(decoded, invocation_id);
    assert_eq!(json["entityId"]["entity"]["kind"], "tool");
    assert_eq!(json["entityId"]["entity"]["name"], "search");
    assert_eq!(json["startIndex"], 42);
}

#[test]
fn entity_invocation_id_protobuf_roundtrip_is_structured() {
    let invocation_id = EntityInvocationId::new(
        OwnedAgentEntityId {
            owner: owner(),
            entity: AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("audit").unwrap()),
        },
        OplogIndex::from_u64(84),
    )
    .unwrap();

    let protobuf: golem_api_grpc::proto::golem::worker::EntityInvocationId =
        invocation_id.clone().into();
    let decoded: EntityInvocationId = protobuf.try_into().unwrap();

    assert_eq!(decoded, invocation_id);
}

#[test]
fn entity_invocation_request_binary_roundtrip_preserves_activation() {
    let owner = owner();
    let activation = activation();
    let request = EntityInvocationRequest {
        entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
        calling_principal: Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id,
        }),
        call_mode: EntityCallMode::Asynchronous,
        operation: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
            attempt_ordinal: 7,
            command_path: vec!["files".to_string(), "search".to_string()],
            args: crate::model::card::ToolInvocationPattern::from_command_and_args(
                &[],
                &["--ignore-case", "needle"],
            )
            .unwrap()
            .args,
            has_stdin: true,
            has_stdout: true,
            declares_stdout: true,
            output_contract: ToolOutputContract {
                result: None,
                errors: Vec::new(),
            },
        }),
        principal: Principal::GolemUser(GolemUserPrincipal {
            account_id: AccountId::new(),
        }),
        plan: EntityInvocationPlanReference::Root {
            plan: EntityInvocationPlan::new(vec![EntityInvocationPlanLayer::Tool { activation }])
                .unwrap(),
        },
        assume_idempotence: false,
    };

    let bytes = desert_rust::serialize_to_byte_vec(&request).unwrap();
    let decoded: EntityInvocationRequest = desert_rust::deserialize(&bytes).unwrap();

    assert_eq!(decoded, request);
}

#[test]
fn entity_invocation_plan_roundtrip_and_descendant_reference_do_not_repeat_plan() {
    let parameters = TypedSchemaValue::new(
        crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::string()),
        crate::schema::SchemaValue::String("outer".to_string()),
    );
    let definition = tool_definition();
    let plan = EntityInvocationPlan::new(vec![
        EntityInvocationPlanLayer::Middleware {
            activation: middleware_activation(),
            parameters: parameters.clone(),
            expected_definition: None,
            presented_definition: None,
            next_effective_definition: definition.clone(),
            compatibility: None,
        },
        EntityInvocationPlanLayer::Middleware {
            activation: middleware_activation(),
            parameters: TypedSchemaValue::new(
                crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::u64()),
                crate::schema::SchemaValue::U64(7),
            ),
            expected_definition: None,
            presented_definition: None,
            next_effective_definition: definition,
            compatibility: None,
        },
        EntityInvocationPlanLayer::Tool {
            activation: host_activation(),
        },
    ])
    .unwrap();
    let root = EntityInvocationPlanReference::Root { plan: plan.clone() };
    let descendant = EntityInvocationPlanReference::Descendant {
        root_start_index: OplogIndex::from_u64(41),
        position: 1,
    };

    let root_bytes = desert_rust::serialize_to_byte_vec(&root).unwrap();
    let descendant_bytes = desert_rust::serialize_to_byte_vec(&descendant).unwrap();
    let decoded: EntityInvocationPlanReference = desert_rust::deserialize(&root_bytes).unwrap();

    assert_eq!(decoded, root);
    let EntityInvocationPlanLayer::Middleware {
        parameters: decoded_parameters,
        ..
    } = plan.layer(0).unwrap()
    else {
        panic!("outer layer must be middleware")
    };
    assert_eq!(decoded_parameters, &parameters);
    assert!(matches!(
        plan.layer(2).unwrap().activation().source(),
        EntityActivationSource::Host { .. }
    ));
    assert!(descendant_bytes.len() < root_bytes.len());
}

#[test]
fn entity_invocation_plan_validates_each_middleware_policy_against_recorded_leaf() {
    use crate::model::agent_secret::CanonicalAgentSecretPath;
    use std::collections::BTreeSet;

    let scope = |keys: &[&str]| {
        SecretKeyScope::Keys(BTreeSet::from_iter(
            keys.iter()
                .map(|key| CanonicalAgentSecretPath(vec![(*key).to_string()])),
        ))
    };
    let set_middleware_scopes =
        |mut activation: EntityActivation, readable: SecretKeyScope, revealable: SecretKeyScope| {
            let EntityActivationPolicy::ToolMiddleware {
                secret_keys_readable,
                secret_keys_revealable,
                ..
            } = &mut activation.policy
            else {
                unreachable!()
            };
            *secret_keys_readable = readable;
            *secret_keys_revealable = revealable;
            activation
        };
    let set_leaf_scopes =
        |mut activation: EntityActivation, readable: SecretKeyScope, revealable: SecretKeyScope| {
            let EntityActivationPolicy::Tool { binding, .. } = &mut activation.policy else {
                unreachable!()
            };
            binding.secret_keys_readable = readable;
            binding.secret_keys_revealable = revealable;
            activation
        };
    let plan = |middlewares: Vec<EntityActivation>, leaf: EntityActivation| {
        let mut layers = middlewares
            .into_iter()
            .map(|activation| EntityInvocationPlanLayer::Middleware {
                activation,
                parameters: TypedSchemaValue::new(
                    crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::tuple(
                        Vec::new(),
                    )),
                    crate::schema::SchemaValue::Tuple {
                        elements: Vec::new(),
                    },
                ),
                expected_definition: None,
                presented_definition: None,
                next_effective_definition: tool_definition(),
                compatibility: None,
            })
            .collect::<Vec<_>>();
        layers.push(EntityInvocationPlanLayer::Tool { activation: leaf });
        EntityInvocationPlan::new(layers)
    };

    let leaf = set_leaf_scopes(activation(), scope(&["a", "b"]), scope(&["a"]));
    assert!(
        plan(
            vec![
                set_middleware_scopes(middleware_activation(), scope(&["a"]), scope(&["a"])),
                set_middleware_scopes(middleware_activation(), scope(&["b"]), scope(&[])),
            ],
            leaf.clone(),
        )
        .is_ok()
    );
    assert!(
        plan(
            vec![set_middleware_scopes(
                middleware_activation(),
                scope(&["outside"]),
                scope(&[]),
            )],
            leaf.clone(),
        )
        .is_err(),
        "middleware readable scope must not exceed the recorded leaf"
    );
    assert!(
        plan(
            vec![set_middleware_scopes(
                middleware_activation(),
                scope(&["a"]),
                scope(&["b"]),
            )],
            leaf.clone(),
        )
        .is_err(),
        "middleware revealable scope must not exceed the recorded leaf"
    );
    assert!(
        plan(
            vec![set_middleware_scopes(
                middleware_activation(),
                scope(&[]),
                scope(&["a"]),
            )],
            leaf,
        )
        .is_err(),
        "middleware revealable scope must remain within its readable scope"
    );
}

#[test]
fn entity_invocation_plan_rejects_middleware_activation_in_tool_layer() {
    let result = EntityInvocationPlan::new(vec![EntityInvocationPlanLayer::Tool {
        activation: middleware_activation(),
    }]);

    assert!(result.is_err());
}

#[test]
fn entity_invocation_plan_rejects_tool_activation_in_middleware_layer() {
    let result = EntityInvocationPlan::new(vec![
        EntityInvocationPlanLayer::Middleware {
            activation: activation(),
            parameters: TypedSchemaValue::new(
                crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::tuple(Vec::new())),
                crate::schema::SchemaValue::Tuple {
                    elements: Vec::new(),
                },
            ),
            expected_definition: None,
            presented_definition: None,
            next_effective_definition: tool_definition(),
            compatibility: None,
        },
        EntityInvocationPlanLayer::Tool {
            activation: activation(),
        },
    ]);

    assert!(result.is_err());
}

#[test]
fn entity_invocation_claim_identity_ignores_pinned_dispatch_derivations_only() {
    let owner = owner();
    let input = TypedSchemaValue::new(
        crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::tuple(Vec::new())),
        crate::schema::SchemaValue::Tuple {
            elements: Vec::new(),
        },
    );
    let request = EntityInvocationRequest {
        entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
        calling_principal: Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id.clone(),
        }),
        call_mode: EntityCallMode::Asynchronous,
        operation: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
            attempt_ordinal: 7,
            command_path: vec!["files".to_string(), "search".to_string()],
            args: crate::model::card::ToolInvocationPattern::from_command_and_args(
                &[],
                &["--recorded-rendering"],
            )
            .unwrap()
            .args,
            has_stdin: true,
            has_stdout: false,
            declares_stdout: false,
            output_contract: ToolOutputContract {
                result: None,
                errors: Vec::new(),
            },
        }),
        principal: Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id,
        }),
        plan: EntityInvocationPlanReference::Root {
            plan: EntityInvocationPlan::new(vec![EntityInvocationPlanLayer::Tool {
                activation: activation(),
            }])
            .unwrap(),
        },
        assume_idempotence: true,
    };
    let identity = EntityInvocationRequestIdentity {
        entity: request.entity.clone(),
        calling_principal: request.calling_principal.clone(),
        call_mode: request.call_mode,
        operation: (&request.operation).into(),
        plan_position: None,
        input: input.clone(),
    };
    let mut differently_pinned = request.clone();
    let EntityInvocationDescriptor::Tool(descriptor) = &mut differently_pinned.operation;
    descriptor.args =
        crate::model::card::ToolInvocationPattern::from_command_and_args(&[], &["--new-rendering"])
            .unwrap()
            .args;
    descriptor.declares_stdout = true;

    assert!(identity.matches(&differently_pinned, &input));

    let EntityInvocationDescriptor::Tool(descriptor) = &mut differently_pinned.operation;
    descriptor.attempt_ordinal = 8;
    assert!(!identity.matches(&differently_pinned, &input));
    let EntityInvocationDescriptor::Tool(descriptor) = &mut differently_pinned.operation;
    descriptor.attempt_ordinal = 7;

    let EntityInvocationDescriptor::Tool(descriptor) = &mut differently_pinned.operation;
    descriptor.command_path.push("other".to_string());
    assert!(!identity.matches(&differently_pinned, &input));
    let EntityInvocationDescriptor::Tool(descriptor) = &mut differently_pinned.operation;
    descriptor.command_path.pop();
    descriptor.has_stdout = true;
    assert!(!identity.matches(&differently_pinned, &input));

    let different_input = TypedSchemaValue::new(
        crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::tuple(vec![
            crate::schema::SchemaType::bool(),
        ])),
        crate::schema::SchemaValue::Tuple {
            elements: vec![crate::schema::SchemaValue::Bool(true)],
        },
    );
    assert!(!identity.matches(&request, &different_input));

    let mut descendant_request = request.clone();
    descendant_request.plan = EntityInvocationPlanReference::Descendant {
        root_start_index: OplogIndex::from_u64(40),
        position: 2,
    };
    assert!(!identity.matches(&descendant_request, &input));
    let mut descendant_identity = identity.clone();
    descendant_identity.plan_position = Some(EntityInvocationPlanPositionIdentity {
        root_start_index: OplogIndex::from_u64(40),
        position: 2,
    });
    assert!(descendant_identity.matches(&descendant_request, &input));
    descendant_identity
        .plan_position
        .as_mut()
        .unwrap()
        .root_start_index = OplogIndex::from_u64(41);
    assert!(!descendant_identity.matches(&descendant_request, &input));
}

#[test]
fn invocation_scope_protobuf_roundtrip_preserves_activation_fingerprint() {
    let owner = owner();
    let scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner.clone(),
                entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
            },
            OplogIndex::from_u64(84),
        )
        .unwrap(),
        OplogIndex::from_u64(81),
        Arc::new(activation()),
        Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id,
        }),
        InvocationExecutionMode::ReplayingCompleted,
        IdempotencyKey::new("scope-protobuf-seed".to_string()),
        false,
        true,
        IdempotencyKey::new("scope-stream-seed".to_string()),
    )
    .unwrap();

    let protobuf: golem_api_grpc::proto::golem::worker::EntityInvocationScope =
        scope.clone().into();
    let decoded: EntityInvocationScope = protobuf.try_into().unwrap();
    let json = serde_json::to_string(&scope).unwrap();
    let json_decoded: EntityInvocationScope = serde_json::from_str(&json).unwrap();

    assert_eq!(decoded, scope);
    assert_eq!(json_decoded, scope);
    assert_eq!(scope.idempotency_key().value, "scope-protobuf-seed");
    assert!(!scope.assume_idempotence());
    assert!(scope.logical_key_positions());
}

#[test]
fn invocation_scope_protobuf_requires_idempotency_key() {
    let owner = owner();
    let scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner.clone(),
                entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
            },
            OplogIndex::from_u64(84),
        )
        .unwrap(),
        OplogIndex::from_u64(81),
        Arc::new(activation()),
        Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id,
        }),
        InvocationExecutionMode::Live,
        IdempotencyKey::new("required-protobuf-seed".to_string()),
        true,
        false,
        IdempotencyKey::new("required-stream-seed".to_string()),
    )
    .unwrap();
    let mut protobuf: golem_api_grpc::proto::golem::worker::EntityInvocationScope = scope.into();
    protobuf.idempotency_key = None;

    assert_eq!(
        EntityInvocationScope::try_from(protobuf).unwrap_err(),
        "Missing EntityInvocationScope.idempotency_key"
    );
}

#[test]
fn middleware_invocation_scope_roundtrips_through_binary_and_protobuf() {
    let owner = owner();
    let middleware_activation = Arc::new(middleware_activation());
    let scope = EntityInvocationScope::new(
        EntityInvocationId::new(
            OwnedAgentEntityId {
                owner: owner.clone(),
                entity: AgentEntity::ToolMiddleware(ToolMiddlewareName::try_from("audit").unwrap()),
            },
            OplogIndex::from_u64(91),
        )
        .unwrap(),
        OplogIndex::from_u64(84),
        middleware_activation.clone(),
        Principal::Agent(AgentPrincipal {
            agent_id: owner.agent_id.clone(),
        }),
        InvocationExecutionMode::ReplayingIncomplete,
        IdempotencyKey::new("middleware-roundtrip-seed".to_string()),
        false,
        true,
        IdempotencyKey::new("middleware-stream-seed".to_string()),
    )
    .unwrap();
    let request = EntityInvocationRequest {
        entity: scope.invocation_id().entity().clone(),
        calling_principal: scope.calling_principal().clone(),
        call_mode: EntityCallMode::Synchronous,
        operation: EntityInvocationDescriptor::Tool(ToolInvocationDescriptor {
            attempt_ordinal: 1,
            command_path: vec!["test".to_string()],
            args: Vec::new(),
            has_stdin: false,
            has_stdout: false,
            declares_stdout: false,
            output_contract: ToolOutputContract {
                result: None,
                errors: Vec::new(),
            },
        }),
        principal: scope.calling_principal().clone(),
        plan: EntityInvocationPlanReference::Root {
            plan: EntityInvocationPlan::new(vec![
                EntityInvocationPlanLayer::Middleware {
                    activation: middleware_activation.as_ref().clone(),
                    parameters: TypedSchemaValue::new(
                        crate::schema::SchemaGraph::anonymous(crate::schema::SchemaType::tuple(
                            Vec::new(),
                        )),
                        crate::schema::SchemaValue::Tuple {
                            elements: Vec::new(),
                        },
                    ),
                    expected_definition: None,
                    presented_definition: None,
                    next_effective_definition: tool_definition(),
                    compatibility: None,
                },
                EntityInvocationPlanLayer::Tool {
                    activation: activation(),
                },
            ])
            .unwrap(),
        },
        assume_idempotence: false,
    };

    let request_bytes = desert_rust::serialize_to_byte_vec(&request).unwrap();
    assert_eq!(
        desert_rust::deserialize::<EntityInvocationRequest>(&request_bytes).unwrap(),
        request
    );
    let protobuf: golem_api_grpc::proto::golem::worker::EntityInvocationScope =
        scope.clone().into();
    assert_eq!(EntityInvocationScope::try_from(protobuf).unwrap(), scope);
}

#[test]
fn activation_protobuf_rejects_content_that_does_not_match_fingerprint() {
    let activation = activation();
    let mut protobuf: golem_api_grpc::proto::golem::worker::EntityActivation = activation.into();
    protobuf.fingerprint.as_mut().unwrap().value[0] ^= 1;

    let result = EntityActivation::try_from(protobuf);

    assert_eq!(
        result.unwrap_err(),
        "EntityActivation fingerprint does not match its contents"
    );
}

#[test]
fn host_activation_roundtrips_through_binary_json_and_protobuf() {
    let activation = host_activation();
    assert!(activation.executable_opt().is_none());

    let bytes = desert_rust::serialize_to_byte_vec(&activation).unwrap();
    assert_eq!(
        desert_rust::deserialize::<EntityActivation>(&bytes).unwrap(),
        activation
    );

    let json = serde_json::to_string(&activation).unwrap();
    assert_eq!(
        serde_json::from_str::<EntityActivation>(&json).unwrap(),
        activation
    );

    let protobuf: golem_api_grpc::proto::golem::worker::EntityActivation =
        activation.clone().into();
    assert!(matches!(
        protobuf.source,
        Some(golem_api_grpc::proto::golem::worker::entity_activation::Source::Host(_))
    ));
    assert_eq!(EntityActivation::try_from(protobuf).unwrap(), activation);
}

#[test]
fn host_activation_rejects_source_policy_identity_mismatches() {
    let activation = host_activation();
    let EntityActivationPolicy::Tool {
        provision, binding, ..
    } = activation.policy
    else {
        unreachable!()
    };

    for (host_tool_id, implementation_version) in [
        (
            HostToolId::try_from("other-host".to_string()).unwrap(),
            "1.2.3".to_string(),
        ),
        (
            HostToolId::try_from("native-search".to_string()).unwrap(),
            "9.9.9".to_string(),
        ),
    ] {
        let result = EntityActivation::new_host(
            host_tool_id,
            implementation_version,
            activation.deployment_revision,
            EntityActivationPolicy::Tool {
                provision: provision.clone(),
                binding: binding.clone(),
            },
            activation.filesystem,
        );
        assert_eq!(
            result.unwrap_err(),
            "Entity host source does not match the tool binding source"
        );
    }
}

#[test]
fn host_activation_rejects_invalid_source_contracts() {
    let activation = host_activation();
    let EntityActivationPolicy::Tool {
        provision, binding, ..
    } = activation.policy
    else {
        unreachable!()
    };
    let host_tool_id = HostToolId::try_from("native-search".to_string()).unwrap();

    assert_eq!(
        EntityActivation::new_host(
            host_tool_id.clone(),
            "  ".to_string(),
            activation.deployment_revision,
            EntityActivationPolicy::Tool { provision, binding },
            activation.filesystem,
        )
        .unwrap_err(),
        "Entity host source implementation version cannot be empty"
    );

    assert_eq!(
        EntityActivation::new_host(
            host_tool_id,
            "1.2.3".to_string(),
            DeploymentRevision::try_from(12_u64).unwrap(),
            middleware_activation().policy,
            FilesystemCapability::Incapable,
        )
        .unwrap_err(),
        "Host entity activation is not supported for tool middleware"
    );
}

#[test]
fn host_activation_protobuf_rejects_empty_host_tool_id() {
    let mut protobuf: golem_api_grpc::proto::golem::worker::EntityActivation =
        host_activation().into();
    let Some(golem_api_grpc::proto::golem::worker::entity_activation::Source::Host(host)) =
        protobuf.source.as_mut()
    else {
        unreachable!()
    };
    host.host_tool_id.clear();

    assert!(EntityActivation::try_from(protobuf).is_err());
}

#[test]
fn activation_json_rejects_content_that_does_not_match_fingerprint() {
    let mut json = serde_json::to_value(activation()).unwrap();
    json["fingerprint"][0] = serde_json::json!(json["fingerprint"][0].as_u64().unwrap() ^ 1);

    let result = serde_json::from_value::<EntityActivation>(json);

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("fingerprint does not match")
    );
}

#[test]
fn malformed_tool_middleware_name_is_rejected_by_json() {
    let result =
        serde_json::from_str::<AgentEntity>(r#"{"kind":"toolMiddleware","name":"Not Kebab Case"}"#);

    assert!(result.is_err());
}

#[test]
fn zero_entity_invocation_start_index_is_rejected_by_protobuf() {
    let entity_id = OwnedAgentEntityId {
        owner: owner(),
        entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
    };
    let protobuf = golem_api_grpc::proto::golem::worker::EntityInvocationId {
        entity_id: Some(entity_id.into()),
        start_index: 0,
    };

    let result = EntityInvocationId::try_from(protobuf);

    assert_eq!(
        result.unwrap_err(),
        "Entity invocation Start index cannot be zero"
    );
}

#[test]
fn invocation_scope_rejects_parent_that_does_not_precede_invocation_start() {
    let owner = owner();
    let activation = Arc::new(activation());
    let entity_id = OwnedAgentEntityId {
        owner: owner.clone(),
        entity: AgentEntity::Tool(ToolName::try_from("search").unwrap()),
    };
    let principal = Principal::Agent(AgentPrincipal {
        agent_id: owner.agent_id,
    });

    for parent_start_index in [OplogIndex::from_u64(42), OplogIndex::from_u64(43)] {
        let scope = EntityInvocationScope::new(
            EntityInvocationId::new(entity_id.clone(), OplogIndex::from_u64(42)).unwrap(),
            parent_start_index,
            activation.clone(),
            principal.clone(),
            InvocationExecutionMode::Live,
            IdempotencyKey::new("invalid-parent-scope-seed".to_string()),
            false,
            true,
            IdempotencyKey::new("invalid-parent-stream-seed".to_string()),
        );

        assert!(
            scope.is_err(),
            "a durable parent Start must precede its nested entity invocation Start"
        );
    }
}

#[test]
fn missing_owner_component_id_is_rejected_by_protobuf() {
    let protobuf = golem_api_grpc::proto::golem::worker::OwnedAgentEntityId {
        environment_id: Some(EnvironmentId::new().into()),
        owner_agent_id: Some(golem_api_grpc::proto::golem::worker::AgentId {
            component_id: None,
            name: "Example(\"owner\")".to_string(),
        }),
        entity: Some(AgentEntity::Tool(ToolName::try_from("search").unwrap()).into()),
    };

    let result = OwnedAgentEntityId::try_from(protobuf);

    assert_eq!(result.unwrap_err(), "Missing AgentId.component_id");
}

#[test]
fn activation_rejects_executable_that_differs_from_binding_source() {
    let activation = activation();
    let result = EntityActivation::new(
        ExecutableTarget::new(
            ComponentId::new(),
            activation.executable_opt().unwrap().component_revision,
        ),
        activation.deployment_revision,
        activation.policy,
        activation.filesystem,
    );

    assert_eq!(
        result.unwrap_err(),
        "Entity executable does not match the tool binding source"
    );
}
