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

use super::account::{AccountError, AccountService};
use super::application::{ApplicationError, ApplicationService};
use super::component::{ComponentError, ComponentService};
use super::environment::{EnvironmentError, EnvironmentService};
use crate::model::component_slug::ComponentSlug;
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationId;
use golem_common::model::environment::EnvironmentId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::Component;
use golem_service_base::model::auth::AuthCtx;
use std::fmt::Debug;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ComponentResolverError {
    #[error("Invalid component reference: {error}")]
    InvalidComponentReference { error: String },
    #[error("Component for reference not found")]
    ComponentNotFound,
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for ComponentResolverError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InvalidComponentReference { .. } => self.to_string(),
            Self::ComponentNotFound => self.to_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    ComponentResolverError,
    AccountError,
    ApplicationError,
    EnvironmentError,
    ComponentError
);

pub struct ComponentResolverService {
    account_service: Arc<AccountService>,
    application_service: Arc<ApplicationService>,
    environment_service: Arc<EnvironmentService>,
    component_service: Arc<ComponentService>,
}

impl ComponentResolverService {
    pub fn new(
        account_service: Arc<AccountService>,
        application_service: Arc<ApplicationService>,
        environment_service: Arc<EnvironmentService>,
        component_service: Arc<ComponentService>,
    ) -> Self {
        Self {
            account_service,
            application_service,
            environment_service,
            component_service,
        }
    }

    pub async fn resolve_deployed_component(
        &self,
        component_reference: String,
        resolving_environment: EnvironmentId,
        resolving_application: ApplicationId,
        resolving_account: AccountId,
        auth: &AuthCtx,
    ) -> Result<Component, ComponentResolverError> {
        let component_slug = ComponentSlug::parse(&component_reference)
            .map_err(|e| ComponentResolverError::InvalidComponentReference { error: e })?;

        // Note checking for parent resources using system authctx does not leak existence here.
        // We don't return them directly to the user and the final call to get the component using the user authctx checks for permissions.

        // TODO: push these down to the database
        match component_slug.into_parts() {
            (
                Some(account_email),
                Some(application_name),
                Some(environment_name),
                component_name,
            ) => {
                let account = self
                    .account_service
                    .get_by_email(&account_email, &AuthCtx::System)
                    .await?;
                let application = self
                    .application_service
                    .get_in_account(account.id, &application_name, &AuthCtx::System)
                    .await?;
                let environment = self
                    .environment_service
                    .get_in_application(application.id, &environment_name, &AuthCtx::System)
                    .await?;

                self.component_service
                    .get_deployed_component_by_name(environment.id, &component_name, auth)
                    .await
                    .map_err(|err| match err {
                        ComponentError::ComponentByNameNotFound(_) => {
                            ComponentResolverError::ComponentNotFound
                        }
                        other => other.into(),
                    })
            }
            (None, Some(application_name), Some(environment_name), component_name) => {
                let application = self
                    .application_service
                    .get_in_account(resolving_account, &application_name, &AuthCtx::System)
                    .await?;
                let environment = self
                    .environment_service
                    .get_in_application(application.id, &environment_name, &AuthCtx::System)
                    .await?;

                self.component_service
                    .get_deployed_component_by_name(environment.id, &component_name, auth)
                    .await
                    .map_err(|err| match err {
                        ComponentError::ComponentByNameNotFound(_) => {
                            ComponentResolverError::ComponentNotFound
                        }
                        other => other.into(),
                    })
            }
            (None, None, Some(environment_name), component_name) => {
                let environment = self
                    .environment_service
                    .get_in_application(resolving_application, &environment_name, &AuthCtx::System)
                    .await?;

                self.component_service
                    .get_deployed_component_by_name(environment.id, &component_name, auth)
                    .await
                    .map_err(|err| match err {
                        ComponentError::ComponentByNameNotFound(_) => {
                            ComponentResolverError::ComponentNotFound
                        }
                        other => other.into(),
                    })
            }
            (None, None, None, component_name) => self
                .component_service
                .get_deployed_component_by_name(resolving_environment, &component_name, auth)
                .await
                .map_err(|err| match err {
                    ComponentError::ComponentByNameNotFound(_) => {
                        ComponentResolverError::ComponentNotFound
                    }
                    other => other.into(),
                }),
            _ => panic!("Received malformed parts from ComponentSlug::into_parts()"),
        }
    }
}
