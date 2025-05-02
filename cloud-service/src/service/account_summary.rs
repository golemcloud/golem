use std::sync::Arc;

use crate::model::AccountSummary;
use crate::repo::account_summary::AccountSummaryRepo;
use crate::{auth::AccountAuthorisation, model::GlobalAction};
use async_trait::async_trait;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use tracing::error;

use super::auth::{AuthService, AuthServiceError};

#[derive(Debug, thiserror::Error)]
pub enum AccountSummaryServiceError {
    #[error("Internal error: {0}")]
    Internal(#[from] RepoError),
    #[error(transparent)]
    AuthError(#[from] AuthServiceError),
}

impl SafeDisplay for AccountSummaryServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            AccountSummaryServiceError::Internal(inner) => inner.to_safe_string(),
            AccountSummaryServiceError::AuthError(inner) => inner.to_safe_string(),
        }
    }
}

#[async_trait]
pub trait AccountSummaryService {
    async fn get(
        &self,
        skip: i32,
        limit: i32,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<AccountSummary>, AccountSummaryServiceError>;
    async fn count(&self, auth: &AccountAuthorisation) -> Result<u64, AccountSummaryServiceError>;
}

pub struct AccountSummaryServiceDefault {
    auth_service: Arc<dyn AuthService>,
    account_summary_repo: Arc<dyn AccountSummaryRepo + Send + Sync>,
}

impl AccountSummaryServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService>,
        account_summary_repo: Arc<dyn AccountSummaryRepo + Send + Sync>,
    ) -> Self {
        Self {
            auth_service,
            account_summary_repo,
        }
    }
}

#[async_trait]
impl AccountSummaryService for AccountSummaryServiceDefault {
    async fn get(
        &self,
        skip: i32,
        limit: i32,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<AccountSummary>, AccountSummaryServiceError> {
        self.auth_service
            .authorize_global_action(auth, &GlobalAction::ViewAccountSummaries)
            .await?;
        match self.account_summary_repo.get(skip, limit).await {
            Ok(account_summary) => Ok(account_summary),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn count(&self, auth: &AccountAuthorisation) -> Result<u64, AccountSummaryServiceError> {
        self.auth_service
            .authorize_global_action(auth, &GlobalAction::ViewAccountCount)
            .await?;
        match self.account_summary_repo.count().await {
            Ok(count) => Ok(count),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }
}
