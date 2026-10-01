use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationName;
use golem_common::model::component::{ComponentName, ComponentRevision};
use golem_common::model::deployment::{DeploymentCreation, DeploymentVersion};
use golem_common::model::environment::{Environment, EnvironmentCreation, EnvironmentName};
use golem_common::model::environment_tool_grant::EnvironmentToolGrantCreation;
use golem_common::model::tool::{
    RemoteToolDeployment, TOOL_METADATA_WIT_VERSION, ToolName, ToolSource,
};
use golem_common::model::tool_release::{
    SystemToolAvailability, ToolRelease, ToolReleaseByCoordinates, ToolReleaseLifecycle,
    ToolReleaseOrigin, ToolReleaseReference,
};
use golem_registry_service::bootstrap::Services;
use golem_registry_service::config::{
    ComponentCompilationConfig, LoginConfig, RegistryServiceConfig,
};
use golem_registry_service::services::builtin_tool_provisioner::{
    BuiltinToolDescriptor, ProvisionReport, provision_builtin_tools, provision_descriptors,
};
use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
use golem_service_base::model::auth::AuthCtx;
use test_r::{test, timeout};
use tokio::task::JoinSet;

pub mod native;

#[test]
#[timeout("120s")]
async fn provisions_bash_release_idempotently_and_rejects_changes_without_mutation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = registry_config(temp_dir.path());
    let owner = config.initial_accounts["builtin_tool_owner"].id;
    let mut join_set = JoinSet::new();
    let services = Services::new(&config, &mut join_set).await.unwrap();
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
    let first_component = services
        .component_service
        .get_staged_component_by_name(env.id, &ComponentName("golem:bash-0-2-0".into()), &auth)
        .await
        .unwrap();
    let first_release = release_named(&services, owner, "bash").await;
    let first_deployments = services
        .deployment_service
        .list_deployments(env.id, None, &auth)
        .await
        .unwrap();
    assert_eq!(first_component.revision, ComponentRevision::INITIAL);
    // One deployment provisions every embedded built-in tool.
    assert_eq!(first_deployments.len(), 1);
    let components_after_boot = services
        .component_service
        .list_staged_components_for_environment(&env, &auth)
        .await
        .unwrap()
        .len();
    let bash_name = ToolName::try_from("bash").unwrap();
    let component_tool = first_component
        .metadata
        .tools()
        .get(&bash_name)
        .expect("the embedded component exports the bash tool");
    assert_eq!(component_tool.definition.version, "0.2.0");
    assert_eq!(first_release.name, bash_name);
    assert_eq!(first_release.version, "0.2.0");
    assert_eq!(first_release.definition, component_tool.definition);
    assert_eq!(first_release.metadata_version, TOOL_METADATA_WIT_VERSION);
    assert!(first_release.immutable);
    assert_eq!(first_release.lifecycle, ToolReleaseLifecycle::Published);
    assert_eq!(first_release.origin, ToolReleaseOrigin::ProtectedSystem);
    assert_eq!(
        first_release.system_availability,
        Some(SystemToolAvailability::Grantable)
    );
    let ToolSource::Component {
        component_id,
        component_revision,
        component_name,
    } = &first_release.source
    else {
        panic!("the built-in Bash release must be component-backed");
    };
    assert_eq!(*component_id, first_component.id);
    assert_eq!(*component_revision, first_component.revision);
    assert_eq!(component_name, &first_component.component_name);

    // Provisioned bytes are not compiled again.
    assert_eq!(
        provision_inventory(&services, owner).await,
        ProvisionReport::default()
    );
    let repeated_component = services
        .component_service
        .get_staged_component_by_name(env.id, &ComponentName("golem:bash-0-2-0".into()), &auth)
        .await
        .unwrap();
    let repeated_release = release_named(&services, owner, "bash").await;
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

    drop(services);
    join_set.shutdown().await;

    let mut restarted_join_set = JoinSet::new();
    let services = Services::new(&config, &mut restarted_join_set)
        .await
        .unwrap();
    let restarted_app = services
        .application_service
        .get_in_account(owner, &ApplicationName("golem-system".into()), &auth)
        .await
        .unwrap();
    let restarted_env = services
        .environment_service
        .get_in_application(
            restarted_app.id,
            &EnvironmentName("builtin-tools".into()),
            &auth,
        )
        .await
        .unwrap();
    let restarted_component = services
        .component_service
        .get_staged_component_by_name(
            restarted_env.id,
            &ComponentName("golem:bash-0-2-0".into()),
            &auth,
        )
        .await
        .unwrap();
    let restarted_release = release_named(&services, owner, "bash").await;
    let restarted_deployments = services
        .deployment_service
        .list_deployments(restarted_env.id, None, &auth)
        .await
        .unwrap();
    assert_eq!(restarted_app.id, app.id);
    assert_eq!(restarted_env.id, env.id);
    assert_eq!(restarted_component.id, first_component.id);
    assert_eq!(restarted_component.revision, first_component.revision);
    assert_eq!(restarted_release.id, first_release.id);
    assert_eq!(restarted_deployments, first_deployments);
    assert_eq!(
        provision_inventory(&services, owner).await,
        ProvisionReport::default()
    );

    let metadata_mismatch = BuiltinToolDescriptor {
        component_name: "golem:bash-0-2-1",
        tool_name: "bash",
        release_version: "0.2.1",
        wasm_bytes: EMBEDDED_BASH,
        retires_older_versions: true,
    };
    let error = provision_descriptors(
        std::slice::from_ref(&metadata_mismatch),
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
    assert!(
        error
            .to_string()
            .contains("metadata does not match release coordinate 0.2.1"),
        "{error:#}"
    );

    let changed_wasm = append_test_custom_section(metadata_mismatch.wasm_bytes);
    let artifact_mismatch = BuiltinToolDescriptor {
        component_name: EMBEDDED_COMPONENT,
        release_version: EMBEDDED_VERSION,
        wasm_bytes: changed_wasm,
        ..metadata_mismatch
    };
    let error = provision_descriptors(
        std::slice::from_ref(&artifact_mismatch),
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
        release_named(&services, owner, "bash").await.id,
        first_release.id
    );
    assert_eq!(
        services
            .component_service
            .list_staged_components_for_environment(&restarted_env, &auth)
            .await
            .unwrap()
            .len(),
        components_after_boot
    );
    assert_eq!(
        services
            .deployment_service
            .list_deployments(restarted_env.id, None, &auth)
            .await
            .unwrap()
            .len(),
        1
    );
}

async fn provision_inventory(services: &Services, owner: AccountId) -> ProvisionReport {
    provision_builtin_tools(
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
    .unwrap()
}

/// Upgrades the embedded release to a new version, provisioned from its own component: the
/// older component stops implementing `bash` in the same deployment that adds the new one, the
/// older release stays pinned to its revision and is superseded once the new one is published,
/// a restart boots cleanly, and provisioned bytes are never compiled again.
#[test]
#[timeout("300s")]
async fn upgrades_bash_through_a_new_component_and_supersedes_the_old_release() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = registry_config(temp_dir.path());
    let owner = config.initial_accounts["builtin_tool_owner"].id;
    let auth = AuthCtx::system();
    let bash = ToolName::try_from("bash").unwrap();
    let mut join_set = JoinSet::new();
    let services = Services::new(&config, &mut join_set).await.unwrap();
    let env = builtin_environment(&services, owner).await;
    let old_component = staged(&services, &env, EMBEDDED_COMPONENT).await;
    let old_release = release_named(&services, owner, "bash").await;
    assert_eq!(old_release.version, EMBEDDED_VERSION);

    let upgrade = BuiltinToolDescriptor {
        component_name: "golem:bash-0-2-1",
        tool_name: "bash",
        release_version: "0.2.1",
        wasm_bytes: retag_version(EMBEDDED_BASH, EMBEDDED_VERSION, "0.2.1"),
        retires_older_versions: true,
    };
    let report = provision(&services, owner, &upgrade).await.unwrap();
    assert_eq!(
        report,
        ProvisionReport {
            extracted: vec!["bash@0.2.1".into()],
            retired: vec![EMBEDDED_COMPONENT.into()],
            deployed: true,
            published: vec!["bash@0.2.1".into()],
            superseded: vec![format!("bash@{EMBEDDED_VERSION}")],
        }
    );

    let new_component = staged(&services, &env, "golem:bash-0-2-1").await;
    let releases = releases_by_version(&services, owner).await;
    assert_eq!(releases.len(), 2);
    let superseded = &releases[EMBEDDED_VERSION];
    assert_eq!(superseded.id, old_release.id);
    assert_eq!(superseded.lifecycle, ToolReleaseLifecycle::Superseded);
    assert_eq!(superseded.source, old_release.source);
    let upgraded = &releases["0.2.1"];
    assert_eq!(upgraded.lifecycle, ToolReleaseLifecycle::Published);
    assert_eq!(
        upgraded.source,
        ToolSource::Component {
            component_id: new_component.id,
            component_revision: new_component.revision,
            component_name: new_component.component_name.clone(),
        }
    );
    // The superseded release's revision is kept and still implements bash, so its grants and
    // agents keep working; the environment's deployment has only the new implementor.
    let pinned = services
        .component_service
        .get_component_revision(old_component.id, old_component.revision, false, &auth)
        .await
        .unwrap();
    assert_eq!(
        pinned.metadata.tools()[&bash].definition.version,
        EMBEDDED_VERSION
    );
    let retired = services
        .component_service
        .get_deployed_component(old_component.id, &auth)
        .await
        .unwrap();
    assert_eq!(retired.revision, old_component.revision.next().unwrap());
    assert!(retired.metadata.tools().is_empty());
    let deployed = services
        .component_service
        .get_deployed_component(new_component.id, &auth)
        .await
        .unwrap();
    assert_eq!(deployed.metadata.tools()[&bash].definition.version, "0.2.1");
    assert_eq!(deployment_count(&services, &env).await, 2);

    // The same bytes at the same version: nothing is compiled, deployed or changed.
    assert_eq!(
        provision(&services, owner, &upgrade).await.unwrap(),
        ProvisionReport::default()
    );

    // An environment that names the superseded version is told which version to switch to, both
    // when it grants the tool and when it deploys it.
    let superseded_message = format!(
        "built-in tool bash@{EMBEDDED_VERSION} was superseded by bash@0.2.1; update the manifest \
         to bash@0.2.1"
    );
    let owner_email = config.initial_accounts["builtin_tool_owner"].email.clone();
    let old_coordinates = ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
        account: owner_email,
        name: bash.clone(),
        version: EMBEDDED_VERSION.to_string(),
    });
    let consumer = services
        .environment_service
        .create(
            env.application_id,
            EnvironmentCreation {
                name: EnvironmentName("consumer".into()),
                compatibility_check: false,
                tool_compatibility_mode: Default::default(),
                version_check: false,
                security_overrides: false,
            },
            &auth,
        )
        .await
        .unwrap();
    let grant = services
        .environment_tool_grant_service
        .create(
            consumer.id,
            EnvironmentToolGrantCreation {
                release: old_coordinates.clone(),
                automatic: true,
            },
            &auth,
        )
        .await
        .unwrap_err();
    assert_eq!(grant.to_string(), superseded_message);
    let deploy = services
        .deployment_write_service
        .create_deployment(
            consumer.id,
            DeploymentCreation {
                mcp_imports: Vec::new(),
                current_revision: None,
                expected_deployment_hash: Default::default(),
                version: DeploymentVersion("pinned-to-superseded".into()),
                publish_tools: vec![],
                remote_tools: vec![RemoteToolDeployment {
                    name: bash.clone(),
                    release: old_coordinates,
                    provision: Default::default(),
                    environment_binding: None,
                    component_bindings: Default::default(),
                    agent_bindings: Default::default(),
                }],
                publish_tool_middlewares: vec![],
                remote_tool_middlewares: vec![],
                universal_tool_middlewares: vec![],
                environment_tool_middleware_bindings: Default::default(),
                agent_tool_middleware_bindings: Default::default(),
                agent_secret_defaults: vec![],
                quota_resource_defaults: vec![],
                retry_policy_defaults: vec![],
                replace_incompatible_agent_secrets: false,
            },
            &auth,
        )
        .await
        .unwrap_err();
    assert_eq!(deploy.to_string(), superseded_message);

    // A restart with the older embedded release boots cleanly and leaves the upgrade in place,
    // and a second boot with the upgrade's bytes compiles nothing.
    drop(services);
    join_set.shutdown().await;
    let mut restarted_join_set = JoinSet::new();
    let services = Services::new(&config, &mut restarted_join_set)
        .await
        .unwrap();
    assert_eq!(releases_by_version(&services, owner).await, releases);
    assert_eq!(deployment_count(&services, &env).await, 2);
    assert_eq!(
        provision(&services, owner, &upgrade).await.unwrap(),
        ProvisionReport::default()
    );

    // The next version takes the same path.
    let next = BuiltinToolDescriptor {
        component_name: "golem:bash-0-2-2",
        tool_name: "bash",
        release_version: "0.2.2",
        wasm_bytes: retag_version(EMBEDDED_BASH, EMBEDDED_VERSION, "0.2.2"),
        retires_older_versions: true,
    };
    assert_eq!(
        provision(&services, owner, &next).await.unwrap(),
        ProvisionReport {
            extracted: vec!["bash@0.2.2".into()],
            retired: vec!["golem:bash-0-2-1".into()],
            deployed: true,
            published: vec!["bash@0.2.2".into()],
            superseded: vec!["bash@0.2.1".into()],
        }
    );
    let releases = releases_by_version(&services, owner).await;
    assert_eq!(
        releases
            .values()
            .map(|release| (release.version.as_str(), release.lifecycle))
            .collect::<Vec<_>>(),
        [
            (EMBEDDED_VERSION, ToolReleaseLifecycle::Superseded),
            ("0.2.1", ToolReleaseLifecycle::Superseded),
            ("0.2.2", ToolReleaseLifecycle::Published),
        ]
    );
    assert_eq!(deployment_count(&services, &env).await, 3);
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
        retires_older_versions: false,
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
        provision_all(&services, owner, descriptors),
        provision_all(&services, owner, descriptors)
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
    let first_deployments = services
        .deployment_service
        .list_deployments(env.id, None, &auth)
        .await
        .unwrap();
    assert_eq!(first_component.revision, ComponentRevision::INITIAL);
    assert_eq!(first_deployments.len(), baseline_deployment_count + 1);

    provision_all(&services, owner, std::slice::from_ref(&descriptor)).await;
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
        retires_older_versions: false,
    };
    let second = BuiltinToolDescriptor {
        component_name: first.component_name,
        tool_name: first.tool_name,
        release_version: "7.3.0",
        wasm_bytes: changed_wasm,
        retires_older_versions: false,
    };

    provision_all(&services, owner, std::slice::from_ref(&first)).await;
    let old_release =
        release_coordinate(&services, owner, first.tool_name, first.release_version).await;
    let (component_id, old_revision) = component_source(&old_release);

    provision_all(&services, owner, std::slice::from_ref(&second)).await;
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
    assert_eq!(
        old_release_after_upgrade.lifecycle,
        ToolReleaseLifecycle::Published,
        "a shared-component tool keeps every published version resolvable by coordinate"
    );

    provision_all(&services, owner, std::slice::from_ref(&second)).await;
    let replayed =
        release_coordinate(&services, owner, second.tool_name, second.release_version).await;
    assert_eq!(replayed.id, new_release.id);
    assert_eq!(component_source(&replayed), (component_id, new_revision));
}

/// A registry that already has a filesystem-tools release published (as this build's own boot
/// leaves one) keeps it published, and grantable by coordinate, once another read-file release is
/// published alongside it. Filesystem tools share one component across versions, so there is
/// nothing for a newer version to retire the older one out of: both stay valid targets for a
/// manifest. This is what main's provisioner already did; the regression under test is this
/// build's provisioner unconditionally superseding every other published release of a tool it
/// republishes, which would apply here too since it does not distinguish shared-component tools
/// from Bash's per-version ones.
#[test]
#[timeout("120s")]
async fn older_filesystem_tool_release_stays_published_after_this_builds_first_boot() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = registry_config(temp_dir.path());
    let owner = config.initial_accounts["builtin_tool_owner"].id;
    let auth = AuthCtx::system();
    let mut join_set = JoinSet::new();
    // Booting already publishes the real, current read-file release (this is what "this build's
    // first boot" means in production): it stands in for the release a manifest may already be
    // pinned to.
    let services = Services::new(&config, &mut join_set).await.unwrap();
    let current_release = release_coordinate(&services, owner, "read-file", "0.3.0").await;
    assert_eq!(current_release.lifecycle, ToolReleaseLifecycle::Published);

    // A later read-file release, from its own self-contained component so it cannot disturb the
    // real filesystem-tools component's other tools. This models the shared-component tools'
    // actual shape (a tool whose versions are not retired by the next one) without depending on
    // the embedded artifact ever changing version.
    let mut other_wasm = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../builtin-tools/filesystem-tools.wasm"
    ))
    .expect("build the filesystem tool component before running this test");
    let mut replacements = 0;
    for offset in 0..other_wasm.len().saturating_sub(5) {
        if &other_wasm[offset..offset + 5] == b"0.3.0" {
            other_wasm[offset..offset + 5].copy_from_slice(b"0.3.1");
            replacements += 1;
        }
    }
    assert!(replacements > 0);
    let other_wasm = Box::leak(other_wasm.into_boxed_slice());
    let other = BuiltinToolDescriptor {
        component_name: "filesystem-tools-other",
        tool_name: "read-file",
        release_version: "0.3.1",
        wasm_bytes: other_wasm,
        retires_older_versions: false,
    };
    provision_all(&services, owner, std::slice::from_ref(&other)).await;
    let other_release = release_coordinate(&services, owner, "read-file", "0.3.1").await;
    assert_eq!(other_release.lifecycle, ToolReleaseLifecycle::Published);

    // Publishing the second release must not retroactively supersede the first: filesystem-tools
    // is a shared-component tool, so every published release stays resolvable by coordinate.
    let current_release_after = release_coordinate(&services, owner, "read-file", "0.3.0").await;
    assert_eq!(current_release_after.id, current_release.id);
    assert_eq!(
        current_release_after.lifecycle,
        ToolReleaseLifecycle::Published,
        "publishing another read-file release must not supersede an existing one for a \
         shared-component tool"
    );

    // A manifest pinned to the original version by coordinate still resolves and can be granted.
    let env = builtin_environment(&services, owner).await;
    let pinned_coordinates = ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
        account: config.initial_accounts["builtin_tool_owner"].email.clone(),
        name: ToolName::try_from("read-file").unwrap(),
        version: "0.3.0".to_string(),
    });
    let consumer = services
        .environment_service
        .create(
            env.application_id,
            EnvironmentCreation {
                name: EnvironmentName("consumer-of-pinned-read-file".into()),
                compatibility_check: false,
                tool_compatibility_mode: Default::default(),
                version_check: false,
                security_overrides: false,
            },
            &auth,
        )
        .await
        .unwrap();
    services
        .environment_tool_grant_service
        .create(
            consumer.id,
            EnvironmentToolGrantCreation {
                release: pinned_coordinates,
                automatic: true,
            },
            &auth,
        )
        .await
        .unwrap();
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
        retires_older_versions: false,
    };
    let second = BuiltinToolDescriptor {
        component_name: first.component_name,
        tool_name: "write-file",
        release_version: first.release_version,
        wasm_bytes: wasm,
        retires_older_versions: false,
    };

    provision_all(&services, owner, std::slice::from_ref(&first)).await;
    let old_release =
        release_coordinate(&services, owner, first.tool_name, first.release_version).await;
    let (component_id, old_revision) = component_source(&old_release);

    let complete = [first, second];
    tokio::join!(
        provision_all(&services, owner, &complete),
        provision_all(&services, owner, &complete)
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

    provision_all(&services, owner, &complete).await;
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

const EMBEDDED_BASH: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../builtin-tools/bash.wasm"
));
const EMBEDDED_VERSION: &str = "0.2.0";
const EMBEDDED_COMPONENT: &str = "golem:bash-0-2-0";

fn registry_config(dir: &std::path::Path) -> RegistryServiceConfig {
    RegistryServiceConfig {
        db: DbConfig::Sqlite(DbSqliteConfig {
            database: dir.join("registry.db").to_string_lossy().into_owned(),
            max_connections: 4,
            foreign_keys: true,
        }),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
            root: dir.join("blobs"),
        }),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    }
}

async fn provision(
    services: &Services,
    owner: AccountId,
    descriptor: &BuiltinToolDescriptor,
) -> anyhow::Result<ProvisionReport> {
    provision_descriptors(
        std::slice::from_ref(descriptor),
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
}

async fn builtin_environment(services: &Services, owner: AccountId) -> Environment {
    let auth = AuthCtx::system();
    let app = services
        .application_service
        .get_in_account(owner, &ApplicationName("golem-system".into()), &auth)
        .await
        .unwrap();
    services
        .environment_service
        .get_in_application(app.id, &EnvironmentName("builtin-tools".into()), &auth)
        .await
        .unwrap()
}

async fn staged(
    services: &Services,
    env: &Environment,
    name: &str,
) -> golem_service_base::model::component::Component {
    services
        .component_service
        .get_staged_component_by_name(env.id, &ComponentName(name.into()), &AuthCtx::system())
        .await
        .unwrap()
}

async fn deployment_count(services: &Services, env: &Environment) -> usize {
    services
        .deployment_service
        .list_deployments(env.id, None, &AuthCtx::system())
        .await
        .unwrap()
        .len()
}

/// The Bash releases, by version.
async fn releases_by_version(
    services: &Services,
    owner: AccountId,
) -> std::collections::BTreeMap<String, ToolRelease> {
    services
        .tool_release_service
        .list_in_account(owner, &AuthCtx::system())
        .await
        .unwrap()
        .into_iter()
        .filter(|release| release.name.as_str() == "bash")
        .map(|release| (release.version.clone(), release))
        .collect()
}

/// The embedded component with its tool's version literal replaced by another of the same
/// length: new bytes whose extracted metadata names the new version. The literal is the one
/// occurrence of the version outside WIT package names (`@x.y.z`), crate paths (`-x.y.z`), longer
/// numbers and a command's `--version` banner (sed's `0.2.0 (uutils)`); provisioning checks the
/// extracted version, so a wrong edit fails loudly.
fn retag_version(wasm: &[u8], from: &str, to: &str) -> &'static [u8] {
    assert_eq!(from.len(), to.len());
    let from = from.as_bytes();
    let found: Vec<usize> = wasm
        .windows(from.len())
        .enumerate()
        .filter(|(at, window)| {
            *window == from
                && *at > 0
                && !matches!(wasm[at - 1], b'@' | b'-' | b'.' | b'0'..=b'9')
                && !wasm[at + from.len()..].starts_with(b" (")
        })
        .map(|(at, _)| at)
        .collect();
    assert_eq!(found.len(), 1, "the tool's version literal at {found:?}");
    let mut changed = wasm.to_vec();
    changed[found[0]..found[0] + to.len()].copy_from_slice(to.as_bytes());
    Box::leak(changed.into_boxed_slice())
}

fn append_test_custom_section(wasm: &[u8]) -> &'static [u8] {
    let name = b"golem-builtin-tool-immutability-test";
    let data = b"changed bytes with identical exported metadata";
    let mut section = Vec::new();
    push_unsigned_leb128(&mut section, name.len());
    section.extend_from_slice(name);
    section.extend_from_slice(data);

    let mut changed = wasm.to_vec();
    changed.push(0);
    push_unsigned_leb128(&mut changed, section.len());
    changed.extend_from_slice(&section);
    Box::leak(changed.into_boxed_slice())
}

fn push_unsigned_leb128(output: &mut Vec<u8>, mut value: usize) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
}

async fn provision_all(
    services: &Services,
    owner: AccountId,
    descriptors: &[BuiltinToolDescriptor],
) {
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
