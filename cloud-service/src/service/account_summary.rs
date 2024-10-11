use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::model::AccountSummary;
use crate::repo::account_summary::AccountSummaryRepo;
use async_trait::async_trait;
use cloud_common::model::Role;
use golem_common::SafeDisplay;
use golem_service_base::repo::RepoError;
use tracing::error;

#[derive(Debug, thiserror::Error)]
pub enum AccountSummaryServiceError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Internal error: {0}")]
    Internal(RepoError),
}

impl AccountSummaryServiceError {
    fn unauthorized(error: impl AsRef<str>) -> Self {
        Self::Unauthorized(error.as_ref().to_string())
    }
}

impl SafeDisplay for AccountSummaryServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            AccountSummaryServiceError::Unauthorized(_) => self.to_string(),
            AccountSummaryServiceError::Internal(inner) => inner.to_safe_string(),
        }
    }
}

impl From<RepoError> for AccountSummaryServiceError {
    fn from(error: RepoError) -> Self {
        AccountSummaryServiceError::Internal(error)
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
    account_summary_repo: Arc<dyn AccountSummaryRepo + Send + Sync>,
}

impl AccountSummaryServiceDefault {
    pub fn new(account_summary_repo: Arc<dyn AccountSummaryRepo + Send + Sync>) -> Self {
        Self {
            account_summary_repo,
        }
    }

    fn check_authorization(
        &self,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountSummaryServiceError> {
        if auth.has_role(&Role::Admin) || auth.has_role(&Role::MarketingAdmin) {
            Ok(())
        } else {
            Err(AccountSummaryServiceError::unauthorized(
                "Insufficient privilege.",
            ))
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
        self.check_authorization(auth)?;
        match self.account_summary_repo.get(skip, limit).await {
            Ok(account_summary) => Ok(account_summary),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }

    async fn count(&self, auth: &AccountAuthorisation) -> Result<u64, AccountSummaryServiceError> {
        self.check_authorization(auth)?;
        match self.account_summary_repo.count().await {
            Ok(count) => Ok(count),
            Err(error) => {
                error!("DB call failed. {}", error);
                Err(error.into())
            }
        }
    }
}
