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

use crate::model::GolemError;

#[async_trait]
pub trait HealthcheckClient {
    async fn healthcheck(&self) -> Result<(), GolemError>;
}

pub struct HealthcheckClientLive<C: golem_gateway_client::api::HealthCheckClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_gateway_client::api::HealthCheckClient + Sync + Send> HealthcheckClient
    for HealthcheckClientLive<C>
{
    async fn healthcheck(&self) -> Result<(), GolemError> {
        self.client.healthcheck().await?;
        Ok(())
    }
}
