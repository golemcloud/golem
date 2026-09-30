use async_zip::tokio::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{
    AgentFilePath, AgentFilePermissions, ComponentName, ComponentRevision,
};
use golem_common::model::environment::EnvironmentName;
use golem_common::model::tool::{ToolName, ToolSource};
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
use test_r::{test, timeout};
use tokio::task::JoinSet;

pub mod native;

async fn zip_bytes(entries: &[(&str, &[u8])]) -> &'static [u8] {
    let archive = tempfile::NamedTempFile::new().unwrap();
    let file = tokio::fs::File::from_std(archive.reopen().unwrap());
    let mut writer = ZipFileWriter::with_tokio(file);
    for (path, contents) in entries {
        let entry = ZipEntryBuilder::new((*path).into(), Compression::Deflate);
        writer.write_entry_whole(entry, contents).await.unwrap();
    }
    writer.close().await.unwrap();
    Box::leak(std::fs::read(archive.path()).unwrap().into_boxed_slice())
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
    let services = Services::new(&config, &mut join_set).await.unwrap();
    let mut wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../builtin-tools/filesystem-tools.wasm"
    ))
    .expect("build the filesystem tool component before running this test");
    let mut replacements = 0;
    for offset in 0..wasm.len().saturating_sub(5) {
        if &wasm[offset..offset + 5] == b"0.3.0" {
            wasm[offset..offset + 5].copy_from_slice(b"6.1.0");
            replacements += 1;
        }
    }
    assert!(replacements > 0);
    let mut renamed_tools = 0;
    for offset in 0..wasm.len().saturating_sub(9) {
        if &wasm[offset..offset + 9] == b"read-file" {
            wasm[offset..offset + 9].copy_from_slice(b"assetfile");
            renamed_tools += 1;
        }
    }
    assert!(renamed_tools > 0);
    let wasm = Box::leak(wasm.into_boxed_slice());
    let files_archive_bytes = zip_bytes(&[("tool-data.txt", b"embedded tool data")]).await;
    let descriptor = BuiltinToolDescriptor {
        component_name: "builtin-tool-assets-test",
        tool_name: "assetfile",
        release_version: "6.1.0",
        wasm_bytes: wasm,
        files_archive_bytes: Some(files_archive_bytes),
        files_manifest_bytes: Some(
            br#"[{"archivePath":"tool-data.txt","targetPath":"/builtin/tool-data.txt","permissions":"read-only"}]"#,
        ),
    };
    let auth = AuthCtx::system();

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
    let baseline_component_count = services
        .component_service
        .list_staged_components_for_environment(&env, &auth)
        .await
        .unwrap()
        .len();
    let baseline_deployment_count = services
        .deployment_service
        .list_deployments(env.id, None, &auth)
        .await
        .unwrap()
        .len();

    let descriptors = std::slice::from_ref(&descriptor);
    tokio::join!(
        provision(&services, owner, descriptors),
        provision(&services, owner, descriptors)
    );
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
    let provisioned_files = &first_component.metadata.tools()
        [&ToolName::try_from(descriptor.tool_name).unwrap()]
        .provision
        .files;
    assert_eq!(provisioned_files.len(), 1);
    assert_eq!(
        provisioned_files[0].path,
        AgentFilePath::from_abs_str("/builtin/tool-data.txt").unwrap()
    );
    assert_eq!(
        provisioned_files[0].permissions,
        AgentFilePermissions::ReadOnly
    );
    assert_eq!(provisioned_files[0].size, 18);
    let first_deployments = services
        .deployment_service
        .list_deployments(env.id, None, &auth)
        .await
        .unwrap();
    assert_eq!(first_component.revision, ComponentRevision::INITIAL);
    assert_eq!(first_deployments.len(), baseline_deployment_count + 1);

    provision(&services, owner, std::slice::from_ref(&descriptor)).await;
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
        baseline_deployment_count + 1
    );

    let mismatch = BuiltinToolDescriptor {
        component_name: "different-builtin-tool-streaming-test",
        ..descriptor
    };
    let error = provision_descriptors(
        std::slice::from_ref(&mismatch),
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
        baseline_component_count + 1
    );
    assert_eq!(
        services
            .deployment_service
            .list_deployments(env.id, None, &auth)
            .await
            .unwrap()
            .len(),
        baseline_deployment_count + 1
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
    let services = Services::new(&config, &mut join_set).await.unwrap();
    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../builtin-tools/filesystem-tools.wasm"
    ))
    .expect("build the filesystem tool component before running this test");
    let mut first_wasm = wasm;
    let mut replacements = 0;
    for offset in 0..first_wasm.len().saturating_sub(5) {
        if &first_wasm[offset..offset + 5] == b"0.3.0" {
            first_wasm[offset..offset + 5].copy_from_slice(b"7.2.0");
            replacements += 1;
        }
    }
    assert!(replacements > 0);
    let mut changed_wasm = first_wasm.clone();
    for offset in 0..changed_wasm.len().saturating_sub(5) {
        if &changed_wasm[offset..offset + 5] == b"7.2.0" {
            changed_wasm[offset..offset + 5].copy_from_slice(b"7.3.0");
        }
    }
    let first_wasm = Box::leak(first_wasm.into_boxed_slice());
    let changed_wasm = Box::leak(changed_wasm.into_boxed_slice());
    let first = BuiltinToolDescriptor {
        component_name: "filesystem-tools",
        tool_name: "read-file",
        release_version: "7.2.0",
        wasm_bytes: first_wasm,
        files_archive_bytes: None,
        files_manifest_bytes: None,
    };
    let second = BuiltinToolDescriptor {
        component_name: first.component_name,
        tool_name: first.tool_name,
        release_version: "7.3.0",
        wasm_bytes: changed_wasm,
        files_archive_bytes: None,
        files_manifest_bytes: None,
    };

    provision(&services, owner, std::slice::from_ref(&first)).await;
    let old_release =
        release_coordinate(&services, owner, first.tool_name, first.release_version).await;
    let (component_id, old_revision) = component_source(&old_release);

    provision(&services, owner, std::slice::from_ref(&second)).await;
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

    provision(&services, owner, std::slice::from_ref(&second)).await;
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
    let services = Services::new(&config, &mut join_set).await.unwrap();
    let mut wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../builtin-tools/filesystem-tools.wasm"
    ))
    .expect("build the filesystem tool component before running this test");
    let mut replacements = 0;
    for offset in 0..wasm.len().saturating_sub(5) {
        if &wasm[offset..offset + 5] == b"0.3.0" {
            wasm[offset..offset + 5].copy_from_slice(b"8.2.0");
            replacements += 1;
        }
    }
    assert!(replacements > 0);
    let wasm = Box::leak(wasm.into_boxed_slice());
    let first = BuiltinToolDescriptor {
        component_name: "filesystem-tools",
        tool_name: "read-file",
        release_version: "8.2.0",
        wasm_bytes: wasm,
        files_archive_bytes: None,
        files_manifest_bytes: None,
    };
    let second = BuiltinToolDescriptor {
        component_name: first.component_name,
        tool_name: "write-file",
        release_version: first.release_version,
        wasm_bytes: wasm,
        files_archive_bytes: None,
        files_manifest_bytes: None,
    };

    provision(&services, owner, std::slice::from_ref(&first)).await;
    let old_release =
        release_coordinate(&services, owner, first.tool_name, first.release_version).await;
    let (component_id, old_revision) = component_source(&old_release);

    let complete = [first, second];
    tokio::join!(
        provision(&services, owner, &complete),
        provision(&services, owner, &complete)
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

    provision(&services, owner, &complete).await;
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

async fn provision(services: &Services, owner: AccountId, descriptors: &[BuiltinToolDescriptor]) {
    provision_descriptors(
        descriptors,
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
