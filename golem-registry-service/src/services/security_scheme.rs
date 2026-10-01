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

use super::environment::{EnvironmentError, EnvironmentService};
use crate::model::api_definition::BoundCompiledRoute;
use crate::model::security_scheme::SecurityScheme;
use crate::repo::deployment::DeploymentRepo;
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::security_scheme::{
    SecuritySchemeAuthExtRevisionRecord, SecuritySchemeRepoError, SecuritySchemeRevisionRecord,
};
use crate::repo::security_scheme::SecuritySchemeRepo;
use crate::services::registry_change_notifier::{
    RegistryChangeNotifier, RequiresNotificationSignalExt,
};
use golem_common::model::account::AccountEmail;
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EnvironmentSecuritySchemeName, EnvironmentSecuritySchemeResourcePattern,
    EnvironmentSecuritySchemeVerb, PermissionTarget,
};
use golem_common::model::environment::{Environment, EnvironmentId, EnvironmentName};
use golem_common::model::security_scheme::{
    AuthorizationCodePkceConfig, SecuritySchemeCreation, SecuritySchemeId, SecuritySchemeLogin,
    SecuritySchemeName, SecuritySchemeRevision, SecuritySchemeUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum SecuritySchemeError {
    #[error("There is already a scheme with name {0}")]
    SecuritySchemeWithNameAlreadyExists(SecuritySchemeName),
    #[error("Invalid redirect url provided")]
    InvalidRedirectUrl,
    #[error("Invalid login configuration: {0}")]
    InvalidLoginConfiguration(String),
    #[error("Invalid custom provider issuer URL: {0}")]
    InvalidCustomProviderIssuerUrl(String),
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Security scheme {0} not found")]
    SecuritySchemeNotFound(SecuritySchemeId),
    #[error("Security scheme for name {0} not found in environment")]
    SecuritySchemeForNameNotFound(SecuritySchemeName),
    #[error("Concurrent update attempt")]
    ConcurrentUpdateAttempt,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

pub(super) fn authorize_security_scheme_permission(
    auth: &AuthCtx,
    environment: &Environment,
    name: Option<&SecuritySchemeName>,
    verb: EnvironmentSecuritySchemeVerb,
) -> Result<(), AuthorizationError> {
    authorize_security_scheme_permission_for_owner(
        auth,
        EnvironmentOwnerPattern::Environment {
            account: environment.owner_account_email.clone(),
            application: environment.application_name.clone(),
            environment: environment.name.clone(),
        },
        name,
        verb,
    )
}

fn authorize_security_scheme_permission_for_owner(
    auth: &AuthCtx,
    owner: EnvironmentOwnerPattern,
    name: Option<&SecuritySchemeName>,
    verb: EnvironmentSecuritySchemeVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentSecurityScheme(
        ClassPermissionTarget {
            verb: Some(verb),
            owner,
            resource: name
                .map(|name| {
                    EnvironmentSecuritySchemeResourcePattern::Name(EnvironmentSecuritySchemeName(
                        name.0.clone(),
                    ))
                })
                .unwrap_or(EnvironmentSecuritySchemeResourcePattern::Any),
        },
    ))
}

fn environment_owner_from_security_scheme(
    security_scheme: &SecuritySchemeAuthExtRevisionRecord,
) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: AccountEmail::new(security_scheme.owner_account_email.clone()),
        application: ApplicationName(security_scheme.application_name.clone()),
        environment: EnvironmentName(security_scheme.environment_name.clone()),
    }
}

impl SafeDisplay for SecuritySchemeError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InvalidRedirectUrl => self.to_string(),
            Self::InvalidLoginConfiguration(_) => self.to_string(),
            Self::InvalidCustomProviderIssuerUrl(_) => self.to_string(),
            Self::SecuritySchemeWithNameAlreadyExists(_) => self.to_string(),
            Self::SecuritySchemeForNameNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::SecuritySchemeNotFound(_) => self.to_string(),
            Self::ConcurrentUpdateAttempt => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    SecuritySchemeError,
    SecuritySchemeRepoError,
    EnvironmentError
);

pub struct SecuritySchemeService {
    security_scheme_repo: Arc<dyn SecuritySchemeRepo>,
    deployment_repo: Arc<dyn DeploymentRepo>,
    environment_service: Arc<EnvironmentService>,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    strict_issuer_url_validation: bool,
}

impl SecuritySchemeService {
    pub fn new(
        security_scheme_repo: Arc<dyn SecuritySchemeRepo>,
        deployment_repo: Arc<dyn DeploymentRepo>,
        environment_service: Arc<EnvironmentService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
        strict_issuer_url_validation: bool,
    ) -> Self {
        Self {
            security_scheme_repo,
            deployment_repo,
            environment_service,
            registry_change_notifier,
            strict_issuer_url_validation,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: SecuritySchemeCreation,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let (http_routing_epoch, environment) =
            crate::services::capture_http_routing_epoch_before_snapshot(
                async {
                    self.deployment_repo
                        .get_http_routing_epoch_if_exists(environment_id.0)
                        .await
                        .map_err(anyhow::Error::from)
                        .map_err(SecuritySchemeError::from)
                },
                || async {
                    self.environment_service
                        .get(environment_id, false, auth)
                        .await
                        .map_err(|err| match err {
                            EnvironmentError::EnvironmentNotFound(_) => {
                                SecuritySchemeError::ParentEnvironmentNotFound(environment_id)
                            }
                            other => other.into(),
                        })
                },
            )
            .await?;
        let http_routing_epoch = http_routing_epoch.ok_or(
            SecuritySchemeError::ParentEnvironmentNotFound(environment_id),
        )?;

        authorize_security_scheme_permission(
            auth,
            &environment,
            Some(&data.name),
            EnvironmentSecuritySchemeVerb::Create,
        )?;

        if self.strict_issuer_url_validation {
            data.provider_type
                .validate_issuer_url_strict()
                .map_err(SecuritySchemeError::InvalidCustomProviderIssuerUrl)?;
        }
        validate_callback_url(&data.redirect_url)?;
        validate_login(&data.login)?;

        let id = SecuritySchemeId::new();

        let redirect_url: RedirectUrl = RedirectUrl::new(data.redirect_url)
            .map_err(|_| SecuritySchemeError::InvalidRedirectUrl)?;
        let scopes: Vec<Scope> = data.scopes.into_iter().map(Scope::new).collect();
        let security_scheme = SecurityScheme {
            id,
            revision: SecuritySchemeRevision::INITIAL,
            name: data.name.clone(),
            environment_id,
            provider_type: data.provider_type,
            client_id: ClientId::new(data.client_id),
            client_secret: ClientSecret::new(data.client_secret),
            redirect_url,
            scopes,
            login: data.login,
        };
        self.validate_current_deployment(&security_scheme, false)
            .await?;
        let record = SecuritySchemeRevisionRecord::from_model(
            security_scheme,
            DeletableRevisionAuditFields::new(auth.actor_account_id().0),
        );

        let result = self
            .security_scheme_repo
            .create(
                environment_id.0,
                http_routing_epoch,
                data.name.0.clone(),
                record,
            )
            .await;

        match result {
            Ok(record) => Ok(record
                .signal_new_events_available(&self.registry_change_notifier)
                .try_into()?),
            Err(SecuritySchemeRepoError::SecuritySchemeViolatesUniqueness) => Err(
                SecuritySchemeError::SecuritySchemeWithNameAlreadyExists(data.name),
            ),
            Err(SecuritySchemeRepoError::ConcurrentModification) => {
                Err(SecuritySchemeError::ConcurrentUpdateAttempt)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn update(
        &self,
        security_scheme_id: SecuritySchemeId,
        update: SecuritySchemeUpdate,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let (http_routing_epoch, (mut security_scheme, owner)) =
            crate::services::capture_http_routing_epoch_before_snapshot(
                async {
                    self.security_scheme_repo
                        .get_http_routing_epoch_for_scheme(security_scheme_id.0)
                        .await
                        .map_err(SecuritySchemeError::from)
                },
                || self.get_with_environment(security_scheme_id, auth),
            )
            .await?;
        let http_routing_epoch = http_routing_epoch.ok_or(
            SecuritySchemeError::SecuritySchemeNotFound(security_scheme_id),
        )?;

        authorize_security_scheme_permission_for_owner(
            auth,
            owner,
            Some(&security_scheme.name),
            EnvironmentSecuritySchemeVerb::Update,
        )?;

        if update.current_revision != security_scheme.revision {
            return Err(SecuritySchemeError::ConcurrentUpdateAttempt);
        };

        security_scheme.revision = security_scheme.revision.next()?;
        if let Some(ref provider_type) = update.provider_type
            && self.strict_issuer_url_validation
        {
            provider_type
                .validate_issuer_url_strict()
                .map_err(SecuritySchemeError::InvalidCustomProviderIssuerUrl)?;
        }
        if let Some(provider_type) = update.provider_type {
            security_scheme.provider_type = provider_type;
        };
        if let Some(client_id) = update.client_id {
            security_scheme.client_id = ClientId::new(client_id);
        };
        if let Some(client_secret) = update.client_secret {
            security_scheme.client_secret = ClientSecret::new(client_secret);
        };
        if let Some(redirect_url) = update.redirect_url {
            validate_callback_url(&redirect_url)?;
            let redirect_url: RedirectUrl = RedirectUrl::new(redirect_url)
                .map_err(|_| SecuritySchemeError::InvalidRedirectUrl)?;
            security_scheme.redirect_url = redirect_url;
        };
        if let Some(scopes) = update.scopes {
            let scopes: Vec<Scope> = scopes.into_iter().map(Scope::new).collect();
            security_scheme.scopes = scopes;
        };
        if let Some(login) = update.login {
            validate_login(&login)?;
            security_scheme.login = login;
        }

        self.validate_current_deployment(&security_scheme, false)
            .await?;

        let audit = DeletableRevisionAuditFields::new(auth.actor_account_id().0);

        let environment_id = security_scheme.environment_id;

        let result = self
            .security_scheme_repo
            .update(
                environment_id.0,
                http_routing_epoch,
                SecuritySchemeRevisionRecord::from_model(security_scheme, audit),
            )
            .await;

        match result {
            Ok(record) => Ok(record
                .signal_new_events_available(&self.registry_change_notifier)
                .try_into()?),
            Err(SecuritySchemeRepoError::ConcurrentModification) => {
                Err(SecuritySchemeError::ConcurrentUpdateAttempt)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn delete(
        &self,
        security_scheme_id: SecuritySchemeId,
        current_revision: SecuritySchemeRevision,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let (http_routing_epoch, (mut security_scheme, owner)) =
            crate::services::capture_http_routing_epoch_before_snapshot(
                async {
                    self.security_scheme_repo
                        .get_http_routing_epoch_for_scheme(security_scheme_id.0)
                        .await
                        .map_err(SecuritySchemeError::from)
                },
                || self.get_with_environment(security_scheme_id, auth),
            )
            .await?;
        let http_routing_epoch = http_routing_epoch.ok_or(
            SecuritySchemeError::SecuritySchemeNotFound(security_scheme_id),
        )?;

        authorize_security_scheme_permission_for_owner(
            auth,
            owner,
            Some(&security_scheme.name),
            EnvironmentSecuritySchemeVerb::Delete,
        )?;

        if current_revision != security_scheme.revision {
            return Err(SecuritySchemeError::ConcurrentUpdateAttempt);
        };

        security_scheme.revision = security_scheme.revision.next()?;

        let environment_id = security_scheme.environment_id;

        self.validate_current_deployment(&security_scheme, true)
            .await?;

        let audit = DeletableRevisionAuditFields::deletion(auth.actor_account_id().0);

        let result = self
            .security_scheme_repo
            .delete(
                environment_id.0,
                http_routing_epoch,
                SecuritySchemeRevisionRecord::from_model(security_scheme, audit),
            )
            .await;

        match result {
            Ok(record) => Ok(record
                .signal_new_events_available(&self.registry_change_notifier)
                .try_into()?),
            Err(SecuritySchemeRepoError::ConcurrentModification) => {
                Err(SecuritySchemeError::ConcurrentUpdateAttempt)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn get(
        &self,
        security_scheme_id: SecuritySchemeId,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let (security_scheme, _) = self.get_with_environment(security_scheme_id, auth).await?;
        Ok(security_scheme)
    }

    pub async fn get_security_schemes_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<SecurityScheme>, SecuritySchemeError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    SecuritySchemeError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        let security_schemes: Vec<SecurityScheme> = self
            .security_scheme_repo
            .get_for_environment(environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        Ok(security_schemes
            .into_iter()
            .filter(|security_scheme: &SecurityScheme| {
                authorize_security_scheme_permission(
                    auth,
                    &environment,
                    Some(&security_scheme.name),
                    EnvironmentSecuritySchemeVerb::View,
                )
                .is_ok()
            })
            .collect())
    }

    pub async fn get_security_scheme_for_environment_and_name(
        &self,
        environment: &Environment,
        name: &SecuritySchemeName,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        authorize_security_scheme_permission(
            auth,
            environment,
            Some(name),
            EnvironmentSecuritySchemeVerb::View,
        )
        .map_err(|_| SecuritySchemeError::SecuritySchemeForNameNotFound(name.clone()))?;

        let result = self
            .security_scheme_repo
            .get_for_environment_and_name(environment.id.0, &name.0)
            .await?
            .ok_or(SecuritySchemeError::SecuritySchemeForNameNotFound(
                name.clone(),
            ))?
            .try_into()?;

        Ok(result)
    }

    pub async fn get_in_environment(
        &self,
        environment_id: EnvironmentId,
        name: &SecuritySchemeName,
        auth: &AuthCtx,
    ) -> Result<SecurityScheme, SecuritySchemeError> {
        let owner = self
            .environment_service
            .get_owner_unchecked(environment_id)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    SecuritySchemeError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;
        authorize_security_scheme_permission_for_owner(
            auth,
            owner,
            Some(name),
            EnvironmentSecuritySchemeVerb::View,
        )
        .map_err(|_| SecuritySchemeError::SecuritySchemeForNameNotFound(name.clone()))?;

        let result = self
            .security_scheme_repo
            .get_for_environment_and_name(environment_id.0, &name.0)
            .await?
            .ok_or(SecuritySchemeError::SecuritySchemeForNameNotFound(
                name.clone(),
            ))?
            .try_into()?;

        Ok(result)
    }

    async fn get_with_environment(
        &self,
        security_scheme_id: SecuritySchemeId,
        auth: &AuthCtx,
    ) -> Result<(SecurityScheme, EnvironmentOwnerPattern), SecuritySchemeError> {
        let record = self
            .security_scheme_repo
            .get_by_id(security_scheme_id.0)
            .await?
            .ok_or(SecuritySchemeError::SecuritySchemeNotFound(
                security_scheme_id,
            ))?;

        let owner = environment_owner_from_security_scheme(&record);
        let security_scheme: SecurityScheme = record.security_scheme.try_into()?;

        authorize_security_scheme_permission_for_owner(
            auth,
            owner.clone(),
            Some(&security_scheme.name),
            EnvironmentSecuritySchemeVerb::View,
        )
        .map_err(|_| SecuritySchemeError::SecuritySchemeNotFound(security_scheme_id))?;

        Ok((security_scheme, owner))
    }

    async fn validate_current_deployment(
        &self,
        candidate: &SecurityScheme,
        deleting: bool,
    ) -> Result<(), SecuritySchemeError> {
        use crate::services::deployment::validate_final_http_api_router_for_origin;
        use golem_service_base::custom_api::{RouteBehaviour, SecuritySchemeDetails};
        use std::collections::HashMap;

        let Some(deployment) = self
            .deployment_repo
            .get_currently_deployed_revision(candidate.environment_id.0)
            .await
            .map_err(anyhow::Error::from)?
        else {
            return Ok(());
        };
        for domain in self
            .deployment_repo
            .list_domains_for_deployment(candidate.environment_id.0, deployment.revision_id)
            .await
            .map_err(anyhow::Error::from)?
        {
            let bound_routes: Vec<BoundCompiledRoute> = self
                .deployment_repo
                .list_compiled_routes_for_domain_and_deployment(
                    candidate.environment_id.0,
                    deployment.revision_id,
                    &domain,
                )
                .await
                .map_err(anyhow::Error::from)?
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()
                .map_err(anyhow::Error::from)?;
            let mut schemes = HashMap::new();
            let mut routes = Vec::with_capacity(bound_routes.len());
            for bound in bound_routes {
                if let Some(details) = bound.security_scheme {
                    schemes.insert(details.name.clone(), details);
                }
                routes.push(bound.route);
            }
            if !routes
                .iter()
                .any(|route| route.security_scheme().as_ref() == Some(&candidate.name))
            {
                continue;
            }
            if deleting {
                return Err(SecuritySchemeError::InvalidLoginConfiguration(
                    "security scheme is used by an active deployment".into(),
                ));
            }
            schemes.insert(
                candidate.name.clone(),
                SecuritySchemeDetails {
                    id: candidate.id,
                    revision: candidate.revision,
                    name: candidate.name.clone(),
                    provider_type: candidate.provider_type.clone(),
                    client_id: candidate.client_id.clone(),
                    client_secret: candidate.client_secret.clone(),
                    redirect_url: candidate.redirect_url.clone(),
                    scopes: candidate.scopes.clone(),
                    login: candidate.login.clone(),
                },
            );
            let Some(public_origin) = routes.iter().find_map(|route| match &route.behaviour {
                RouteBehaviour::OpenApiSpec(behavior) => Some(behavior.scheme.origin(
                    &golem_common::model::domain_registration::Domain(domain.clone()),
                )),
                _ => None,
            }) else {
                return Err(SecuritySchemeError::InternalError(anyhow::anyhow!(
                    "Active HTTP API deployment for {domain} has no OpenAPI route"
                )));
            };
            let mut errors = Vec::new();
            validate_final_http_api_router_for_origin(
                &golem_common::model::domain_registration::Domain(domain),
                &public_origin,
                &routes,
                &schemes,
                &mut errors,
            );
            if !errors.is_empty() {
                return Err(SecuritySchemeError::InvalidLoginConfiguration(format!(
                    "security scheme update conflicts with an active deployment: {errors:?}"
                )));
            }
        }
        Ok(())
    }
}

fn validate_callback_url(value: &str) -> Result<(), SecuritySchemeError> {
    let url = url::Url::parse(value).map_err(|_| SecuritySchemeError::InvalidRedirectUrl)?;
    if !is_https_or_loopback_http(&url)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query_pairs().any(|(name, _)| {
            matches!(
                name.as_ref(),
                "code" | "state" | "error" | "error_description"
            )
        })
    {
        return Err(SecuritySchemeError::InvalidRedirectUrl);
    }
    Ok(())
}

fn validate_login(login: &SecuritySchemeLogin) -> Result<(), SecuritySchemeError> {
    let SecuritySchemeLogin::AuthorizationCodePkce(AuthorizationCodePkceConfig {
        redirect_uris,
        origins,
    }) = login
    else {
        return Ok(());
    };

    if redirect_uris.is_empty() {
        return invalid_login("at least one frontend redirect URI is required");
    }
    if origins.is_empty() {
        return invalid_login("at least one frontend origin is required");
    }
    if has_duplicates(redirect_uris) {
        return invalid_login("frontend redirect URIs must be unique");
    }
    if has_duplicates(origins) {
        return invalid_login("frontend origins must be unique");
    }

    let parsed_origins = origins
        .iter()
        .map(|value| {
            let url = url::Url::parse(value)
                .map_err(|_| "frontend origin must be an absolute URL".to_string())?;
            let canonical = url.origin().ascii_serialization();
            if !is_https_or_loopback_http(&url)
                || url.host_str().is_none()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.path() != "/"
                || url.query().is_some()
                || url.fragment().is_some()
                || canonical != *value
            {
                return Err(
                    "frontend origin must be an exact HTTPS origin or loopback HTTP origin"
                        .to_string(),
                );
            }
            Ok(canonical)
        })
        .collect::<Result<std::collections::HashSet<_>, _>>()
        .map_err(SecuritySchemeError::InvalidLoginConfiguration)?;

    for value in redirect_uris {
        let url = url::Url::parse(value).map_err(|_| {
            SecuritySchemeError::InvalidLoginConfiguration(
                "frontend redirect URI must be an absolute URL".to_string(),
            )
        })?;
        if !is_https_or_loopback_http(&url)
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.query_pairs().any(|(name, _)| {
                matches!(
                    name.as_ref(),
                    "code" | "error" | "error_description" | "state"
                )
            })
        {
            return invalid_login(
                "frontend redirect URI must be HTTPS (or loopback HTTP), without credentials, fragment, or OAuth response parameters",
            );
        }
        if !parsed_origins.contains(&url.origin().ascii_serialization()) {
            return invalid_login("every frontend redirect URI origin must be configured");
        }
    }

    Ok(())
}

fn invalid_login<T>(message: &str) -> Result<T, SecuritySchemeError> {
    Err(SecuritySchemeError::InvalidLoginConfiguration(
        message.to_string(),
    ))
}

fn has_duplicates(values: &[String]) -> bool {
    let mut unique = std::collections::HashSet::new();
    values.iter().any(|value| !unique.insert(value))
}

fn is_https_or_loopback_http(url: &url::Url) -> bool {
    if url.scheme() == "https" {
        return true;
    }
    if url.scheme() != "http" {
        return false;
    }
    match url.host() {
        Some(url::Host::Domain("localhost")) => true,
        Some(url::Host::Ipv4(address)) => address.is_loopback(),
        Some(url::Host::Ipv6(address)) => address.is_loopback(),
        _ => false,
    }
}

#[cfg(test)]
mod callback_url_tests {
    use super::*;
    use test_r::test;

    #[test]
    fn callback_url_rejects_oauth_response_parameter_collisions() {
        for parameter in ["code", "error", "error_description", "state"] {
            assert!(
                validate_callback_url(&format!(
                    "https://api.example.com/oidc/callback?{parameter}=fixed"
                ))
                .is_err(),
                "callback URL containing {parameter} was accepted"
            );
        }
    }
}
