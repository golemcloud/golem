// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use golem_common::model::WorkerId;
use golem_common::model::account::AccountId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum LimitServiceError {
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
    #[error("Limit exceeded: {0}")]
    LimitExceeded(String),
}

impl SafeDisplay for LimitServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InternalError(_) => "Internal error".to_string(),
            Self::LimitExceeded(_) => self.to_string(),
        }
    }
}

error_forwarding!(LimitServiceError, RegistryServiceError);

#[async_trait]
pub trait LimitService: Send + Sync {
    async fn update_worker_limit(
        &self,
        account_id: AccountId,
        worker_id: &WorkerId,
        added: bool,
    ) -> Result<(), LimitServiceError>;

    async fn update_worker_connection_limit(
        &self,
        account_id: AccountId,
        worker_id: &WorkerId,
        added: bool,
    ) -> Result<(), LimitServiceError>;
}

pub struct RemoteLimitService {
    client: Arc<dyn RegistryService>,
}

impl RemoteLimitService {
    pub fn new(client: Arc<dyn RegistryService>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl LimitService for RemoteLimitService {
    async fn update_worker_limit(
        &self,
        account_id: AccountId,
        worker_id: &WorkerId,
        added: bool,
    ) -> Result<(), LimitServiceError> {
        self.client
            .update_worker_limit(account_id, worker_id, added)
            .await
            .map_err(|e| match e {
                RegistryServiceError::LimitExceeded(msg) => LimitServiceError::LimitExceeded(msg),
                other => other.into(),
            })?;
        Ok(())
    }

    async fn update_worker_connection_limit(
        &self,
        account_id: AccountId,
        worker_id: &WorkerId,
        added: bool,
    ) -> Result<(), LimitServiceError> {
        self.client
            .update_worker_connection_limit(account_id, worker_id, added)
            .await
            .map_err(|e| match e {
                RegistryServiceError::LimitExceeded(msg) => LimitServiceError::LimitExceeded(msg),
                other => other.into(),
            })?;
        Ok(())
    }
}
