use super::*;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::diff::Hash;
use golem_common::model::tool::{HostToolId, ToolName, ToolSource};
use golem_common::model::tool_release::{
    SystemToolAvailability, ToolReleaseId, ToolReleaseLifecycle, ToolReleaseOrigin,
};
use golem_common::schema::SchemaGraph;
use golem_common::schema::tool::{CommandTree, Tool};
use test_r::test;

#[test]
fn plan_exposes_ambient_identity_metadata_and_defaults() {
    let account_id = AccountId::new();
    let release_id = ToolReleaseId::new();
    let name = ToolName::try_from("native-test").unwrap();
    let source = ToolSource::Host {
        host_tool_id: HostToolId::try_from("native-test".to_string()).unwrap(),
        implementation_version: "1".to_string(),
    };
    let now = chrono::Utc::now();
    let release = ToolRelease {
        id: release_id,
        owner_account_id: account_id,
        name: name.clone(),
        version: "1".to_string(),
        source: source.clone(),
        definition: Tool {
            version: "1".to_string(),
            commands: CommandTree { nodes: vec![] },
            schema: SchemaGraph::empty(),
        },
        metadata_version: "0.1.0".to_string(),
        metadata_digest: Hash::new(blake3::hash(b"metadata")),
        immutable: true,
        lifecycle: ToolReleaseLifecycle::Published,
        origin: ToolReleaseOrigin::ProtectedSystem,
        system_availability: Some(SystemToolAvailability::Ambient),
        created_at: now,
        created_by: account_id,
        state_changed_at: now,
        state_changed_by: account_id,
    };
    let environment_binding = ToolBindingInput::default();
    let owner = golem_common::model::account::AccountSummary {
        id: account_id,
        name: "System".to_string(),
        email: AccountEmail::new("system@golem.cloud"),
    };
    let catalog = NativeToolCatalog::default();
    *catalog.entries.write().unwrap() = vec![AmbientToolDeployment {
        release: release.clone(),
        owner,
        provision: ToolProvisionConfig::default(),
        environment_binding: environment_binding.clone(),
    }];
    let entry = &catalog.plan_entries()[0];
    assert_eq!(entry.release_id, release_id);
    assert_eq!(entry.environment_binding, environment_binding);
}
