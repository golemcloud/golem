use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::account::AccountId;
use golem_common::model::agent::extraction::extract_component_metadata_from_bytes;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::environment::EnvironmentName;
use golem_common::model::tool::ToolSource;
use golem_common::model::tool_release::ToolRelease;
use golem_registry_service::bootstrap::Services;
use golem_registry_service::config::{
    ComponentCompilationConfig, LoginConfig, RegistryServiceConfig,
};
use golem_registry_service::services::builtin_tool_provisioner::{
    BuiltinToolDescriptor, provision_descriptors,
};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::model::auth::AuthCtx;
use std::collections::BTreeMap;
use std::sync::Arc;
use test_r::{test, timeout};
use tokio::task::JoinSet;

pub mod native;

#[test]
async fn filesystem_artifact_exports_five_closed_world_tools_requiring_filesystem_access() {
    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../builtin-tools/filesystem-tools.wasm"
    ))
    .expect("build the filesystem tool component before running this test");
    let metadata = extract_component_metadata_from_bytes(&wasm, true, true)
        .await
        .unwrap();
    let expected = ["read-file", "write-file", "edit-file", "ls", "grep"];

    assert_eq!(metadata.tools.len(), expected.len());
    for name in expected {
        let tool = metadata
            .tools
            .iter()
            .find(|tool| tool.name() == Some(name))
            .unwrap_or_else(|| panic!("missing filesystem tool '{name}'"));
        assert_eq!(tool.version, "0.1.0", "{name}");
        assert!(tool.requires_filesystem, "{name}");
        assert!(
            tool.commands
                .nodes
                .iter()
                .filter_map(|node| node.body.as_ref())
                .all(|body| body
                    .annotations
                    .as_ref()
                    .is_some_and(|annotations| !annotations.open_world)),
            "{name}"
        );
    }
}

#[test]
#[timeout("120s")]
async fn provisions_component_tool_release_idempotently_and_rejects_mismatch_without_mutation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = RegistryServiceConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: temp_dir
                .path()
                .join("registry.db")
                .to_string_lossy()
                .into_owned(),
            max_connections: 4,
            foreign_keys: true,
        }),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    let owner = config.initial_accounts["builtin_tool_owner"].id;
    let mut join_set = JoinSet::new();
    let services = Services::new_without_component_builtins(&config, &mut join_set)
        .await
        .unwrap();
    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-components/",
        "golem_it_tool_streaming_rust_provider_release.wasm"
    ))
    .expect("build the tool-streaming test component before running this test");
    let artifacts = BTreeMap::from([("streaming", Arc::new(wasm))]);
    let descriptor = BuiltinToolDescriptor {
        component_name: "builtin-tool-streaming-test",
        artifact_id: "streaming",
        tool_name: "streaming",
        release_version: "1.0.0",
    };
    let auth = AuthCtx::system();

    let descriptors = std::slice::from_ref(&descriptor);
    tokio::join!(
        provision(&services, owner, descriptors, &artifacts),
        provision(&services, owner, descriptors, &artifacts)
    );
    let app = services
        .application_service
        .get_in_account(owner, &ApplicationName("golem-system".into()), &auth)
        .await
        .unwrap();
    let env = services
        .environment_service
        .get_in_application(app.id, &EnvironmentName("builtin-tools".into()), &auth)
        .await
        .unwrap();
    let first_component = services
        .component_service
        .get_staged_component_by_name(
            env.id,
            &ComponentName(descriptor.component_name.into()),
            &auth,
        )
        .await
        .unwrap();
    let first_release = release_named(&services, owner, descriptor.tool_name).await;
    let first_deployments = services
        .deployment_service
        .list_deployments(env.id, None, &auth)
        .await
        .unwrap();
    assert_eq!(first_component.revision, ComponentRevision::INITIAL);
    assert_eq!(first_deployments.len(), 1);

    provision(
        &services,
        owner,
        std::slice::from_ref(&descriptor),
        &artifacts,
    )
    .await;
    let repeated_component = services
        .component_service
        .get_staged_component_by_name(
            env.id,
            &ComponentName(descriptor.component_name.into()),
            &auth,
        )
        .await
        .unwrap();
    let repeated_release = release_named(&services, owner, descriptor.tool_name).await;
    assert_eq!(repeated_component.id, first_component.id);
    assert_eq!(repeated_component.revision, first_component.revision);
    assert_eq!(repeated_release.id, first_release.id);
    assert_eq!(
        services
            .deployment_service
            .list_deployments(env.id, None, &auth)
            .await
            .unwrap()
            .len(),
        1
    );

    let mismatch = BuiltinToolDescriptor {
        component_name: "different-builtin-tool-streaming-test",
        ..descriptor
    };
    let error = provision_descriptors(
        std::slice::from_ref(&mismatch),
        &artifacts,
        owner,
        &services.auth_service,
        &services.application_service,
        &services.environment_service,
        &services.component_service,
        &services.component_write_service,
        &services.deployment_service,
        &services.deployment_write_service,
        &services.tool_release_service,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("immutable"), "{error:#}");
    assert_eq!(
        release_named(&services, owner, descriptor.tool_name)
            .await
            .id,
        first_release.id
    );
    assert_eq!(
        services
            .component_service
            .list_staged_components_for_environment(&env, &auth)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        services
            .deployment_service
            .list_deployments(env.id, None, &auth)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[test]
#[timeout("120s")]
async fn changed_component_creates_a_revision_without_repointing_the_old_release() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = RegistryServiceConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: temp_dir
                .path()
                .join("registry.db")
                .to_string_lossy()
                .into_owned(),
            max_connections: 4,
            foreign_keys: true,
        }),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    let owner = config.initial_accounts["builtin_tool_owner"].id;
    let mut join_set = JoinSet::new();
    let services = Services::new_without_component_builtins(&config, &mut join_set)
        .await
        .unwrap();
    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../builtin-tools/filesystem-tools.wasm"
    ))
    .expect("build the filesystem tool component before running this test");
    let mut first_wasm = wasm;
    let replacements = replace_embedded_tool_versions(&mut first_wasm, b"0.1.0", b"7.2.0");
    assert_eq!(replacements, 5);
    let mut changed_wasm = first_wasm.clone();
    assert_eq!(
        replace_embedded_tool_versions(&mut changed_wasm, b"7.2.0", b"7.3.0"),
        5
    );
    let artifacts = BTreeMap::from([
        ("filesystem_tools_first", Arc::new(first_wasm)),
        ("filesystem_tools_second", Arc::new(changed_wasm)),
    ]);
    let first = BuiltinToolDescriptor {
        component_name: "filesystem-tools",
        artifact_id: "filesystem_tools_first",
        tool_name: "read-file",
        release_version: "7.2.0",
    };
    let second = BuiltinToolDescriptor {
        component_name: first.component_name,
        artifact_id: "filesystem_tools_second",
        tool_name: first.tool_name,
        release_version: "7.3.0",
    };

    provision(&services, owner, std::slice::from_ref(&first), &artifacts).await;
    let old_release =
        release_coordinate(&services, owner, first.tool_name, first.release_version).await;
    let (component_id, old_revision) = component_source(&old_release);

    provision(&services, owner, std::slice::from_ref(&second), &artifacts).await;
    let new_release =
        release_coordinate(&services, owner, second.tool_name, second.release_version).await;
    let (new_component_id, new_revision) = component_source(&new_release);
    assert_eq!(new_component_id, component_id);
    assert_eq!(new_revision, old_revision.next().unwrap());

    let old_release_after_upgrade =
        release_coordinate(&services, owner, first.tool_name, first.release_version).await;
    assert_eq!(old_release_after_upgrade.id, old_release.id);
    assert_eq!(
        component_source(&old_release_after_upgrade),
        (component_id, old_revision)
    );

    provision(&services, owner, std::slice::from_ref(&second), &artifacts).await;
    let replayed =
        release_coordinate(&services, owner, second.tool_name, second.release_version).await;
    assert_eq!(replayed.id, new_release.id);
    assert_eq!(component_source(&replayed), (component_id, new_revision));
}

#[test]
#[timeout("120s")]
async fn same_artifact_adds_missing_tool_with_complete_metadata_and_is_retry_safe() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = RegistryServiceConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: temp_dir
                .path()
                .join("registry.db")
                .to_string_lossy()
                .into_owned(),
            max_connections: 4,
            foreign_keys: true,
        }),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    let owner = config.initial_accounts["builtin_tool_owner"].id;
    let mut join_set = JoinSet::new();
    let services = Services::new_without_component_builtins(&config, &mut join_set)
        .await
        .unwrap();
    let mut wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../builtin-tools/filesystem-tools.wasm"
    ))
    .expect("build the filesystem tool component before running this test");
    let replacements = replace_embedded_tool_versions(&mut wasm, b"0.1.0", b"8.2.0");
    assert_eq!(replacements, 5);
    let artifacts = BTreeMap::from([("filesystem_tools", Arc::new(wasm))]);
    let first = BuiltinToolDescriptor {
        component_name: "filesystem-tools",
        artifact_id: "filesystem_tools",
        tool_name: "read-file",
        release_version: "8.2.0",
    };
    let second = BuiltinToolDescriptor {
        component_name: first.component_name,
        artifact_id: first.artifact_id,
        tool_name: "write-file",
        release_version: first.release_version,
    };

    provision(&services, owner, std::slice::from_ref(&first), &artifacts).await;
    let old_release =
        release_coordinate(&services, owner, first.tool_name, first.release_version).await;
    let (component_id, old_revision) = component_source(&old_release);

    let complete = [first, second];
    tokio::join!(
        provision(&services, owner, &complete, &artifacts),
        provision(&services, owner, &complete, &artifacts)
    );
    let new_release = release_coordinate(
        &services,
        owner,
        complete[1].tool_name,
        complete[1].release_version,
    )
    .await;
    let (new_component_id, new_revision) = component_source(&new_release);
    assert_eq!(new_component_id, component_id);
    assert_eq!(new_revision, old_revision.next().unwrap());

    let old_release_after_retry = release_coordinate(
        &services,
        owner,
        complete[0].tool_name,
        complete[0].release_version,
    )
    .await;
    assert_eq!(old_release_after_retry.id, old_release.id);
    assert_eq!(
        component_source(&old_release_after_retry),
        (component_id, old_revision)
    );

    provision(&services, owner, &complete, &artifacts).await;
    let replayed = release_coordinate(
        &services,
        owner,
        complete[1].tool_name,
        complete[1].release_version,
    )
    .await;
    assert_eq!(replayed.id, new_release.id);
    assert_eq!(component_source(&replayed), (component_id, new_revision));
}

async fn provision(
    services: &Services,
    owner: AccountId,
    descriptors: &[BuiltinToolDescriptor],
    artifacts: &BTreeMap<&str, Arc<Vec<u8>>>,
) {
    provision_descriptors(
        descriptors,
        artifacts,
        owner,
        &services.auth_service,
        &services.application_service,
        &services.environment_service,
        &services.component_service,
        &services.component_write_service,
        &services.deployment_service,
        &services.deployment_write_service,
        &services.tool_release_service,
    )
    .await
    .unwrap();
}

fn replace_embedded_tool_versions(wasm: &mut [u8], from: &[u8; 5], to: &[u8; 5]) -> usize {
    let (start, end) = first_core_module_data_section(wasm);
    assert!(end - start >= from.len());
    let mut replacements = 0;
    for offset in start..=end - from.len() {
        if &wasm[offset..offset + from.len()] == from {
            wasm[offset..offset + to.len()].copy_from_slice(to);
            replacements += 1;
        }
    }
    replacements
}

fn first_core_module_data_section(wasm: &[u8]) -> (usize, usize) {
    assert_eq!(&wasm[..8], b"\0asm\r\0\x01\0");
    let mut cursor = 8;
    while cursor < wasm.len() {
        let section_id = wasm[cursor];
        cursor += 1;
        let section_size = read_unsigned_leb128(wasm, &mut cursor);
        let section_end = cursor + section_size;
        assert!(section_end <= wasm.len());
        if section_id == 1 {
            assert_eq!(&wasm[cursor..cursor + 8], b"\0asm\x01\0\0\0");
            let mut module_cursor = cursor + 8;
            while module_cursor < section_end {
                let module_section_id = wasm[module_cursor];
                module_cursor += 1;
                let module_section_size = read_unsigned_leb128(wasm, &mut module_cursor);
                let module_section_end = module_cursor + module_section_size;
                assert!(module_section_end <= section_end);
                if module_section_id == 11 {
                    return (module_cursor, module_section_end);
                }
                module_cursor = module_section_end;
            }
            panic!("first core module has no data section");
        }
        cursor = section_end;
    }
    panic!("component has no core module");
}

fn read_unsigned_leb128(bytes: &[u8], cursor: &mut usize) -> usize {
    let mut value = 0usize;
    let mut shift = 0;
    loop {
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= usize::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
        assert!(shift < usize::BITS);
    }
}

async fn release_named(services: &Services, owner: AccountId, name: &str) -> ToolRelease {
    services
        .tool_release_service
        .list_in_account(owner, &AuthCtx::system())
        .await
        .unwrap()
        .into_iter()
        .find(|release| release.name.as_str() == name)
        .unwrap()
}

async fn release_coordinate(
    services: &Services,
    owner: AccountId,
    name: &str,
    version: &str,
) -> ToolRelease {
    services
        .tool_release_service
        .list_in_account(owner, &AuthCtx::system())
        .await
        .unwrap()
        .into_iter()
        .find(|release| release.name.as_str() == name && release.version == version)
        .unwrap()
}

fn component_source(
    release: &ToolRelease,
) -> (
    golem_common::model::component::ComponentId,
    ComponentRevision,
) {
    match release.source {
        ToolSource::Component {
            component_id,
            component_revision,
            ..
        } => (component_id, component_revision),
        _ => panic!("expected a component-backed release"),
    }
}
