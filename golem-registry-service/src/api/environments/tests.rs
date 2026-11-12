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

use crate::api::make_open_api_service;
use crate::bootstrap::Services;
use crate::config::{ComponentCompilationConfig, LoginConfig, RegistryServiceConfig};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::environment::{EnvironmentCreation, EnvironmentId, EnvironmentName};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::model::auth::AuthCtx;
use poem::http::StatusCode;
use poem::test::TestClient;
use serde_json::{Value, json};
use test_r::{test, timeout};
use tokio::task::JoinSet;

struct Fixture {
    services: Services,
    environment_id: EnvironmentId,
    bearer: String,
    account_email: String,
    _tasks: JoinSet<anyhow::Result<()>>,
    _directory: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let db = DbSqliteConfig {
        database: directory
            .path()
            .join("registry.db")
            .to_string_lossy()
            .into(),
        max_connections: 4,
        foreign_keys: true,
    };
    let config = RegistryServiceConfig {
        db: DbConfig::Sqlite(db),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    let root = &config.initial_accounts["root"];
    let mut tasks = JoinSet::new();
    let services = Services::new_without_component_builtins(&config, &mut tasks)
        .await
        .unwrap();
    let auth = AuthCtx::system();
    let app = services
        .application_service
        .create(
            root.id,
            ApplicationCreation {
                name: ApplicationName("tool-middleware-lookup".into()),
            },
            &auth,
        )
        .await
        .unwrap();
    let env = services
        .environment_service
        .create(
            app.id,
            EnvironmentCreation {
                name: EnvironmentName("test".into()),
                compatibility_check: false,
                tool_compatibility_mode: Default::default(),
                version_check: false,
                security_overrides: false,
            },
            &auth,
        )
        .await
        .unwrap();
    Fixture {
        services,
        environment_id: env.id,
        bearer: format!("Bearer {}", root.token.as_ref().unwrap().secret()),
        account_email: root.email.as_str().to_string(),
        _tasks: tasks,
        _directory: directory,
    }
}

/// Deploys the empty environment through the REST API and returns (revision, currentRevision).
async fn deploy_empty_environment(
    api: &TestClient<impl poem::Endpoint>,
    fixture: &Fixture,
) -> (u64, Value) {
    let plan = api
        .get(format!("/v1/envs/{}/plan", fixture.environment_id))
        .header("Authorization", &fixture.bearer)
        .send()
        .await;
    plan.assert_status_is_ok();
    let plan: Value = plan.0.into_body().into_json().await.unwrap();
    let deployment = api
        .post(format!("/v1/envs/{}/deployments", fixture.environment_id))
        .header("Authorization", &fixture.bearer)
        .body_json(&json!({
            "expectedDeploymentHash": plan["deploymentHash"],
            "version": "0.0.1",
            "agentSecretDefaults": [],
            "quotaResourceDefaults": [],
            "retryPolicyDefaults": []
        }))
        .send()
        .await;
    deployment.assert_status_is_ok();
    let deployment: Value = deployment.0.into_body().into_json().await.unwrap();
    (
        deployment["revision"].as_u64().unwrap(),
        deployment["currentRevision"].clone(),
    )
}

#[test]
#[timeout("120s")]
async fn unknown_tool_middleware_name_is_a_client_error_and_the_registry_keeps_serving() {
    let fixture = fixture().await;
    let api = TestClient::new(make_open_api_service(&fixture.services));
    let (revision, _) = deploy_empty_environment(&api, &fixture).await;
    let base = format!(
        "/v1/envs/{}/deployments/{revision}/tool-middlewares",
        fixture.environment_id
    );

    // A name that breaks the tool-name rules. 400 once the name type checks itself, 404 before.
    let invalid = api
        .get(format!("{base}/Audit_Log"))
        .header("Authorization", &fixture.bearer)
        .send()
        .await;
    assert!(
        invalid.0.status().is_client_error(),
        "unexpected status {}",
        invalid.0.status()
    );

    // A valid name that the deployment does not hold.
    let absent = api
        .get(format!("{base}/audit-log"))
        .header("Authorization", &fixture.bearer)
        .send()
        .await;
    absent.assert_status(StatusCode::NOT_FOUND);
    absent
        .assert_json(json!({
            "code": "TOOL_NOT_FOUND",
            "error": "Tool middleware audit-log not found"
        }))
        .await;

    // The registry still answers.
    let list = api
        .get(&base)
        .header("Authorization", &fixture.bearer)
        .send()
        .await;
    list.assert_status_is_ok();
    list.assert_json(json!({ "values": [] })).await;
}

async fn body_text(response: poem::test::TestResponse) -> String {
    response.0.into_body().into_string().await.unwrap()
}

#[test]
#[timeout("120s")]
async fn requests_with_invalid_tool_names_are_refused_with_bad_request() {
    const TOOL_RULE: &str = "Tool name must start with a lowercase ASCII letter (a-z), but got 'R'";
    const MIDDLEWARE_RULE: &str =
        "Tool middleware name must start with a lowercase ASCII letter (a-z), but got 'A'";

    let fixture = fixture().await;
    let api = TestClient::new(make_open_api_service(&fixture.services));
    let (revision, current_revision) = deploy_empty_environment(&api, &fixture).await;
    let env = fixture.environment_id;

    let refused = |response: poem::test::TestResponse, rule: &'static str| async move {
        response.assert_status(StatusCode::BAD_REQUEST);
        let text = body_text(response).await;
        assert!(
            text.contains(rule),
            "body {text:?} does not contain {rule:?}"
        );
    };

    refused(
        api.get(format!(
            "/v1/envs/{env}/deployments/{revision}/tool-middlewares/Audit_Log"
        ))
        .header("Authorization", &fixture.bearer)
        .send()
        .await,
        MIDDLEWARE_RULE,
    )
    .await;
    refused(
        api.get(format!(
            "/v1/envs/{env}/deployments/{revision}/tools/Report_v2"
        ))
        .header("Authorization", &fixture.bearer)
        .send()
        .await,
        TOOL_RULE,
    )
    .await;

    let plan = api
        .get(format!("/v1/envs/{env}/plan"))
        .header("Authorization", &fixture.bearer)
        .send()
        .await;
    plan.assert_status_is_ok();
    let plan: Value = plan.0.into_body().into_json().await.unwrap();
    let deploy_body = |extra: Value| {
        let mut body = json!({
            "currentRevision": current_revision,
            "expectedDeploymentHash": plan["deploymentHash"],
            "version": "0.0.2",
            "agentSecretDefaults": [],
            "quotaResourceDefaults": [],
            "retryPolicyDefaults": []
        });
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        body
    };
    for (extra, rule) in [
        (json!({ "publishTools": ["Report/v2"] }), TOOL_RULE),
        (
            json!({ "publishToolMiddlewares": ["Audit_Log"] }),
            MIDDLEWARE_RULE,
        ),
        (
            json!({ "universalToolMiddlewares": [{ "name": "Audit_Log", "parameters": {} }] }),
            MIDDLEWARE_RULE,
        ),
    ] {
        refused(
            api.post(format!("/v1/envs/{env}/deployments"))
                .header("Authorization", &fixture.bearer)
                .body_json(&deploy_body(extra))
                .send()
                .await,
            rule,
        )
        .await;
    }

    refused(
        api.post(format!("/v1/envs/{env}/tools"))
            .header("Authorization", &fixture.bearer)
            .body_json(&json!({
                "release": {
                    "type": "ByCoordinates",
                    "account": fixture.account_email,
                    "name": "Report/v2",
                    "version": "1.0.0"
                },
                "automatic": false
            }))
            .send()
            .await,
        TOOL_RULE,
    )
    .await;

    // None of the refused requests created a deployment.
    let deployments = api
        .get(format!("/v1/envs/{env}/deployments"))
        .header("Authorization", &fixture.bearer)
        .send()
        .await;
    deployments.assert_status_is_ok();
    let deployments: Value = deployments.0.into_body().into_json().await.unwrap();
    assert_eq!(deployments["values"].as_array().unwrap().len(), 1);
}

/// A registry before the name checks accepted any text as a middleware name inside a component's
/// tool binding and stored it. These tests rewrite stored bytes to that shape; the registry must
/// still read the rows, so that the user can remove the binding.
mod stored_invalid_binding_name {
    use crate::api::make_open_api_service;
    use crate::bootstrap::Services;
    use crate::config::{ComponentCompilationConfig, LoginConfig, RegistryServiceConfig};
    use golem_common::config::{DbConfig, DbSqliteConfig};
    use golem_common::model::Empty;
    use golem_common::model::application::{ApplicationCreation, ApplicationName};
    use golem_common::model::component::{
        ComponentCreation, ComponentName, ComponentUpdate, ToolDeploymentConfigUpdate,
    };
    use golem_common::model::component_metadata::{ComponentMetadata, KnownExports};
    use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::tool::{
        ToolBindingInput, ToolDeploymentMetadata, ToolName, ToolProvisionConfig,
    };
    use golem_common::model::tool_middleware::{ToolMiddlewareInstallation, ToolMiddlewareName};
    use golem_common::schema::SchemaGraph;
    use golem_common::schema::tool::{
        CommandBody, CommandIndex, CommandNode, CommandTree, Doc, Globals, Positionals, Tool,
    };
    use golem_service_base::config::{BlobStorageConfig, LocalFileSystemBlobStorageConfig};
    use golem_service_base::model::auth::AuthCtx;
    use golem_service_base::repo::Blob;
    use poem::http::StatusCode;
    use poem::test::TestClient;
    use serde_json::json;
    use std::collections::BTreeMap;
    use test_r::{test, timeout};
    use tokio::task::JoinSet;

    fn config(directory: &std::path::Path) -> RegistryServiceConfig {
        RegistryServiceConfig {
            db: DbConfig::Sqlite(DbSqliteConfig {
                database: directory.join("registry.db").to_string_lossy().into(),
                max_connections: 4,
                foreign_keys: true,
            }),
            login: LoginConfig::Disabled(Empty {}),
            blob_storage: BlobStorageConfig::LocalFileSystem(LocalFileSystemBlobStorageConfig {
                root: directory.join("blobs"),
            }),
            component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
            ..Default::default()
        }
    }

    fn probe_tool() -> Tool {
        let node = |name: &str, subcommands, body| CommandNode {
            name: name.to_string(),
            aliases: Vec::new(),
            doc: Doc::default(),
            globals: Globals::default(),
            subcommands,
            body,
        };
        let body = CommandBody {
            positionals: Positionals::default(),
            options: Vec::new(),
            flags: Vec::new(),
            constraints: Vec::new(),
            stdin: None,
            stdout: None,
            stderr: None,
            result: None,
            errors: Vec::new(),
            annotations: None,
        };
        Tool {
            version: "1.0.0".to_string(),
            requires_filesystem: false,
            commands: CommandTree {
                nodes: vec![
                    node("probe", vec![CommandIndex(1)], None),
                    node("run", Vec::new(), Some(body)),
                ],
            },
            schema: SchemaGraph::empty(),
        }
    }

    /// A component without tools. The committed test wasms do not export the tool guest, so the
    /// tool and its binding are written into the stored metadata afterwards.
    fn creation() -> ComponentCreation {
        ComponentCreation {
            component_name: ComponentName("probe-tools".into()),
            config_schema: Default::default(),
            component_provision_config: Default::default(),
            agent_types: Vec::new(),
            agent_type_provision_configs: BTreeMap::new(),
            tools: Vec::new(),
            tool_deployment_configs: BTreeMap::new(),
            tool_middlewares: Vec::new(),
            tool_middleware_provision_configs: BTreeMap::new(),
        }
    }

    /// The metadata of a component whose tool `probe` binds the middleware `audit-log` for a
    /// component that the environment does not hold.
    fn metadata_with_binding() -> ComponentMetadata {
        let binding = ToolBindingInput {
            middleware: Some(vec![ToolMiddlewareInstallation {
                name: ToolMiddlewareName::try_from("audit-log").unwrap(),
                version: None,
                parameters: NormalizedJsonValue::new(json!({})),
                account: None,
                secret_keys_readable: None,
                secret_keys_revealable: None,
                filesystem_access: Default::default(),
            }]),
            ..Default::default()
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
                ToolName::try_from("probe").unwrap(),
                ToolDeploymentMetadata {
                    definition: probe_tool(),
                    provision: ToolProvisionConfig {
                        config: NormalizedJsonValue::new(json!({})),
                        env: BTreeMap::new(),
                        plugins: Vec::new(),
                        files: Vec::new(),
                    },
                    environment_binding: None,
                    component_bindings: BTreeMap::from([(
                        ComponentName("absent-component".into()),
                        binding,
                    )]),
                    agent_bindings: BTreeMap::new(),
                },
            )]),
        )
    }

    /// Test only: rewrites every `audit-log` in a stored blob column to `Audit_Log`, which are
    /// the bytes a registry without the name checks writes for that REST request.
    async fn store_invalid_name(pool: &sqlx::SqlitePool, table: &str, column: &str) -> usize {
        const GOOD: &[u8] = b"audit-log";
        const BAD: &[u8] = b"Audit_Log";
        let rows: Vec<(i64, Vec<u8>)> =
            sqlx::query_as(&format!("SELECT rowid, {column} FROM {table}"))
                .fetch_all(pool)
                .await
                .unwrap();
        let mut rewritten = 0;
        for (rowid, mut bytes) in rows {
            let mut hits = 0;
            let mut i = 0;
            while i + GOOD.len() <= bytes.len() {
                if &bytes[i..i + GOOD.len()] == GOOD {
                    bytes[i..i + GOOD.len()].copy_from_slice(BAD);
                    hits += 1;
                    i += GOOD.len();
                } else {
                    i += 1;
                }
            }
            if hits > 0 {
                sqlx::query(&format!("UPDATE {table} SET {column} = ? WHERE rowid = ?"))
                    .bind(bytes)
                    .bind(rowid)
                    .execute(pool)
                    .await
                    .unwrap();
                rewritten += hits;
            }
        }
        rewritten
    }

    #[test]
    #[timeout("300s")]
    async fn a_stored_invalid_binding_name_still_reads_and_can_be_removed() {
        let directory = tempfile::tempdir().unwrap();
        let config = config(directory.path());
        let root = config.initial_accounts["root"].clone();
        let bearer = format!("Bearer {}", root.token.as_ref().unwrap().secret());
        let auth = AuthCtx::system();
        let wasm = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../golem-common/wasm/shopping-cart-resource-versioned.wasm"
        ))
        .unwrap();

        // The registry creates the component.
        let mut tasks = JoinSet::new();
        let services = Services::new_without_component_builtins(&config, &mut tasks)
            .await
            .unwrap();
        let app = services
            .application_service
            .create(
                root.id,
                ApplicationCreation {
                    name: ApplicationName("stored-binding".into()),
                },
                &auth,
            )
            .await
            .unwrap();
        let env = services
            .environment_service
            .create(
                app.id,
                EnvironmentCreation {
                    name: EnvironmentName("test".into()),
                    compatibility_check: false,
                    tool_compatibility_mode: Default::default(),
                    version_check: false,
                    security_overrides: false,
                },
                &auth,
            )
            .await
            .unwrap();
        let user_auth = services
            .auth_service
            .authenticate_token(root.token.clone().unwrap())
            .await
            .unwrap();
        let component = services
            .component_write_service
            .create(env.id, creation(), wasm, None, &user_auth)
            .await
            .unwrap();
        drop(services);
        tasks.shutdown().await;

        // The same rows as an older registry writes them for the name `Audit_Log`.
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect(&format!(
                "sqlite://{}",
                directory.path().join("registry.db").display()
            ))
            .await
            .unwrap();
        sqlx::query("UPDATE component_revisions SET metadata = ?")
            .bind(Blob::new(metadata_with_binding()))
            .execute(&pool)
            .await
            .unwrap();
        assert!(store_invalid_name(&pool, "component_revisions", "metadata").await > 0);
        pool.close().await;

        // After a restart, the rows still read and the binding can be removed.
        let mut tasks = JoinSet::new();
        let services = Services::new_without_component_builtins(&config, &mut tasks)
            .await
            .unwrap();
        let api = TestClient::new(make_open_api_service(&services));
        let user_auth = services
            .auth_service
            .authenticate_token(root.token.clone().unwrap())
            .await
            .unwrap();
        for path in [
            format!("/v1/components/{}", component.id),
            format!("/v1/envs/{}/components", env.id),
        ] {
            let response = api.get(&path).header("Authorization", &bearer).send().await;
            response.assert_status(StatusCode::OK);
        }
        let stored = services
            .component_service
            .get_staged_component(component.id, &user_auth)
            .await
            .unwrap();
        assert_eq!(
            stored.metadata.tools()[&ToolName::try_from("probe").unwrap()]
                .component_bindings
                .len(),
            1
        );

        services
            .component_write_service
            .update(
                component.id,
                ComponentUpdate {
                    current_revision: component.revision,
                    config_schema: None,
                    component_provision_config: None,
                    agent_types: None,
                    agent_type_provision_config_updates: None,
                    tools: None,
                    tool_deployment_config_updates: Some(BTreeMap::from([(
                        ToolName::try_from("probe").unwrap(),
                        ToolDeploymentConfigUpdate {
                            provision: None,
                            environment_binding: Default::default(),
                            component_bindings: Some(BTreeMap::new()),
                            agent_bindings: None,
                        },
                    )])),
                    tool_middlewares: None,
                    tool_middleware_provision_config_updates: None,
                    allow_incompatible_config: false,
                },
                None,
                None,
                &user_auth,
            )
            .await
            .unwrap();
        let updated = services
            .component_service
            .get_staged_component(component.id, &user_auth)
            .await
            .unwrap();
        assert!(
            updated.metadata.tools()[&ToolName::try_from("probe").unwrap()]
                .component_bindings
                .is_empty()
        );
        api.get(format!("/v1/components/{}", component.id))
            .header("Authorization", &bearer)
            .send()
            .await
            .assert_status(StatusCode::OK);

        tasks.shutdown().await;
    }
}
