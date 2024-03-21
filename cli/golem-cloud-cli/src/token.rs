// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
    /// List the existing tokens
    #[command()]
    List {},

    /// Add a new token
    #[command()]
    Add {
        /// Expiration date of the generated token
        #[arg(long, value_parser = parse_instant, default_value = "2100-01-01T00:00:00Z")]
        expires_at: DateTime<Utc>,
    },

    /// Delete an existing token
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
