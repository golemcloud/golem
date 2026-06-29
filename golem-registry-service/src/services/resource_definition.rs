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

use super::environment::{EnvironmentError, EnvironmentService};
use super::registry_change_notifier::{RegistryChangeNotifier, RequiresNotificationSignalExt};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::resource_definition::{
    ResourceDefinitionAuthExtRevisionRecord, ResourceDefinitionCreationArgs,
    ResourceDefinitionRepoError, ResourceDefinitionRevisionRecord,
};
use crate::repo::resource_definition::ResourceDefinitionRepo;
use golem_common::model::account::AccountEmail;
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EnvironmentResourceDefinitionName,
    EnvironmentResourceDefinitionResourcePattern, EnvironmentResourceDefinitionVerb,
    PermissionTarget,
};
use golem_common::model::environment::{Environment, EnvironmentId, EnvironmentName};
use golem_common::model::quota::{
    ResourceDefinition, ResourceDefinitionCreation, ResourceDefinitionId,
    ResourceDefinitionRevision, ResourceDefinitionUpdate, ResourceLimit, ResourceName,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_service_base::repo::RepoError;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum ResourceDefinitionError {
    #[error("Resource definition {0} not found")]
    ResourceDefinitionNotFound(ResourceDefinitionId),
    #[error("Resource definition for name {0} not found in environment")]
    ResourceDefinitionByNameNotFound(ResourceName),
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Resource definition with name {0} already exists in this environment")]
    ResourceDefinitionForNameAlreadyExists(ResourceName),
    #[error("Concurrent update attempt")]
    ConcurrentUpdate,
    #[error("Limit type of a resource cannot be changed")]
    LimitTypeCannotBeChanged,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

fn authorize_resource_definition_permission(
    auth: &AuthCtx,
    environment: &Environment,
    name: Option<&ResourceName>,
    verb: EnvironmentResourceDefinitionVerb,
) -> Result<(), AuthorizationError> {
    authorize_resource_definition_permission_for_owner(
        auth,
        EnvironmentOwnerPattern::Environment {
            account: environment.owner_account_email.clone(),
            application: environment.application_name.clone(),
            environment: environment.name.clone(),
        },
        name,
        verb,
    )
}

fn authorize_resource_definition_permission_for_owner(
    auth: &AuthCtx,
    owner: EnvironmentOwnerPattern,
    name: Option<&ResourceName>,
    verb: EnvironmentResourceDefinitionVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentResourceDefinition(
        ClassPermissionTarget {
            verb: Some(verb),
            owner,
            resource: name
                .map(|name| {
                    EnvironmentResourceDefinitionResourcePattern::Name(
                        EnvironmentResourceDefinitionName(name.0.clone()),
                    )
                })
                .unwrap_or(EnvironmentResourceDefinitionResourcePattern::Any),
        },
    ))
}

fn environment_owner_from_resource_definition(
    resource_definition: &ResourceDefinitionAuthExtRevisionRecord,
) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: AccountEmail::new(resource_definition.owner_account_email.clone()),
        application: ApplicationName(resource_definition.application_name.clone()),
        environment: EnvironmentName(resource_definition.environment_name.clone()),
    }
}

impl SafeDisplay for ResourceDefinitionError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::ResourceDefinitionNotFound(_) => self.to_string(),
            Self::ResourceDefinitionByNameNotFound(_) => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::ResourceDefinitionForNameAlreadyExists(_) => self.to_string(),
            Self::ConcurrentUpdate => self.to_string(),
            Self::LimitTypeCannotBeChanged => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(
    ResourceDefinitionError,
    EnvironmentError,
    ResourceDefinitionRepoError,
    RepoError
);

pub struct ResourceDefinitionService {
    environment_service: Arc<EnvironmentService>,
    resource_definition_repo: Arc<dyn ResourceDefinitionRepo>,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

impl ResourceDefinitionService {
    pub fn new(
        environment_service: Arc<EnvironmentService>,
        resource_definition_repo: Arc<dyn ResourceDefinitionRepo>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            environment_service,
            resource_definition_repo,
            registry_change_notifier,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: ResourceDefinitionCreation,
        auth: &AuthCtx,
    ) -> Result<ResourceDefinition, ResourceDefinitionError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(id) => {
                    ResourceDefinitionError::ParentEnvironmentNotFound(id)
                }
                other => other.into(),
            })?;

        authorize_resource_definition_permission(
            auth,
            &environment,
            Some(&data.name),
            EnvironmentResourceDefinitionVerb::Create,
        )?;

        let id = ResourceDefinitionId::new();
        let creation_args = ResourceDefinitionCreationArgs::new(
            id,
            environment_id,
            &data.limit,
            data.name.clone(),
            data.enforcement_action,
            data.unit,
            data.units,
            auth.actor_account_id(),
        )?;

        let stored_resource_definition: ResourceDefinition = self
            .resource_definition_repo
            .create(creation_args)
            .await
            .map_err(|err| match err {
                ResourceDefinitionRepoError::ConcurrentModification => {
                    ResourceDefinitionError::ConcurrentUpdate
                }
                ResourceDefinitionRepoError::ResourceDefinitionViolatesUniqueness => {
                    ResourceDefinitionError::ResourceDefinitionForNameAlreadyExists(data.name)
                }
                other => other.into(),
            })?
            .signal_new_events_available(&self.registry_change_notifier)
            .try_into()?;

        Ok(stored_resource_definition)
    }

    pub async fn update(
        &self,
        resource_definition_id: ResourceDefinitionId,
        update: ResourceDefinitionUpdate,
        auth: &AuthCtx,
    ) -> Result<ResourceDefinition, ResourceDefinitionError> {
        let (mut resource_definition, owner) = self
            .get_with_environment(resource_definition_id, auth)
            .await?;

        authorize_resource_definition_permission_for_owner(
            auth,
            owner,
            Some(&resource_definition.name),
            EnvironmentResourceDefinitionVerb::Update,
        )?;

        if update.current_revision != resource_definition.revision {
            Err(ResourceDefinitionError::ConcurrentUpdate)?
        };

        resource_definition.revision = resource_definition.revision.next()?;
        if let Some(limit) = update.limit {
            match (&mut resource_definition.limit, limit) {
                (ResourceLimit::Capacity(current), ResourceLimit::Capacity(updated)) => {
                    current.value = updated.value;
                }
                (ResourceLimit::Concurrency(current), ResourceLimit::Concurrency(updated)) => {
                    current.value = updated.value;
                }
                (ResourceLimit::Rate(current), ResourceLimit::Rate(updated)) => {
                    current.value = updated.value;
                    current.period = updated.period;
                }
                _ => return Err(ResourceDefinitionError::LimitTypeCannotBeChanged),
            }
        };
        if let Some(enforcement_action) = update.enforcement_action {
            resource_definition.enforcement_action = enforcement_action;
        };
        if let Some(unit) = update.unit {
            resource_definition.unit = unit;
        };
        if let Some(units) = update.units {
            resource_definition.units = units;
        };

        let record = ResourceDefinitionRevisionRecord::from_model(
            resource_definition,
            DeletableRevisionAuditFields::new(auth.actor_account_id().0),
        )?;

        let stored_resource_definition: ResourceDefinition = self
            .resource_definition_repo
            .update(record)
            .await
            .map_err(|err| match err {
                ResourceDefinitionRepoError::ConcurrentModification => {
                    ResourceDefinitionError::ConcurrentUpdate
                }
                other => other.into(),
            })?
            .signal_new_events_available(&self.registry_change_notifier)
            .try_into()?;

        Ok(stored_resource_definition)
    }

    pub async fn delete(
        &self,
        resource_definition_id: ResourceDefinitionId,
        current_revision: ResourceDefinitionRevision,
        auth: &AuthCtx,
    ) -> Result<(), ResourceDefinitionError> {
        let (mut resource_definition, owner) = self
            .get_with_environment(resource_definition_id, auth)
            .await?;

        authorize_resource_definition_permission_for_owner(
            auth,
            owner,
            Some(&resource_definition.name),
            EnvironmentResourceDefinitionVerb::Delete,
        )?;

        if current_revision != resource_definition.revision {
            Err(ResourceDefinitionError::ConcurrentUpdate)?
        };

        resource_definition.revision = resource_definition.revision.next()?;

        let record = ResourceDefinitionRevisionRecord::from_model(
            resource_definition,
            DeletableRevisionAuditFields::deletion(auth.actor_account_id().0),
        )?;

        self.resource_definition_repo
            .delete(record)
            .await
            .map_err(|err| match err {
                ResourceDefinitionRepoError::ConcurrentModification => {
                    ResourceDefinitionError::ConcurrentUpdate
                }
                other => other.into(),
            })?
            .signal_new_events_available(&self.registry_change_notifier);

        Ok(())
    }

    pub async fn get(
        &self,
        resource_definition_id: ResourceDefinitionId,
        auth: &AuthCtx,
    ) -> Result<ResourceDefinition, ResourceDefinitionError> {
        let (resource_definition, _) = self
            .get_with_environment(resource_definition_id, auth)
            .await?;
        Ok(resource_definition)
    }

    pub async fn get_revision(
        &self,
        resource_definition_id: ResourceDefinitionId,
        revision: ResourceDefinitionRevision,
        auth: &AuthCtx,
    ) -> Result<ResourceDefinition, ResourceDefinitionError> {
        let record = self
            .resource_definition_repo
            .get_revision(resource_definition_id.0, revision.into())
            .await?
            .ok_or_else(|| {
                ResourceDefinitionError::ResourceDefinitionNotFound(resource_definition_id)
            })?;

        let owner = environment_owner_from_resource_definition(&record);
        let resource_definition: ResourceDefinition = record.resource_definition.try_into()?;

        authorize_resource_definition_permission_for_owner(
            auth,
            owner,
            Some(&resource_definition.name),
            EnvironmentResourceDefinitionVerb::View,
        )
        .map_err(|_| ResourceDefinitionError::ResourceDefinitionNotFound(resource_definition_id))?;

        Ok(resource_definition)
    }

    pub async fn get_in_environment(
        &self,
        environment_id: EnvironmentId,
        resource_name: &ResourceName,
        auth: &AuthCtx,
    ) -> Result<ResourceDefinition, ResourceDefinitionError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ResourceDefinitionError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        authorize_resource_definition_permission(
            auth,
            &environment,
            Some(resource_name),
            EnvironmentResourceDefinitionVerb::View,
        )
        .map_err(|_| {
            ResourceDefinitionError::ResourceDefinitionByNameNotFound(resource_name.clone())
        })?;

        let resource_definition: ResourceDefinition = self
            .resource_definition_repo
            .get_by_environment_and_name(environment_id.0, &resource_name.0)
            .await?
            .ok_or_else(|| {
                ResourceDefinitionError::ResourceDefinitionByNameNotFound(resource_name.clone())
            })?
            .try_into()?;

        Ok(resource_definition)
    }

    pub async fn list_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<ResourceDefinition>, ResourceDefinitionError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    ResourceDefinitionError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        self.list_in_fetched_environment(&environment, auth).await
    }

    pub async fn list_in_fetched_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Vec<ResourceDefinition>, ResourceDefinitionError> {
        let resource_definitions: Vec<ResourceDefinition> = self
            .resource_definition_repo
            .list_in_environment(environment.id.0)
            .await?
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?;

        Ok(resource_definitions
            .into_iter()
            .filter(|resource_definition| {
                authorize_resource_definition_permission(
                    auth,
                    environment,
                    Some(&resource_definition.name),
                    EnvironmentResourceDefinitionVerb::View,
                )
                .is_ok()
            })
            .collect())
    }

    async fn get_with_environment(
        &self,
        resource_definition_id: ResourceDefinitionId,
        auth: &AuthCtx,
    ) -> Result<(ResourceDefinition, EnvironmentOwnerPattern), ResourceDefinitionError> {
        let record = self
            .resource_definition_repo
            .get(resource_definition_id.0)
            .await?
            .ok_or_else(|| {
                ResourceDefinitionError::ResourceDefinitionNotFound(resource_definition_id)
            })?;

        let owner = environment_owner_from_resource_definition(&record);
        let resource_definition: ResourceDefinition = record.resource_definition.try_into()?;

        authorize_resource_definition_permission_for_owner(
            auth,
            owner.clone(),
            Some(&resource_definition.name),
            EnvironmentResourceDefinitionVerb::View,
        )
        .map_err(|_| ResourceDefinitionError::ResourceDefinitionNotFound(resource_definition_id))?;

        Ok((resource_definition, owner))
    }
}
