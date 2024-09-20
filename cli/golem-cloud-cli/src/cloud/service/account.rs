use crate::cloud::clients::account::AccountClient;
use crate::cloud::model::text::account::{AccountAddView, AccountGetView, AccountUpdateView};
use async_trait::async_trait;
use golem_cli::cloud::AccountId;
use golem_cli::model::{GolemError, GolemResult};
use golem_cloud_client::model::AccountData;

#[async_trait]
pub trait AccountService {
    async fn get(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError>;
    async fn update(
        &self,
        account_name: Option<String>,
        account_email: Option<String>,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError>;
    async fn add(
        &self,
        account_name: String,
        account_email: String,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError>;
}

pub struct AccountServiceLive {
    pub account_id: AccountId,
    pub client: Box<dyn AccountClient + Sync + Send>,
}

#[async_trait]
impl AccountService for AccountServiceLive {
    async fn get(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        let account = self.client.get(account_id).await?;
        Ok(GolemResult::Ok(Box::new(AccountGetView(account))))
    }

    async fn update(
        &self,
        account_name: Option<String>,
        account_email: Option<String>,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        let existing = self.client.get(account_id).await?;
        let name = account_name.unwrap_or(existing.name);
        let email = account_email.unwrap_or(existing.email);
        let updated = AccountData { name, email };
        let account = self.client.put(account_id, updated).await?;
        Ok(GolemResult::Ok(Box::new(AccountUpdateView(account))))
    }

    async fn add(
        &self,
        account_name: String,
        account_email: String,
    ) -> Result<GolemResult, GolemError> {
        let data = AccountData {
            name: account_name,
            email: account_email,
        };

        let account = self.client.post(data).await?;

        Ok(GolemResult::Ok(Box::new(AccountAddView(account))))
    }

    async fn delete(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        self.client.delete(account_id).await?;
        Ok(GolemResult::Str("Deleted".to_string()))
    }
}
