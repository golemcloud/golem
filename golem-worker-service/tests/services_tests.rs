// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use async_trait::async_trait;
use chrono::Utc;
use golem_common::config::{DbPostgresConfig, DbSqliteConfig, RedisConfig};
use golem_common::model::auth::{AuthCtx, Namespace, ProjectAction, TokenSecret};
use golem_common::model::component::{ComponentOwner, VersionedComponentId};
use golem_common::model::component_constraint::{FunctionConstraints, FunctionSignature};
use golem_common::model::{AccountId, ComponentId, ComponentType, ProjectId, RetryConfig};
use golem_common::redis::RedisPool;
use golem_service_base::clients::auth::AuthServiceError;
use golem_service_base::db;
use golem_service_base::db::postgres::PostgresPool;
use golem_service_base::db::sqlite::SqlitePool;
use golem_service_base::migration::{Migrations, MigrationsDir};
use golem_service_base::model::{Component, ComponentName};
use golem_wasm_ast::analysis::analysed_type::str;
use golem_wasm_ast::analysis::{
    AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
    AnalysedInstance,
};
use golem_worker_service::api;
use golem_worker_service::gateway_api_definition::http::HttpApiDefinition;
use golem_worker_service::gateway_api_definition::http::HttpApiDefinitionRequest;
use golem_worker_service::gateway_api_definition::{ApiDefinitionId, ApiVersion};
use golem_worker_service::gateway_api_deployment::{ApiDeploymentRequest, ApiSite, ApiSiteString};
use golem_worker_service::gateway_execution::gateway_session::{
    DataKey, DataValue, GatewaySession, GatewaySessionError, RedisGatewaySession,
    RedisGatewaySessionExpiration, SessionId, SqliteGatewaySession, SqliteGatewaySessionExpiration,
};
use golem_worker_service::gateway_security::{
    AuthorizationUrl, DefaultIdentityProvider, GolemIdentityProviderMetadata, IdentityProvider,
    IdentityProviderError, OpenIdClient, Provider, SecurityScheme, SecuritySchemeIdentifier,
};
use golem_worker_service::model::{ApiDomain, Certificate, CertificateRequest, DomainRequest};
use golem_worker_service::repo::api_certificate::{ApiCertificateRepo, DbApiCertificateRepo};
use golem_worker_service::repo::api_domain::{ApiDomainRepo, DbApiDomainRepo};
use golem_worker_service::repo::security_scheme::{DbSecuritySchemeRepo, SecuritySchemeRepo};
use golem_worker_service::repo::{api_definition, api_deployment};
use golem_worker_service::service::api_certificate::{
    CertificateManager, CertificateService, CertificateServiceDefault, InMemoryCertificateManager,
};
use golem_worker_service::service::api_domain::{
    ApiDomainService, ApiDomainServiceDefault, InMemoryRegisterDomain, RegisterDomain,
};
use golem_worker_service::service::auth::AuthService;
use golem_worker_service::service::component::{
    ComponentResult, ComponentService, ComponentServiceError,
};
use golem_worker_service::service::gateway::api_definition::{
    ApiDefinitionError, ApiDefinitionIdWithVersion, ApiDefinitionService,
    ApiDefinitionServiceConfig, ApiDefinitionServiceDefault,
};
use golem_worker_service::service::gateway::api_deployment::{
    ApiDeploymentError, ApiDeploymentService, ApiDeploymentServiceDefault,
};
use golem_worker_service::service::gateway::http_api_definition_validator::HttpApiDefinitionValidator;
use golem_worker_service::service::gateway::security_scheme::{
    DefaultSecuritySchemeService, SecuritySchemeService,
};
use golem_worker_service::service::gateway::{ComponentView, ConversionContext};
use openidconnect::core::{
    CoreClaimName, CoreClaimType, CoreClientAuthMethod, CoreGrantType, CoreIdTokenClaims,
    CoreIdTokenVerifier, CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm,
    CoreJwsSigningAlgorithm, CoreProviderMetadata, CoreResponseMode, CoreResponseType,
    CoreSubjectIdentifierType, CoreTokenResponse,
};
use openidconnect::{
    AuthUrl, AuthenticationContextClass, AuthorizationCode, ClientId, ClientSecret, CsrfToken,
    IssuerUrl, JsonWebKeySetUrl, Nonce, RedirectUrl, RegistrationUrl, ResponseTypes, Scope,
    TokenUrl, UserInfoUrl,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use test_r::test;
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ImageExt};
use testcontainers_modules::postgres::Postgres;
use uuid::uuid;
use uuid::Uuid;

test_r::enable!();

struct TestAuthService;

impl TestAuthService {
    fn new() -> Self {
        Self
    }
}

#[async_trait]
impl AuthService for TestAuthService {
    async fn get_account(&self, ctx: &AuthCtx) -> Result<AccountId, AuthServiceError> {
        Ok(AccountId::from(ctx.token_secret.value.to_string().as_str()))
    }

    async fn authorize_project_action(
        &self,
        project_id: &ProjectId,
        _permission: ProjectAction,
        ctx: &AuthCtx,
    ) -> Result<Namespace, AuthServiceError> {
        Ok(Namespace::new(
            project_id.clone(),
            AccountId::from(ctx.token_secret.value.to_string().as_str()),
        ))
    }

    async fn is_authorized_by_component(
        &self,
        component_id: &ComponentId,
        _permission: ProjectAction,
        ctx: &AuthCtx,
    ) -> Result<Namespace, AuthServiceError> {
        Ok(Namespace::new(
            ProjectId(component_id.0),
            AccountId::from(ctx.token_secret.value.to_string().as_str()),
        ))
    }
}

pub struct TestSessionStore;

#[async_trait]
impl GatewaySession for TestSessionStore {
    async fn insert(
        &self,
        _session_id: SessionId,
        _data_key: DataKey,
        _data_value: DataValue,
    ) -> Result<(), GatewaySessionError> {
        Ok(())
    }

    async fn get(
        &self,
        _session_id: &SessionId,
        _data_key: &DataKey,
    ) -> Result<DataValue, GatewaySessionError> {
        Err(GatewaySessionError::InternalError(
            "Backend unimplemented".to_string(),
        ))
    }
}

#[derive(Clone)]
pub struct TestIdentityProvider;

#[async_trait]
impl IdentityProvider for TestIdentityProvider {
    async fn get_provider_metadata(
        &self,
        _provider: &Provider,
    ) -> Result<GolemIdentityProviderMetadata, IdentityProviderError> {
        Ok(get_test_provider_metadata())
    }

    async fn exchange_code_for_tokens(
        &self,
        _client: &OpenIdClient,
        _code: &AuthorizationCode,
    ) -> Result<CoreTokenResponse, IdentityProviderError> {
        Err(IdentityProviderError::ClientInitError(
            "Not implemented".to_string(),
        ))
    }

    async fn get_client(
        &self,
        security_scheme: &SecurityScheme,
    ) -> Result<OpenIdClient, IdentityProviderError> {
        let identity_provider = DefaultIdentityProvider;
        identity_provider.get_client(security_scheme).await
    }

    fn get_id_token_verifier<'a>(&self, client: &'a OpenIdClient) -> CoreIdTokenVerifier<'a> {
        let provider = DefaultIdentityProvider;
        provider.get_id_token_verifier(client)
    }

    fn get_claims(
        &self,
        _id_token_verifier: &CoreIdTokenVerifier,
        _core_token_response: CoreTokenResponse,
        _nonce: &Nonce,
    ) -> Result<CoreIdTokenClaims, IdentityProviderError> {
        Err(IdentityProviderError::ClientInitError(
            "Not implemented".to_string(),
        ))
    }

    fn get_authorization_url(
        &self,
        client: &OpenIdClient,
        scopes: Vec<Scope>,
        _state: Option<CsrfToken>,
        _nonce: Option<Nonce>,
    ) -> AuthorizationUrl {
        let identity_provider = DefaultIdentityProvider;
        identity_provider.get_authorization_url(
            client,
            scopes,
            Some(CsrfToken::new("token".to_string())),
            Some(Nonce::new("nonce".to_string())),
        )
    }
}

fn get_test_provider_metadata() -> GolemIdentityProviderMetadata {
    let all_signing_algs = vec![
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
        CoreJwsSigningAlgorithm::EcdsaP256Sha256,
        CoreJwsSigningAlgorithm::EcdsaP384Sha384,
        CoreJwsSigningAlgorithm::EcdsaP521Sha512,
        CoreJwsSigningAlgorithm::HmacSha256,
        CoreJwsSigningAlgorithm::HmacSha384,
        CoreJwsSigningAlgorithm::HmacSha512,
        CoreJwsSigningAlgorithm::RsaSsaPssSha256,
        CoreJwsSigningAlgorithm::RsaSsaPssSha384,
        CoreJwsSigningAlgorithm::RsaSsaPssSha512,
        CoreJwsSigningAlgorithm::None,
    ];
    let all_encryption_algs = vec![
        CoreJweKeyManagementAlgorithm::RsaPkcs1V15,
        CoreJweKeyManagementAlgorithm::RsaOaep,
        CoreJweKeyManagementAlgorithm::RsaOaepSha256,
        CoreJweKeyManagementAlgorithm::AesKeyWrap128,
        CoreJweKeyManagementAlgorithm::AesKeyWrap192,
        CoreJweKeyManagementAlgorithm::AesKeyWrap256,
        CoreJweKeyManagementAlgorithm::EcdhEs,
        CoreJweKeyManagementAlgorithm::EcdhEsAesKeyWrap128,
        CoreJweKeyManagementAlgorithm::EcdhEsAesKeyWrap192,
        CoreJweKeyManagementAlgorithm::EcdhEsAesKeyWrap256,
    ];
    let new_provider_metadata = CoreProviderMetadata::new(
        IssuerUrl::new("https://accounts.google.com".to_string()).unwrap(),
        AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string()).unwrap(),
        JsonWebKeySetUrl::new("https://www.googleapis.com/oauth2/v3/certs".to_string()).unwrap(),
        vec![ResponseTypes::new(vec![CoreResponseType::Code])],
        vec![
            CoreSubjectIdentifierType::Public,
            CoreSubjectIdentifierType::Pairwise,
        ],
        all_signing_algs.clone(),
        Default::default(),
    )
    .set_request_object_signing_alg_values_supported(Some(all_signing_algs.clone()))
    .set_token_endpoint_auth_signing_alg_values_supported(Some(vec![
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha256,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha384,
        CoreJwsSigningAlgorithm::RsaSsaPkcs1V15Sha512,
        CoreJwsSigningAlgorithm::EcdsaP256Sha256,
        CoreJwsSigningAlgorithm::EcdsaP384Sha384,
        CoreJwsSigningAlgorithm::EcdsaP521Sha512,
        CoreJwsSigningAlgorithm::HmacSha256,
        CoreJwsSigningAlgorithm::HmacSha384,
        CoreJwsSigningAlgorithm::HmacSha512,
        CoreJwsSigningAlgorithm::RsaSsaPssSha256,
        CoreJwsSigningAlgorithm::RsaSsaPssSha384,
        CoreJwsSigningAlgorithm::RsaSsaPssSha512,
    ]))
    .set_scopes_supported(Some(vec![
        Scope::new("email".to_string()),
        Scope::new("phone".to_string()),
        Scope::new("profile".to_string()),
        Scope::new("openid".to_string()),
        Scope::new("address".to_string()),
        Scope::new("offline_access".to_string()),
        Scope::new("openid".to_string()),
    ]))
    .set_userinfo_signing_alg_values_supported(Some(all_signing_algs))
    .set_id_token_encryption_enc_values_supported(Some(vec![
        CoreJweContentEncryptionAlgorithm::Aes128CbcHmacSha256,
        CoreJweContentEncryptionAlgorithm::Aes192CbcHmacSha384,
        CoreJweContentEncryptionAlgorithm::Aes256CbcHmacSha512,
        CoreJweContentEncryptionAlgorithm::Aes128Gcm,
        CoreJweContentEncryptionAlgorithm::Aes192Gcm,
        CoreJweContentEncryptionAlgorithm::Aes256Gcm,
    ]))
    .set_grant_types_supported(Some(vec![
        CoreGrantType::AuthorizationCode,
        CoreGrantType::Implicit,
        CoreGrantType::JwtBearer,
        CoreGrantType::RefreshToken,
    ]))
    .set_response_modes_supported(Some(vec![
        CoreResponseMode::Query,
        CoreResponseMode::Fragment,
        CoreResponseMode::FormPost,
    ]))
    .set_require_request_uri_registration(Some(true))
    .set_registration_endpoint(Some(
        RegistrationUrl::new(
            "https://accounts.google.com/openidconnect-rs/\
                 rp-response_type-code/registration"
                .to_string(),
        )
        .unwrap(),
    ))
    .set_claims_parameter_supported(Some(true))
    .set_request_object_encryption_enc_values_supported(Some(vec![
        CoreJweContentEncryptionAlgorithm::Aes128CbcHmacSha256,
        CoreJweContentEncryptionAlgorithm::Aes192CbcHmacSha384,
        CoreJweContentEncryptionAlgorithm::Aes256CbcHmacSha512,
        CoreJweContentEncryptionAlgorithm::Aes128Gcm,
        CoreJweContentEncryptionAlgorithm::Aes192Gcm,
        CoreJweContentEncryptionAlgorithm::Aes256Gcm,
    ]))
    .set_userinfo_endpoint(Some(
        UserInfoUrl::new("https://openidconnect.googleapis.com/v1/userinfo".to_string()).unwrap(),
    ))
    .set_token_endpoint_auth_methods_supported(Some(vec![
        CoreClientAuthMethod::ClientSecretPost,
        CoreClientAuthMethod::ClientSecretBasic,
        CoreClientAuthMethod::ClientSecretJwt,
        CoreClientAuthMethod::PrivateKeyJwt,
    ]))
    .set_claims_supported(Some(
        vec![
            "name",
            "given_name",
            "middle_name",
            "picture",
            "email_verified",
            "birthdate",
            "sub",
            "address",
            "zoneinfo",
            "email",
            "gender",
            "preferred_username",
            "family_name",
            "website",
            "profile",
            "phone_number_verified",
            "nickname",
            "updated_at",
            "phone_number",
            "locale",
        ]
        .iter()
        .map(|claim| CoreClaimName::new((*claim).to_string()))
        .collect(),
    ))
    .set_request_object_encryption_alg_values_supported(Some(all_encryption_algs.clone()))
    .set_claim_types_supported(Some(vec![
        CoreClaimType::Normal,
        CoreClaimType::Aggregated,
        CoreClaimType::Distributed,
    ]))
    .set_request_uri_parameter_supported(Some(true))
    .set_request_parameter_supported(Some(true))
    .set_token_endpoint(Some(
        TokenUrl::new("https://oauth2.googleapis.com/token".to_string()).unwrap(),
    ))
    .set_id_token_encryption_alg_values_supported(Some(all_encryption_algs.clone()))
    .set_userinfo_encryption_alg_values_supported(Some(all_encryption_algs))
    .set_userinfo_encryption_enc_values_supported(Some(vec![
        CoreJweContentEncryptionAlgorithm::Aes128CbcHmacSha256,
        CoreJweContentEncryptionAlgorithm::Aes192CbcHmacSha384,
        CoreJweContentEncryptionAlgorithm::Aes256CbcHmacSha512,
        CoreJweContentEncryptionAlgorithm::Aes128Gcm,
        CoreJweContentEncryptionAlgorithm::Aes192Gcm,
        CoreJweContentEncryptionAlgorithm::Aes256Gcm,
    ]))
    .set_acr_values_supported(Some(vec![AuthenticationContextClass::new(
        "PASSWORD".to_string(),
    )]));

    new_provider_metadata
}

struct TestComponentService;

impl TestComponentService {
    pub fn test_component() -> Component {
        use golem_common::model::component_metadata::ComponentMetadata;
        use golem_service_base::model::ComponentName;

        let id = VersionedComponentId {
            component_id: ComponentId::try_from("0b6d9cd8-f373-4e29-8a5a-548e61b868a5").unwrap(),
            version: 0,
        };

        Component {
            owner: ComponentOwner {
                account_id: AccountId {
                    value: uuid!("6588448A-3BA7-46DF-A437-DA826ED15C3D").to_string(),
                },
                project_id: ProjectId(uuid!("8877FD9B-9C80-4EA1-9926-67581EE48DBA")),
            },
            versioned_component_id: id.clone(),
            component_name: ComponentName("test".to_string()),
            component_size: 0,
            metadata: ComponentMetadata::from_parts(
                Self::get_metadata(),
                vec![],
                HashMap::new(),
                Some("golem:it".to_string()),
                None,
                vec![],
            ),
            created_at: Utc::now(),
            component_type: ComponentType::Durable,
            files: vec![],
            installed_plugins: vec![],
            env: HashMap::new(),
        }
    }

    fn get_metadata() -> Vec<AnalysedExport> {
        let analysed_export = AnalysedExport::Instance(AnalysedInstance {
            name: "golem:it/api".to_string(),
            functions: vec![AnalysedFunction {
                name: "get-cart-contents".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "a".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult { typ: str() }),
            }],
        });

        vec![analysed_export]
    }
}

#[async_trait]
impl ComponentService for TestComponentService {
    async fn get_by_version(
        &self,
        _component_id: &ComponentId,
        _version: u64,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component> {
        Ok(Self::test_component())
    }

    async fn get_latest(
        &self,
        _component_id: &ComponentId,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component> {
        Ok(Self::test_component())
    }

    async fn get_by_name(
        &self,
        name: &ComponentName,
        _namespace: &Namespace,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<Component> {
        let test_component = Self::test_component();
        if name.0 == test_component.component_name.0 {
            Ok(test_component)
        } else {
            Err(ComponentServiceError::NotFound(format!(
                "component not found for name: {name}"
            )))
        }
    }

    async fn create_or_update_constraints(
        &self,
        _component_id: &ComponentId,
        _constraints: FunctionConstraints,
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<FunctionConstraints> {
        Ok(FunctionConstraints {
            constraints: vec![],
        })
    }

    async fn delete_constraints(
        &self,
        _component_id: &ComponentId,
        _constraints: &[FunctionSignature],
        _auth_ctx: &AuthCtx,
    ) -> ComponentResult<FunctionConstraints> {
        Ok(FunctionConstraints {
            constraints: vec![],
        })
    }
}

struct SqliteDb {
    db_path: String,
}

impl Default for SqliteDb {
    fn default() -> Self {
        Self {
            db_path: format!("/tmp/golem-worker-{}.db", Uuid::new_v4()),
        }
    }
}

impl Drop for SqliteDb {
    fn drop(&mut self) {
        std::fs::remove_file(&self.db_path).unwrap();
    }
}

async fn start_docker_postgres() -> (DbPostgresConfig, ContainerAsync<Postgres>) {
    let container = tryhard::retry_fn(|| Postgres::default().with_tag("14.7-alpine").start())
        .retries(5)
        .exponential_backoff(Duration::from_millis(10))
        .max_delay(Duration::from_secs(10))
        .await
        .expect("Failed to start postgres container");

    let config = DbPostgresConfig {
        host: "localhost".to_string(),
        port: container
            .get_host_port_ipv4(5432)
            .await
            .expect("Failed to get port"),
        database: "postgres".to_string(),
        username: "postgres".to_string(),
        password: "postgres".to_string(),
        schema: Some("test".to_string()),
        max_connections: 10,
    };

    (config, container)
}

async fn start_docker_redis() -> (
    RedisConfig,
    ContainerAsync<testcontainers_modules::redis::Redis>,
) {
    let container = tryhard::retry_fn(|| {
        testcontainers_modules::redis::Redis::default()
            .with_tag("6.2.6")
            .start()
    })
    .retries(5)
    .exponential_backoff(Duration::from_millis(10))
    .max_delay(Duration::from_secs(10))
    .await
    .expect("Failed to start redis container");

    let redis_config = RedisConfig {
        host: "localhost".to_string(),
        port: container
            .get_host_port_ipv4(6379)
            .await
            .expect("Failed to get port"),
        database: 0,
        tracing: false,
        pool_size: 10,
        retries: RetryConfig::default(),
        key_prefix: "".to_string(),
        username: None,
        password: None,
    };

    (redis_config, container)
}

fn test_namespace() -> Namespace {
    Namespace {
        account_id: AccountId {
            value: uuid!("a92803c1-186a-4367-bc00-23faffb5c932").to_string(),
        },
        project_id: ProjectId(uuid!("44f28456-d0c2-45d2-aaad-6e85462b6f18")),
    }
}

fn test_auth_ctx() -> AuthCtx {
    AuthCtx {
        token_secret: TokenSecret {
            value: uuid!("d9dc8622-7239-416f-96ca-0f8ac7b43746"),
        },
    }
}

#[test]
pub async fn test_with_postgres_db() {
    let (db_config, _container) = start_docker_postgres().await;

    db::postgres::migrate(
        &db_config,
        MigrationsDir::new("../golem-worker-service/db/migration".into()).postgres_migrations(),
    )
    .await
    .unwrap();

    let db_pool = PostgresPool::configured(&db_config).await.unwrap();

    let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo + Sync + Send> =
        Arc::new(api_definition::DbApiDefinitionRepo::new(db_pool.clone()));
    let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo + Sync + Send> =
        Arc::new(api_deployment::DbApiDeploymentRepo::new(db_pool.clone()));

    let security_scheme_repo: Arc<dyn SecuritySchemeRepo + Sync + Send> =
        Arc::new(DbSecuritySchemeRepo::new(db_pool.clone()));

    let api_certificate_repo: Arc<dyn ApiCertificateRepo> =
        Arc::new(DbApiCertificateRepo::new(db_pool.clone()));

    let api_domain_repo: Arc<dyn ApiDomainRepo> = Arc::new(DbApiDomainRepo::new(db_pool.clone()));

    test_services(
        api_definition_repo,
        api_deployment_repo,
        security_scheme_repo,
        api_certificate_repo,
        api_domain_repo,
    )
    .await;
}

#[test]
pub async fn test_gateway_session_with_sqlite() {
    let db = SqliteDb::default();
    let db_config = DbSqliteConfig {
        database: db.db_path.clone(),
        max_connections: 10,
    };

    let db_pool = SqlitePool::configured(&db_config).await.unwrap();

    let data_value = DataValue(serde_json::Value::String(
        Nonce::new_random().secret().to_string(),
    ));

    let value = insert_and_get_session_with_sqlite(
        SessionId("test1".to_string()),
        DataKey::nonce(),
        data_value.clone(),
        db_pool.clone(),
    )
    .await
    .expect("Expecting a value for longer expiry");

    assert_eq!(value, data_value.clone());
}

#[test]
pub async fn test_gateway_session_with_sqlite_expired() {
    let db = SqliteDb::default();
    let db_config = DbSqliteConfig {
        database: db.db_path.clone(),
        max_connections: 10,
    };

    let pool = SqlitePool::configured(&db_config).await.unwrap();

    let data_value = DataValue(serde_json::Value::String(
        Nonce::new_random().secret().to_string(),
    ));

    let expiration =
        SqliteGatewaySessionExpiration::new(Duration::from_secs(1), Duration::from_secs(1));

    let sqlite_session = SqliteGatewaySession::new(pool.clone(), expiration.clone())
        .await
        .expect("Failed to create sqlite session");

    let session_store = Arc::new(sqlite_session);

    let data_key = DataKey::nonce();
    let session_id = SessionId("test1".to_string());

    session_store
        .insert(session_id.clone(), data_key.clone(), data_value)
        .await
        .expect("Insert to session failed");

    SqliteGatewaySession::cleanup_expired(pool, SqliteGatewaySession::current_time() + 10)
        .await
        .expect("Failed to cleanup expired sessions");

    let result = session_store.get(&session_id, &data_key).await;

    assert!(matches!(
        result,
        Err(GatewaySessionError::MissingValue { .. })
    ));
}

#[test]
pub async fn test_gateway_session_redis() {
    let (redis_config, _container) = start_docker_redis().await;

    let redis = RedisPool::configured(&redis_config).await.unwrap();

    let data_value = DataValue(serde_json::Value::String(
        Nonce::new_random().secret().to_string(),
    ));

    // Longer Expiry in Redis returns value
    let value = insert_and_get_with_redis(
        SessionId("test1".to_string()),
        DataKey::nonce(),
        data_value.clone(),
        60 * 60,
        &redis,
    )
    .await
    .expect("Expecting a value for longer expiry");

    assert_eq!(value, data_value.clone());

    // Instant expiry in Redis returns missing value, and we should get missing value
    let result = insert_and_get_with_redis(
        SessionId("test2".to_string()),
        DataKey::nonce(),
        data_value.clone(),
        0,
        &redis,
    )
    .await;

    assert!(matches!(
        result,
        Err(GatewaySessionError::MissingValue { .. })
    ));
}

async fn insert_and_get_with_redis(
    session_id: SessionId,
    data_key: DataKey,
    data_value: DataValue,
    redis_expiry_in_seconds: u64,
    redis: &RedisPool,
) -> Result<DataValue, GatewaySessionError> {
    let session_store = Arc::new(RedisGatewaySession::new(
        redis.clone(),
        RedisGatewaySessionExpiration::new(Duration::from_secs(redis_expiry_in_seconds)),
    ));

    session_store
        .insert(session_id.clone(), data_key.clone(), data_value)
        .await?;

    session_store.get(&session_id, &data_key).await
}

async fn insert_and_get_session_with_sqlite(
    session_id: SessionId,
    data_key: DataKey,
    data_value: DataValue,
    db_pool: SqlitePool,
) -> Result<DataValue, GatewaySessionError> {
    let sqlite_session =
        SqliteGatewaySession::new(db_pool, SqliteGatewaySessionExpiration::default())
            .await
            .map_err(|err| GatewaySessionError::InternalError(err.to_string()))?;

    let session_store = Arc::new(sqlite_session);

    session_store
        .insert(session_id.clone(), data_key.clone(), data_value)
        .await?;

    session_store.get(&session_id, &data_key).await
}

#[test]
pub async fn test_with_sqlite_db() {
    let db = SqliteDb::default();
    let db_config = DbSqliteConfig {
        database: db.db_path.clone(),
        max_connections: 10,
    };

    db::sqlite::migrate(
        &db_config,
        MigrationsDir::new("../golem-worker-service/db/migration".into()).sqlite_migrations(),
    )
    .await
    .unwrap();

    let db_pool = SqlitePool::configured(&db_config).await.unwrap();

    let api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo> =
        Arc::new(api_definition::DbApiDefinitionRepo::new(db_pool.clone()));

    let api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo> =
        Arc::new(api_deployment::DbApiDeploymentRepo::new(db_pool.clone()));

    let security_scheme_repo: Arc<dyn SecuritySchemeRepo> =
        Arc::new(DbSecuritySchemeRepo::new(db_pool.clone()));

    let api_certificate_repo: Arc<dyn ApiCertificateRepo> =
        Arc::new(DbApiCertificateRepo::new(db_pool.clone()));

    let api_domain_repo: Arc<dyn ApiDomainRepo> = Arc::new(DbApiDomainRepo::new(db_pool.clone()));

    test_services(
        api_definition_repo,
        api_deployment_repo,
        security_scheme_repo,
        api_certificate_repo,
        api_domain_repo,
    )
    .await;
}

async fn test_services(
    api_definition_repo: Arc<dyn api_definition::ApiDefinitionRepo>,
    api_deployment_repo: Arc<dyn api_deployment::ApiDeploymentRepo>,
    security_scheme_repo: Arc<dyn SecuritySchemeRepo>,
    api_certificate_repo: Arc<dyn ApiCertificateRepo>,
    api_domain_repo: Arc<dyn ApiDomainRepo>,
) {
    let component_service: Arc<dyn ComponentService> = Arc::new(TestComponentService {});

    let api_definition_validator_service = Arc::new(HttpApiDefinitionValidator {});

    let identity_provider_resolver = Arc::new(TestIdentityProvider);

    let security_scheme_service: Arc<dyn SecuritySchemeService> = Arc::new(
        DefaultSecuritySchemeService::new(security_scheme_repo, identity_provider_resolver),
    );

    let definition_service: Arc<dyn ApiDefinitionService> =
        Arc::new(ApiDefinitionServiceDefault::new(
            component_service.clone(),
            api_definition_repo.clone(),
            api_deployment_repo.clone(),
            security_scheme_service.clone(),
            api_definition_validator_service.clone(),
            ApiDefinitionServiceConfig::default(),
        ));

    let deployment_service: Arc<dyn ApiDeploymentService> =
        Arc::new(ApiDeploymentServiceDefault::new(
            api_deployment_repo.clone(),
            api_definition_repo.clone(),
            component_service.clone(),
        ));

    let certificate_manager: Arc<dyn CertificateManager + Sync + Send> =
        Arc::new(InMemoryCertificateManager::default());

    let auth_service: Arc<dyn AuthService> = Arc::new(TestAuthService::new());

    let certificate_service: Arc<dyn CertificateService + Sync + Send> =
        Arc::new(CertificateServiceDefault::new(
            auth_service.clone(),
            certificate_manager.clone(),
            api_certificate_repo.clone(),
        ));

    let domain_register_service: Arc<dyn RegisterDomain> =
        Arc::new(InMemoryRegisterDomain::default());

    let domain_service: Arc<dyn ApiDomainService> = Arc::new(ApiDomainServiceDefault::new(
        auth_service.clone(),
        domain_register_service.clone(),
        api_domain_repo.clone(),
    ));

    test_security_crud(security_scheme_service.clone()).await;
    test_definition_crud(definition_service.clone()).await;
    test_delete_non_existing(definition_service.clone()).await;
    test_deployment(definition_service.clone(), deployment_service.clone()).await;
    test_deployment_conflict(definition_service.clone(), deployment_service.clone()).await;
    test_certificate_service(certificate_service).await;
    test_domain_service(domain_service).await;
}

async fn test_deployment(
    definition_service: Arc<dyn ApiDefinitionService>,
    deployment_service: Arc<dyn ApiDeploymentService>,
) {
    let def1 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/1/foo/{user-id}",
        "${ let userid: u64 = request.path.user-id; let res = if userid>100u64 then 0u64 else 1u64; let my-worker=instance[golem:it](\"shopping-cart-${res}\"); let not_found: u64 = 401; let success: u64 = 200; let result = my-worker.get-cart-contents[api](\"foo\"); let status = if result == \"admin\" then not_found else success; {status: status } }",
        false,
    ).await;

    let def2draft = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/2/foo/{user-id}",
        "${ let userid: u64 = request.path.user-id; let res = if userid>100u64 then 0u64 else 1u64; let worker-name = \"shopping-cart-${res}\"; let my-worker = instance[golem:it](worker-name); let not_found: u64 = 401; let success: u64 = 200; let result = my-worker.get-cart-contents[api](\"foo\"); let status = if result == \"admin\" then not_found else success; {status: status } }",
        true,
    ).await;
    let def2 = HttpApiDefinitionRequest {
        draft: false,
        ..def2draft.clone()
    };
    let def3 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/3/foo/{user-id}?{id}",
        "${ let userid: u64 = request.path.user-id; let res = if userid>100u64 then 0u64 else 1u64; let worker = instance[golem:it](\"shopping-cart-${res}\"); let not_found: u64 = 401; let success: u64 = 200; let result = worker.get-cart-contents[api](\"foo\"); let status = if result == \"admin\" then not_found else success; {status: status } }",
        false,
    ).await;
    let def4 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/4/foo/{user-id}",
        "${ let userid: u64 = request.path.user-id; let res = if userid>100u64 then 0u64 else 1u64; let worker = instance[golem:it](\"shopping-cart-${res}\"); let not_found: u64 = 401; let success: u64 = 200; let result = worker.get-cart-contents[api](\"foo\"); let status = if result == \"admin\" then not_found else success; {status: status } }",
        false,
    ).await;

    definition_service
        .create(&def1, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    definition_service
        .create(&def2draft, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    definition_service
        .create(&def3, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    definition_service
        .create(&def4, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();

    let definitions: Vec<HttpApiDefinition> = definition_service
        .get_all(&test_namespace(), &test_auth_ctx())
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    assert_eq!(definitions.len(), 4);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2draft.clone(), def3.clone(), def4.clone()]
    ));

    let deployment = get_api_deployment("test.com", None, vec![&def1.id.0, &def2.id.0]);
    deployment_service
        .deploy(&deployment, &test_auth_ctx())
        .await
        .unwrap();

    let definitions: Vec<HttpApiDefinition> = definition_service
        .get_all(&test_namespace(), &test_auth_ctx())
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    assert_eq!(definitions.len(), 4);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2.clone(), def3.clone(), def4.clone()]
    ));

    let definitions: Vec<HttpApiDefinition> = deployment_service
        .get_definitions_by_site(&test_namespace(), &ApiSiteString("test.com".to_string()))
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 2);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2.clone()]
    ));

    let deployment = get_api_deployment("test.com", Some("my"), vec![&def4.id.0]);
    deployment_service
        .deploy(&deployment, &test_auth_ctx())
        .await
        .unwrap();

    let definitions: Vec<HttpApiDefinition> = deployment_service
        .get_definitions_by_site(&test_namespace(), &ApiSiteString("my.test.com".to_string()))
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 1);
    assert!(contains_definitions(definitions, vec![def4.clone()]));

    let deployment = get_api_deployment("test.com", None, vec![&def3.id.0]);
    deployment_service
        .deploy(&deployment, &test_auth_ctx())
        .await
        .unwrap();

    let deployment = deployment_service
        .get_by_site(&test_namespace(), &ApiSiteString("test.com".to_string()))
        .await
        .unwrap();
    assert!(deployment.is_some());

    let deployments = deployment_service
        .get_by_id(&test_namespace(), Some(def3.id.clone()))
        .await
        .unwrap();
    assert!(!deployments.is_empty());

    let deployments = deployment_service
        .get_by_id(&test_namespace(), None)
        .await
        .unwrap();
    assert_eq!(deployments.len(), 2);
    assert!(!deployments.is_empty());

    let definitions: Vec<HttpApiDefinition> = deployment_service
        .get_definitions_by_site(&test_namespace(), &ApiSiteString("test.com".to_string()))
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 3);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2.clone(), def3.clone()]
    ));

    deployment_service
        .undeploy(
            &test_namespace(),
            ApiSiteString("test.com".to_string()),
            ApiDefinitionIdWithVersion {
                id: def3.id.clone(),
                version: def3.version.clone(),
            },
            &test_auth_ctx(),
        )
        .await
        .unwrap();

    let definitions: Vec<HttpApiDefinition> = deployment_service
        .get_definitions_by_site(&test_namespace(), &ApiSiteString("test.com".to_string()))
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();

    assert_eq!(definitions.len(), 2);
    assert!(contains_definitions(
        definitions,
        vec![def1.clone(), def2.clone()]
    ));

    assert!(
        deployment_service
            .delete(
                &test_namespace(),
                &test_auth_ctx(),
                &ApiSiteString("test.com".to_string()),
            )
            .await
            .is_ok(),
        "Deployment not found"
    );

    let definitions = deployment_service
        .get_definitions_by_site(&test_namespace(), &ApiSiteString("test.com".to_string()))
        .await
        .unwrap();
    assert!(definitions.is_empty());

    let deployment = deployment_service
        .get_by_site(&test_namespace(), &ApiSiteString("test.com".to_string()))
        .await
        .unwrap();
    assert!(deployment.is_none());

    let definition1 = definition_service
        .get(&def1.id, &def1.version, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    assert!(definition1.is_some_and(|x| x.draft));

    let definition2 = definition_service
        .get(&def2.id, &def2.version, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    assert!(definition2.is_some_and(|x| x.draft));
}

async fn test_deployment_conflict(
    definition_service: Arc<dyn ApiDefinitionService>,
    deployment_service: Arc<dyn ApiDeploymentService>,
) {
    let def1 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/get1",
        "${ let worker = instance[golem:it](\"worker1\"); let status: u64 = 200; { headers: { ContentType: \"json\", userid: \"foo\"}, body: worker.get-cart-contents(\"foo\"), status: status }  }",
        false,
    ).await;
    let def2 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/get2",
        "${ let worker = instance[golem:it](\"worker2\"); {body: worker.get-cart-contents(\"foo\")} }",
        true,
    ).await;

    let def3 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/get1",
        "${ let worker = instance[golem:it](\"worker2\"); {body: worker.get-cart-contents(\"foo\")} }",
        false,
    ).await;

    definition_service
        .create(&def1, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    definition_service
        .create(&def2, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    definition_service
        .create(&def3, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();

    let deployment = get_api_deployment("test-conflict.com", None, vec![&def1.id.0, &def2.id.0]);
    deployment_service
        .deploy(&deployment, &test_auth_ctx())
        .await
        .unwrap();

    let deployment = get_api_deployment("test-conflict.com", None, vec![&def3.id.0]);
    let deployment_result = deployment_service
        .deploy(&deployment, &test_auth_ctx())
        .await;
    assert!(deployment_result.is_err());
    assert_eq!(
        deployment_result.unwrap_err().to_string(),
        ApiDeploymentError::ApiDefinitionsConflict("/api/get1".to_string()).to_string()
    );

    let delete_result = definition_service
        .delete(&def1.id, &def1.version, &test_namespace(), &test_auth_ctx())
        .await;
    assert!(delete_result.is_err());
    assert_eq!(
        delete_result.unwrap_err().to_string(),
        ApiDefinitionError::ApiDefinitionDeployed("test-conflict.com".to_string()).to_string()
    );
}

async fn test_security_crud(security_scheme_service: Arc<dyn SecuritySchemeService>) {
    let security_identifier = SecuritySchemeIdentifier::new("test".to_string());

    let security_scheme = get_security(&security_identifier);

    let insert = security_scheme_service
        .create(&test_namespace(), &security_scheme)
        .await
        .expect("Failed to create security scheme");

    let get = security_scheme_service
        .get(&security_identifier, &test_namespace())
        .await
        .expect("Failed to get security scheme");

    assert_eq!(insert.security_scheme, security_scheme);
    assert_eq!(get.security_scheme, security_scheme);
    assert_eq!(insert.provider_metadata, get_test_provider_metadata());
    assert_eq!(insert.provider_metadata, get.provider_metadata)
}

async fn test_definition_crud(definition_service: Arc<dyn ApiDefinitionService>) {
    let def1v1 = get_api_definition(
        &Uuid::new_v4().to_string(),
        "0.0.1",
        "/api/{user}/get1",
        "${ let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; let worker = instance(\"shopping-cart-${res}\"); let not_found: u64 = 401; let success: u64 = 200; let result = worker.get-cart-contents(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
        false,
    ).await;
    let def1v1_upd = get_api_definition(
        &def1v1.id.0,
        "0.0.1",
        "/api/{user}/get1/1",
        "${ let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; let worker = instance(\"shopping-cart-${res}\"); let not_found: u64 = 401; let success: u64 = 200; let result = worker.get-cart-contents(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
        false,
    ).await;
    let def1v2 = get_api_definition(
        &def1v1.id.0,
        "0.0.2",
        "/api/{user}/get1/2",
        "${ let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; let worker = instance(\"shopping-cart-${res}\"); let not_found: u64 = 401; let success: u64 = 200; let result = worker.get-cart-contents(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
        true,
    ).await;

    let def1v2_upd = get_api_definition(
        &def1v1.id.0,
        "0.0.2",
        "/api/{user}/get1/22",
        "${ let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; let worker = instance(\"shopping-cart-${res}\"); let not_found: u64 = 401; let success: u64 = 200; let result = worker.get-cart-contents(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
        true,
    ).await;

    let def1v3 = get_api_definition(
        "test-def;;",
        "0.0.2",
        "/api/{user}/get1/22v3",
        "${ let userid: u64 = request.path.user; let res = if userid>100u64 then 0u64 else 1u64; let worker = instance(\"shopping-cart-${res}\"); let not_found: u64 = 401; let success: u64 = 200; let result = worker.get-cart-contents(\"foo\"); let status = if result == \"admin\" then not_found else success; status }",
        true,
    ).await;

    definition_service
        .create(&def1v1, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    definition_service
        .create(&def1v2, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    assert!(
        definition_service
            .create(&def1v3, &test_namespace(), &test_auth_ctx(),)
            .await
            .is_err(),
        "Definition name should be invalid"
    );

    let definitions: Vec<HttpApiDefinition> = definition_service
        .get_all_versions(&def1v1.id, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    assert_eq!(definitions.len(), 2);
    assert!(contains_definitions(
        definitions,
        vec![def1v1.clone(), def1v2.clone()]
    ));

    let update_result = definition_service
        .update(&def1v1_upd, &test_namespace(), &test_auth_ctx())
        .await;

    assert!(update_result.is_err());
    assert_eq!(
        update_result.unwrap_err().to_string(),
        ApiDefinitionError::ApiDefinitionNotDraft(def1v1_upd.id).to_string()
    );

    let update_result = definition_service
        .update(&def1v2_upd, &test_namespace(), &test_auth_ctx())
        .await;
    assert!(update_result.is_ok());

    let definitions: Vec<HttpApiDefinition> = definition_service
        .get_all_versions(&def1v1.id, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.into())
        .collect::<Vec<_>>();
    assert_eq!(definitions.len(), 2);
    assert!(contains_definitions(
        definitions,
        vec![def1v1.clone(), def1v2_upd.clone()]
    ));

    assert!(
        definition_service
            .delete(
                &def1v1.id,
                &def1v1.version,
                &test_namespace(),
                &test_auth_ctx(),
            )
            .await
            .is_ok(),
        "Failed to delete definition"
    );
    assert!(
        definition_service
            .delete(
                &def1v2.id,
                &def1v2.version,
                &test_namespace(),
                &test_auth_ctx(),
            )
            .await
            .is_ok(),
        "Failed to delete definition"
    );

    let definitions = definition_service
        .get_all_versions(&def1v1.id, &test_namespace(), &test_auth_ctx())
        .await
        .unwrap();
    assert!(definitions.is_empty());
}

async fn test_delete_non_existing(definition_service: Arc<dyn ApiDefinitionService>) {
    let delete_result = definition_service
        .delete(
            &ApiDefinitionId("non-existing".to_string()),
            &ApiVersion("0.0.1".to_string()),
            &test_namespace(),
            &test_auth_ctx(),
        )
        .await;

    assert!(delete_result.is_err(), "definition should not exist");
}

fn get_api_deployment(
    host: &str,
    subdomain: Option<&str>,
    definitions: Vec<&str>,
) -> ApiDeploymentRequest {
    let api_definition_keys: Vec<ApiDefinitionIdWithVersion> = definitions
        .into_iter()
        .map(|id| ApiDefinitionIdWithVersion {
            id: ApiDefinitionId(id.to_string()),
            version: ApiVersion("0.0.1".to_string()),
        })
        .collect();

    ApiDeploymentRequest {
        namespace: test_namespace(),
        api_definition_keys,
        site: ApiSite {
            host: host.to_string(),
            subdomain: subdomain.map(|s| s.to_string()),
        },
    }
}

fn get_security(security_schema_identifier: &SecuritySchemeIdentifier) -> SecurityScheme {
    SecurityScheme::new(
        Provider::Google,
        security_schema_identifier.clone(),
        ClientId::new("client_id_foo".to_string()),
        ClientSecret::new("client_secret_foo".to_string()),
        RedirectUrl::new("http://localhost:8080/auth/callback".to_string()).unwrap(),
        vec![
            Scope::new("openid".to_string()),
            Scope::new("user".to_string()),
            Scope::new("email".to_string()),
        ],
    )
}

async fn get_api_definition(
    id: &str,
    version: &str,
    path_pattern: &str,
    response_mapping: &str,
    draft: bool,
) -> HttpApiDefinitionRequest {
    let yaml_string = format!(
        r#"
          id: {id}
          version: {version}
          draft: {draft}
          routes:
          - method: Get
            path: {path_pattern}
            binding:
              component:
                name: test-component
                version: 0
              response: '{response_mapping}'
        "#
    );

    struct TestConversionContext;

    #[async_trait]
    impl ConversionContext for TestConversionContext {
        async fn component_by_name(&self, name: &ComponentName) -> Result<ComponentView, String> {
            if name.0 == "test-component" {
                Ok(ComponentView {
                    name: ComponentName("test-component".to_string()),
                    id: ComponentId(uuid!("0b6d9cd8-f373-4e29-8a5a-548e61b868a5")),
                    latest_version: 0,
                })
            } else {
                Err("component not found".to_string())
            }
        }
        async fn component_by_id(
            &self,
            _component_id: &ComponentId,
        ) -> Result<ComponentView, String> {
            unimplemented!()
        }
    }

    let api: api::dto::HttpApiDefinitionRequest =
        serde_yaml::from_str(yaml_string.as_str()).unwrap();
    api.into_core(&TestConversionContext.boxed()).await.unwrap()
}

fn contains_definitions(
    result: Vec<HttpApiDefinition>,
    expected: Vec<HttpApiDefinitionRequest>,
) -> bool {
    let requests: Vec<HttpApiDefinitionRequest> =
        result.into_iter().map(|x| x.into()).collect::<Vec<_>>();

    for value in expected.into_iter() {
        if !requests.contains(&value) {
            return false;
        }
    }

    true
}

async fn test_certificate_service(certificate_service: Arc<dyn CertificateService>) {
    let auth_ctx = AuthCtx::new(TokenSecret::new(Uuid::new_v4()));

    let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
        .parse::<ProjectId>()
        .unwrap();

    let certificate_request1 = CertificateRequest {
        project_id: project_id.clone(),
        domain_name: "*.golem.test1".to_string(),
        certificate_body: "body1".to_string(),
        certificate_private_key: "key1".to_string(),
    };

    let certificate_request2 = CertificateRequest {
        project_id: project_id.clone(),
        domain_name: "*.golem.test2".to_string(),
        certificate_body: "body2".to_string(),
        certificate_private_key: "key2".to_string(),
    };

    let certificate1 = certificate_service
        .create(&certificate_request1, &auth_ctx)
        .await
        .unwrap();

    let certificate2 = certificate_service
        .create(&certificate_request2, &auth_ctx)
        .await
        .unwrap();

    let certificate_id1 = certificate1.id.clone();
    let certificate_id2 = certificate2.id.clone();

    let certificate1_result1 = certificate_service
        .get(project_id.clone(), Some(certificate_id1.clone()), &auth_ctx)
        .await
        .unwrap();

    let certificate2_result1 = certificate_service
        .get(project_id.clone(), Some(certificate_id2.clone()), &auth_ctx)
        .await
        .unwrap();

    let certificate_result2 = certificate_service
        .get(project_id.clone(), None, &auth_ctx)
        .await
        .unwrap();

    certificate_service
        .delete(&project_id, &certificate_id1, &auth_ctx)
        .await
        .unwrap();

    let certificate1_result3 = certificate_service
        .get(project_id.clone(), Some(certificate_id1.clone()), &auth_ctx)
        .await
        .unwrap_or(vec![]);

    let certificate_result3 = certificate_service
        .get(project_id.clone(), None, &auth_ctx)
        .await
        .unwrap();

    certificate_service
        .delete(&project_id, &certificate_id2, &auth_ctx)
        .await
        .unwrap();

    let certificate2_result3 = certificate_service
        .get(project_id.clone(), Some(certificate_id2.clone()), &auth_ctx)
        .await
        .unwrap();

    let certificate_result4 = certificate_service
        .get(project_id, None, &auth_ctx)
        .await
        .unwrap();

    assert!(contains_certificates(
        certificate1_result1,
        vec![certificate1.clone()]
    ));
    assert!(contains_certificates(
        certificate2_result1,
        vec![certificate2.clone()]
    ));
    assert_eq!(certificate_result2.len(), 2);

    assert!(certificate1_result3.is_empty());
    assert!(certificate2_result3.is_empty());
    assert!(contains_certificates(
        certificate_result3,
        vec![certificate2]
    ));
    assert!(certificate_result4.is_empty());
}

fn contains_certificates(result: Vec<Certificate>, expected: Vec<Certificate>) -> bool {
    for value in expected.into_iter() {
        if !result.iter().any(|c| {
            c.id == value.id
                && c.project_id == value.project_id
                && c.domain_name == value.domain_name
                && c.created_at
                    .map(|c| c.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
                    == value
                        .created_at
                        .map(|c| c.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        }) {
            return false;
        }
    }

    true
}

async fn test_domain_service(domain_service: Arc<dyn ApiDomainService>) {
    let auth_ctx = AuthCtx::new(TokenSecret::new(Uuid::new_v4()));

    let project_id = "15d70aa5-2e23-4ee3-b65c-4e1d702836a3"
        .parse::<ProjectId>()
        .unwrap();

    let domain_name = "my-domain.com".to_string();

    let domain_request = DomainRequest {
        project_id: project_id.clone(),
        domain_name: domain_name.clone(),
    };

    let domain = domain_service
        .create_or_update(&domain_request, &auth_ctx)
        .await
        .unwrap();

    let expected = ApiDomain::new(
        &domain_request,
        vec![],
        domain.created_at.unwrap_or(Utc::now()),
    );

    assert_eq!(domain, expected);

    let result = domain_service.get(&project_id, &auth_ctx).await.unwrap();

    domain_service
        .delete(&project_id, domain_name.as_str(), &auth_ctx)
        .await
        .unwrap();

    let result2 = domain_service.get(&project_id, &auth_ctx).await.unwrap();

    assert!(!result.is_empty());
    assert_eq!(result[0].domain_name, domain.domain_name);
    assert_eq!(result[0].name_servers, domain.name_servers);
    assert!(result2.is_empty());
}
