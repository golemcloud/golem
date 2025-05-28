use crate::auth::AccountAuthorisation;
use crate::model::{Account, AccountAction, AccountData, GlobalAction, Plan};
use crate::repo::account::{AccountRecord, AccountRepo};
use crate::service::plan::{PlanError, PlanService};
use async_trait::async_trait;
use cloud_common::model::PlanId;
use golem_common::model::AccountId;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;
use std::sync::Arc;
use tracing::{error, info};

use super::auth::{AuthService, AuthServiceError, ViewableAccounts};

#[derive(Debug, thiserror::Error)]
pub enum AccountError {
    #[error("Account Not Found: {0}")]
    AccountNotFound(AccountId),
    #[error("Arg Validation error: {}", .0.join(", "))]
    ArgValidation(Vec<String>),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Internal error: {0}")]
    InternalRepoError(#[from] RepoError),
    #[error(transparent)]
    InternalPlanError(#[from] PlanError),
    #[error(transparent)]
    AuthError(#[from] AuthServiceError),
}

impl SafeDisplay for AccountError {
    fn to_safe_string(&self) -> String {
        match self {
            AccountError::AccountNotFound(_) => self.to_string(),
            AccountError::ArgValidation(_) => self.to_string(),
            AccountError::Internal(_) => self.to_string(),
            AccountError::InternalRepoError(inner) => inner.to_safe_string(),
            AccountError::InternalPlanError(inner) => inner.to_safe_string(),
            AccountError::AuthError(inner) => inner.to_safe_string(),
        }
    }
}

impl From<String> for AccountError {
    fn from(error: String) -> Self {
        AccountError::Internal(error)
    }
}

#[async_trait]
pub trait AccountService: Send + Sync {
    async fn create(
        &self,
        id: &AccountId,
        account: &AccountData,
        auth: &AccountAuthorisation,
    ) -> Result<Account, AccountError>;

    async fn update(
        &self,
        account_id: &AccountId,
        account: &AccountData,
        auth: &AccountAuthorisation,
    ) -> Result<Account, AccountError>;

    async fn get(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Account, AccountError>;

    /// Get all matching accounts. This will return your account + all accounts that you got access through at least one grant.
    async fn find(
        &self,
        email: Option<&str>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Account>, AccountError>;

    async fn get_plan(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Plan, AccountError>;

    async fn delete(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountError>;
}

pub struct AccountServiceDefault {
    auth_service: Arc<dyn AuthService>,
    account_repo: Arc<dyn AccountRepo + Sync + Send>,
    plan_service: Arc<dyn PlanService + Sync + Send>,
}

impl AccountServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService>,
        account_repo: Arc<dyn AccountRepo + Sync + Send>,
        plan_service: Arc<dyn PlanService + Sync + Send>,
    ) -> Self {
        AccountServiceDefault {
            auth_service,
            account_repo,
            plan_service,
        }
    }

    async fn get_default_plan_id(&self) -> Result<PlanId, AccountError> {
        let plan_id = self
            .plan_service
            .get_default_plan()
            .await
            .map(|plan| plan.plan_id)?;

        Ok(plan_id)
    }
}

#[async_trait]
impl AccountService for AccountServiceDefault {
    async fn create(
        &self,
        id: &AccountId,
        account: &AccountData,
        auth: &AccountAuthorisation,
    ) -> Result<Account, AccountError> {
        self.auth_service
            .authorize_global_action(auth, &GlobalAction::CreateAccount)
            .await?;

        let plan_id = self.get_default_plan_id().await?;
        info!("Creating account: {}", id);
        match self
            .account_repo
            .create(&AccountRecord {
                id: id.clone().value,
                name: account.name.clone(),
                email: account.email.clone(),
                plan_id: plan_id.0,
            })
            .await
        {
            Ok(Some(account_record)) => Ok(account_record.into()),
            Ok(None) => Err(format!("Duplicated account on fresh id: {}", id).into()),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }

    async fn update(
        &self,
        account_id: &AccountId,
        account: &AccountData,
        auth: &AccountAuthorisation,
    ) -> Result<Account, AccountError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::UpdateAccount)
            .await?;

        info!("Updating account: {}", account_id);
        let current_account = self.account_repo.get(&account_id.value).await?;
        let plan_id = match current_account {
            Some(current_account) => current_account.plan_id,
            None => self.get_default_plan_id().await?.0,
        };
        let result = self
            .account_repo
            .update(&AccountRecord {
                id: account_id.value.clone(),
                name: account.name.clone(),
                email: account.email.clone(),
                plan_id,
            })
            .await;
        match result {
            Ok(account_record) => Ok(account_record.into()),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }

    async fn get(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Account, AccountError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::ViewAccount)
            .await?;

        info!("Get account: {}", account_id);

        let result = self.account_repo.get(&account_id.value).await;
        match result {
            Ok(Some(account_record)) => Ok(account_record.into()),
            Ok(None) => Err(AccountError::AccountNotFound(account_id.clone())),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }

    /// Get all users
    async fn find(
        &self,
        email: Option<&str>,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Account>, AccountError> {
        let visible_accounts = self.auth_service.viewable_accounts(auth).await?;

        let results = match visible_accounts {
            ViewableAccounts::All => self.account_repo.find_all(email).await?,
            ViewableAccounts::Limited { account_ids } => {
                let ids = account_ids
                    .into_iter()
                    .map(|ai| ai.value)
                    .collect::<Vec<_>>();
                self.account_repo.find(&ids, email).await?
            }
        };

        Ok(results.into_iter().map(|v| v.into()).collect())
    }

    async fn get_plan(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Plan, AccountError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::ViewPlan)
            .await?;

        info!("Get plan: {}", account_id);

        let result = self.account_repo.get(&account_id.value).await;
        match result {
            Ok(Some(account_record)) => {
                match self.plan_service.get(&PlanId(account_record.plan_id)).await {
                    Ok(Some(plan)) => Ok(plan),
                    Ok(None) => Err(format!(
                        "Could not find plan with id: {}",
                        account_record.plan_id
                    )
                    .into()),
                    Err(err) => {
                        error!("DB call failed. {:?}", err);
                        Err(err.into())
                    }
                }
            }
            Ok(None) => Err(AccountError::AccountNotFound(account_id.clone())),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }

    async fn delete(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountError> {
        self.auth_service
            .authorize_account_action(auth, account_id, &AccountAction::DeleteAccount)
            .await?;

        if auth.token.account_id == *account_id {
            return Err(AccountError::ArgValidation(vec![
                "Cannot delete current account.".to_string(),
            ]));
        }
        let result = self.account_repo.delete(&account_id.value).await;
        match result {
            Ok(_) => Ok(()),
            Err(err) => {
                error!("DB call failed. {}", err);
                Err(err.into())
            }
        }
    }
}
