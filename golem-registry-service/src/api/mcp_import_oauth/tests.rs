use crate::api::make_open_api_service;
use crate::bootstrap::Services;
use crate::config::{ComponentCompilationConfig, LoginConfig, RegistryServiceConfig};
use golem_common::config::{DbConfig, DbSqliteConfig};
use golem_common::model::Empty;
use golem_common::model::application::{ApplicationCreation, ApplicationName};
use golem_common::model::card::owner::EmptyOwnerPattern;
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    ClassPermissionPattern, EffectiveSurface, NetworkResourcePattern, NetworkVerb,
    PermissionPattern, PortPattern,
};
use golem_common::model::environment::{EnvironmentCreation, EnvironmentName};
use golem_common::model::mcp_import::{
    McpImportAuthInput, McpImportBasicAuth, McpImportDeployment, McpImportSource,
};
use golem_common::model::security_scheme::{Provider, SecuritySchemeCreation, SecuritySchemeName};
use golem_common::model::tool::ToolBindingInput;
use golem_common::model::tool_middleware::{CompiledToolMiddlewareChain, RegisteredToolMiddleware};
use golem_service_base::clients::registry::{
    GrpcRegistryService, GrpcRegistryServiceConfig, RegistryService, RegistryServiceError,
};
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
async fn operator_and_runtime_routes_authenticate_and_target_exact_import() {
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
                tool_compatibility_mode: Default::default(),
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
    writer.execute(sqlx::query("INSERT INTO deployment_tool_middleware_snapshots (environment_id,deployment_revision_id,registered_middlewares,compiled_chains,compatibility_mode) VALUES ($1,1,$2,$3,'strict-equality')")
        .bind(env.id.0).bind(Blob::new(Vec::<RegisteredToolMiddleware>::new())).bind(Blob::new(Vec::<CompiledToolMiddlewareChain>::new()))).await.unwrap();
    let binding = ToolBindingInput::default();
    writer.execute(sqlx::query("INSERT INTO deployment_tool_middleware_bindings (environment_id,deployment_revision_id,scope,agent_type_name,tool_name,merge_mode,has_installations,config_keys_readable,secret_keys_readable,secret_keys_revealable) VALUES ($1,1,'universal','','',NULL,true,$2,$3,$4)")
        .bind(env.id.0).bind(Blob::new(binding.config_keys_readable)).bind(Blob::new(binding.secret_keys_readable)).bind(Blob::new(binding.secret_keys_revealable))).await.unwrap();
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
    let declared_path = format!("/v1/envs/{}/mcp-imports/oauth", env.id);
    let declared_import = json!({
        "import": {
            "url": "https://resource.invalid/mcp",
            "securityScheme": "provider"
        }
    });
    api.post(format!("{declared_path}/status"))
        .body_json(&declared_import)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
    for (suffix, body) in [
        ("authorize", declared_import.clone()),
        (
            "complete",
            json!({
                "import": declared_import["import"].clone(),
                "callback": {"state": "private-state", "code": "private-code"}
            }),
        ),
        ("disconnect", declared_import.clone()),
    ] {
        api.post(format!("{declared_path}/{suffix}"))
            .body_json(&body)
            .send()
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }
    let declared_status = api
        .post(format!("{declared_path}/status"))
        .header("Authorization", &bearer)
        .body_json(&declared_import)
        .send()
        .await;
    declared_status.assert_status_is_ok();
    declared_status
        .assert_json(json!({
            "environmentId": env.id.0,
            "securityScheme": "provider",
            "status": "authorization-required"
        }))
        .await;
    let declared_disconnected = api
        .post(format!("{declared_path}/disconnect"))
        .header("Authorization", &bearer)
        .body_json(&declared_import)
        .send()
        .await;
    declared_disconnected.assert_status_is_ok();
    declared_disconnected
        .assert_json(json!({
            "environmentId": env.id.0,
            "securityScheme": "provider",
            "status": "revoked"
        }))
        .await;
    api.post(format!("{declared_path}/authorize"))
        .header("Authorization", &bearer)
        .body_json(&json!({
            "import": {
                "url": "https://resource.invalid/mcp",
                "auth": {"bearer": "private-bearer"},
                "securityScheme": "provider"
            }
        }))
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
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
            "status": "revoked"
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

    let port = crate::grpc::start_grpc_server(
        &crate::config::GrpcApiConfig {
            port: 0,
            ..Default::default()
        },
        &services,
        &mut tasks,
    )
    .await
    .unwrap();
    let client = GrpcRegistryService::new(&GrpcRegistryServiceConfig {
        host: "127.0.0.1".into(),
        port,
        ..Default::default()
    });
    let recipient = RecipientPattern::Account {
        account: root.email.clone(),
    };
    let network = PermissionPattern::Network(ClassPermissionPattern {
        verb: Some(NetworkVerb::Connect),
        owner: EmptyOwnerPattern,
        recipient: recipient.clone(),
        resource: NetworkResourcePattern::host_port("resource.invalid", PortPattern::single(443)),
    });
    let runtime = AuthCtx::agent_with_effective_surface(
        root.id,
        root.email.clone(),
        EffectiveSurface::from_grants(&[network], &[], &[], &[], &recipient).unwrap(),
    );
    let mut source = McpImportSource {
        environment_id: env.id,
        deployment_revision: 1_u64.try_into().unwrap(),
        import_index: 3,
        upstream_tool_name: "tool".into(),
    };
    assert!(matches!(
        client.get_mcp_runtime_credential(&source, &runtime).await,
        Err(RegistryServiceError::BadRequest(_))
    ));
    assert!(matches!(
        client
            .get_mcp_runtime_credential(&source, &AuthCtx::System)
            .await,
        Err(RegistryServiceError::Unauthorized(_))
    ));
    for (offset, auth) in [
        None,
        Some(McpImportAuthInput {
            bearer: Some("private-bearer".into()),
            basic: None,
        }),
        Some(McpImportAuthInput {
            bearer: None,
            basic: Some(McpImportBasicAuth {
                user: "private-user".into(),
                password: "private-password".into(),
            }),
        }),
    ]
    .into_iter()
    .enumerate()
    {
        source.import_index = offset as u32 + 4;
        let (import, credential) = McpImportDeployment {
            url: "https://resource.invalid/mcp".into(),
            auth,
            security_scheme: None,
            prefix: None,
            include: None,
            exclude: None,
            version: None,
        }
        .into_parts(env.id)
        .unwrap();
        writer.execute(sqlx::query("INSERT INTO deployment_mcp_imports (environment_id,deployment_revision_id,import_index,import_hash,import_config,inline_credential) VALUES ($1,1,$2,$3,$4,$5)")
            .bind(env.id.0).bind(i64::from(source.import_index)).bind(vec![0_u8;32]).bind(Blob::new(import)).bind(credential.clone().map(Blob::new))).await.unwrap();
        let received = client
            .get_mcp_runtime_credential(&source, &runtime)
            .await
            .unwrap();
        assert_eq!(received.credential, credential);
        assert!(received.oauth_grant_generation.is_none());
        assert!(!format!("{received:?}").contains("private-"));
        client
            .report_mcp_resource_unauthorized(&source, &runtime, None)
            .await
            .unwrap();
        assert_eq!(
            client
                .get_mcp_runtime_credential(&source, &runtime)
                .await
                .unwrap()
                .credential,
            credential
        );
    }
}
