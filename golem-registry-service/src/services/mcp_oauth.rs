// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at http://license.golem.cloud/LICENSE

use crate::model::security_scheme::SecurityScheme;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::environment::EnvironmentRepo;
use crate::repo::mcp_oauth::McpOAuthGrantRepo;
use crate::repo::model::environment::EnvironmentRepoError;
use crate::repo::model::mcp_oauth::{
    McpOAuthAuthorization, McpOAuthFlowSecrets, McpOAuthGrantKey, McpOAuthGrantStatus,
    McpOAuthSession, McpOAuthTokens,
};
use crate::repo::model::security_scheme::SecuritySchemeRepoError;
use crate::repo::security_scheme::SecuritySchemeRepo;
use crate::services::account_usage::AccountUsageService;
use crate::services::account_usage::error::AccountUsageError;
use crate::services::security_scheme::authorize_security_scheme_permission;
use chrono::{DateTime, Utc};
use golem_common::model::account::AccountId;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::mcp_import::{McpImport, McpImportCredential, McpImportSource};
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_common::{SafeDisplay, error_forwarding};
use golem_mcp_import::oauth::{self, AuthorizationServer, OAuthClient};
use golem_mcp_import::transport::sender::HttpSender;
use golem_mcp_import::transport::{HttpSend, TransportError};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use oauth2::{AuthorizationCode, PkceCodeVerifier, RefreshToken, TokenResponse};
use std::sync::Arc;
use std::time::Duration;
use url::Url;

pub mod http_policy;

#[derive(Debug, thiserror::Error)]
pub enum McpOAuthError {
    #[error("MCP import or its environment/deployment was not found")]
    ImportNotFound,
    #[error("MCP security scheme was not found")]
    SchemeNotFound,
    #[error("MCP import does not use a security scheme")]
    NotOAuth,
    #[error("MCP import credential context changed")]
    ContextChanged,
    #[error("MCP credential owner does not match the owning agent's environment")]
    OwnerMismatch,
    #[error("Authorize the MCP import using security scheme {0}")]
    AuthorizationRequired(SecuritySchemeName),
    /// Terminal for this operation: never automatically retry an ambiguous refresh.
    /// Cancellation or a crashed owner can leave a claim in flight. Only explicit
    /// reauthorization replaces that claim; a waiter cannot release or reuse it.
    #[error("MCP OAuth refresh did not finish; reauthorize the import using security scheme {0}")]
    RefreshUnresolved(SecuritySchemeName),
    #[error("Invalid, expired, or already consumed MCP OAuth callback")]
    InvalidCallback,
    #[error("MCP OAuth consent was denied")]
    ConsentDenied,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    AccountUsage(#[from] AccountUsageError),
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

error_forwarding!(
    McpOAuthError,
    RepoError,
    EnvironmentRepoError,
    SecuritySchemeRepoError
);

impl SafeDisplay for McpOAuthError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".into(),
            Self::Unauthorized(error) => error.to_safe_string(),
            Self::AccountUsage(error) => error.to_safe_string(),
            _ => self.to_string(),
        }
    }
}

/// Private runtime result. Never put the credential in API output or an oplog.
#[derive(Debug)]
pub struct McpCredential {
    pub credential: Option<McpImportCredential>,
    pub oauth_grant: Option<McpOAuthAuthorization>,
}

/// Values decoded once from the provider callback; descriptions are not retained.
pub struct McpOAuthCallback {
    pub state: String,
    pub issuer: Option<String>,
    pub code: Option<String>,
    pub error: Option<String>,
}

pub struct McpOAuthService {
    environments: Arc<dyn EnvironmentRepo>,
    deployments: Arc<dyn DeploymentRepo>,
    schemes: Arc<dyn SecuritySchemeRepo>,
    grants: Arc<dyn McpOAuthGrantRepo>,
    limits: oauth::Limits,
    usage: Arc<AccountUsageService>,
}

struct ResolvedImport {
    environment: Environment,
    import: McpImport,
    scheme: Option<SecurityScheme>,
}

impl ResolvedImport {
    fn oauth(&self) -> Result<(&SecurityScheme, McpOAuthGrantKey), McpOAuthError> {
        let scheme = self.scheme.as_ref().ok_or(McpOAuthError::NotOAuth)?;
        Ok((
            scheme,
            McpOAuthGrantKey {
                environment_id: self.environment.id.0,
                security_scheme_id: scheme.id.0,
                security_scheme_revision: scheme.revision.into(),
                credential_owner_account_id: self.environment.owner_account_id.0,
                resource_url: self.import.url.clone(),
            },
        ))
    }

    fn authorize_operator(&self, auth: &AuthCtx) -> Result<(), McpOAuthError> {
        let (scheme, _) = self.oauth()?;
        authorize_security_scheme_permission(
            auth,
            &self.environment,
            Some(&scheme.name),
            golem_common::model::card::EnvironmentSecuritySchemeVerb::Update,
        )?;
        Ok(())
    }
}

impl McpOAuthService {
    pub fn new(
        environments: Arc<dyn EnvironmentRepo>,
        deployments: Arc<dyn DeploymentRepo>,
        schemes: Arc<dyn SecuritySchemeRepo>,
        grants: Arc<dyn McpOAuthGrantRepo>,
        limits: oauth::Limits,
        usage: Arc<AccountUsageService>,
    ) -> Self {
        Self {
            environments,
            deployments,
            schemes,
            grants,
            limits,
            usage,
        }
    }

    async fn operator_sender(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
    ) -> Result<HttpSender<http_policy::McpHttpPolicy>, McpOAuthError> {
        let resolved = self.resolve(source).await?;
        let (scheme, _) = resolved.oauth()?;
        let policy = http_policy::McpHttpPolicy::operator(
            auth,
            &resolved.environment,
            &scheme.name,
            self.usage.clone(),
        )?;
        Ok(HttpSender::new(policy)?)
    }

    pub async fn authorize(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
    ) -> Result<Url, McpOAuthError> {
        let mut sender = self.operator_sender(source, auth).await?;
        self.begin(source, auth, &mut sender).await
    }

    pub async fn complete_operator(
        &self,
        source: &McpImportSource,
        callback: McpOAuthCallback,
        auth: &AuthCtx,
    ) -> Result<SecuritySchemeName, McpOAuthError> {
        let mut sender = self.operator_sender(source, auth).await?;
        self.complete(source, callback, auth, &mut sender).await
    }

    pub async fn status(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
    ) -> Result<(SecuritySchemeName, McpOAuthGrantStatus), McpOAuthError> {
        let resolved = self.resolve(source).await?;
        resolved.authorize_operator(auth)?;
        let (scheme, key) = resolved.oauth()?;
        let status = self
            .grants
            .load(&key)
            .await?
            .map(|grant| grant.status)
            .unwrap_or(McpOAuthGrantStatus::ReauthorizationRequired);
        Ok((scheme.name.clone(), status))
    }

    async fn resolve(&self, source: &McpImportSource) -> Result<ResolvedImport, McpOAuthError> {
        let environment: Environment = self
            .environments
            .get_by_id(source.environment_id.0, false)
            .await?
            .ok_or(McpOAuthError::ImportNotFound)?
            .try_into()?;
        let state = self
            .deployments
            .get_tool_deployment_state(source.environment_id.0, source.deployment_revision.into())
            .await?
            .ok_or(McpOAuthError::ImportNotFound)?;
        let import = state
            .mcp_imports
            .into_iter()
            .find(|entry| entry.import_index == i64::from(source.import_index))
            .ok_or(McpOAuthError::ImportNotFound)?
            .import_config
            .into_value();
        let scheme = match &import.security_scheme {
            Some(name) => Some(
                self.schemes
                    .get_for_environment_and_name(source.environment_id.0, &name.0)
                    .await?
                    .ok_or(McpOAuthError::SchemeNotFound)?
                    .try_into()?,
            ),
            None => None,
        };
        Ok(ResolvedImport {
            environment,
            import,
            scheme,
        })
    }

    /// Probe the selected import without credentials before OAuth discovery.
    /// The sender admits every resource and provider request in this chain.
    pub async fn begin<S: HttpSend<Error = McpOAuthError> + Send>(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
        sender: &mut S,
    ) -> Result<Url, McpOAuthError> {
        let resolved = self.resolve(source).await?;
        resolved.authorize_operator(auth)?;
        let (scheme, key) = resolved.oauth()?;
        let issuer = scheme
            .provider_type
            .issuer_url()
            .map_err(|_| TransportError::Configuration("invalid OAuth issuer".into()))?;
        oauth::authorization_metadata_urls(issuer.as_str())?;
        oauth::https_url(&key.resource_url)?;
        let transport = golem_mcp_import::transport::Client::new(
            &key.resource_url,
            resolved.import.version.as_deref(),
            golem_mcp_import::transport::Limits {
                timeout: self.limits.timeout,
                request_bytes: self.limits.request_bytes,
                ..Default::default()
            },
        )?;
        let discovery = tokio::time::timeout(self.limits.timeout, async {
            let challenge = transport
                .authorization_challenge(sender, self.limits.challenge_bytes)
                .await?;
            oauth::discover(
                sender,
                &key.resource_url,
                issuer.as_str(),
                &challenge,
                self.limits,
            )
            .await
        })
        .await
        .map_err(|_| TransportError::Timeout)??;
        let scopes = if scheme.scopes.is_empty() {
            discovery.scopes
        } else {
            scheme.scopes.iter().map(|s| s.to_string()).collect()
        };
        let client = client(&discovery.server, scheme, &key.resource_url)?;
        let request = client.authorize(&scopes)?;
        // Discovery can take time; do not bind consent to a retired scheme.
        let current = self.resolve(source).await?;
        current.authorize_operator(auth)?;
        if current.oauth()?.1 != key {
            return Err(McpOAuthError::ContextChanged);
        }
        self.grants
            .begin_authorization(
                key,
                state_hash(request.state.secret()),
                (Utc::now() + chrono::Duration::minutes(10)).into(),
                McpOAuthFlowSecrets {
                    pkce_verifier: request.verifier.secret().clone(),
                    session: McpOAuthSession {
                        deployment_revision: source.deployment_revision.into(),
                        import_index: source.import_index,
                        authorized_by: auth.actor_account_id().0,
                        server: discovery.server.metadata().clone(),
                        scopes,
                    },
                },
            )
            .await?;
        Ok(request.url)
    }

    /// Authenticated operator completion (including a CLI-forwarded callback).
    /// Shared-store claiming precedes every token POST; crashes never release it.
    pub async fn complete<S: HttpSend<Error = McpOAuthError> + Send>(
        &self,
        expected_source: &McpImportSource,
        callback: McpOAuthCallback,
        auth: &AuthCtx,
        sender: &mut S,
    ) -> Result<SecuritySchemeName, McpOAuthError> {
        let claim = self
            .grants
            .claim_callback(
                expected_source.environment_id.0,
                &state_hash(&callback.state),
                Utc::now().into(),
            )
            .await?
            .ok_or(McpOAuthError::InvalidCallback)?;
        let result = async {
            let session = &claim.flow.session;
            if session.authorized_by != auth.actor_account_id().0 {
                return Err(McpOAuthError::InvalidCallback);
            }
            let source = session_source(&claim.key, session)?;
            if source.environment_id != expected_source.environment_id
                || source.deployment_revision != expected_source.deployment_revision
                || source.import_index != expected_source.import_index
            {
                return Err(McpOAuthError::InvalidCallback);
            }
            let resolved = self.resolve(&source).await?;
            resolved.authorize_operator(auth)?;
            let (scheme, key) = resolved.oauth()?;
            if key != claim.key {
                return Err(McpOAuthError::ContextChanged);
            }
            let server =
                AuthorizationServer::validate(session.server.clone(), &session.server.issuer)?;
            oauth::validate_callback_issuer(
                server.issuer(),
                server.requires_callback_issuer(),
                callback.issuer.as_deref(),
            )?;
            let code = match (callback.code, callback.error) {
                (Some(code), None) if !code.is_empty() => code,
                (None, Some(_)) => return Err(McpOAuthError::ConsentDenied),
                _ => return Err(McpOAuthError::InvalidCallback),
            };
            let issued_at = Utc::now();
            let token = client(&server, scheme, &key.resource_url)?
                .exchange_code(
                    sender,
                    AuthorizationCode::new(code),
                    PkceCodeVerifier::new(claim.flow.pkce_verifier.clone()),
                    callback.issuer.as_deref(),
                    self.limits,
                )
                .await?;
            let tokens = tokens(token, issued_at, session.clone(), None)?;
            let current = self.resolve(&source).await?;
            current.authorize_operator(auth)?;
            if current.oauth()?.1 != key {
                return Err(McpOAuthError::ContextChanged);
            }
            if !self
                .grants
                .publish_exchange(&key, claim.generation, tokens)
                .await?
            {
                return Err(McpOAuthError::ContextChanged);
            }
            Ok(scheme.name.clone())
        }
        .await;
        if result.is_err() {
            // This is a fenced state transition, not a retry of the exchange.
            self.grants
                .fail_exchange(&claim.key, claim.generation)
                .await?;
        }
        result
    }

    pub async fn disconnect(
        &self,
        source: &McpImportSource,
        auth: &AuthCtx,
    ) -> Result<SecuritySchemeName, McpOAuthError> {
        let resolved = self.resolve(source).await?;
        resolved.authorize_operator(auth)?;
        let (scheme, key) = resolved.oauth()?;
        self.grants.revoke(&key).await?;
        Ok(scheme.name.clone())
    }

    pub async fn runtime_credential(
        &self,
        source: &McpImportSource,
        auth: AuthCtx,
    ) -> Result<McpCredential, McpOAuthError> {
        let resolved = self.resolve(source).await?;
        let policy =
            http_policy::McpHttpPolicy::runtime(auth, &resolved.environment, self.usage.clone())?;
        let uri = resolved
            .import
            .url
            .parse()
            .map_err(|_| TransportError::Denied)?;
        policy.authorize(&uri)?;
        let owner = resolved.environment.owner_account_id;
        let mut sender = HttpSender::new(policy)?;
        self.credential(source, owner, &mut sender).await
    }

    pub async fn report_resource_unauthorized(
        &self,
        source: &McpImportSource,
        auth: AuthCtx,
        used_generation: Option<uuid::Uuid>,
    ) -> Result<(), McpOAuthError> {
        let resolved = self.resolve(source).await?;
        let policy =
            http_policy::McpHttpPolicy::runtime(auth, &resolved.environment, self.usage.clone())?;
        let uri = resolved
            .import
            .url
            .parse()
            .map_err(|_| TransportError::Denied)?;
        policy.authorize(&uri)?;
        let (Ok((_, key)), Some(used_generation)) = (resolved.oauth(), used_generation) else {
            return Ok(());
        };
        self.grants
            .expire_access_token(&key, used_generation, Utc::now().into())
            .await?;
        Ok(())
    }

    /// Trusted runtime path after tool admission, never an administrative token
    /// API. The sender carries the live calling agent's network/quota context.
    pub async fn credential<S: HttpSend<Error = McpOAuthError> + Send>(
        &self,
        source: &McpImportSource,
        owner: AccountId,
        sender: &mut S,
    ) -> Result<McpCredential, McpOAuthError> {
        let resolved = self.resolve(source).await?;
        if resolved.environment.owner_account_id != owner {
            return Err(McpOAuthError::OwnerMismatch);
        }
        if resolved.scheme.is_none() {
            let credential = if resolved.import.auth.is_some() {
                Some(
                    self.deployments
                        .get_deployment_mcp_import_credential(
                            source.environment_id.0,
                            source.deployment_revision.into(),
                            source.import_index,
                        )
                        .await?
                        .ok_or_else(|| anyhow::anyhow!("missing deployed MCP inline credential"))?,
                )
            } else {
                None
            };
            return Ok(McpCredential {
                credential,
                oauth_grant: None,
            });
        }
        let (scheme, key) = resolved.oauth()?;
        let deadline = tokio::time::Instant::now() + self.limits.timeout;
        let mut waited = false;
        let mut backoff = Duration::from_millis(50);
        loop {
            let grant = self
                .grants
                .load(&key)
                .await?
                .ok_or_else(|| McpOAuthError::AuthorizationRequired(scheme.name.clone()))?;
            match grant.status {
                McpOAuthGrantStatus::Granted => {
                    let stored = grant
                        .tokens
                        .ok_or_else(|| anyhow::anyhow!("granted MCP OAuth token missing"))?;
                    if stored.expires_at.is_none_or(|expiry| expiry > Utc::now()) {
                        if waited && self.resolve(source).await?.oauth()?.1 != key {
                            return Err(McpOAuthError::ContextChanged);
                        }
                        return Ok(McpCredential {
                            credential: Some(McpImportCredential::Bearer {
                                token: stored.access_token,
                            }),
                            oauth_grant: Some(McpOAuthAuthorization {
                                key,
                                generation: grant.generation,
                            }),
                        });
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Err(McpOAuthError::RefreshUnresolved(scheme.name.clone()));
                    }
                    if self.resolve(source).await?.oauth()?.1 != key {
                        return Err(McpOAuthError::ContextChanged);
                    }
                    let Some(claim) = self.grants.claim_refresh(&key, grant.generation).await?
                    else {
                        waited = true;
                        continue;
                    };
                    let result = async {
                        let refresh = claim.tokens.refresh_token.as_ref().ok_or_else(|| {
                            McpOAuthError::AuthorizationRequired(scheme.name.clone())
                        })?;
                        let session = &claim.tokens.session;
                        let server = AuthorizationServer::validate(
                            session.server.clone(),
                            &session.server.issuer,
                        )?;
                        let issued_at = Utc::now();
                        // Waiting for a peer and exchanging a token share one
                        // budget; taking over must not restart the deadline.
                        let timeout =
                            deadline.saturating_duration_since(tokio::time::Instant::now());
                        if timeout.is_zero() {
                            return Err(McpOAuthError::RefreshUnresolved(scheme.name.clone()));
                        }
                        let token = client(&server, scheme, &key.resource_url)?
                            .refresh(
                                sender,
                                &RefreshToken::new(refresh.clone()),
                                oauth::Limits {
                                    timeout,
                                    ..self.limits
                                },
                            )
                            .await?;
                        let next = tokens(token, issued_at, session.clone(), Some(&claim.tokens))?;
                        if self.resolve(source).await?.oauth()?.1 != key {
                            return Err(McpOAuthError::ContextChanged);
                        }
                        let credential = McpCredential {
                            credential: Some(McpImportCredential::Bearer {
                                token: next.access_token.clone(),
                            }),
                            oauth_grant: Some(McpOAuthAuthorization {
                                key: key.clone(),
                                generation: claim.generation,
                            }),
                        };
                        if !self
                            .grants
                            .publish_refresh(&key, claim.generation, next)
                            .await?
                        {
                            return Err(McpOAuthError::ContextChanged);
                        }
                        Ok(credential)
                    }
                    .await;
                    if let Err(error) = &result {
                        if matches!(
                            error,
                            McpOAuthError::Transport(TransportError::Denied)
                                | McpOAuthError::AccountUsage(_)
                                | McpOAuthError::RefreshUnresolved(_)
                        ) {
                            // These errors are raised before dispatch.
                            // Restore the unused token under the claim's CAS fence.
                            self.grants
                                .publish_refresh(&key, claim.generation, claim.tokens)
                                .await?;
                        } else {
                            self.grants.fail_refresh(&key, claim.generation).await?;
                        }
                    }
                    return result;
                }
                McpOAuthGrantStatus::Refreshing => {
                    if tokio::time::Instant::now() >= deadline {
                        return Err(McpOAuthError::RefreshUnresolved(scheme.name.clone()));
                    }
                    waited = true;
                    tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + backoff))
                        .await;
                    backoff = (backoff * 2).min(Duration::from_secs(1));
                }
                _ => return Err(McpOAuthError::AuthorizationRequired(scheme.name.clone())),
            }
        }
    }
}

fn state_hash(state: &str) -> Vec<u8> {
    blake3::derive_key("golem MCP OAuth callback state", state.as_bytes()).to_vec()
}

fn session_source(
    key: &McpOAuthGrantKey,
    session: &McpOAuthSession,
) -> Result<McpImportSource, McpOAuthError> {
    Ok(McpImportSource {
        environment_id: EnvironmentId(key.environment_id),
        deployment_revision: session.deployment_revision.try_into()?,
        import_index: session.import_index,
        upstream_tool_name: String::new(),
    })
}

fn client(
    server: &AuthorizationServer,
    scheme: &SecurityScheme,
    resource: &str,
) -> Result<OAuthClient, McpOAuthError> {
    let secret = scheme.client_secret.secret();
    Ok(server.client(
        scheme.client_id.as_str(),
        (!secret.is_empty()).then_some(secret.as_str()),
        scheme.redirect_url.as_str(),
        resource,
    )?)
}

fn tokens(
    response: oauth2::basic::BasicTokenResponse,
    issued_at: DateTime<Utc>,
    session: McpOAuthSession,
    previous: Option<&McpOAuthTokens>,
) -> Result<McpOAuthTokens, McpOAuthError> {
    let expires_at = response
        .expires_in()
        .map(|duration| {
            chrono::Duration::from_std(duration)
                .ok()
                .and_then(|duration| issued_at.checked_add_signed(duration))
                .ok_or_else(|| TransportError::Protocol("invalid OAuth token expiry".into()))
        })
        .transpose()?;
    if expires_at.is_some_and(|expiry| expiry <= Utc::now()) {
        return Err(
            TransportError::Protocol("OAuth provider issued an expired token".into()).into(),
        );
    }
    Ok(McpOAuthTokens {
        access_token: response.access_token().secret().clone(),
        refresh_token: response
            .refresh_token()
            .map(|token| token.secret().clone())
            .or_else(|| previous.and_then(|token| token.refresh_token.clone())),
        expires_at,
        scopes: response
            .scopes()
            .map(|scopes| scopes.iter().map(|s| s.to_string()).collect())
            .unwrap_or_else(|| {
                previous
                    .map(|token| &token.scopes)
                    .unwrap_or(&session.scopes)
                    .clone()
            }),
        session,
    })
}

#[cfg(test)]
mod tests;
