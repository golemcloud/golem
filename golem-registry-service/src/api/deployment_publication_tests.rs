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
use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::model::auth::AuthCtx;
use poem::http::StatusCode;
use poem::test::TestClient;
use serde_json::{Value, json};
use test_r::{test, timeout};
use tokio::task::JoinSet;

#[test]
#[timeout("120s")]
async fn a_deploy_that_publishes_an_unimplemented_tool_or_middleware_is_a_bad_request() {
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
