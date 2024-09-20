use crate::cloud::clients::grant::GrantClient;
use crate::cloud::model::text::account::GrantGetView;
use crate::cloud::model::Role;
use async_trait::async_trait;
use golem_cli::cloud::AccountId;
use golem_cli::model::{GolemError, GolemResult};

#[async_trait]
pub trait GrantService {
    async fn get(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError>;
    async fn add(
        &self,
        role: Role,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(
        &self,
        role: Role,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError>;
}

pub struct GrantServiceLive {
    pub account_id: AccountId,
    pub client: Box<dyn GrantClient + Send + Sync>,
}

#[async_trait]
impl GrantService for GrantServiceLive {
    async fn get(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        let roles = self.client.get_all(account_id).await?;

        Ok(GolemResult::Ok(Box::new(GrantGetView(roles))))
    }

    async fn add(
        &self,
        role: Role,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        self.client.put(account_id, role).await?;

        Ok(GolemResult::Str("Role granted".to_string()))
    }

    async fn delete(
        &self,
        role: Role,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        self.client.delete(account_id, role).await?;

        Ok(GolemResult::Str("Role removed".to_string()))
    }
}
