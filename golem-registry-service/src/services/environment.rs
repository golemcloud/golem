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

use super::account_usage::AccountUsageService;
use super::account_usage::error::{AccountUsageError, LimitExceededError};
use super::application::ApplicationService;
use super::registry_change_notifier::{RegistryChangeNotifier, RequiresNotificationSignalExt};
use crate::repo::environment::{
    EnvironmentRepo, EnvironmentRevisionRecord, EnvironmentVisibilityFilter,
    EnvironmentVisibilityScope,
};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::environment::EnvironmentRepoError;
use crate::repo::model::environment_plugin_grant::EnvironmentPluginGrantRecord;
use crate::repo::plugin::PluginRepo;
use crate::services::application::ApplicationError;
use golem_common::model::account::{AccountEmail, AccountId};
use golem_common::model::application::{ApplicationId, ApplicationName};
use golem_common::model::card::owner::ApplicationOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EffectiveSurface,
    EnvironmentResourcePattern, EnvironmentVerb, PermissionTarget,
};
use golem_common::model::environment::{
    Environment, EnvironmentCreation, EnvironmentId, EnvironmentName, EnvironmentRevision,
    EnvironmentUpdate, EnvironmentWithDetails,
};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::{IntoAnyhow, SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tap::Pipe;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentError {
    #[error("Environment with this name already exists")]
    EnvironmentWithNameAlreadyExists,
    #[error("Environment not found for id {0}")]
    EnvironmentNotFound(EnvironmentId),
    #[error("Environment not found for name {0}")]
    EnvironmentByNameNotFound(EnvironmentName),
    #[error("Application {0} not found")]
    ParentApplicationNotFound(ApplicationId),
    #[error("Concurrent update attempt")]
    ConcurrentModification,
    #[error(transparent)]
    LimitExceeded(LimitExceededError),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for EnvironmentError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::EnvironmentWithNameAlreadyExists => self.to_string(),
            Self::EnvironmentNotFound(_) => self.to_string(),
            Self::EnvironmentByNameNotFound(_) => self.to_string(),
            Self::ParentApplicationNotFound(_) => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::LimitExceeded(inner) => inner.to_safe_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    EnvironmentError,
    RepoError,
    ApplicationError,
    EnvironmentRepoError
);

impl From<AccountUsageError> for EnvironmentError {
    fn from(value: AccountUsageError) -> Self {
        match value {
            AccountUsageError::LimitExceeded(inner) => EnvironmentError::LimitExceeded(inner),
            other => Self::InternalError(other.into_anyhow()),
        }
    }
}

pub struct EnvironmentService {
    environment_repo: Arc<dyn EnvironmentRepo>,
    application_service: Arc<ApplicationService>,
    account_usage_service: Arc<AccountUsageService>,
    plugin_repo: Arc<dyn PluginRepo>,
    builtin_plugin_owner_account_id: AccountId,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

impl EnvironmentService {
    pub fn new(
        environment_repo: Arc<dyn EnvironmentRepo>,
        application_service: Arc<ApplicationService>,
        account_usage_service: Arc<AccountUsageService>,
        plugin_repo: Arc<dyn PluginRepo>,
        builtin_plugin_owner_account_id: AccountId,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            environment_repo,
            application_service,
            account_usage_service,
            plugin_repo,
            builtin_plugin_owner_account_id,
            registry_change_notifier,
        }
    }

    pub async fn create(
        &self,
        application_id: ApplicationId,
        data: EnvironmentCreation,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentError> {
        let application = self
            .application_service
            .get(application_id, auth)
            .await
            .map_err(|err| match err {
                ApplicationError::ApplicationNotFound(application_id) => {
                    EnvironmentError::ParentApplicationNotFound(application_id)
                }
                other => other.into(),
            })?;

        authorize_environment_permission(
            auth,
            &application.account_email,
            &application.name,
            EnvironmentVerb::Create,
            EnvironmentResourcePattern::Environment(CardEnvironmentName(data.name.0.clone())),
        )?;

        self.account_usage_service
            .ensure_environment_within_limits(application.account_id)
            .await?;

        let record = EnvironmentRevisionRecord::creation(data, auth.actor_account_id());

        let builtin_plugins = self
            .plugin_repo
            .list_by_account(self.builtin_plugin_owner_account_id.0)
            .await?;

        let plugin_grants: Vec<EnvironmentPluginGrantRecord> = builtin_plugins
            .into_iter()
            .map(|p| {
                EnvironmentPluginGrantRecord::creation(
                    EnvironmentId(record.environment_id),
                    PluginRegistrationId(p.plugin_id),
                    auth.actor_account_id(),
                )
            })
            .collect();

        let result = self
            .environment_repo
            .create_with_plugin_grants(application_id.0, record, plugin_grants)
            .await
            .map_err(|err| match err {
                EnvironmentRepoError::EnvironmentViolatesUniqueness => {
                    EnvironmentError::EnvironmentWithNameAlreadyExists
                }
                other => other.into(),
            })?
            .try_into_model(
                application.name,
                application.account_id,
                application.account_email,
            )?;

        Ok(result)
    }

    pub async fn update(
        &self,
        environment_id: EnvironmentId,
        update: EnvironmentUpdate,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentError> {
        let mut environment = self.get(environment_id, false, auth).await?;

        authorize_environment_model(auth, &environment, EnvironmentVerb::Update)?;

        if update.current_revision != environment.revision {
            return Err(EnvironmentError::ConcurrentModification);
        };

        environment.revision = environment.revision.next()?;
        if let Some(new_name) = update.name {
            environment.name = new_name
        };
        if let Some(compatibility_check) = update.compatibility_check {
            environment.compatibility_check = compatibility_check;
        }
        if let Some(version_check) = update.version_check {
            environment.version_check = version_check;
        }
        if let Some(security_overrides) = update.security_overrides {
            environment.security_overrides = security_overrides;
        }

        let application_name = environment.application_name.clone();
        let owner_account_id = environment.owner_account_id;
        let owner_account_email = environment.owner_account_email.clone();
        let audit = DeletableRevisionAuditFields::new(auth.actor_account_id().0);
        let record = EnvironmentRevisionRecord::from_model(environment, audit);

        let result = self
            .environment_repo
            .update(record)
            .await
            .map_err(|err| match err {
                EnvironmentRepoError::ConcurrentModification => {
                    EnvironmentError::ConcurrentModification
                }
                EnvironmentRepoError::EnvironmentViolatesUniqueness => {
                    EnvironmentError::EnvironmentWithNameAlreadyExists
                }
                other => other.into(),
            })?
            .try_into_model(application_name, owner_account_id, owner_account_email)?;

        Ok(result)
    }

    pub async fn delete(
        &self,
        environment_id: EnvironmentId,
        current_revision: EnvironmentRevision,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentError> {
        let mut environment = self.get(environment_id, false, auth).await?;

        authorize_environment_model(auth, &environment, EnvironmentVerb::Delete)?;

        if current_revision != environment.revision {
            return Err(EnvironmentError::ConcurrentModification);
        };

        environment.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.actor_account_id().0);
        let record = EnvironmentRevisionRecord::from_model(environment, audit);

        self.environment_repo
            .delete(record)
            .await
            .map_err(|err| match err {
                EnvironmentRepoError::ConcurrentModification => {
                    EnvironmentError::ConcurrentModification
                }
                other => other.into(),
            })?
            .signal_new_events_available(&self.registry_change_notifier);

        Ok(())
    }

    pub async fn get(
        &self,
        environment_id: EnvironmentId,
        include_deleted: bool,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentError> {
        let environment: Environment = self
            .environment_repo
            .get_by_id(environment_id.0, include_deleted)
            .await?
            .ok_or(EnvironmentError::EnvironmentNotFound(environment_id))?
            .try_into()?;

        authorize_environment_model(auth, &environment, EnvironmentVerb::View)
            .map_err(|_| EnvironmentError::EnvironmentNotFound(environment_id))?;

        Ok(environment)
    }

    pub async fn get_in_application(
        &self,
        application_id: ApplicationId,
        name: &EnvironmentName,
        auth: &AuthCtx,
    ) -> Result<Environment, EnvironmentError> {
        let application = self
            .application_service
            .get(application_id, auth)
            .await
            .map_err(|err| match err {
                ApplicationError::ApplicationNotFound(application_id) => {
                    EnvironmentError::ParentApplicationNotFound(application_id)
                }
                other => other.into(),
            })?;

        authorize_environment_permission(
            auth,
            &application.account_email,
            &application.name,
            EnvironmentVerb::View,
            EnvironmentResourcePattern::Environment(CardEnvironmentName(name.0.clone())),
        )
        .map_err(|_| EnvironmentError::EnvironmentByNameNotFound(name.clone()))?;

        let result = self
            .environment_repo
            .get_by_name(application_id.0, &name.0)
            .await?
            .ok_or(EnvironmentError::EnvironmentByNameNotFound(name.clone()))?
            .try_into_model(
                application.name,
                application.account_id,
                application.account_email,
            )?;

        Ok(result)
    }

    pub async fn list_in_application(
        &self,
        application_id: ApplicationId,
        auth: &AuthCtx,
    ) -> Result<Vec<Environment>, EnvironmentError> {
        let application = self
            .application_service
            .get(application_id, auth)
            .await
            .map_err(|err| match err {
                ApplicationError::ApplicationNotFound(application_id) => {
                    EnvironmentError::ParentApplicationNotFound(application_id)
                }
                other => other.into(),
            })?;

        let environments = self
            .environment_repo
            .list_by_app(application_id.0)
            .await?
            .into_iter()
            .map(|record| {
                record.try_into_model(
                    application.name.clone(),
                    application.account_id,
                    application.account_email.clone(),
                )
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(environments
            .into_iter()
            .filter(|environment| {
                authorize_environment_model(auth, environment, EnvironmentVerb::View).is_ok()
            })
            .collect())
    }

    pub async fn list_visible_environments(
        &self,
        account_email: Option<&AccountEmail>,
        app_name: Option<&ApplicationName>,
        env_name: Option<&EnvironmentName>,
        auth: &AuthCtx,
    ) -> Result<Vec<EnvironmentWithDetails>, EnvironmentError> {
        // When we go for an admin ui / view, this should be extended with an optional, admin-only parameter that allows listing for a different account.
        let visibility_filter = visible_environment_filter(auth);

        self.environment_repo
            .list_visible_to_account(
                auth.access_account_id().0,
                &visibility_filter,
                account_email.map(|ae| ae.as_str()),
                app_name.map(|an| an.0.as_str()),
                env_name.map(|en| en.0.as_str()),
            )
            .await?
            .into_iter()
            .map(EnvironmentWithDetails::try_from)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // The repo fetches candidates; card authorization decides visibility.
            .filter(|e| authorize_environment_details(auth, e, EnvironmentVerb::View).is_ok())
            .collect::<Vec<_>>()
            .pipe(Ok)
    }
}

fn visible_environment_filter(auth: &AuthCtx) -> EnvironmentVisibilityFilter {
    match auth {
        AuthCtx::System => EnvironmentVisibilityFilter::All,
        AuthCtx::User(user) => visible_environment_filter_from_surface(&user.effective_surface),
        AuthCtx::AdminImpersonation(ctx) => {
            visible_environment_filter_from_surface(&ctx.effective_surface)
        }
        AuthCtx::Agent(agent) => {
            EnvironmentVisibilityFilter::from_scopes([EnvironmentVisibilityScope::account(
                agent.account_email.as_str(),
            )])
        }
    }
}

fn visible_environment_filter_from_surface(
    effective_surface: &EffectiveSurface,
) -> EnvironmentVisibilityFilter {
    EnvironmentVisibilityFilter::from_scopes(
        effective_surface
            .lower
            .iter()
            .flat_map(|surface| surface.positive.iter())
            .filter_map(visible_environment_scope_from_target),
    )
}

fn visible_environment_scope_from_target(
    target: &PermissionTarget,
) -> Option<EnvironmentVisibilityScope> {
    let PermissionTarget::Environment(target) = target else {
        return None;
    };

    if !matches!(target.verb, None | Some(EnvironmentVerb::View)) {
        return None;
    }

    let env_name = match &target.resource {
        EnvironmentResourcePattern::Any => None,
        EnvironmentResourcePattern::Environment(environment)
        | EnvironmentResourcePattern::Revision { environment, .. } => Some(environment.0.clone()),
    };

    match &target.owner {
        ApplicationOwnerPattern::AnyApplications => {
            Some(EnvironmentVisibilityScope::any_owner(env_name))
        }
        ApplicationOwnerPattern::AccountApplications { account } => {
            Some(EnvironmentVisibilityScope {
                account_email: Some(account.as_str().to_string()),
                app_name: None,
                env_name,
            })
        }
        ApplicationOwnerPattern::Application {
            account,
            application,
        } => Some(EnvironmentVisibilityScope::application(
            account.as_str(),
            application.0.clone(),
            env_name,
        )),
    }
}

fn authorize_environment_permission(
    auth: &AuthCtx,
    account_email: &AccountEmail,
    application_name: &ApplicationName,
    verb: EnvironmentVerb,
    resource: EnvironmentResourcePattern,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::Environment(ClassPermissionTarget {
        verb: Some(verb),
        owner: ApplicationOwnerPattern::Application {
            account: account_email.clone(),
            application: application_name.clone(),
        },
        resource,
    }))
}

fn authorize_environment_model(
    auth: &AuthCtx,
    environment: &Environment,
    verb: EnvironmentVerb,
) -> Result<(), AuthorizationError> {
    authorize_environment_permission(
        auth,
        &environment.owner_account_email,
        &environment.application_name,
        verb,
        EnvironmentResourcePattern::Environment(CardEnvironmentName(environment.name.0.clone())),
    )
}

fn authorize_environment_details(
    auth: &AuthCtx,
    environment: &EnvironmentWithDetails,
    verb: EnvironmentVerb,
) -> Result<(), AuthorizationError> {
    authorize_environment_permission(
        auth,
        &environment.account.email,
        &environment.application.name,
        verb,
        EnvironmentResourcePattern::Environment(CardEnvironmentName(
            environment.environment.name.0.clone(),
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::card::{EffectiveSurface, GrantSurface};
    use test_r::test;

    fn environment_target(
        account: &str,
        application: &str,
        verb: Option<EnvironmentVerb>,
        resource: EnvironmentResourcePattern,
    ) -> PermissionTarget {
        PermissionTarget::Environment(ClassPermissionTarget {
            verb,
            owner: ApplicationOwnerPattern::Application {
                account: AccountEmail::new(account),
                application: ApplicationName(application.to_string()),
            },
            resource,
        })
    }

    #[test]
    fn visible_environment_filter_uses_lower_view_grants_only_and_normalizes_scopes() {
        let surface = EffectiveSurface {
            source_card_ids: Vec::new(),
            lower: vec![GrantSurface {
                positive: vec![
                    PermissionTarget::Environment(ClassPermissionTarget {
                        verb: Some(EnvironmentVerb::View),
                        owner: ApplicationOwnerPattern::AccountApplications {
                            account: AccountEmail::new("owner@golem"),
                        },
                        resource: EnvironmentResourcePattern::Any,
                    }),
                    environment_target(
                        "owner@golem",
                        "narrower-app",
                        Some(EnvironmentVerb::View),
                        EnvironmentResourcePattern::Environment(CardEnvironmentName(
                            "narrower-env".to_string(),
                        )),
                    ),
                    environment_target(
                        "shared@golem",
                        "shared-app",
                        Some(EnvironmentVerb::View),
                        EnvironmentResourcePattern::Environment(CardEnvironmentName(
                            "shared-env".to_string(),
                        )),
                    ),
                    environment_target(
                        "ignored@golem",
                        "ignored-app",
                        Some(EnvironmentVerb::Deploy),
                        EnvironmentResourcePattern::Any,
                    ),
                ],
                negative: vec![environment_target(
                    "owner@golem",
                    "negative-app",
                    Some(EnvironmentVerb::View),
                    EnvironmentResourcePattern::Any,
                )],
            }],
            upper: Vec::new(),
        };

        let filter = visible_environment_filter_from_surface(&surface);

        assert_eq!(
            filter,
            EnvironmentVisibilityFilter::Scopes(vec![
                EnvironmentVisibilityScope {
                    account_email: Some("owner@golem".to_string()),
                    app_name: None,
                    env_name: None,
                },
                EnvironmentVisibilityScope {
                    account_email: Some("shared@golem".to_string()),
                    app_name: Some("shared-app".to_string()),
                    env_name: Some("shared-env".to_string()),
                },
            ])
        );
    }
}
