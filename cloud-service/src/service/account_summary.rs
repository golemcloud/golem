use std::sync::Arc;

use crate::auth::AccountAuthorisation;
use crate::model::AccountSummary;
use crate::repo::account_summary::AccountSummaryRepo;
use crate::repo::RepoError;
use async_trait::async_trait;
use cloud_common::model::Role;
use tracing::error;

#[derive(Debug, Clone)]
pub enum AccountSummaryServiceError {
    Unexpected(String),
    Unauthorized(String),
}

impl From<RepoError> for AccountSummaryServiceError {
    fn from(_error: RepoError) -> Self {
        AccountSummaryServiceError::Unexpected("DB call failed.".to_string())
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
            Err(AccountSummaryServiceError::Unauthorized(
                "Insufficient privilege.".to_string(),
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

#[derive(Default)]
pub struct AccountSummaryServiceNoOp {}

#[async_trait]
impl AccountSummaryService for AccountSummaryServiceNoOp {
    async fn get(
        &self,
        _skip: i32,
        _limit: i32,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<AccountSummary>, AccountSummaryServiceError> {
        Ok(vec![])
    }

    async fn count(&self, _auth: &AccountAuthorisation) -> Result<u64, AccountSummaryServiceError> {
        Ok(0)
    }
}
