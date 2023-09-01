use async_trait::async_trait;
use crate::clients::CloudAuthentication;
use crate::clients::token::TokenClient;
use crate::model::{AccountId, GolemError, GolemResult};
use crate::TokenSubcommand;


#[async_trait]
pub trait TokenHandler {
    async fn handle(&self, auth: &CloudAuthentication, account_id: Option<AccountId>, subcommand: TokenSubcommand) -> Result<GolemResult, GolemError>;
}

pub struct TokenHandlerLive<C: TokenClient + Send + Sync> {
    pub client: C
}

#[async_trait]
impl <C: TokenClient + Send + Sync> TokenHandler for TokenHandlerLive<C> {
    async fn handle(&self, auth: &CloudAuthentication, account_id: Option<AccountId>, subcommand: TokenSubcommand) -> Result<GolemResult, GolemError> {
        let account_id = account_id.unwrap_or(auth.account_id());
        match subcommand {
            TokenSubcommand::List { } => {
                let token = self.client.get_all(&account_id, auth).await?;
                Ok(GolemResult::Ok(Box::new(token)))
            }
            TokenSubcommand::Add { expires_at } => {
                let token = self.client.post(&account_id, expires_at, auth).await?;
                Ok(GolemResult::Ok(Box::new(token)))
            }
            TokenSubcommand::Delete { token_id } => {
                self.client.delete(&account_id, token_id, auth).await?;
                Ok(GolemResult::Str("Deleted".to_string()))
            }
        }
    }
}