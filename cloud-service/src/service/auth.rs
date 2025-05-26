use async_trait::async_trait;
use cloud_common::model::{
    ProjectAction, ProjectActions, ProjectAuthorisedActions, ProjectPermisison, Role, TokenSecret,
};
use golem_service_base::repo::RepoError;
use std::collections::HashSet;
use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::model::ProjectPolicy;
use crate::model::{AccountAction, GlobalAction};
use crate::repo::account::AccountRepo;
use crate::repo::account_grant::AccountGrantRepo;
use crate::repo::project::ProjectRepo;
use crate::repo::project_grant::ProjectGrantRepo;
use crate::repo::project_policy::ProjectPolicyRepo;
use crate::service::token::{TokenService, TokenServiceError};
use golem_common::model::{AccountId, ProjectId};
use golem_common::SafeDisplay;

#[derive(Debug, thiserror::Error)]
pub enum AuthServiceError {
    #[error("Invalid Token: {0}")]
    InvalidToken(String),
    #[error(transparent)]
    InternalTokenServiceError(TokenServiceError),
    #[error("Action is only allowed for [{}]", allowed_roles.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(", "))]
    RoleMissing { allowed_roles: Vec<Role> },
    #[error("Action is not allowed on different accounts")]
    AccountOwnershipRequired,
    #[error("Access to account `{account_id}` is not allowed")]
    AccountAccessForbidden { account_id: AccountId },
    #[error("Access to project `{project_id}` is not allowed")]
    ProjectAccessForbidden { project_id: ProjectId },
    #[error("Action `{requested_action}` is not allowed on project `{project_id}`")]
    ProjectActionForbidden {
        requested_action: ProjectAction,
        project_id: ProjectId,
    },
    #[error(transparent)]
    InternalRepoError(RepoError),
}

impl AuthServiceError {
    fn invalid_token(error: impl AsRef<str>) -> Self {
        AuthServiceError::InvalidToken(error.as_ref().to_string())
    }
}

impl SafeDisplay for AuthServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            AuthServiceError::InvalidToken(_) => self.to_string(),
            AuthServiceError::InternalTokenServiceError(inner) => inner.to_safe_string(),
            AuthServiceError::RoleMissing { .. } => self.to_string(),
            AuthServiceError::AccountOwnershipRequired => self.to_string(),
            AuthServiceError::AccountAccessForbidden { .. } => self.to_string(),
            AuthServiceError::ProjectAccessForbidden { .. } => self.to_string(),
            AuthServiceError::ProjectActionForbidden { .. } => self.to_string(),
            AuthServiceError::InternalRepoError(inner) => inner.to_safe_string(),
        }
    }
}

impl From<TokenServiceError> for AuthServiceError {
    fn from(error: TokenServiceError) -> Self {
        match error {
            TokenServiceError::UnknownToken(id) => {
                AuthServiceError::invalid_token(format!("Invalid token id: {}", id))
            }
            _ => AuthServiceError::InternalTokenServiceError(error),
        }
    }
}

impl From<RepoError> for AuthServiceError {
    fn from(error: RepoError) -> Self {
        Self::InternalRepoError(error)
    }
}

#[derive(Debug, Clone)]
pub enum ViewableProjects {
    /// Special case for admins, they can see all projects even if no grant is present.
    All,
    OwnedAndAdditional {
        additional_project_ids: Vec<ProjectId>,
    },
}

#[derive(Debug, Clone)]
pub enum ViewableAccounts {
    /// Special case for admins, they can see all accounts even if no grant is present.
    All,
    Limited {
        account_ids: Vec<AccountId>,
    },
}

#[derive(Debug, Clone)]
pub struct AuthorizedProjectAction {
    pub own_account_id: AccountId,
    pub project_owner_account_id: AccountId,
    pub project_id: ProjectId,
    pub action: ProjectAction,
}

#[async_trait]
pub trait AuthService: Send + Sync {
    async fn authorization(
        &self,
        secret: &TokenSecret,
    ) -> Result<AccountAuthorisation, AuthServiceError>;

    /// All projects that are viewable from this account (that support ProjectAction::ViewProject)
    async fn viewable_projects(
        &self,
        auth: &AccountAuthorisation,
    ) -> Result<ViewableProjects, AuthServiceError>;

    /// All accounts that are viewable from this account (that support AccountAction::ViewAccount)
    async fn viewable_accounts(
        &self,
        auth: &AccountAuthorisation,
    ) -> Result<ViewableAccounts, AuthServiceError>;

    async fn authorize_global_action(
        &self,
        auth: &AccountAuthorisation,
        requested_action: &GlobalAction,
    ) -> Result<(), AuthServiceError>;

    async fn authorize_account_action(
        &self,
        auth: &AccountAuthorisation,
        account_id: &AccountId,
        requested_action: &AccountAction,
    ) -> Result<(), AuthServiceError>;

    async fn authorize_project_action(
        &self,
        auth: &AccountAuthorisation,
        project_id: &ProjectId,
        requested_action: &ProjectAction,
    ) -> Result<AuthorizedProjectAction, AuthServiceError>;

    async fn get_project_actions(
        &self,
        auth: &AccountAuthorisation,
        project_id: &ProjectId,
    ) -> Result<ProjectAuthorisedActions, AuthServiceError>;
}

/// This is the foundation of all other services. Avoid depending on other services here and instead query the repositories directly
/// to avoid cyclical dependencies / logic.
pub struct AuthServiceDefault {
    token_service: Arc<dyn TokenService + Send + Sync>,
    account_repo: Arc<dyn AccountRepo + Send + Sync>,
    account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync>,
    project_repo: Arc<dyn ProjectRepo + Sync + Send>,
    project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
    project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send>,
}

impl AuthServiceDefault {
    pub fn new(
        token_service: Arc<dyn TokenService + Send + Sync>,
        account_repo: Arc<dyn AccountRepo + Send + Sync>,
        account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync>,
        project_repo: Arc<dyn ProjectRepo + Sync + Send>,
        project_policy_repo: Arc<dyn ProjectPolicyRepo + Sync + Send>,
        project_grant_repo: Arc<dyn ProjectGrantRepo + Sync + Send>,
    ) -> Self {
        AuthServiceDefault {
            token_service,
            account_repo,
            account_grant_repo,
            project_repo,
            project_policy_repo,
            project_grant_repo,
        }
    }
}

#[async_trait]
impl AuthService for AuthServiceDefault {
    async fn authorization(
        &self,
        secret: &TokenSecret,
    ) -> Result<AccountAuthorisation, AuthServiceError> {
        let token = self
            .token_service
            .get_by_secret(secret)
            .await?
            .ok_or(AuthServiceError::invalid_token("Unknown token secret."))?;

        let account_roles = self.account_grant_repo.get(&token.account_id).await?;

        let now = chrono::Utc::now();
        if token.expires_at > now {
            Ok(AccountAuthorisation::new(token, account_roles))
        } else {
            Err(AuthServiceError::invalid_token("Expired auth token."))
        }
    }

    async fn authorize_global_action(
        &self,
        auth: &AccountAuthorisation,
        requested_action: &GlobalAction,
    ) -> Result<(), AuthServiceError> {
        match requested_action {
            GlobalAction::CreateAccount => limit_to_roles(auth, &[Role::Admin]),
            GlobalAction::ViewAccountSummaries => {
                limit_to_roles(auth, &[Role::Admin, Role::MarketingAdmin])
            }
            GlobalAction::ViewAccountCount => {
                limit_to_roles(auth, &[Role::Admin, Role::MarketingAdmin])
            }
        }
    }

    async fn authorize_account_action(
        &self,
        auth: &AccountAuthorisation,
        account_id: &AccountId,
        requested_action: &AccountAction,
    ) -> Result<(), AuthServiceError> {
        let account = self.account_repo.get(&account_id.value).await?;

        if account.is_none() {
            Err(AuthServiceError::AccountAccessForbidden {
                account_id: account_id.clone(),
            })?
        };

        match requested_action {
            AccountAction::ViewAccount => {
                if auth.has_account(account_id) {
                    Ok(())
                } else {
                    let visible_accounts = self.viewable_accounts(auth).await?;
                    match visible_accounts {
                        ViewableAccounts::All => Ok(()),
                        ViewableAccounts::Limited { .. } => {
                            Err(AuthServiceError::AccountAccessForbidden {
                                account_id: account_id.clone(),
                            })
                        }
                    }
                }
            }
            AccountAction::UpdateAccount => {
                limit_to_account_or_roles(auth, account_id, &[Role::Admin])
            }
            AccountAction::ViewPlan => limit_to_account_or_roles(auth, account_id, &[Role::Admin]),
            AccountAction::CreateProject => {
                limit_to_account_or_roles(auth, account_id, &[Role::Admin])
            }
            AccountAction::DeleteAccount => limit_to_roles(auth, &[Role::Admin]),
            AccountAction::ViewAccountGrants => {
                limit_to_account_or_roles(auth, account_id, &[Role::Admin])
            }
            AccountAction::CreateAccountGrant => limit_to_roles(auth, &[Role::Admin]),
            AccountAction::DeleteAccountGrant => limit_to_roles(auth, &[Role::Admin]),
            AccountAction::ViewDefaultProject => {
                limit_to_account_or_roles(auth, account_id, &[Role::Admin])
            }
            AccountAction::ListProjectGrants => limit_to_roles(auth, &[Role::Admin]),
            AccountAction::ViewLimits => {
                limit_to_account_or_roles(auth, account_id, &[Role::Admin])
            }
            AccountAction::UpdateLimits => limit_to_roles(auth, &[Role::Admin]),
        }
    }

    async fn authorize_project_action(
        &self,
        auth: &AccountAuthorisation,
        project_id: &ProjectId,
        requested_action: &ProjectAction,
    ) -> Result<AuthorizedProjectAction, AuthServiceError> {
        let actions = self.get_project_actions(auth, project_id).await?;
        let has_permission = match requested_action {
            ProjectAction::ViewProject => !actions.actions.actions.is_empty(),
            ProjectAction::BatchUpdatePluginInstallations => {
                actions
                    .actions
                    .actions
                    .contains(&ProjectPermisison::CreatePluginInstallation)
                    && actions
                        .actions
                        .actions
                        .contains(&ProjectPermisison::UpdatePluginInstallation)
                    && actions
                        .actions
                        .actions
                        .contains(&ProjectPermisison::DeletePluginInstallation)
            }
            other => {
                let converted = ProjectPermisison::try_from(other.clone()).map_err(|_| {
                    AuthServiceError::ProjectActionForbidden {
                        project_id: project_id.clone(),
                        requested_action: other.clone(),
                    }
                })?;
                actions.actions.actions.contains(&converted)
            }
        };
        if has_permission {
            Ok(AuthorizedProjectAction {
                own_account_id: auth.token.account_id.clone(),
                project_owner_account_id: actions.owner_account_id,
                project_id: actions.project_id,
                action: requested_action.clone(),
            })
        } else {
            Err(AuthServiceError::ProjectActionForbidden {
                project_id: project_id.clone(),
                requested_action: requested_action.clone(),
            })?
        }
    }

    async fn viewable_projects(
        &self,
        auth: &AccountAuthorisation,
    ) -> Result<ViewableProjects, AuthServiceError> {
        if auth.has_admin() {
            return Ok(ViewableProjects::All);
        };

        let grants = self
            .project_grant_repo
            .get_by_grantee_account(&auth.token.account_id.value)
            .await?;

        let additional_project_ids = grants
            .into_iter()
            .map(|pg| ProjectId(pg.grantor_project_id))
            .collect();

        Ok(ViewableProjects::OwnedAndAdditional {
            additional_project_ids,
        })
    }

    async fn viewable_accounts(
        &self,
        auth: &AccountAuthorisation,
    ) -> Result<ViewableAccounts, AuthServiceError> {
        if auth.has_admin() {
            return Ok(ViewableAccounts::All);
        };

        let grants = self
            .project_grant_repo
            .get_by_grantee_account(&auth.token.account_id.value)
            .await?;

        let project_ids = grants
            .into_iter()
            .map(|pg| pg.grantor_project_id)
            .collect::<Vec<_>>();

        let owner_accounts = self.project_repo.get_owners(&project_ids).await?;

        let mut account_ids = owner_accounts
            .into_iter()
            .map(|value| AccountId { value })
            .collect::<Vec<_>>();
        account_ids.push(auth.token.account_id.clone());

        Ok(ViewableAccounts::Limited { account_ids })
    }

    async fn get_project_actions(
        &self,
        auth: &AccountAuthorisation,
        project_id: &ProjectId,
    ) -> Result<ProjectAuthorisedActions, AuthServiceError> {
        tracing::info!("Get project authorisations for project: {}", project_id);
        let project = self.project_repo.get(&project_id.0).await?;

        let project = if let Some(project) = project {
            project
        } else {
            Err(AuthServiceError::ProjectAccessForbidden {
                project_id: project_id.clone(),
            })?
        };

        let owner_account_id = AccountId::from(project.owner_account_id.as_str());

        // fast path. we are admin and get full access
        // if auth.
        if auth.has_admin() || auth.has_account(&owner_account_id) {
            return Ok(ProjectAuthorisedActions {
                project_id: project_id.clone(),
                owner_account_id,
                actions: ProjectActions::all(),
            });
        };

        let policy_ids = {
            let result = self
                .project_grant_repo
                .get_by_project(&project_id.0)
                .await?;

            result
                .into_iter()
                .filter(|p| p.grantee_account_id == auth.token.account_id.value)
                .map(|p| p.project_policy_id)
                .collect::<Vec<_>>()
        };

        // if the user has no policies allowing access to that project, we don't want to leak information about the project existing
        if policy_ids.is_empty() {
            Err(AuthServiceError::ProjectAccessForbidden {
                project_id: project_id.clone(),
            })?
        }

        let actions = {
            let result = self.project_policy_repo.get_all(policy_ids).await?;

            let actions = result
                .into_iter()
                .flat_map(|pp| ProjectPolicy::from(pp).project_actions.actions)
                .collect::<HashSet<_>>();
            ProjectActions { actions }
        };

        Ok(ProjectAuthorisedActions {
            project_id: project_id.clone(),
            owner_account_id,
            actions,
        })
    }
}

fn limit_to_roles(
    auth: &AccountAuthorisation,
    allowed_roles: &[Role],
) -> Result<(), AuthServiceError> {
    for allowed_role in allowed_roles {
        if auth.has_role(allowed_role) {
            return Ok(());
        }
    }
    Err(AuthServiceError::RoleMissing {
        allowed_roles: allowed_roles.to_vec(),
    })
}

fn limit_to_account_or_roles(
    auth: &AccountAuthorisation,
    account_id: &AccountId,
    allowed_roles: &[Role],
) -> Result<(), AuthServiceError> {
    if auth.has_account(account_id) {
        Ok(())
    } else {
        limit_to_roles(auth, allowed_roles)
    }
}
