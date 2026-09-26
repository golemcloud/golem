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
    let first_release = only_release(&services, owner).await;
    let first_deployments = services
        .deployment_service
        .list_deployments(env.id, None, &auth)
        .await
        .unwrap();
    assert_eq!(first_component.revision, ComponentRevision::INITIAL);
    assert_eq!(first_deployments.len(), 1);
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
    let repeated_release = only_release(&services, owner).await;
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
    let restarted_release = only_release(&services, owner).await;
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
        tool_name: "bash",
        release_version: "0.2.1",
        wasm_bytes: EMBEDDED_BASH,
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
        release_version: "0.2.0",
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
    assert_eq!(only_release(&services, owner).await.id, first_release.id);
    assert_eq!(
        services
            .component_service
            .list_staged_components_for_environment(&restarted_env, &auth)
            .await
            .unwrap()
            .len(),
        1
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
    let old_release = only_release(&services, owner).await;
    assert_eq!(old_release.version, EMBEDDED_VERSION);

    let upgrade = BuiltinToolDescriptor {
        tool_name: "bash",
        release_version: "0.2.1",
        wasm_bytes: retag_version(EMBEDDED_BASH, EMBEDDED_VERSION, "0.2.1"),
    };
    assert_eq!(upgrade.component_name(), "golem:bash-0-2-1");
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
        tool_name: "bash",
        release_version: "0.2.2",
        wasm_bytes: retag_version(EMBEDDED_BASH, EMBEDDED_VERSION, "0.2.2"),
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

const EMBEDDED_BASH: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../plugins/builtin-tools/bash.wasm"
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

async fn only_release(services: &Services, owner: AccountId) -> ToolRelease {
    let releases = services
        .tool_release_service
        .list_in_account(owner, &AuthCtx::system())
        .await
        .unwrap();
    assert_eq!(releases.len(), 1);
    releases.into_iter().next().unwrap()
}
