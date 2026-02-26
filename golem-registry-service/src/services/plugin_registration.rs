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
use super::component::{ComponentError, ComponentService};
use crate::repo::model::audit::ImmutableAuditFields;
use crate::repo::model::plugin::PluginRecord;
use crate::repo::plugin::PluginRepo;
use golem_common::model::account::AccountId;
use golem_common::model::plugin_registration::{
    OplogProcessorPluginSpec, PluginRegistrationCreation, PluginRegistrationId, PluginSpecDto,
};
use golem_common::{error_forwarding, SafeDisplay};
use golem_service_base::model::auth::AccountAction;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::model::plugin_registration::{PluginRegistration, PluginSpec};
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum PluginRegistrationError {
    #[error("Registered plugin not found for id {0}")]
    PluginRegistrationNotFound(PluginRegistrationId),
    #[error("Target component for oplog processor does not exist")]
    OplogProcessorComponentDoesNotExist,
    #[error("Plugin with this name and version already exists")]
    PluginNameAndVersionAlreadyExists,
    #[error("Parent account {0} not found")]
    ParentAccountNotFound(AccountId),
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for PluginRegistrationError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::PluginRegistrationNotFound(_) => self.to_string(),
            Self::OplogProcessorComponentDoesNotExist => self.to_string(),
            Self::PluginNameAndVersionAlreadyExists => self.to_string(),
            Self::ParentAccountNotFound(_) => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    PluginRegistrationError,
    AccountError,
    RepoError,
    ComponentError
);

pub struct PluginRegistrationService {
    plugin_repo: Arc<dyn PluginRepo>,
    account_service: Arc<AccountService>,
    component_service: Arc<ComponentService>,
}

impl PluginRegistrationService {
    pub fn new(
        plugin_repo: Arc<dyn PluginRepo>,
        account_service: Arc<AccountService>,
        component_service: Arc<ComponentService>,
    ) -> Self {
        Self {
            plugin_repo,
            account_service,
            component_service,
        }
    }

    pub async fn register_plugin(
        &self,
        account_id: AccountId,
        data: PluginRegistrationCreation,
        auth: &AuthCtx,
    ) -> Result<PluginRegistration, PluginRegistrationError> {
        self.account_service
            .get(account_id, auth)
            .await
            .map_err(|err| match err {
                AccountError::AccountNotFound(account_id) => {
                    PluginRegistrationError::ParentAccountNotFound(account_id)
                }
                other => other.into(),
            })?;

        auth.authorize_account_action(account_id, AccountAction::RegisterPlugin)?;

        let spec = match data.spec {
            PluginSpecDto::OplogProcessor(inner) => {
                self.validate_oplog_processor_plugin(&inner, auth).await?;
                PluginSpec::OplogProcessor(inner)
            }
        };

        let id = PluginRegistrationId::new();

        let registration = PluginRegistration {
            id,
            account_id,
            name: data.name,
            version: data.version,
            description: data.description,
            icon: data.icon.0,
            homepage: data.homepage,
            spec,
        };

        let audit = ImmutableAuditFields::new(auth.account_id().0);

        let record = PluginRecord::from_model(registration, audit);

        let created = self
            .plugin_repo
            .create(record)
            .await?
            .ok_or(PluginRegistrationError::PluginNameAndVersionAlreadyExists)?
            .try_into()?;

        Ok(created)
    }

    pub async fn unregister_plugin(
        &self,
        plugin_id: PluginRegistrationId,
        auth: &AuthCtx,
    ) -> Result<PluginRegistration, PluginRegistrationError> {
        let plugin = self.get_plugin(plugin_id, false, auth).await?;

        auth.authorize_account_action(plugin.account_id, AccountAction::DeletePlugin)?;

        let plugin = self
            .plugin_repo
            .delete(plugin_id.0, auth.account_id().0)
            .await?
            .ok_or(PluginRegistrationError::PluginRegistrationNotFound(
                plugin_id,
            ))?
            .try_into()?;

        Ok(plugin)
    }

    pub async fn get_plugin(
        &self,
        plugin_id: PluginRegistrationId,
        include_deleted: bool,
        auth: &AuthCtx,
    ) -> Result<PluginRegistration, PluginRegistrationError> {
        let plugin: PluginRegistration = self
            .plugin_repo
            .get_by_id(plugin_id.0, include_deleted)
            .await?
            .ok_or(PluginRegistrationError::PluginRegistrationNotFound(
                plugin_id,
            ))?
            .try_into()?;

        auth.authorize_account_action(plugin.account_id, AccountAction::ViewPlugin)
            .map_err(|_| PluginRegistrationError::PluginRegistrationNotFound(plugin_id))?;

        Ok(plugin)
    }

    pub async fn list_plugins_in_account(
        &self,
        account_id: AccountId,
        auth: &AuthCtx,
    ) -> Result<Vec<PluginRegistration>, PluginRegistrationError> {
        // Optimally this is fetched together with the plugin data instead of up front
        // see EnvironmentService::list_in_application for a better pattern
        self.account_service
            .get(account_id, auth)
            .await
            .map_err(|err| match err {
                AccountError::AccountNotFound(account_id) => {
                    PluginRegistrationError::ParentAccountNotFound(account_id)
                }
                other => other.into(),
            })?;

        auth.authorize_account_action(account_id, AccountAction::ViewPlugin)?;

        let plugins: Vec<PluginRegistration> = self
            .plugin_repo
            .list_by_account(account_id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        Ok(plugins)
    }

    async fn validate_oplog_processor_plugin(
        &self,
        definition: &OplogProcessorPluginSpec,
        auth: &AuthCtx,
    ) -> Result<(), PluginRegistrationError> {
        // Note: any component that the user has _currently_ access to is valid as a source of an oplog
        // processor plugin.
        let component = self
            .component_service
            .get_component_revision(
                definition.component_id,
                definition.component_revision,
                false,
                auth,
            )
            .await
            .map_err(|err| match err {
                ComponentError::ComponentNotFound(_) => {
                    PluginRegistrationError::OplogProcessorComponentDoesNotExist
                }
                other => other.into(),
            })?;

        let implements_oplog_processor_interface = component
            .metadata
            .oplog_processor()
            .ok()
            .flatten()
            .is_some();

        if !implements_oplog_processor_interface {
            return Err(PluginRegistrationError::OplogProcessorComponentDoesNotExist);
        }

        Ok(())
    }
}
