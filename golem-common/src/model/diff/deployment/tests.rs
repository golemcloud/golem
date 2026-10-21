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
    Deployment, EffectiveToolBinding, RemoteToolDeployment, ToolMiddlewareBindingInput,
    effective_tool_binding, remote_tool_deployments, remote_tool_middleware_deployments,
};
use crate::model::account::{AccountEmail, AccountId};
use crate::model::agent::AgentTypeName;
use crate::model::agent_secret::CanonicalAgentSecretPath;
use crate::model::component::{ComponentId, ComponentName, ComponentRevision};
use crate::model::deployment::DeploymentRevision;
use crate::model::diff::{Hash, Hashable};
use crate::model::json::NormalizedJsonValue;
use crate::model::mcp_import::{McpImport, McpImportAuth, McpInlineCredentialKind};
use crate::model::security_scheme::SecuritySchemeName;
use crate::model::tool::{
    ConfigKeyScope, HostToolId, RegisteredTool, SecretKeyScope, ToolBindingInput,
    ToolFilesystemAccess, ToolName, ToolProvisionConfig, ToolSource,
};
use crate::model::tool_middleware::{
    RegisteredToolMiddleware, ToolMiddlewareMergeMode, ToolMiddlewareSource,
};
use crate::model::tool_middleware_release::ToolMiddlewareReleaseId;
use crate::model::tool_release::ToolReleaseId;
use crate::schema::SchemaGraph;
use crate::schema::tool::{
    CommandNode, CommandTree, Doc, Globals, Tool, ToolMiddleware, ToolMiddlewareScope,
};
use std::collections::{BTreeMap, BTreeSet};
use test_r::test;

fn remote_tool() -> RemoteToolDeployment {
    RemoteToolDeployment {
        release_id: ToolReleaseId::new(),
        version: "1.0.0".to_string(),
        source_digest: crate::model::tool_release::tool_source_digest(&ToolSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            component_name: ComponentName("publisher-tools".to_string()),
        }),
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("publisher@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Hash::new(blake3::hash(b"metadata-a")),
        provision: ToolProvisionConfig::default(),
        component_bindings: BTreeMap::new(),
        bindings: BTreeMap::new(),
        environment_middleware_binding: None,
        component_middleware_bindings: BTreeMap::new(),
        agent_middleware_bindings: BTreeMap::new(),
    }
}

fn deployment_hash(tool: RemoteToolDeployment, published: bool) -> Hash {
    Deployment {
        remote_tools: BTreeMap::from([("grep".to_string(), tool.into())]),
        published_tools: if published {
            BTreeSet::from(["local-tool".to_string()])
        } else {
            BTreeSet::new()
        },
        ..Deployment::default()
    }
    .hash()
    .unwrap()
}

#[test]
fn remote_tool_middleware_binding_changes_deployment_hash() {
    let agent = AgentTypeName("Agent".to_string());
    let mut first = remote_tool();
    first.agent_middleware_bindings.insert(
        agent.clone(),
        ToolMiddlewareBindingInput {
            config_keys_readable: ConfigKeyScope::default(),
            secret_keys_readable: SecretKeyScope::default(),
            secret_keys_revealable: SecretKeyScope::default(),
            middleware: None,
            middleware_merge_mode: Some(ToolMiddlewareMergeMode::Append),
        },
    );
    let mut changed = first.clone();
    changed
        .agent_middleware_bindings
        .get_mut(&agent)
        .unwrap()
        .middleware_merge_mode = Some(ToolMiddlewareMergeMode::Replace);

    assert_ne!(
        deployment_hash(first, false),
        deployment_hash(changed, false)
    );
}

fn mcp_import() -> McpImport {
    McpImport {
        url: "http://internal.example/mcp".to_string(),
        auth: Some(McpImportAuth {
            kind: McpInlineCredentialKind::Bearer,
            credential_digest: Hash::new(blake3::hash(b"credential-a")),
        }),
        security_scheme: None,
        prefix: Some("upstream".to_string()),
        include: Some(vec!["read-*".to_string()]),
        exclude: None,
        version: Some(crate::base_model::mcp_import::PROTOCOL_VERSION.to_string()),
    }
}

#[test]
fn mcp_import_hash_covers_order_credentials_auth_scheme_filters_prefix_and_protocol() {
    let first = mcp_import();
    let second = McpImport {
        url: "https://other.example/mcp".to_string(),
        ..first.clone()
    };
    let ordered = |a: McpImport, b: McpImport| {
        Deployment {
            mcp_imports: BTreeMap::from([("0".to_string(), a.into()), ("1".to_string(), b.into())]),
            ..Deployment::default()
        }
        .hash()
        .unwrap()
    };
    assert_ne!(
        ordered(first.clone(), second.clone()),
        ordered(second, first.clone())
    );

    for changed in [
        McpImport {
            auth: Some(McpImportAuth {
                kind: McpInlineCredentialKind::Bearer,
                credential_digest: Hash::new(blake3::hash(b"credential-b")),
            }),
            ..first.clone()
        },
        McpImport {
            auth: Some(McpImportAuth {
                kind: McpInlineCredentialKind::Basic,
                credential_digest: Hash::new(blake3::hash(b"credential-a")),
            }),
            ..first.clone()
        },
        McpImport {
            auth: None,
            security_scheme: Some(SecuritySchemeName("oauth".to_string())),
            ..first.clone()
        },
        McpImport {
            include: None,
            exclude: Some(vec!["write-*".to_string()]),
            ..first.clone()
        },
        McpImport {
            prefix: Some("other".to_string()),
            ..first.clone()
        },
        McpImport {
            version: Some("2026-01-01".to_string()),
            ..first.clone()
        },
        McpImport {
            url: "https://internal.example/mcp".to_string(),
            ..first.clone()
        },
    ] {
        assert_ne!(first.hash().unwrap(), changed.hash().unwrap());
    }
}
fn registered_middleware(
    deployment_revision: DeploymentRevision,
    release_id: ToolMiddlewareReleaseId,
) -> RegisteredToolMiddleware {
    RegisteredToolMiddleware {
        deployment_revision,
        release_id: Some(release_id),
        definition: ToolMiddleware {
            name: "audit".to_string(),
            version: "1.0.0".to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            scope: ToolMiddlewareScope::Universal,
            parameter_schema: SchemaGraph::empty(),
        },
        provision: ToolProvisionConfig::default(),
        source: ToolMiddlewareSource::Component {
            component_id: ComponentId(uuid::Uuid::from_u128(1)),
            component_revision: ComponentRevision::INITIAL,
            component_name: ComponentName("middleware".to_string()),
        },
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("owner@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Hash::new(blake3::hash(b"metadata")),
    }
}

#[test]
fn middleware_registration_hash_ignores_deployment_revision_and_local_publication_id() {
    let release_a = ToolMiddlewareReleaseId::new();
    let release_b = ToolMiddlewareReleaseId::new();
    let first = registered_middleware(DeploymentRevision::INITIAL, release_a);
    let mut later = first.clone();
    later.deployment_revision = DeploymentRevision::try_from(42_u64).unwrap();
    let remote_a = remote_tool_middleware_deployments([first], &BTreeSet::new()).unwrap();
    let remote_b = remote_tool_middleware_deployments([later], &BTreeSet::new()).unwrap();
    assert_eq!(
        remote_a["audit"].hash().unwrap(),
        remote_b["audit"].hash().unwrap()
    );

    let published = BTreeSet::from(["audit".to_string()]);
    let local_a = remote_tool_middleware_deployments(
        [registered_middleware(
            DeploymentRevision::INITIAL,
            release_a,
        )],
        &published,
    )
    .unwrap();
    let local_b = remote_tool_middleware_deployments(
        [registered_middleware(
            DeploymentRevision::INITIAL,
            release_b,
        )],
        &published,
    )
    .unwrap();
    assert!(local_a.is_empty());
    assert!(local_b.is_empty());
}

#[test]
fn remote_middleware_metadata_changes_hash() {
    let release = ToolMiddlewareReleaseId::new();
    let first = registered_middleware(DeploymentRevision::INITIAL, release);
    let mut changed = first.clone();
    changed.metadata_digest = Hash::new(blake3::hash(b"changed metadata"));
    let first = remote_tool_middleware_deployments([first], &BTreeSet::new()).unwrap();
    let changed = remote_tool_middleware_deployments([changed], &BTreeSet::new()).unwrap();
    assert_ne!(
        first["audit"].hash().unwrap(),
        changed["audit"].hash().unwrap()
    );
}

#[test]
fn middleware_snapshot_changes_deployment_hash() {
    let base = Deployment::default().hash().unwrap();
    let with_middleware = Deployment {
        universal_tool_middlewares: vec![
            crate::model::tool_middleware::ToolMiddlewareInstallation {
                name: "audit".try_into().unwrap(),
                version: Some("1.0.0".to_string()),
                parameters: NormalizedJsonValue::new(serde_json::json!({})),
                account: None,
                secret_keys_readable: None,
                secret_keys_revealable: None,
                filesystem_access: ToolFilesystemAccess::Denied,
            },
        ],
        ..Deployment::default()
    }
    .hash()
    .unwrap();

    assert_ne!(base, with_middleware);
}

#[test]
fn middleware_binding_projection_hash_retains_explicit_default_bindings() {
    let agent = AgentTypeName("Agent".to_string());
    let tool = ToolName::try_from("grep").unwrap();
    let empty_tool = ToolName::try_from("empty").unwrap();
    let environment = BTreeMap::from([(empty_tool.clone(), ToolBindingInput::default())]);
    let mut agents = BTreeMap::from([(
        agent,
        BTreeMap::from([
            (
                tool,
                ToolBindingInput {
                    middleware_merge_mode: Some(ToolMiddlewareMergeMode::Append),
                    ..Default::default()
                },
            ),
            (empty_tool, ToolBindingInput::default()),
        ]),
    )]);
    agents.insert(AgentTypeName("Unbound".to_string()), BTreeMap::new());

    let (environment_bindings, agent_bindings) =
        super::tool_middleware_binding_inputs(&environment, &agents);
    assert_eq!(environment_bindings.len(), 1);
    assert_eq!(agent_bindings.len(), 1);
    assert_eq!(agent_bindings["Agent"].len(), 2);
    assert_eq!(
        agent_bindings["Agent"]["grep"].middleware_merge_mode,
        Some(ToolMiddlewareMergeMode::Append)
    );

    let projected = Deployment {
        environment_tool_middleware_bindings: environment_bindings.clone(),
        agent_tool_middleware_bindings: agent_bindings,
        ..Deployment::default()
    };
    assert_ne!(
        projected.hash().unwrap(),
        Deployment::default().hash().unwrap()
    );

    let environment_only = Deployment {
        environment_tool_middleware_bindings: environment_bindings,
        ..Deployment::default()
    };
    let agent_only = Deployment {
        agent_tool_middleware_bindings: BTreeMap::from([(
            "Agent".to_string(),
            BTreeMap::from([(
                "empty".to_string(),
                super::ToolMiddlewareBindingInput::from(&ToolBindingInput::default()),
            )]),
        )]),
        ..Deployment::default()
    };
    let absent = Deployment::default().hash().unwrap();
    assert_ne!(environment_only.hash().unwrap(), absent);
    assert_ne!(agent_only.hash().unwrap(), absent);
}

#[test]
fn middleware_binding_authority_scopes_each_change_deployment_hash() {
    use crate::model::agent_config::CanonicalAgentConfigPath;
    use crate::model::agent_secret::CanonicalAgentSecretPath;

    let tool = ToolName::try_from("grep").unwrap();
    let hash = |binding: ToolBindingInput| {
        let (environment_tool_middleware_bindings, _) = super::tool_middleware_binding_inputs(
            &BTreeMap::from([(tool.clone(), binding)]),
            &BTreeMap::new(),
        );
        Deployment {
            environment_tool_middleware_bindings,
            ..Deployment::default()
        }
        .hash()
        .unwrap()
    };
    let default_hash = Deployment::default().hash().unwrap();

    for binding in [
        ToolBindingInput {
            config_keys_readable: ConfigKeyScope::Keys(BTreeSet::from([CanonicalAgentConfigPath(
                vec!["config".to_string()],
            )])),
            ..Default::default()
        },
        ToolBindingInput {
            secret_keys_readable: SecretKeyScope::Keys(BTreeSet::from([CanonicalAgentSecretPath(
                vec!["readable".to_string()],
            )])),
            ..Default::default()
        },
        ToolBindingInput {
            secret_keys_revealable: SecretKeyScope::Keys(BTreeSet::from([
                CanonicalAgentSecretPath(vec!["revealable".to_string()]),
            ])),
            ..Default::default()
        },
    ] {
        assert_ne!(hash(binding), default_hash);
    }
}

#[test]
fn middleware_filesystem_policy_changes_deployment_hash() {
    let installation =
        |filesystem_access| crate::model::tool_middleware::ToolMiddlewareInstallation {
            name: "audit".try_into().unwrap(),
            version: None,
            parameters: NormalizedJsonValue::new(serde_json::json!({})),
            account: None,
            secret_keys_readable: None,
            secret_keys_revealable: None,
            filesystem_access,
        };
    let hash = |filesystem_access| {
        Deployment {
            universal_tool_middlewares: vec![installation(filesystem_access)],
            ..Deployment::default()
        }
        .hash()
        .unwrap()
    };

    assert_ne!(
        hash(ToolFilesystemAccess::Allowed),
        hash(ToolFilesystemAccess::Denied)
    );
}

#[test]
fn changing_one_duplicate_middleware_secret_selector_changes_deployment_hash() {
    let installation = || crate::model::tool_middleware::ToolMiddlewareInstallation {
        name: "audit".try_into().unwrap(),
        version: Some("1.0.0".to_string()),
        parameters: NormalizedJsonValue::new(serde_json::json!({})),
        account: None,
        secret_keys_readable: None,
        secret_keys_revealable: None,
        filesystem_access: ToolFilesystemAccess::Denied,
    };
    let original = Deployment {
        universal_tool_middlewares: vec![installation(), installation()],
        ..Deployment::default()
    };
    let mut changed = original.clone();
    changed.universal_tool_middlewares[0].secret_keys_readable =
        Some(SecretKeyScope::Keys(BTreeSet::from([
            CanonicalAgentSecretPath(vec!["token".to_string()]),
        ])));

    assert_eq!(
        changed.universal_tool_middlewares[1], original.universal_tool_middlewares[1],
        "same-named occurrences must retain independent selector state"
    );
    assert_ne!(original.hash().unwrap(), changed.hash().unwrap());
}

fn registered_tool(
    name: &str,
    source: ToolSource,
    release_id: Option<ToolReleaseId>,
) -> RegisteredTool {
    RegisteredTool {
        deployment_revision: DeploymentRevision::INITIAL,
        release_id,
        definition: Tool {
            requires_filesystem: false,
            version: "1.0.0".to_string(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: name.to_string(),
                    aliases: Vec::new(),
                    doc: Doc::default(),
                    globals: Globals::default(),
                    subcommands: Vec::new(),
                    body: None,
                }],
            },
            schema: SchemaGraph::empty(),
        },
        provision: ToolProvisionConfig::default(),
        component_bindings: BTreeMap::new(),
        source,
        owner_account_id: AccountId::new(),
        owner_account_email: AccountEmail::new("owner@example.com"),
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Hash::new(blake3::hash(name.as_bytes())),
    }
}

#[test]
fn remote_tool_classification_uses_release_and_publication_identity() {
    let local = registered_tool(
        "local",
        ToolSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            component_name: ComponentName("local-component".to_string()),
        },
        None,
    );
    let published_release_id = ToolReleaseId::new();
    let published = registered_tool(
        "published",
        ToolSource::Component {
            component_id: ComponentId::new(),
            component_revision: ComponentRevision::INITIAL,
            component_name: ComponentName("publisher".to_string()),
        },
        Some(published_release_id),
    );
    let remote_release_id = ToolReleaseId::new();
    let remote = registered_tool(
        "remote",
        ToolSource::Host {
            host_tool_id: HostToolId::try_from("remote-host".to_string()).unwrap(),
            implementation_version: "1".to_string(),
        },
        Some(remote_release_id),
    );

    let classified = remote_tool_deployments(
        [local, published, remote],
        Vec::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeSet::from(["published".to_string()]),
    )
    .unwrap();

    assert_eq!(
        classified.keys().map(String::as_str).collect::<Vec<_>>(),
        ["remote"]
    );
    assert_eq!(
        classified["remote"].as_value().unwrap().release_id,
        remote_release_id
    );
}

#[test]
fn remote_host_tool_without_release_identity_is_rejected() {
    let tool = registered_tool(
        "remote",
        ToolSource::Host {
            host_tool_id: HostToolId::try_from("remote-host".to_string()).unwrap(),
            implementation_version: "1".to_string(),
        },
        None,
    );

    let error = remote_tool_deployments(
        [tool],
        Vec::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeSet::new(),
    )
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("non-local tool remote has no release id")
    );
}

#[test]
fn remote_tool_identity_hash_covers_release_source_metadata_provision_and_bindings() {
    let base = remote_tool();
    let base_hash = deployment_hash(base.clone(), false);

    let mut changed_release = base.clone();
    changed_release.release_id = ToolReleaseId::new();
    assert_ne!(base_hash, deployment_hash(changed_release, false));

    let mut changed_version = base.clone();
    changed_version.version = "2".to_string();
    assert_ne!(base_hash, deployment_hash(changed_version, false));

    let mut changed_source = base.clone();
    changed_source.source_digest =
        crate::model::tool_release::tool_source_digest(&ToolSource::Host {
            host_tool_id: HostToolId::try_from("native-grep".to_string()).unwrap(),
            implementation_version: "2026.08".to_string(),
        });
    assert_ne!(base_hash, deployment_hash(changed_source, false));

    let mut changed_metadata = base.clone();
    changed_metadata.metadata_digest = Hash::new(blake3::hash(b"metadata-b"));
    assert_ne!(base_hash, deployment_hash(changed_metadata, false));

    let mut changed_provision = base.clone();
    changed_provision.provision.config =
        NormalizedJsonValue::new(serde_json::json!({ "consumer": true }));
    assert_ne!(base_hash, deployment_hash(changed_provision, false));

    let mut changed_component_baseline = base.clone();
    changed_component_baseline.component_bindings.insert(
        "consumer".to_string(),
        EffectiveToolBinding {
            parameters: NormalizedJsonValue::new(serde_json::json!({ "limit": 5 })),
            config_keys_readable: ConfigKeyScope::All,
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: ToolFilesystemAccess::Unset,
        },
    );
    assert!(changed_component_baseline.bindings.is_empty());
    assert_ne!(
        base_hash,
        deployment_hash(changed_component_baseline, false)
    );

    let mut changed_binding = base.clone();
    changed_binding.bindings.insert(
        AgentTypeName("Agent".to_string()),
        EffectiveToolBinding {
            parameters: NormalizedJsonValue::new(serde_json::json!({ "limit": 5 })),
            config_keys_readable: ConfigKeyScope::All,
            secret_keys_readable: SecretKeyScope::All,
            secret_keys_revealable: SecretKeyScope::All,
            filesystem_access: ToolFilesystemAccess::Unset,
        },
    );
    assert_ne!(base_hash, deployment_hash(changed_binding, false));

    assert_eq!(base_hash, deployment_hash(base, true));
}

#[test]
fn ambient_override_equal_to_environment_default_has_same_effective_binding() {
    let environment = ToolBindingInput {
        version: None,
        parameters: NormalizedJsonValue::new(serde_json::json!({ "limit": 5 })),
        account: None,
        config_keys_readable: ConfigKeyScope::All,
        secret_keys_readable: SecretKeyScope::All,
        secret_keys_revealable: SecretKeyScope::All,
        ..ToolBindingInput::default()
    };

    assert_eq!(
        effective_tool_binding(Some(&environment), None),
        effective_tool_binding(Some(&environment), Some(&environment))
    );
}
