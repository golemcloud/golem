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
use crate::component_introspection::{ExportedInstance, TopLevelExport};
use crate::model::account::{AccountEmail, AccountId};
use crate::model::agent::AgentTypeName;
use crate::model::agent_secret::CanonicalAgentSecretPath;
use crate::model::card::CardId;
use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
use crate::model::deployment::DeploymentRevision;
use crate::model::json::NormalizedJsonValue;
use crate::model::tool::{
    CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolBindingInput, ToolBindingOwner,
    ToolDeploymentMetadata, ToolDeploymentState, ToolFilesystemAccess, ToolName,
    ToolProvisionConfig, ToolSource,
};
use crate::model::tool_middleware::{
    CompiledToolMiddlewareChain, CompiledToolMiddlewareOccurrence, RegisteredToolMiddleware,
    ToolMiddlewareInstallation, ToolMiddlewareName, ToolMiddlewareSource,
};
use crate::schema::tool::{
    CommandNode, CommandTree, Doc, Globals, Tool, ToolMiddleware, ToolMiddlewareScope,
};
use crate::schema::{SchemaGraph, SchemaValue, TypedSchemaValue};
use std::collections::BTreeSet;
use test_r::test;

fn instance_export(name: &str) -> TopLevelExport {
    TopLevelExport::Instance(ExportedInstance {
        name: name.to_string(),
        functions: vec![],
    })
}

#[test]
fn extract_known_exports_collects_supported_interfaces() {
    let exports = vec![
        instance_export("example:ignored/foo@1.0.0"),
        instance_export("golem:agent/guest@1.5.0"),
        instance_export("golem:api/save-snapshot@1.5.0"),
        instance_export("golem:api/load-snapshot@1.5.0"),
        instance_export("golem:api/oplog-processor@1.5.0"),
        instance_export("golem:tool/guest@0.1.0"),
        instance_export("golem:tool/tool-middleware-guest@0.1.0"),
    ];

    let known = extract_known_exports(&exports).unwrap();

    assert_eq!(
        known,
        KnownExports {
            agent_guest_interface: Some("golem:agent/guest@1.5.0".to_string()),
            save_snapshot_interface: Some("golem:api/save-snapshot@1.5.0".to_string()),
            load_snapshot_interface: Some("golem:api/load-snapshot@1.5.0".to_string()),
            oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".to_string()),
            tool_guest_interface: Some("golem:tool/guest@0.1.0".to_string()),
            tool_middleware_guest_interface: Some(
                "golem:tool/tool-middleware-guest@0.1.0".to_string(),
            ),
        }
    );
}

#[test]
fn extract_known_exports_rejects_duplicate_capabilities() {
    let exports = vec![
        instance_export("golem:api/save-snapshot@1.5.0"),
        instance_export("golem:api/save-snapshot@1.6.0"),
    ];

    let error = extract_known_exports(&exports).unwrap_err();

    assert!(error.reason.contains("Duplicate save snapshot export"));
    assert!(error.reason.contains("golem:api/save-snapshot@1.5.0"));
    assert!(error.reason.contains("golem:api/save-snapshot@1.6.0"));
}

#[test]
fn component_metadata_helpers_use_exact_known_export_names() {
    let metadata = ComponentMetadata::from_parts(
        KnownExports {
            agent_guest_interface: Some("golem:agent/guest@1.5.0".to_string()),
            save_snapshot_interface: Some("golem:api/save-snapshot@1.5.0".to_string()),
            load_snapshot_interface: Some("golem:api/load-snapshot@1.5.0".to_string()),
            oplog_processor_interface: Some("golem:api/oplog-processor@1.5.0".to_string()),
            tool_guest_interface: None,
            tool_middleware_guest_interface: None,
        },
        vec![],
        vec![],
        BTreeMap::new(),
    );

    assert_eq!(
        metadata.agent_initialize_function_name(),
        Some("golem:agent/guest@1.5.0.{initialize}".to_string())
    );
    assert_eq!(
        metadata.agent_invoke_function_name(),
        Some("golem:agent/guest@1.5.0.{invoke}".to_string())
    );
    assert_eq!(
        metadata.save_snapshot_function_name(),
        Some("golem:api/save-snapshot@1.5.0.{save}".to_string())
    );
    assert_eq!(
        metadata.load_snapshot_function_name(),
        Some("golem:api/load-snapshot@1.5.0.{load}".to_string())
    );
    assert_eq!(
        metadata.oplog_processor_function_name(),
        Some("golem:api/oplog-processor@1.5.0.{process}".to_string())
    );
}

#[test]
fn component_metadata_grpc_roundtrip_preserves_agent_initial_permissions() {
    let agent_type = AgentTypeName("Cart".to_string());
    let card_id = CardId::new();
    let card = crate::model::card::PolymorphicCard {
        card_id,
        parent_ids: Vec::new(),
        lower_positive: crate::model::card::default_agent_initial_permission_grants(
            crate::model::card::recipient::RecipientPattern::Any,
        ),
        lower_negative: Vec::new(),
        upper_positive: Vec::new(),
        upper_negative: Vec::new(),
        created_at: chrono::Utc::now(),
        expires_at: None,
        system_card: false,
    };

    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        Vec::new(),
        Vec::new(),
        BTreeMap::from([(
            agent_type.clone(),
            AgentTypeProvisionConfig {
                initial_permissions: card.clone(),
                env: BTreeMap::new(),
                config: Vec::new(),
                plugins: Vec::new(),
                files: Vec::new(),
            },
        )]),
    )
    .with_component_config(
        crate::schema::ComponentConfigSchema::default(),
        ComponentProvisionConfig {
            initial_permissions: card.clone(),
            ..Default::default()
        },
    );

    let proto: golem_api_grpc::proto::golem::component::ComponentMetadata =
        metadata.try_into().unwrap();
    let decoded = ComponentMetadata::try_from(proto).unwrap();

    assert_eq!(
        decoded.agent_type_initial_permission_card(&agent_type),
        Some(&card)
    );
    assert_eq!(
        decoded.component_provision_config().initial_permissions,
        card
    );
}

#[test]
fn component_metadata_grpc_roundtrip_preserves_shared_memory_flag() {
    let metadata = ComponentMetadata::from_parts(
        KnownExports::default(),
        vec![LinearMemory {
            initial: LinearMemory::PAGE_SIZE,
            maximum: Some(2 * LinearMemory::PAGE_SIZE),
            shared: true,
        }],
        Vec::new(),
        BTreeMap::new(),
    );

    let proto: golem_api_grpc::proto::golem::component::ComponentMetadata =
        metadata.try_into().unwrap();
    let decoded = ComponentMetadata::try_from(proto).unwrap();

    assert!(decoded.memories()[0].shared);
}

fn sample_tool() -> Tool {
    Tool {
        requires_filesystem: false,
        version: "1.2.3".to_string(),
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "grep".to_string(),
                aliases: vec!["search".to_string()],
                doc: Doc {
                    summary: "Search files".to_string(),
                    description: "Searches files recursively".to_string(),
                    examples: Vec::new(),
                },
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: None,
            }],
        },
        schema: SchemaGraph::empty(),
    }
}

fn metadata_with_tool() -> ComponentMetadata {
    let tool_name = ToolName::try_from("grep").unwrap();
    let secret_path =
        CanonicalAgentSecretPath(vec!["credentials".to_string(), "github".to_string()]);
    let installation = |name: &str,
                        secret_keys_readable: Option<SecretKeyScope>,
                        secret_keys_revealable: Option<SecretKeyScope>| {
        ToolMiddlewareInstallation {
            name: ToolMiddlewareName::try_from(name).unwrap(),
            version: Some("1.0.0".to_string()),
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            account: None,
            secret_keys_readable,
            secret_keys_revealable,
            filesystem_access: Default::default(),
        }
    };
    let binding = ToolBindingInput {
        version: Some("1.2.3".to_string()),
        parameters: NormalizedJsonValue::new(serde_json::json!({ "root": "/workspace" })),
        account: Some(crate::model::account::AccountEmail::new(
            "owner@example.com",
        )),
        config_keys_readable: crate::model::tool::ConfigKeyScope::All,
        secret_keys_readable: SecretKeyScope::Keys(BTreeSet::from([secret_path.clone()])),
        secret_keys_revealable: SecretKeyScope::Keys(BTreeSet::new()),
        filesystem_access: Default::default(),
        middleware: Some(vec![
            installation("omitted", None, None),
            installation(
                "wildcard",
                Some(SecretKeyScope::All),
                Some(SecretKeyScope::All),
            ),
            installation(
                "empty",
                Some(SecretKeyScope::Keys(BTreeSet::new())),
                Some(SecretKeyScope::Keys(BTreeSet::new())),
            ),
            installation(
                "concrete",
                Some(SecretKeyScope::Keys(BTreeSet::from([secret_path.clone()]))),
                Some(SecretKeyScope::Keys(BTreeSet::from([secret_path]))),
            ),
        ]),
        middleware_merge_mode: Default::default(),
    };
    let component_binding = ToolBindingInput {
        parameters: NormalizedJsonValue::new(serde_json::json!({ "root": "/component" })),
        ..binding.clone()
    };

    ComponentMetadata::from_parts_with_tools(
        KnownExports {
            tool_guest_interface: Some("golem:tool/guest@0.1.0".to_string()),
            ..KnownExports::default()
        },
        Vec::new(),
        Vec::new(),
        BTreeMap::new(),
        BTreeMap::from([(
            tool_name,
            ToolDeploymentMetadata {
                definition: sample_tool(),
                provision: ToolProvisionConfig {
                    config: NormalizedJsonValue::new(serde_json::json!({
                        "logLevel": "debug"
                    })),
                    env: BTreeMap::from([("RUST_LOG".to_string(), "debug".to_string())]),
                    plugins: Vec::new(),
                    files: Vec::new(),
                },
                environment_binding: Some(binding.clone()),
                component_bindings: BTreeMap::from([(
                    ComponentName("CallerComponent".to_string()),
                    component_binding,
                )]),
                agent_bindings: BTreeMap::from([(
                    AgentTypeName("CoderAgent".to_string()),
                    binding,
                )]),
            },
        )]),
    )
}

#[test]
fn component_metadata_grpc_roundtrip_preserves_tool_envelope() {
    let metadata = metadata_with_tool();
    let proto: golem_api_grpc::proto::golem::component::ComponentMetadata =
        metadata.clone().try_into().unwrap();
    let decoded = ComponentMetadata::try_from(proto.clone()).unwrap();

    assert_eq!(decoded, metadata);

    let mut missing_scope = proto;
    missing_scope
        .tools
        .values_mut()
        .next()
        .unwrap()
        .environment_binding
        .as_mut()
        .unwrap()
        .config_keys_readable = None;
    let error = ComponentMetadata::try_from(missing_scope).unwrap_err();
    assert!(error.contains("Missing ToolBindingInput.config_keys_readable"));
}

#[test]
fn component_metadata_serializes_tools_under_tools_field() {
    let json = serde_json::to_value(metadata_with_tool()).unwrap();

    assert!(json.get("tools").is_some());
    assert!(json.get("toolDeploymentMetadata").is_none());
}

#[test]
fn component_metadata_binary_roundtrip_preserves_tool_envelope() {
    let metadata = metadata_with_tool();
    let bytes = crate::serialization::serialize(&metadata).unwrap();
    let decoded: ComponentMetadata = crate::serialization::deserialize(&bytes).unwrap();

    assert_eq!(decoded, metadata);
}

#[test]
fn tool_deployment_state_proto_rejects_registered_tool_from_another_revision() {
    let registered_tool = RegisteredTool {
        deployment_revision: DeploymentRevision::try_from(1_u64).unwrap(),
        release_id: None,
        definition: sample_tool(),
        provision: ToolProvisionConfig::default(),
        component_bindings: BTreeMap::new(),
        source: ToolSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::try_from(1_u64).unwrap(),
            component_name: ComponentName("tools:grep".to_string()),
        },
        owner_account_id: AccountId(uuid::Uuid::new_v4()),
        owner_account_email: AccountEmail::new("owner@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Default::default(),
    };
    let proto = golem_api_grpc::proto::golem::registry::ToolDeploymentState {
        deployment_revision: 2,
        registered_tools: vec![registered_tool.into()],
        tool_bindings: Vec::new(),
        mcp_imports: Vec::new(),
        tool_middleware_configuration: desert_rust::serialize_to_byte_vec(
            &crate::model::tool_middleware::ToolMiddlewareConfiguration::default(),
        )
        .unwrap(),
        registered_tool_middlewares: Vec::new(),
        tool_middleware_chains: Vec::new(),
    };

    let decoded = ToolDeploymentState::try_from(proto);

    assert!(
        decoded.is_err(),
        "a coherent deployment snapshot must reject entries from another revision"
    );
}

#[test]
fn tool_deployment_state_proto_rejects_revealable_secrets_outside_readable_scope() {
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let tool_name = ToolName::try_from("grep").unwrap();
    let agent_type_name = AgentTypeName("Agent".to_string());
    let source = ToolSource::Component {
        component_id: ComponentId::new(),
        component_revision: ComponentRevision::try_from(1_u64).unwrap(),
        component_name: ComponentName("tools:grep".to_string()),
    };
    let owner_account_id = AccountId(uuid::Uuid::new_v4());
    let owner_account_email = AccountEmail::new("owner@example.com");
    let definition = sample_tool();
    let metadata_digest =
        crate::model::tool_release::tool_metadata_digest("0.1.0", &definition).unwrap();
    let registered = RegisteredTool {
        deployment_revision,
        release_id: None,
        definition,
        provision: ToolProvisionConfig::default(),
        component_bindings: BTreeMap::new(),
        source: source.clone(),
        owner_account_id,
        owner_account_email: owner_account_email.clone(),
        metadata_version: "0.1.0".to_string(),
        metadata_digest,
    };
    let binding = CompiledToolBinding {
        deployment_revision,
        release_id: None,
        owner: ToolBindingOwner::AgentType {
            agent_type_name: agent_type_name.clone(),
        },
        tool_name: tool_name.clone(),
        version: registered.definition.version.clone(),
        metadata_version: registered.metadata_version.clone(),
        metadata_digest,
        account_id: owner_account_id,
        account_email: owner_account_email,
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        config_keys_readable: crate::model::tool::ConfigKeyScope::All,
        secret_keys_readable: SecretKeyScope::Keys(BTreeSet::new()),
        secret_keys_revealable: SecretKeyScope::All,
        filesystem_access: crate::model::tool::ToolFilesystemAccess::Unset,
        source,
    };
    let component_id = ComponentId::new();
    let mut component_binding = binding.clone();
    component_binding.owner = ToolBindingOwner::ComponentBaseline { component_id };
    let state = ToolDeploymentState {
        deployment_revision,
        registered_tools: BTreeMap::from([(tool_name.clone(), registered)]),
        tool_bindings: BTreeMap::from([
            (
                ToolBindingOwner::AgentType { agent_type_name },
                BTreeMap::from([(tool_name, binding)]),
            ),
            (
                ToolBindingOwner::ComponentBaseline { component_id },
                BTreeMap::from([(component_binding.tool_name.clone(), component_binding)]),
            ),
        ]),
        mcp_imports: Vec::new(),
        tool_middleware_configuration: Default::default(),
        registered_tool_middlewares: BTreeMap::new(),
        tool_middleware_chains: BTreeMap::new(),
    };
    let proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState = state.into();

    let decoded = ToolDeploymentState::try_from(proto);

    assert!(
        decoded.is_err(),
        "compiled revealable secret keys must remain a subset of readable keys"
    );
}

#[test]
fn tool_deployment_state_proto_roundtrip_preserves_coherent_snapshot() {
    let deployment_revision = DeploymentRevision::try_from(1_u64).unwrap();
    let tool_name = ToolName::try_from("grep").unwrap();
    let agent_type_name = AgentTypeName("Agent".to_string());
    let source = ToolSource::Component {
        component_id: ComponentId::new(),
        component_revision: ComponentRevision::try_from(1_u64).unwrap(),
        component_name: ComponentName("tools:grep".to_string()),
    };
    let owner_account_id = AccountId(uuid::Uuid::new_v4());
    let owner_account_email = AccountEmail::new("owner@example.com");
    let definition = sample_tool();
    let metadata_digest =
        crate::model::tool_release::tool_metadata_digest("0.1.0", &definition).unwrap();
    let registered = RegisteredTool {
        deployment_revision,
        release_id: None,
        definition,
        provision: ToolProvisionConfig::default(),
        component_bindings: BTreeMap::new(),
        source: source.clone(),
        owner_account_id,
        owner_account_email: owner_account_email.clone(),
        metadata_version: "0.1.0".to_string(),
        metadata_digest,
    };
    let binding = CompiledToolBinding {
        deployment_revision,
        release_id: None,
        owner: ToolBindingOwner::AgentType {
            agent_type_name: agent_type_name.clone(),
        },
        tool_name: tool_name.clone(),
        version: registered.definition.version.clone(),
        metadata_version: registered.metadata_version.clone(),
        metadata_digest,
        account_id: owner_account_id,
        account_email: owner_account_email,
        parameters: NormalizedJsonValue::new(serde_json::json!({ "root": "/workspace" })),
        config_keys_readable: crate::model::tool::ConfigKeyScope::All,
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        filesystem_access: crate::model::tool::ToolFilesystemAccess::Allowed,
        source,
    };
    let component_id = ComponentId::new();
    let mut component_binding = binding.clone();
    component_binding.owner = ToolBindingOwner::ComponentBaseline { component_id };
    let state = ToolDeploymentState {
        deployment_revision,
        registered_tools: BTreeMap::from([(tool_name.clone(), registered)]),
        tool_bindings: BTreeMap::from([
            (
                ToolBindingOwner::AgentType { agent_type_name },
                BTreeMap::from([(tool_name, binding)]),
            ),
            (
                ToolBindingOwner::ComponentBaseline { component_id },
                BTreeMap::from([(component_binding.tool_name.clone(), component_binding)]),
            ),
        ]),
        mcp_imports: Vec::new(),
        tool_middleware_configuration: crate::model::tool_middleware::ToolMiddlewareConfiguration {
            compatibility_mode:
                crate::schema::tool::compatibility::ToolCompatibilityMode::StrictEquality,
            environment_bindings: BTreeMap::from([(
                ToolName::try_from("future-mcp-tool").unwrap(),
                crate::model::tool::ToolBindingInput {
                    middleware: Some(Vec::new()),
                    middleware_merge_mode: Some(
                        crate::model::tool_middleware::ToolMiddlewareMergeMode::Replace,
                    ),
                    ..Default::default()
                },
            )]),
            ..Default::default()
        },
        registered_tool_middlewares: BTreeMap::new(),
        tool_middleware_chains: BTreeMap::new(),
    };

    let mut mismatched_binding_proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState =
        state.clone().into();
    mismatched_binding_proto.tool_bindings[0].deployment_revision = 2;
    assert!(
        ToolDeploymentState::try_from(mismatched_binding_proto).is_err(),
        "a coherent deployment snapshot must reject bindings from another revision"
    );

    let mut mismatched_release_proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState =
        state.clone().into();
    mismatched_release_proto.tool_bindings[0].tool_release_id =
        Some(crate::model::tool_release::ToolReleaseId::new().0.into());
    assert!(
        ToolDeploymentState::try_from(mismatched_release_proto).is_err(),
        "a coherent deployment snapshot must reject mismatched release identities"
    );

    let mut mismatched_digest_proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState =
        state.clone().into();
    mismatched_digest_proto.tool_bindings[0].metadata_digest =
        Some(crate::model::diff::Hash::empty().into());
    assert!(
        ToolDeploymentState::try_from(mismatched_digest_proto).is_err(),
        "a coherent deployment snapshot must reject mismatched metadata digests"
    );

    let proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState = state.clone().into();
    assert!(proto.registered_tools[0].source.is_some());
    assert!(proto.registered_tools[0].tagged_source.is_some());
    assert!(proto.tool_bindings[0].source.is_some());
    assert!(proto.tool_bindings[0].tagged_source.is_some());
    let decoded = ToolDeploymentState::try_from(proto.clone()).unwrap();

    assert_eq!(decoded, state);

    let mut middleware_state = state.clone();
    let owner = middleware_state
        .tool_bindings
        .keys()
        .find(|owner| matches!(owner, ToolBindingOwner::AgentType { .. }))
        .unwrap()
        .clone();
    let tool_name = middleware_state.tool_bindings[&owner]
        .keys()
        .next()
        .unwrap()
        .clone();
    let key = CanonicalAgentSecretPath(vec!["allowed".to_string()]);
    let allowed = SecretKeyScope::Keys(BTreeSet::from([key]));
    let binding = middleware_state
        .tool_bindings
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap();
    binding.secret_keys_readable = allowed.clone();
    binding.secret_keys_revealable = allowed.clone();
    let middleware_name = ToolMiddlewareName::try_from("audit").unwrap();
    let middleware_definition = ToolMiddleware {
        name: middleware_name.to_string(),
        version: "1.0.0".to_string(),
        aliases: Vec::new(),
        doc: Doc::default(),
        parameter_schema: SchemaGraph::empty(),
        scope: ToolMiddlewareScope::Universal,
    };
    let middleware = RegisteredToolMiddleware {
        deployment_revision,
        release_id: None,
        metadata_digest: crate::model::tool_middleware_release::tool_middleware_metadata_digest(
            "0.1.0",
            &middleware_definition,
        )
        .unwrap(),
        definition: middleware_definition,
        provision: ToolProvisionConfig::default(),
        source: ToolMiddlewareSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            component_name: ComponentName("middleware:audit".to_string()),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("middleware@example.com"),
        metadata_version: "0.1.0".to_string(),
    };
    middleware_state
        .registered_tool_middlewares
        .insert(middleware_name, middleware.clone());
    let effective_definition = middleware_state.registered_tools[&tool_name]
        .definition
        .clone();
    middleware_state.tool_middleware_chains.insert(
        owner.clone(),
        BTreeMap::from([(
            tool_name.clone(),
            CompiledToolMiddlewareChain {
                deployment_revision,
                owner: owner.clone(),
                tool_name: tool_name.clone(),
                effective_definition: effective_definition.clone(),
                occurrences: vec![CompiledToolMiddlewareOccurrence {
                    middleware,
                    parameters: TypedSchemaValue::new(
                        SchemaGraph::empty(),
                        SchemaValue::Record { fields: Vec::new() },
                    ),
                    provision: ToolProvisionConfig::default(),
                    config_keys_readable: Default::default(),
                    secret_keys_readable: allowed.clone(),
                    secret_keys_revealable: allowed.clone(),
                    filesystem_access: ToolFilesystemAccess::Unset,
                    expected_definition: None,
                    presented_definition: None,
                    next_effective_definition: effective_definition,
                    compatibility: None,
                }],
            },
        )]),
    );
    let valid_middleware_proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState =
        middleware_state.clone().into();
    assert!(ToolDeploymentState::try_from(valid_middleware_proto).is_ok());

    let mut excessive_readable = middleware_state.clone();
    excessive_readable
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences[0]
        .secret_keys_readable = SecretKeyScope::All;
    assert!(
        ToolDeploymentState::try_from(
            golem_api_grpc::proto::golem::registry::ToolDeploymentState::from(excessive_readable)
        )
        .is_err()
    );

    let mut excessive_revealable = middleware_state.clone();
    excessive_revealable
        .tool_bindings
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .secret_keys_revealable = SecretKeyScope::Keys(BTreeSet::new());
    assert!(
        ToolDeploymentState::try_from(
            golem_api_grpc::proto::golem::registry::ToolDeploymentState::from(excessive_revealable)
        )
        .is_err()
    );

    let mut revealable_outside_readable = middleware_state;
    revealable_outside_readable
        .tool_middleware_chains
        .get_mut(&owner)
        .unwrap()
        .get_mut(&tool_name)
        .unwrap()
        .occurrences[0]
        .secret_keys_readable = SecretKeyScope::Keys(BTreeSet::new());
    assert!(
        ToolDeploymentState::try_from(
            golem_api_grpc::proto::golem::registry::ToolDeploymentState::from(
                revealable_outside_readable
            )
        )
        .is_err()
    );

    let mut missing_scope_proto = proto.clone();
    missing_scope_proto.tool_bindings[0].config_keys_readable = None;
    let error = ToolDeploymentState::try_from(missing_scope_proto).unwrap_err();
    assert!(error.contains("missing CompiledToolBinding.config_keys_readable"));

    let mut legacy_proto = proto;
    legacy_proto.registered_tools[0].tagged_source = None;
    legacy_proto.registered_tools[0].metadata_digest = None;
    legacy_proto.tool_bindings[0].tagged_source = None;
    legacy_proto.tool_bindings[0].metadata_digest = None;
    let decoded_legacy = ToolDeploymentState::try_from(legacy_proto).unwrap();

    assert_eq!(decoded_legacy, state);
}

#[test]
fn tool_deployment_state_proto_roundtrip_preserves_host_source() {
    let deployment_revision = DeploymentRevision::INITIAL;
    let tool_name = ToolName::try_from("grep").unwrap();
    let definition = sample_tool();
    let metadata_version = "0.1.0".to_string();
    let state = ToolDeploymentState {
        deployment_revision,
        registered_tools: BTreeMap::from([(
            tool_name,
            RegisteredTool {
                deployment_revision,
                release_id: None,
                metadata_digest: crate::model::tool_release::tool_metadata_digest(
                    &metadata_version,
                    &definition,
                )
                .unwrap(),
                definition,
                provision: ToolProvisionConfig::default(),
                component_bindings: BTreeMap::new(),
                source: ToolSource::Host {
                    host_tool_id: crate::model::tool::HostToolId::try_from(
                        "host-search".to_string(),
                    )
                    .unwrap(),
                    implementation_version: "host-v1".to_string(),
                },
                owner_account_id: AccountId::SYSTEM,
                owner_account_email: AccountEmail::new("system@golem.cloud"),
                metadata_version,
            },
        )]),
        tool_bindings: BTreeMap::new(),
        mcp_imports: vec![crate::model::mcp_import::McpImport {
            url: "http://internal.example/mcp".to_string(),
            auth: Some(crate::model::mcp_import::McpImportAuth {
                kind: crate::model::mcp_import::McpInlineCredentialKind::Bearer,
                credential_digest: crate::model::diff::Hash::new(blake3::hash(b"credential")),
            }),
            security_scheme: None,
            prefix: Some("upstream".to_string()),
            include: Some(Vec::new()),
            exclude: None,
            version: Some(crate::base_model::mcp_import::PROTOCOL_VERSION.to_string()),
        }],
        tool_middleware_configuration: Default::default(),
        registered_tool_middlewares: BTreeMap::new(),
        tool_middleware_chains: BTreeMap::new(),
    };

    let proto: golem_api_grpc::proto::golem::registry::ToolDeploymentState = state.clone().into();
    assert!(proto.registered_tools[0].source.is_none());
    assert!(proto.registered_tools[0].tagged_source.is_some());

    assert_eq!(ToolDeploymentState::try_from(proto).unwrap(), state);
}
