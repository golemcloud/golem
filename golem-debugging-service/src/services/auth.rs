// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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
use golem_common::model::AgentId;
use golem_common::model::auth::TokenSecret;
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use golem_common::model::card::{
    AgentResourcePattern, AgentVerb, ClassPermissionTarget, PermissionTarget,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use std::sync::Arc;

#[async_trait]
pub trait AuthService: Send + Sync {
    async fn authenticate_token(&self, token: TokenSecret) -> Result<AuthCtx, AuthServiceError>;
    async fn check_user_allowed_to_debug_agent(
        &self,
        component: &Component,
        agent_id: &AgentId,
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

    async fn check_user_allowed_to_debug_agent(
        &self,
        component: &Component,
        agent_id: &AgentId,
        auth_ctx: &AuthCtx,
    ) -> Result<(), AuthServiceError> {
        auth_ctx
            .authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
                owner: AgentOwnerPattern::Agent {
                    account: component.account_id.to_string(),
                    application: component.application_name.0.clone(),
                    environment: component.environment_name.0.clone(),
                    component: component.component_name.0.clone(),
                    agent: AgentOwnerLeafPattern::Agent(agent_id.agent_id.clone()),
                },
                verb: Some(AgentVerb::Debug),
                resource: AgentResourcePattern::Any,
            }))
            .map_err(|_| AuthServiceError::DebuggingNotAllowed)?;

        Ok(())
    }
}
