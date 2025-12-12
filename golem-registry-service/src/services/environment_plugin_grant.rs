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

use super::environment::{EnvironmentError, EnvironmentService};
use super::plugin_registration::{PluginRegistrationError, PluginRegistrationService};
use crate::repo::environment_plugin_grant::EnvironmentPluginGrantRepo;
use crate::repo::model::environment_plugin_grant::{
    EnvironmentPluginGrantRecord, EnvironmentPluginGrantRepoError,
};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::environment_plugin_grant::{
    EnvironmentPluginGrant, EnvironmentPluginGrantCreation, EnvironmentPluginGrantId,
};
use golem_common::model::plugin_registration::PluginRegistrationId;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::EnvironmentAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum EnvironmentPluginGrantError {
    #[error("Referenced plugin {0} not found")]
    ReferencedPluginNotFound(PluginRegistrationId),
    #[error("Parent environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Environment plugin grant {0} not found")]
    EnvironmentPluginGrantNotFound(EnvironmentPluginGrantId),
    #[error("Grant for this plugin already exists in this environment")]
    GrantForPluginAlreadyExists,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for EnvironmentPluginGrantError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::EnvironmentPluginGrantNotFound(_) => self.to_string(),
            Self::GrantForPluginAlreadyExists => self.to_string(),
            Self::ReferencedPluginNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    EnvironmentPluginGrantError,
    EnvironmentPluginGrantRepoError,
    EnvironmentError,
    PluginRegistrationError
);

pub struct EnvironmentPluginGrantService {
    environment_plugin_grant_repo: Arc<dyn EnvironmentPluginGrantRepo>,
    environment_service: Arc<EnvironmentService>,
    plugin_registration_service: Arc<PluginRegistrationService>,
}

impl EnvironmentPluginGrantService {
    pub fn new(
        environment_plugin_grant_repo: Arc<dyn EnvironmentPluginGrantRepo>,
        environment_service: Arc<EnvironmentService>,
        plugin_registration_service: Arc<PluginRegistrationService>,
    ) -> Self {
        Self {
            environment_plugin_grant_repo,
            environment_service,
            plugin_registration_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: EnvironmentPluginGrantCreation,
        auth: &AuthCtx,
    ) -> Result<EnvironmentPluginGrant, EnvironmentPluginGrantError> {
        let environment = self
            .environment_service
            .get(&environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    EnvironmentPluginGrantError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        let plugin_registration = self
            .plugin_registration_service
            .get_plugin(&data.plugin_registration_id, false, auth)
            .await
            .map_err(|err| match err {
                PluginRegistrationError::PluginRegistrationNotFound(plugin_registration_id) => {
                    EnvironmentPluginGrantError::ReferencedPluginNotFound(plugin_registration_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateEnvironmentPluginGrant,
        )?;

        let record = EnvironmentPluginGrantRecord::creation(
            environment_id,
            data.plugin_registration_id,
            *auth.account_id(),
        );

        let created: EnvironmentPluginGrant = self
            .environment_plugin_grant_repo
            .create(record)
            .await
            .map_err(|err| match err {
                EnvironmentPluginGrantRepoError::PluginGrantViolatesUniqueness => {
                    EnvironmentPluginGrantError::GrantForPluginAlreadyExists
                }
                other => other.into(),
            })?
            .into_model(plugin_registration.into());

        Ok(created)
    }

    pub async fn delete(
        &self,
        environment_plugin_grant_id: &EnvironmentPluginGrantId,
        auth: &AuthCtx,
    ) -> Result<(), EnvironmentPluginGrantError> {
        let (_, environment) = self
            .get_by_id_with_environment(environment_plugin_grant_id, false, auth)
            .await?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteEnvironmentPluginGrant,
        )?;

        self.environment_plugin_grant_repo
            .delete(&environment_plugin_grant_id.0, &auth.account_id().0)
            .await?;

        Ok(())
    }

    pub async fn list_in_environment(
        &self,
        environment_id: &EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<EnvironmentPluginGrant>, EnvironmentPluginGrantError> {
        // Optimally this is fetched together with the grant data instead of up front
        // see EnvironmentService::list_in_application for a better pattern
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(environment_id) => {
                    EnvironmentPluginGrantError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewEnvironmentPluginGrant,
        )?;

        let grants: Vec<EnvironmentPluginGrant> = self
            .environment_plugin_grant_repo
            .list_by_environment(&environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<Vec<_>, _>>()?;

        Ok(grants)
    }

    pub async fn get_by_id(
        &self,
        environment_plugin_grant_id: &EnvironmentPluginGrantId,
        include_deleted: bool,
        auth: &AuthCtx,
    ) -> Result<EnvironmentPluginGrant, EnvironmentPluginGrantError> {
        Ok(self
            .get_by_id_with_environment(environment_plugin_grant_id, include_deleted, auth)
            .await?
            .0)
    }

    // will return not found for the plugin even if it exists if it's not in the right environment
    pub async fn get_active_by_id_for_environment(
        &self,
        environment_plugin_grant_id: &EnvironmentPluginGrantId,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<EnvironmentPluginGrant, EnvironmentPluginGrantError> {
        let grant: EnvironmentPluginGrant = self
            .environment_plugin_grant_repo
            .get_by_id(&environment_plugin_grant_id.0, false)
            .await?
            .ok_or(EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(
                *environment_plugin_grant_id,
            ))?
            .try_into()?;

        if grant.environment_id != environment.id {
            return Err(EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(
                *environment_plugin_grant_id,
            ));
        };

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewEnvironmentPluginGrant,
        )
        .map_err(|_| {
            EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(
                *environment_plugin_grant_id,
            )
        })?;

        Ok(grant)
    }

    async fn get_by_id_with_environment(
        &self,
        environment_plugin_grant_id: &EnvironmentPluginGrantId,
        include_deleted: bool,
        auth: &AuthCtx,
    ) -> Result<(EnvironmentPluginGrant, Environment), EnvironmentPluginGrantError> {
        let grant: EnvironmentPluginGrant = self
            .environment_plugin_grant_repo
            .get_by_id(&environment_plugin_grant_id.0, include_deleted)
            .await?
            .ok_or(EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(
                *environment_plugin_grant_id,
            ))?
            .try_into()?;

        let environment = self
            .environment_service
            .get(&grant.environment_id, include_deleted, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(
                        *environment_plugin_grant_id,
                    )
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            &environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewEnvironmentPluginGrant,
        )
        .map_err(|_| {
            EnvironmentPluginGrantError::EnvironmentPluginGrantNotFound(
                *environment_plugin_grant_id,
            )
        })?;

        Ok((grant, environment))
    }
}
