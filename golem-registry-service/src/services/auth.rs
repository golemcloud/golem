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

use crate::repo::account::AccountRepo;
use crate::repo::card::CardRepo;
use crate::repo::model::account::AccountRepoError;
use crate::repo::model::card::CardRepoError;
use crate::services::account::{AccountError, AccountService};
use crate::services::permission_share::{PermissionShareError, PermissionShareService};
use chrono::Utc;
use golem_common::model::account::{Account, AccountId};
use golem_common::model::auth::{TokenId, TokenSecret};
use golem_common::model::card::owner::{ApplicationOwnerPattern, EnvironmentOwnerPattern};
use golem_common::model::card::recipient::RecipientPattern;
use golem_common::model::card::{
    Card, ClassPermissionTarget, ComponentName as CardComponentName, ComponentResourcePattern,
    ComponentVerb, DomainNamePattern, EffectiveSurface, EnvironmentAgentSecretKeyPathPattern,
    EnvironmentAgentSecretResourcePattern, EnvironmentAgentSecretVerb,
    EnvironmentDomainRegistrationResourcePattern, EnvironmentDomainRegistrationVerb,
    EnvironmentHttpApiDeploymentResourcePattern, EnvironmentHttpApiDeploymentVerb,
    EnvironmentMcpDeploymentName, EnvironmentMcpDeploymentResourcePattern,
    EnvironmentMcpDeploymentVerb, EnvironmentName as CardEnvironmentName,
    EnvironmentResourceDefinitionName, EnvironmentResourceDefinitionResourcePattern,
    EnvironmentResourceDefinitionVerb, EnvironmentResourcePattern, EnvironmentRetryPolicyName,
    EnvironmentRetryPolicyResourcePattern, EnvironmentRetryPolicyVerb,
    EnvironmentSecuritySchemeName, EnvironmentSecuritySchemeResourcePattern,
    EnvironmentSecuritySchemeVerb, EnvironmentShareResourcePattern, EnvironmentShareVerb,
    EnvironmentVerb, PermissionTarget,
};
use golem_common::model::component::ComponentName;
use golem_common::model::domain_registration::Domain;
use golem_common::model::environment::Environment;
use golem_common::model::environment_share::EnvironmentShareId;
use golem_common::model::quota::ResourceName;
use golem_common::model::security_scheme::SecuritySchemeName;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{
    AdminImpersonationAuthCtx, AuthCtx, AuthorizationError, GlobalAction, UserAuthCtx,
};
use golem_service_base::repo::RepoError;
use std::collections::BTreeSet;
use std::sync::Arc;
use tracing::warn;

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Could not authenticate user using token")]
    CouldNotAuthenticate,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

pub fn authorize_environment_permission(
    auth: &AuthCtx,
    environment: &Environment,
    verb: EnvironmentVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::Environment(ClassPermissionTarget {
        verb: Some(verb),
        owner: ApplicationOwnerPattern::Application {
            account: environment.owner_account_id.to_string(),
            application: environment.application_name.0.clone(),
        },
        resource: EnvironmentResourcePattern::Environment(CardEnvironmentName(
            environment.name.0.clone(),
        )),
    }))
}

pub fn authorize_component_permission(
    auth: &AuthCtx,
    environment: &Environment,
    component_name: &ComponentName,
    verb: ComponentVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::Component(ClassPermissionTarget {
        verb: Some(verb),
        owner: environment_owner(environment),
        resource: ComponentResourcePattern::Component(CardComponentName(component_name.0.clone())),
    }))
}

pub fn authorize_environment_share_permission(
    auth: &AuthCtx,
    environment: &Environment,
    environment_share_id: Option<EnvironmentShareId>,
    verb: EnvironmentShareVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentShare(ClassPermissionTarget {
        verb: Some(verb),
        owner: environment_owner(environment),
        resource: environment_share_id
            .map(EnvironmentShareResourcePattern::Share)
            .unwrap_or(EnvironmentShareResourcePattern::Any),
    }))
}

pub fn authorize_domain_registration_permission(
    auth: &AuthCtx,
    environment: &Environment,
    domain: Option<&Domain>,
    verb: EnvironmentDomainRegistrationVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentDomainRegistration(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
            resource: domain
                .map(|domain| {
                    EnvironmentDomainRegistrationResourcePattern::Domain(domain_name_pattern(
                        domain,
                    ))
                })
                .unwrap_or(EnvironmentDomainRegistrationResourcePattern::Any),
        },
    ))
}

pub fn authorize_security_scheme_permission(
    auth: &AuthCtx,
    environment: &Environment,
    name: Option<&SecuritySchemeName>,
    verb: EnvironmentSecuritySchemeVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentSecurityScheme(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
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

pub fn authorize_http_api_deployment_permission(
    auth: &AuthCtx,
    environment: &Environment,
    domain: Option<&Domain>,
    verb: EnvironmentHttpApiDeploymentVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentHttpApiDeployment(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
            resource: domain
                .map(
                    |domain| EnvironmentHttpApiDeploymentResourcePattern::DomainPath {
                        domain: domain.0.clone(),
                        path_glob: "/**".to_string(),
                    },
                )
                .unwrap_or(EnvironmentHttpApiDeploymentResourcePattern::Any),
        },
    ))
}

pub fn authorize_mcp_deployment_permission(
    auth: &AuthCtx,
    environment: &Environment,
    domain: Option<&Domain>,
    verb: EnvironmentMcpDeploymentVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentMcpDeployment(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
            resource: domain
                .map(|domain| {
                    EnvironmentMcpDeploymentResourcePattern::Name(EnvironmentMcpDeploymentName(
                        domain.0.clone(),
                    ))
                })
                .unwrap_or(EnvironmentMcpDeploymentResourcePattern::Any),
        },
    ))
}

pub fn authorize_agent_secret_permission(
    auth: &AuthCtx,
    environment: &Environment,
    key: Option<&[String]>,
    verb: EnvironmentAgentSecretVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentAgentSecret(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
            resource: key
                .map(|key| {
                    EnvironmentAgentSecretResourcePattern::Key(
                        EnvironmentAgentSecretKeyPathPattern::parse(&key.join("."))
                            .expect("agent secret keys are valid card resources"),
                    )
                })
                .unwrap_or(EnvironmentAgentSecretResourcePattern::Any),
        },
    ))
}

pub fn authorize_resource_definition_permission(
    auth: &AuthCtx,
    environment: &Environment,
    name: Option<&ResourceName>,
    verb: EnvironmentResourceDefinitionVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentResourceDefinition(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
            resource: name
                .map(|name| {
                    EnvironmentResourceDefinitionResourcePattern::Name(
                        EnvironmentResourceDefinitionName(name.0.clone()),
                    )
                })
                .unwrap_or(EnvironmentResourceDefinitionResourcePattern::Any),
        },
    ))
}

pub fn authorize_retry_policy_permission(
    auth: &AuthCtx,
    environment: &Environment,
    name: Option<&str>,
    verb: EnvironmentRetryPolicyVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentRetryPolicy(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: environment_owner(environment),
            resource: name
                .map(|name| {
                    EnvironmentRetryPolicyResourcePattern::Name(EnvironmentRetryPolicyName(
                        name.to_string(),
                    ))
                })
                .unwrap_or(EnvironmentRetryPolicyResourcePattern::Any),
        },
    ))
}

fn environment_owner(environment: &Environment) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: environment.owner_account_id.to_string(),
        application: environment.application_name.0.clone(),
        environment: environment.name.0.clone(),
    }
}

fn domain_name_pattern(domain: &Domain) -> DomainNamePattern {
    DomainNamePattern {
        labels: domain
            .0
            .split('.')
            .map(|label| golem_common::model::card::DomainLabel(label.to_string()))
            .collect(),
    }
}

impl SafeDisplay for AuthError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::CouldNotAuthenticate => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    AuthError,
    AccountError,
    AccountRepoError,
    CardRepoError,
    RepoError,
    PermissionShareError
);

pub struct AuthService {
    account_repo: Arc<dyn AccountRepo>,
    account_service: Arc<AccountService>,
    card_repo: Arc<dyn CardRepo>,
    permission_share_service: Arc<PermissionShareService>,
}

impl AuthService {
    pub fn new(
        account_repo: Arc<dyn AccountRepo>,
        account_service: Arc<AccountService>,
        card_repo: Arc<dyn CardRepo>,
        permission_share_service: Arc<PermissionShareService>,
    ) -> Self {
        Self {
            account_repo,
            account_service,
            card_repo,
            permission_share_service,
        }
    }

    pub async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthError> {
        let record = self
            .account_repo
            .get_by_secret(token.secret())
            .await?
            .ok_or(AuthError::CouldNotAuthenticate)?;

        // IMPORTANT: make sure the token is still valid
        if *record.token_expires_at.as_utc() <= Utc::now() {
            warn!(
                "Tried to resolve an expired token {}",
                TokenId(record.token_id)
            );
            return Err(AuthError::CouldNotAuthenticate);
        };

        let impersonated_by = record.impersonated_by.map(AccountId);
        let target_account: Account = record.value.try_into()?;

        match impersonated_by {
            // Normal login flow
            None => {
                let account_roles = BTreeSet::from_iter(target_account.roles.clone());
                let effective_surface = self.materialize_effective_surface(&target_account).await?;

                Ok(AuthCtx::User(UserAuthCtx {
                    account_id: target_account.id,
                    account_roles,
                    account_plan_id: target_account.plan_id,
                    effective_surface,
                }))
            }
            // Impersonation flow
            Some(admin_account_id) => {
                // Ensure the admin account is still alive and still has impersonation rights
                let admin_account: Account = self
                    .account_service
                    .get(admin_account_id, &AuthCtx::System)
                    .await
                    .map_err(|_| AuthError::CouldNotAuthenticate)?;

                {
                    let account_roles = BTreeSet::from_iter(admin_account.roles.clone());
                    let effective_surface =
                        self.materialize_effective_surface(&admin_account).await?;
                    let admin_auth_ctx = AuthCtx::User(UserAuthCtx {
                        account_id: admin_account.id,
                        account_roles,
                        account_plan_id: admin_account.plan_id,
                        effective_surface,
                    });

                    if admin_auth_ctx
                        .authorize_global_action(GlobalAction::ImpersonateUser)
                        .is_err()
                    {
                        warn!(
                            "Admin that minted the token ({}), is no longer allowed to impersonate. Failing auth",
                            admin_account_id
                        );
                        return Err(AuthError::CouldNotAuthenticate);
                    };
                }

                let target_account_roles = BTreeSet::from_iter(target_account.roles.clone());
                let effective_surface = self.materialize_effective_surface(&target_account).await?;

                Ok(AuthCtx::AdminImpersonation(AdminImpersonationAuthCtx {
                    admin_account_id,
                    target_account_id: target_account.id,
                    target_account_roles,
                    target_account_plan_id: target_account.plan_id,
                    effective_surface,
                }))
            }
        }
    }

    async fn materialize_effective_surface(
        &self,
        account: &Account,
    ) -> Result<EffectiveSurface, AuthError> {
        let account_root_card: Card = self
            .card_repo
            .get(account.account_root_card_id)
            .await?
            .ok_or_else(|| {
                tracing::warn!(
                    "Account root card {} for account {} does not exist",
                    account.account_root_card_id,
                    account.id
                );
                AuthError::CouldNotAuthenticate
            })?
            .try_into()?;

        let share_cards = self
            .permission_share_service
            .active_share_cards_for_target(account.id)
            .await?;

        let mut cards = Vec::with_capacity(1 + share_cards.len());
        cards.push(account_root_card);
        cards.extend(share_cards);

        let account_recipient = RecipientPattern::Account {
            account: account.email.as_str().to_string(),
        };

        EffectiveSurface::from_cards(&cards, &account_recipient).map_err(|err| {
            AuthError::InternalError(anyhow::anyhow!(
                "Failed to materialize effective surface for account {}: {:?}",
                account.id,
                err
            ))
        })
    }
}
