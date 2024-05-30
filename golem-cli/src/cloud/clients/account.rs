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

use crate::cloud::model::AccountId;
use async_trait::async_trait;
use golem_cloud_client::model::{Account, AccountData, Plan};
use tracing::info;

use crate::model::GolemError;

#[async_trait]
pub trait AccountClient {
    async fn get(&self, id: &AccountId) -> Result<Account, GolemError>;
    async fn get_plan(&self, id: &AccountId) -> Result<Plan, GolemError>;
    async fn put(&self, id: &AccountId, data: AccountData) -> Result<Account, GolemError>;
    async fn post(&self, data: AccountData) -> Result<Account, GolemError>;
    async fn delete(&self, id: &AccountId) -> Result<(), GolemError>;
}

pub struct AccountClientLive<C: golem_cloud_client::api::AccountClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_client::api::AccountClient + Sync + Send> AccountClient
    for AccountClientLive<C>
{
    async fn get(&self, id: &AccountId) -> Result<Account, GolemError> {
        info!("Getting account {id}");
        Ok(self.client.account_id_get(&id.id).await?)
    }

    async fn get_plan(&self, id: &AccountId) -> Result<Plan, GolemError> {
        info!("Getting account plan of {id}.");
        Ok(self.client.account_id_plan_get(&id.id).await?)
    }

    async fn put(&self, id: &AccountId, data: AccountData) -> Result<Account, GolemError> {
        info!("Updating account {id}.");
        Ok(self.client.account_id_put(&id.id, &data).await?)
    }

    async fn post(&self, data: AccountData) -> Result<Account, GolemError> {
        info!("Creating account.");
        Ok(self.client.post(&data).await?)
    }

    async fn delete(&self, id: &AccountId) -> Result<(), GolemError> {
        info!("Deleting account {id}.");
        let _ = self.client.account_id_delete(&id.id).await?;
        Ok(())
    }
}
