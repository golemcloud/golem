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

use crate::cloud::clients::account::AccountClient;
use crate::cloud::model::text::{AccountViewAdd, AccountViewGet, AccountViewUpdate};
use crate::cloud::model::AccountId;
use crate::model::{GolemError, GolemResult};
use async_trait::async_trait;
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
        Ok(GolemResult::Ok(Box::new(AccountViewGet(account))))
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
        Ok(GolemResult::Ok(Box::new(AccountViewUpdate(account))))
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

        Ok(GolemResult::Ok(Box::new(AccountViewAdd(account))))
    }

    async fn delete(&self, account_id: Option<AccountId>) -> Result<GolemResult, GolemError> {
        let account_id = account_id.as_ref().unwrap_or(&self.account_id);
        self.client.delete(account_id).await?;
        Ok(GolemResult::Str("Deleted".to_string()))
    }
}
