use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::environment::EnvironmentName;
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
    let wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test-components/",
        "golem_it_tool_streaming_rust_provider_release.wasm"
    ))
    .expect("build the tool-streaming test component before running this test");
    let wasm = Box::leak(wasm.into_boxed_slice());
    let descriptor = BuiltinToolDescriptor {
        component_name: "builtin-tool-streaming-test",
        tool_name: "streaming",
        release_version: "1.0.0",
        wasm_bytes: wasm,
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

    provision(&services, owner, std::slice::from_ref(&descriptor)).await;
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
