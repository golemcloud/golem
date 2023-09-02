use async_trait::async_trait;
use golem_client::model::AccountData;
use crate::model::{AccountId, GolemError, GolemResult};
use crate::AccountSubcommand;
use crate::clients::account::AccountClient;
use crate::clients::CloudAuthentication;

#[async_trait]
pub trait AccountHandler {
    async fn handle(&self, token: &CloudAuthentication, account_id: Option<AccountId>, subcommand: AccountSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct AccountHandlerLive<C: AccountClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: AccountClient + Sync + Send> AccountHandler for AccountHandlerLive<C> {
    async fn handle(&self, auth: &CloudAuthentication, account_id: Option<AccountId>, subcommand: AccountSubcommand) -> Result<GolemResult, GolemError> {
        match subcommand {
            AccountSubcommand::Get { } => {
                let account = self.client.get(&account_id.unwrap_or(auth.account_id()), auth).await?;
                Ok(GolemResult::Ok(Box::new(account)))
            }
            AccountSubcommand::Update { account_name, account_email } => {
                let id = account_id.unwrap_or(auth.account_id());
                let existing = self.client.get(&id, auth).await?;
                let name = account_name.unwrap_or(existing.name);
                let email = account_email.unwrap_or(existing.email);
                let updated = AccountData {
                    name,
                    email
                };
                let account = self.client.put(&id, updated, auth).await?;
                Ok(GolemResult::Ok(Box::new(account)))
            }
            AccountSubcommand::New { account_name, account_email } => {
                let data = AccountData {
                    name: account_name,
                    email: account_email
                };

                let account = self.client.post(data, auth).await?;

                Ok(GolemResult::Ok(Box::new(account)))
            }
            AccountSubcommand::Delete { } => {
                let id = account_id.unwrap_or(auth.account_id());
                self.client.delete(&id, auth).await?;
                Ok(GolemResult::Str("Deleted".to_string()))
            }
            AccountSubcommand::Grant { .. } => { todo!() }
        }
    }
}