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

use crate::clients::gateway::healthcheck::HealthcheckClient;
use crate::model::{GolemError, GolemResult};

#[async_trait]
pub trait HealthcheckHandler {
    async fn handle(&self) -> Result<GolemResult, GolemError>;
}

pub struct HealthcheckHandlerLive<H: HealthcheckClient + Sync + Send> {
    pub healthcheck: H,
}

#[async_trait]
impl<H: HealthcheckClient + Sync + Send> HealthcheckHandler for HealthcheckHandlerLive<H> {
    async fn handle(&self) -> Result<GolemResult, GolemError> {
        self.healthcheck.healthcheck().await?;

        Ok(GolemResult::Str("Online".to_string()))
    }
}
