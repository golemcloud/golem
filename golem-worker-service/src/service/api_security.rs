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

use crate::service::auth::AuthService;
use async_trait::async_trait;
use golem_common::model::auth::ProjectAction;
use golem_common::model::auth::{AuthCtx, Namespace};
use golem_common::model::ProjectId;
use golem_service_base::clients::auth::AuthServiceError;
use golem_worker_service_base::gateway_security::{
    SecurityScheme, SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata,
};
use golem_worker_service_base::service::gateway::security_scheme::{
    SecuritySchemeService as BaseSecuritySchemeService,
    SecuritySchemeServiceError as BaseSecuritySchemeServiceError,
};
use std::sync::Arc;

type BaseService = Arc<dyn BaseSecuritySchemeService<Namespace> + Sync + Send>;

#[async_trait]
pub trait SecuritySchemeService {
    async fn get(
        &self,
        security_scheme_name: &SecuritySchemeIdentifier,
        project_id: &ProjectId,
        ctx: &AuthCtx,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError>;

    async fn create(
        &self,
        security_scheme: &SecurityScheme,
        project_id: &ProjectId,
        ctx: &AuthCtx,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError>;
}

#[derive(Clone)]
pub struct SecuritySchemeServiceDefault {
    auth_service: Arc<dyn AuthService + Sync + Send>,
    base_service: BaseService,
}

impl SecuritySchemeServiceDefault {
    pub fn new(
        auth_service: Arc<dyn AuthService + Sync + Send>,
        base_service: BaseService,
    ) -> Self {
        Self {
            auth_service,
            base_service,
        }
    }
}

#[derive(Debug)]
pub enum SecuritySchemeServiceError {
    Auth(AuthServiceError),
    Base(BaseSecuritySchemeServiceError),
}

#[async_trait]
impl SecuritySchemeService for SecuritySchemeServiceDefault {
    async fn get(
        &self,
        security_scheme_name: &SecuritySchemeIdentifier,
        project_id: &ProjectId,
        ctx: &AuthCtx,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::ViewApiDefinition, ctx)
            .await
            .map_err(SecuritySchemeServiceError::Auth)?;

        self.base_service
            .get(security_scheme_name, &namespace)
            .await
            .map_err(SecuritySchemeServiceError::Base)
    }

    async fn create(
        &self,
        security_scheme: &SecurityScheme,
        project_id: &ProjectId,
        ctx: &AuthCtx,
    ) -> Result<SecuritySchemeWithProviderMetadata, SecuritySchemeServiceError> {
        let namespace = self
            .auth_service
            .authorize_project_action(project_id, ProjectAction::CreateApiDefinition, ctx)
            .await
            .map_err(SecuritySchemeServiceError::Auth)?;

        self.base_service
            .create(&namespace, security_scheme)
            .await
            .map_err(SecuritySchemeServiceError::Base)
    }
}
