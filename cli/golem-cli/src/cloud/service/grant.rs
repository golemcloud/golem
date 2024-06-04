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

use crate::cloud::clients::grant::GrantClient;
use crate::cloud::model::{AccountId, Role};
use crate::model::{GolemError, GolemResult};
use async_trait::async_trait;

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

        Ok(GolemResult::Ok(Box::new(roles)))
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
