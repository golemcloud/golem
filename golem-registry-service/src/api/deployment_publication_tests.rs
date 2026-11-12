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
use crate::repo::component::{ComponentRepo, DbComponentRepo};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::component::ComponentRevisionRecord;
use crate::repo::model::hash::SqlBlake3Hash;
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::account::AccountId;
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::component_metadata::{ComponentMetadata, KnownExports};
use golem_common::model::deployment::DeploymentPlan;
use golem_common::model::diff::Hashable;
use golem_common::model::environment::{EnvironmentCreation, EnvironmentId, EnvironmentName};
use golem_common::model::environment_tool_grant::EnvironmentToolGrantCreation;
use golem_common::model::tool::{ToolDeploymentMetadata, ToolName, ToolProvisionConfig};
use golem_common::model::tool_release::{
    ToolReleaseByCoordinates, ToolReleaseLifecycle, ToolReleaseReference,
};
use golem_common::schema::graph::SchemaGraph;
use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::repo::Blob;
use poem::http::StatusCode;
use poem::test::TestClient;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use test_r::{test, timeout};
use tokio::task::JoinSet;
use uuid::Uuid;

fn registry_config(directory: &Path) -> (RegistryServiceConfig, DbSqliteConfig) {
    let db = DbSqliteConfig {
        database: directory.join("registry.db").to_string_lossy().into(),
        max_connections: 4,
        foreign_keys: true,
    };
    let config = RegistryServiceConfig {
        db: DbConfig::Sqlite(db.clone()),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    (config, db)
}

fn zeta_tool(alias: Option<&str>) -> Tool {
    Tool {
        version: "1.0.0".to_string(),
        requires_filesystem: false,
        commands: CommandTree {
            nodes: vec![CommandNode {
                name: "zeta".to_string(),
                aliases: alias.map(str::to_string).into_iter().collect(),
                doc: Doc::default(),
                globals: Globals::default(),
                subcommands: Vec::new(),
                body: None,
            }],
        },
        schema: SchemaGraph::empty(),
    }
}

/// Stores a component with one unbound tool straight through the repository. A deploy reads only
/// the stored records, never the binary, and the component write service would insist on a binary
/// that exports `golem:tool/guest@0.1.0`.
async fn store_zeta_component(
    repo: &impl ComponentRepo,
    owner: AccountId,
    environment: EnvironmentId,
    tool: Tool,
) {
    let tools = BTreeMap::from([(
        ToolName::try_from("zeta".to_string()).unwrap(),
        ToolDeploymentMetadata {
            definition: tool,
            provision: ToolProvisionConfig::default(),
            environment_binding: None,
            component_bindings: BTreeMap::new(),
            agent_bindings: BTreeMap::new(),
        },
    )]);
    let metadata = ComponentMetadata::from_parts_with_tools(
        KnownExports {
            tool_guest_interface: Some("golem:tool/guest@0.1.0".to_string()),
            ..KnownExports::default()
        },
        Vec::new(),
        None,
        None,
        Vec::new(),
        BTreeMap::new(),
        tools,
    );
    repo.create(
        environment.0,
        "tools:zeta",
        ComponentRevisionRecord {
            component_id: Uuid::new_v4(),
            revision_id: 0,
            hash: SqlBlake3Hash::empty(),
            audit: DeletableRevisionAuditFields::new(owner.0),
            size: 0.into(),
            metadata: Blob::new(metadata),
            object_store_key: String::new(),
            binary_hash: SqlBlake3Hash::empty(),
        },
        Vec::new(),
    )
    .await
    .unwrap();
}

#[test]
#[timeout("120s")]
async fn a_deploy_that_publishes_an_unimplemented_tool_or_middleware_is_a_bad_request() {
    let directory = tempfile::tempdir().unwrap();
    let (config, _db) = registry_config(directory.path());
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
                name: ApplicationName("deploy-publication".into()),
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

    let api = TestClient::new(make_open_api_service(&services));
    let bearer = format!("Bearer {}", root.token.as_ref().unwrap().secret());

    let plan = api
        .get(format!("/v1/envs/{}/plan", env.id))
        .header("Authorization", &bearer)
        .send()
        .await;
    plan.assert_status_is_ok();
    let plan: Value = plan.0.into_body().into_json().await.unwrap();
    let hash = plan["deploymentHash"].clone();

    let deploy_path = format!("/v1/envs/{}/deployments", env.id);
    let deploy_body = |publish: Option<(&str, &str)>| {
        let mut body = json!({
            "expectedDeploymentHash": hash,
            "version": "0.0.1",
            "agentSecretDefaults": [],
            "quotaResourceDefaults": [],
            "retryPolicyDefaults": [],
        });
        if let Some((field, name)) = publish {
            body[field] = json!([name]);
        }
        body
    };

    let cases = [
        (
            "publishTools",
            "report-v2",
            "TOOL_PUBLICATION_TOOL_NOT_FOUND",
            "Tool report-v2 selected for publication is not implemented by this deployment",
        ),
        (
            "publishToolMiddlewares",
            "audit-log",
            "TOOL_MIDDLEWARE_PUBLICATION_MIDDLEWARE_NOT_FOUND",
            "ToolMiddleware audit-log selected for publication is not implemented by this deployment",
        ),
    ];
    for (field, name, code, message) in cases {
        let response = api
            .post(&deploy_path)
            .header("Authorization", &bearer)
            .body_json(&deploy_body(Some((field, name))))
            .send()
            .await;
        let status = response.0.status();
        let body: Value = response.0.into_body().into_json().await.unwrap();
        assert_eq!(status, StatusCode::BAD_REQUEST, "{field}: {body}");
        assert_eq!(body["code"], code, "{field}: {body}");
        assert_eq!(body["errors"], json!([message]), "{field}: {body}");
    }

    // A rejected deploy stores nothing, so the same fixture still deploys.
    api.post(&deploy_path)
        .header("Authorization", &bearer)
        .body_json(&deploy_body(None))
        .send()
        .await
        .assert_status_is_ok();
}

#[test]
#[timeout("120s")]
async fn a_deploy_blocked_by_a_strict_following_grant_names_the_grant() {
    let directory = tempfile::tempdir().unwrap();
    let (config, db) = registry_config(directory.path());
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
                name: ApplicationName("strict-following".into()),
            },
            &auth,
        )
        .await
        .unwrap();
    let mut environments = BTreeMap::new();
    for (name, version_check) in [
        ("checked", true),
        ("follower", true),
        ("checked-again", true),
        ("loose", false),
    ] {
        let env = services
            .environment_service
            .create(
                app.id,
                EnvironmentCreation {
                    name: EnvironmentName(name.into()),
                    compatibility_check: false,
                    tool_compatibility_mode: Default::default(),
                    version_check,
                    security_overrides: false,
                },
                &auth,
            )
            .await
            .unwrap();
        environments.insert(name, env.id);
    }

    // Store every component before the first deploy, so that the two pools never write at the
    // same time.
    let pool = SqlitePool::configured(&db).await.unwrap();
    let component_repo = DbComponentRepo::logged(pool.clone());
    store_zeta_component(
        &component_repo,
        root.id,
        environments["checked"],
        zeta_tool(None),
    )
    .await;
    for name in ["checked-again", "loose"] {
        store_zeta_component(
            &component_repo,
            root.id,
            environments[name],
            zeta_tool(Some("changed")),
        )
        .await;
    }
    pool.close().await;

    let api = TestClient::new(make_open_api_service(&services));
    let bearer = format!("Bearer {}", root.token.as_ref().unwrap().secret());
    let deploy = |environment: EnvironmentId, hash: Value| {
        let api = &api;
        let bearer = &bearer;
        async move {
            let response = api
                .post(format!("/v1/envs/{environment}/deployments"))
                .header("Authorization", bearer)
                .body_json(&json!({
                    "expectedDeploymentHash": hash,
                    "version": "0.0.1",
                    "agentSecretDefaults": [],
                    "quotaResourceDefaults": [],
                    "retryPolicyDefaults": [],
                    "publishTools": ["zeta"],
                }))
                .send()
                .await;
            let status = response.0.status();
            let body: Value = response.0.into_body().into_json().await.unwrap();
            (status, body)
        }
    };

    let plan_hash = |environment: EnvironmentId| {
        let api = &api;
        let bearer = &bearer;
        async move {
            let plan = api
                .get(format!("/v1/envs/{environment}/plan"))
                .header("Authorization", bearer)
                .send()
                .await;
            plan.assert_status_is_ok();
            let plan: DeploymentPlan = plan.0.into_body().into_json().await.unwrap();
            let mut hash_input = plan.to_diffable();
            hash_input.published_tools.insert("zeta".to_string());
            json!(hash_input.hash().unwrap())
        }
    };

    // A version-checked environment publishes zeta 1.0.0.
    let (status, body) = deploy(
        environments["checked"],
        plan_hash(environments["checked"]).await,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Another version-checked environment follows that release by coordinates.
    services
        .environment_tool_grant_service
        .create(
            environments["follower"],
            EnvironmentToolGrantCreation {
                release: ToolReleaseReference::ByCoordinates(ToolReleaseByCoordinates {
                    account: root.email.clone(),
                    name: ToolName::try_from("zeta".to_string()).unwrap(),
                    version: "1.0.0".to_string(),
                }),
                automatic: false,
            },
            &auth,
        )
        .await
        .unwrap();

    let (status, body) = deploy(
        environments["loose"],
        plan_hash(environments["loose"]).await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(
        body["code"], "TOOL_RELEASE_STRICT_FOLLOWING_GRANT_CONFLICT",
        "{body}"
    );
    assert_eq!(
        body["error"],
        "Tool release coordinate is followed by a grant in a version-checked environment; use a new version",
        "{body}"
    );

    let (status, body) = deploy(
        environments["checked-again"],
        plan_hash(environments["checked-again"]).await,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert_eq!(body["code"], "TOOL_RELEASE_IMMUTABLE_CONFLICT", "{body}");
    assert_eq!(
        body["error"], "Tool release coordinate already exists with different immutable metadata",
        "{body}"
    );

    // Neither refused deploy changed the release.
    let releases = services
        .tool_release_service
        .list_in_account(root.id, &auth)
        .await
        .unwrap();
    let zeta: Vec<_> = releases
        .iter()
        .filter(|release| release.name.as_str() == "zeta")
        .collect();
    assert_eq!(zeta.len(), 1, "{zeta:?}");
    assert_eq!(zeta[0].version, "1.0.0");
    assert_eq!(zeta[0].lifecycle, ToolReleaseLifecycle::Published);
    assert_eq!(zeta[0].definition, zeta_tool(None));
}
