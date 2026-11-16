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
        _tasks: tasks,
        _directory: directory,
    }
}

/// Deploys the empty environment through the REST API and returns its revision.
async fn deploy_empty_environment(api: &TestClient<impl poem::Endpoint>, fixture: &Fixture) -> u64 {
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
    deployment["revision"].as_u64().unwrap()
}

#[test]
#[timeout("120s")]
async fn unknown_tool_middleware_name_is_a_client_error_and_the_registry_keeps_serving() {
    let fixture = fixture().await;
    let api = TestClient::new(make_open_api_service(&fixture.services));
    let revision = deploy_empty_environment(&api, &fixture).await;
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
