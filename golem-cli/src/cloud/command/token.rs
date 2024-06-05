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

use chrono::{DateTime, Utc};
use clap::Subcommand;

use crate::cloud::model::{AccountId, TokenId};
use crate::cloud::service::token::TokenService;
use crate::model::{GolemError, GolemResult};

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

impl TokenSubcommand {
    pub async fn handle(
        self,
        account_id: Option<AccountId>,
        service: &(dyn TokenService + Send + Sync),
    ) -> Result<GolemResult, GolemError> {
        match self {
            TokenSubcommand::List {} => service.list(account_id).await,
            TokenSubcommand::Add { expires_at } => service.add(expires_at, account_id).await,
            TokenSubcommand::Delete { token_id } => service.delete(token_id, account_id).await,
        }
    }
}
