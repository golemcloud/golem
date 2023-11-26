use async_trait::async_trait;
use chrono::{DateTime, Utc};
use clap::Subcommand;

use crate::clients::token::TokenClient;
use crate::clients::CloudAuthentication;
use crate::model::{AccountId, GolemError, GolemResult, TokenId};

fn parse_instant(
    s: &str,
) -> Result<DateTime<Utc>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    match s.parse::<DateTime<Utc>>() {
        Ok(dt) => Ok(dt),
        Err(err) => Err(err.into()),
    }
}

#[derive(Subcommand, Debug)]
#[command()]
pub enum TokenSubcommand {
    #[command()]
    List {},

    #[command()]
    Add {
        #[arg(long, value_parser = parse_instant, default_value = "2100-01-01T00:00:00Z")]
        expires_at: DateTime<Utc>,
    },

    #[command()]
    Delete {
        #[arg(value_name = "TOKEN")]
        token_id: TokenId,
    },
}

#[async_trait]
pub trait TokenHandler {
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        account_id: Option<AccountId>,
        subcommand: TokenSubcommand,
    ) -> Result<GolemResult, GolemError>;
}

pub struct TokenHandlerLive<C: TokenClient + Send + Sync> {
    pub client: C,
}

#[async_trait]
impl<C: TokenClient + Send + Sync> TokenHandler for TokenHandlerLive<C> {
    async fn handle(
        &self,
        auth: &CloudAuthentication,
        account_id: Option<AccountId>,
        subcommand: TokenSubcommand,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.unwrap_or(auth.account_id());
        match subcommand {
            TokenSubcommand::List {} => {
                let token = self.client.get_all(&account_id).await?;
                Ok(GolemResult::Ok(Box::new(token)))
            }
            TokenSubcommand::Add { expires_at } => {
                let token = self.client.post(&account_id, expires_at).await?;
                Ok(GolemResult::Ok(Box::new(token)))
            }
            TokenSubcommand::Delete { token_id } => {
                self.client.delete(&account_id, token_id).await?;
                Ok(GolemResult::Str("Deleted".to_string()))
            }
        }
    }
}
