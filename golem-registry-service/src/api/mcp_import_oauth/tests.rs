use crate::api::make_open_api_service;
use crate::bootstrap::Services;
use crate::config::{ComponentCompilationConfig, LoginConfig, RegistryServiceConfig};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
use golem_common::model::mcp_import::McpImportDeployment;
use golem_common::model::security_scheme::{Provider, SecuritySchemeCreation, SecuritySchemeName};
use golem_service_base::config::BlobStorageConfig;
use golem_service_base::db::PoolApi;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::repo::{Blob, SqlDateTime};
use poem::http::StatusCode;
use poem::test::TestClient;
use serde_json::json;
use test_r::{test, timeout};
use tokio::task::JoinSet;

#[test]
#[timeout("120s")]
async fn operator_routes_authenticate_and_target_exact_import() {
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
        db: DbConfig::Sqlite(db.clone()),
        login: LoginConfig::Disabled(Empty {}),
        blob_storage: BlobStorageConfig::default_in_memory(),
        component_compilation: ComponentCompilationConfig::Disabled(Empty {}),
        ..Default::default()
    };
    let root = &config.initial_accounts["root"];
    let mut tasks = JoinSet::new();
    let services = Services::new(&config, &mut tasks).await.unwrap();
    let auth = AuthCtx::system();
    let app = services
        .application_service
        .create(
            root.id,
            ApplicationCreation {
                name: ApplicationName("oauth-routes".into()),
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
                version_check: false,
                security_overrides: false,
            },
            &auth,
        )
        .await
        .unwrap();
    services
        .security_scheme_service
        .create(
            env.id,
            SecuritySchemeCreation {
                name: SecuritySchemeName("provider".into()),
                provider_type: Provider::Google(Empty {}),
                client_id: "test-client".into(),
                client_secret: "test-secret".into(),
                redirect_url: "http://127.0.0.1:8765/callback".into(),
                scopes: vec!["tools".into()],
            },
            &auth,
        )
        .await
        .unwrap();

    let pool = SqlitePool::configured(&db).await.unwrap();
    let mut writer = pool.with_rw("oauth-routes-test", "seed");
    writer.execute(sqlx::query("INSERT INTO deployment_revisions (environment_id,revision_id,version,hash,created_at,created_by) VALUES ($1,1,'v1',$2,$3,$4)")
        .bind(env.id.0).bind(vec![0_u8; 32]).bind(SqlDateTime::now()).bind(root.id.0)).await.unwrap();
    let (import, _) = McpImportDeployment {
        url: "https://resource.invalid/mcp".into(),
        auth: None,
        security_scheme: Some(SecuritySchemeName("provider".into())),
        prefix: None,
        include: None,
        exclude: None,
        version: None,
    }
    .into_parts(env.id)
    .unwrap();
    writer.execute(sqlx::query("INSERT INTO deployment_mcp_imports (environment_id,deployment_revision_id,import_index,import_hash,import_config) VALUES ($1,1,3,$2,$3)")
        .bind(env.id.0).bind(vec![0_u8; 32]).bind(Blob::new(import))).await.unwrap();

    let api = TestClient::new(make_open_api_service(&services));
    let path = format!("/v1/envs/{}/deployments/1/mcp-imports/3/oauth", env.id);
    let bearer = format!("Bearer {}", root.token.as_ref().unwrap().secret());
    api.get(&path)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    for (method, suffix) in [
        ("POST", "/authorize"),
        ("POST", "/complete"),
        ("DELETE", ""),
    ] {
        api.request(method.parse().unwrap(), format!("{path}{suffix}"))
            .body_json(&json!({"state":"private-state","code":"private-code"}))
            .send()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
    let status = api.get(&path).header("Authorization", &bearer).send().await;
    status.assert_status_is_ok();
    status
        .assert_json(json!({
            "environmentId": env.id.0,
            "deploymentRevision": 1,
            "importIndex": 3,
            "securityScheme": "provider",
            "status": "authorization-required"
        }))
        .await;
    let missing = path.replace("/mcp-imports/3/", "/mcp-imports/0/");
    api.post(format!("{missing}/authorize"))
        .header("Authorization", &bearer)
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
    api.get(path.replace("/deployments/1/", "/deployments/2/"))
        .header("Authorization", &bearer)
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let callback = api
        .post(format!("{path}/complete"))
        .header("Authorization", &bearer)
        .body_json(&json!({"state":"private-state","code":"private-code"}))
        .send()
        .await;
    callback.assert_status(StatusCode::BAD_REQUEST);
    callback
        .assert_json(json!({
            "errors": ["Invalid, expired, or already consumed MCP OAuth callback"],
            "code": golem_common::model::api::error_code::INVALID_OAUTH_SESSION
        }))
        .await;
    let disconnected = api
        .delete(&path)
        .header("Authorization", &bearer)
        .send()
        .await;
    disconnected.assert_status_is_ok();
    disconnected
        .json()
        .await
        .value()
        .object()
        .get("status")
        .assert_string("revoked");
    api.get(&path)
        .header("Authorization", &bearer)
        .send()
        .await
        .json()
        .await
        .value()
        .object()
        .get("status")
        .assert_string("revoked");
}
