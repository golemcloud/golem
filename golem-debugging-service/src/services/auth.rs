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
use golem_common::model::auth::TokenSecret;
use golem_common::model::environment::EnvironmentId;
use golem_common::{error_forwarding, SafeDisplay};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::auth::EnvironmentAction;
use std::sync::Arc;

#[async_trait]
pub trait AuthService: Send + Sync {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError>;
    async fn check_user_allowed_to_debug_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth_ctx: &AuthCtx,
    ) -> Result<(), AuthServiceError>;
}

#[derive(Debug, thiserror::Error)]
pub enum AuthServiceError {
    #[error("Could not authenticate user using token")]
    CouldNotAuthenticate,
    #[error("User is not allowed to debug")]
    DebuggingNotAllowed,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AuthServiceError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::CouldNotAuthenticate => self.to_string(),
            Self::DebuggingNotAllowed => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AuthServiceError, RegistryServiceError);

pub struct GrpcAuthService {
    client: Arc<dyn RegistryService>,
}

impl GrpcAuthService {
    pub fn new(client: Arc<dyn RegistryService>) -> Self {
        Self { client }
    }
}

#[async_trait]
impl AuthService for GrpcAuthService {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError> {
        self.client
            .authenticate_token(&token)
            .await
            .map_err(|e| match e {
                RegistryServiceError::CouldNotAuthenticate(_) => {
                    AuthServiceError::CouldNotAuthenticate
                }
                other => other.into(),
            })
    }

    async fn check_user_allowed_to_debug_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth_ctx: &AuthCtx,
    ) -> Result<(), AuthServiceError> {
        let auth_details = self
            .client
            .get_auth_details_for_environment(environment_id, false, auth_ctx)
            .await
            .map_err(|e| match e {
                RegistryServiceError::NotFound(_) => AuthServiceError::DebuggingNotAllowed,
                other => other.into(),
            })?;

        auth_ctx
            .authorize_environment_action(
                auth_details.account_id_owning_environment,
                &auth_details.environment_roles_from_shares,
                EnvironmentAction::DebugWorker,
            )
            .map_err(|_| AuthServiceError::DebuggingNotAllowed)?;

        Ok(())
    }
}
