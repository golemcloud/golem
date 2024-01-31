use std::sync::Arc;

use async_trait::async_trait;
use golem_common::model::AccountId;
use tracing::error;

use crate::auth::AccountAuthorisation;
use crate::model::Role;
use crate::repo::account_grant::AccountGrantRepo;
use crate::repo::RepoError;

#[derive(Debug, Clone)]
pub enum AccountGrantServiceError {
    ArgValidation(Vec<String>),
    Unexpected(String),
    Unauthorized(String),
}

impl From<RepoError> for AccountGrantServiceError {
    fn from(error: RepoError) -> Self {
        match error {
            RepoError::Internal(_) => {
                AccountGrantServiceError::Unexpected("DB call failed.".to_string())
            }
        }
    }
}

#[async_trait]
pub trait AccountGrantService {
    async fn get(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Role>, AccountGrantServiceError>;
    async fn add(
        &self,
        account_id: &AccountId,
        role: &Role,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError>;
    async fn remove(
        &self,
        account_id: &AccountId,
        role: &Role,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError>;
}

pub struct AccountGrantServiceDefault {
    account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync>,
}

impl AccountGrantServiceDefault {
    pub fn new(account_grant_repo: Arc<dyn AccountGrantRepo + Send + Sync>) -> Self {
        Self { account_grant_repo }
    }

    fn check_authorization(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError> {
        if auth.has_account_or_role(account_id, &Role::Admin) {
            Ok(())
        } else {
            Err(AccountGrantServiceError::Unauthorized(
                "Access to another account.".to_string(),
            ))
        }
    }

    fn check_admin(&self, auth: &AccountAuthorisation) -> Result<(), AccountGrantServiceError> {
        if auth.has_role(&Role::Admin) {
            Ok(())
        } else {
            Err(AccountGrantServiceError::Unauthorized(
                "Admin role required.".to_string(),
            ))
        }
    }
}

#[async_trait]
impl AccountGrantService for AccountGrantServiceDefault {
    async fn get(
        &self,
        account_id: &AccountId,
        auth: &AccountAuthorisation,
    ) -> Result<Vec<Role>, AccountGrantServiceError> {
        self.check_authorization(account_id, auth)?;
        match self.account_grant_repo.get(account_id).await {
            Ok(roles) => Ok(roles),
            Err(error) => {
                error!("DB call failed. {:?}", error);
                Err(error.into())
            }
        }
    }

    async fn add(
        &self,
        account_id: &AccountId,
        role: &Role,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError> {
        self.check_admin(auth)?;
        match self.account_grant_repo.add(account_id, role).await {
            Ok(_) => Ok(()),
            Err(error) => {
                error!("DB call failed. {:?}", error);
                Err(error.into())
            }
        }
    }

    async fn remove(
        &self,
        account_id: &AccountId,
        role: &Role,
        auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError> {
        self.check_admin(auth)?;
        if auth.token.account_id == *account_id && role == &Role::Admin {
            return Err(AccountGrantServiceError::ArgValidation(vec![
                "Cannot remove Admin role from current account.".to_string(),
            ]));
        };
        match self.account_grant_repo.remove(account_id, role).await {
            Ok(_) => Ok(()),
            Err(error) => {
                error!("DB call failed. {:?}", error);
                Err(error.into())
            }
        }
    }
}

#[derive(Default)]
pub struct AccountGrantServiceNoOp {}

#[async_trait]
impl AccountGrantService for AccountGrantServiceNoOp {
    async fn get(
        &self,
        _account_id: &AccountId,
        _auth: &AccountAuthorisation,
    ) -> Result<Vec<Role>, AccountGrantServiceError> {
        Ok(vec![])
    }

    async fn add(
        &self,
        _account_id: &AccountId,
        _role: &Role,
        _auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError> {
        Ok(())
    }

    async fn remove(
        &self,
        _account_id: &AccountId,
        _role: &Role,
        _auth: &AccountAuthorisation,
    ) -> Result<(), AccountGrantServiceError> {
        Ok(())
    }
}
