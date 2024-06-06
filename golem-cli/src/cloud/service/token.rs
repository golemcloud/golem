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

use crate::cloud::clients::token::TokenClient;
use crate::cloud::model::{AccountId, TokenId};
use crate::model::{GolemError, GolemResult};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[async_trait]
pub trait TokenService {
    async fn list(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError>;
    async fn add(
        &self,
        expires_at: DateTime<Utc>,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError>;
    async fn delete(
        &self,
        token_id: TokenId,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError>;
}

pub struct TokenServiceLive {
    pub account_id: AccountId,
    pub client: Box<dyn TokenClient + Send + Sync>,
}

#[async_trait]
impl TokenService for TokenServiceLive {
    async fn list(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        let token = self.client.get_all(account_id).await?;
        Ok(GolemResult::Ok(Box::new(token)))
    }

    async fn add(
        &self,
        expires_at: DateTime<Utc>,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        let token = self.client.post(account_id, expires_at).await?;
        Ok(GolemResult::Ok(Box::new(token)))
    }

    async fn delete(
        &self,
        token_id: TokenId,
        account_id: Option<AccountId>,
    ) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        self.client.delete(account_id, token_id).await?;
        Ok(GolemResult::Str("Deleted".to_string()))
    }
}
