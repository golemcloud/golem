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
use super::registry_change_notifier::{RegistryChangeNotifier, RequiresNotificationSignalExt};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use crate::repo::model::retry_policy::{
    RetryPolicyCreationRecord, RetryPolicyRepoError, RetryPolicyRevisionRecord,
};
use crate::repo::retry_policy::RetryPolicyRepo;
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::retry_policy::{
    RetryPolicyCreation, RetryPolicyId, RetryPolicyRevision, RetryPolicyUpdate,
};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::auth::{AuthCtx, AuthorizationError, EnvironmentAction};
use golem_service_base::model::retry_policy::StoredRetryPolicy;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum RetryPolicyError {
    #[error("Invalid predicate JSON: {0}")]
    InvalidPredicateJson(String),
    #[error("Invalid policy JSON: {0}")]
    InvalidPolicyJson(String),
    #[error("Retry policy for name {name} already exists in environment")]
    RetryPolicyForNameAlreadyExists { name: String },
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Retry policy {0} not found")]
    RetryPolicyNotFound(RetryPolicyId),
    #[error("Concurrent update attempt")]
    ConcurrentModification,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for RetryPolicyError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::InvalidPredicateJson(_) => self.to_string(),
            Self::InvalidPolicyJson(_) => self.to_string(),
            Self::RetryPolicyForNameAlreadyExists { .. } => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::RetryPolicyNotFound(_) => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(RetryPolicyError, EnvironmentError, RetryPolicyRepoError);

pub struct RetryPolicyService {
    retry_policy_repo: Arc<dyn RetryPolicyRepo>,
    environment_service: Arc<EnvironmentService>,
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

impl RetryPolicyService {
    pub fn new(
        retry_policy_repo: Arc<dyn RetryPolicyRepo>,
        environment_service: Arc<EnvironmentService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            retry_policy_repo,
            environment_service,
            registry_change_notifier,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: RetryPolicyCreation,
        auth: &AuthCtx,
    ) -> Result<StoredRetryPolicy, RetryPolicyError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    RetryPolicyError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateRetryPolicy,
        )?;

        serde_json::from_str::<golem_common::model::retry_policy::Predicate>(&data.predicate_json)
            .map_err(|e| RetryPolicyError::InvalidPredicateJson(e.to_string()))?;

        serde_json::from_str::<golem_common::model::retry_policy::RetryPolicy>(&data.policy_json)
            .map_err(|e| RetryPolicyError::InvalidPolicyJson(e.to_string()))?;

        let id = RetryPolicyId::new();
        let name = data.name.clone();

        let result = self
            .retry_policy_repo
            .create(RetryPolicyCreationRecord::new(
                id,
                environment_id,
                data.name,
                data.priority,
                data.predicate_json,
                data.policy_json,
                auth.account_id(),
            ))
            .await;

        match result {
            Ok(record) => Ok(record
                .signal_new_events_available(&self.registry_change_notifier)
                .try_into()?),
            Err(RetryPolicyRepoError::NameViolatesUniqueness) => {
                Err(RetryPolicyError::RetryPolicyForNameAlreadyExists { name })
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn update(
        &self,
        retry_policy_id: RetryPolicyId,
        update: RetryPolicyUpdate,
        auth: &AuthCtx,
    ) -> Result<StoredRetryPolicy, RetryPolicyError> {
        let (mut retry_policy, environment) =
            self.get_with_environment(retry_policy_id, auth).await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::UpdateRetryPolicy,
        )?;

        if update.current_revision != retry_policy.revision {
            return Err(RetryPolicyError::ConcurrentModification);
        };

        retry_policy.revision = retry_policy.revision.next()?;

        if let Some(new_priority) = update.priority {
            retry_policy.priority = new_priority;
        }

        if let Some(ref new_predicate_json) = update.predicate_json {
            serde_json::from_str::<golem_common::model::retry_policy::Predicate>(
                new_predicate_json,
            )
            .map_err(|e| RetryPolicyError::InvalidPredicateJson(e.to_string()))?;
            retry_policy.predicate_json = new_predicate_json.clone();
        }

        if let Some(ref new_policy_json) = update.policy_json {
            serde_json::from_str::<golem_common::model::retry_policy::RetryPolicy>(new_policy_json)
                .map_err(|e| RetryPolicyError::InvalidPolicyJson(e.to_string()))?;
            retry_policy.policy_json = new_policy_json.clone();
        }

        let audit = DeletableRevisionAuditFields::new(auth.account_id().0);

        let result = self
            .retry_policy_repo
            .update(RetryPolicyRevisionRecord::from_model(retry_policy, audit))
            .await;

        match result {
            Ok(record) => Ok(record
                .signal_new_events_available(&self.registry_change_notifier)
                .try_into()?),
            Err(RetryPolicyRepoError::ConcurrentModification) => {
                Err(RetryPolicyError::ConcurrentModification)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn delete(
        &self,
        retry_policy_id: RetryPolicyId,
        current_revision: RetryPolicyRevision,
        auth: &AuthCtx,
    ) -> Result<StoredRetryPolicy, RetryPolicyError> {
        let (mut retry_policy, environment) =
            self.get_with_environment(retry_policy_id, auth).await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteRetryPolicy,
        )?;

        if retry_policy.revision != current_revision {
            return Err(RetryPolicyError::ConcurrentModification);
        }

        retry_policy.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.account_id().0);

        let result = self
            .retry_policy_repo
            .delete(RetryPolicyRevisionRecord::from_model(retry_policy, audit))
            .await;

        match result {
            Ok(record) => Ok(record
                .signal_new_events_available(&self.registry_change_notifier)
                .try_into()?),
            Err(RetryPolicyRepoError::ConcurrentModification) => {
                Err(RetryPolicyError::ConcurrentModification)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn get(
        &self,
        retry_policy_id: RetryPolicyId,
        auth: &AuthCtx,
    ) -> Result<StoredRetryPolicy, RetryPolicyError> {
        let (retry_policy, _) = self.get_with_environment(retry_policy_id, auth).await?;
        Ok(retry_policy)
    }

    pub async fn list_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<StoredRetryPolicy>, RetryPolicyError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    RetryPolicyError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        self.list_in_fetched_environment(&environment, auth).await
    }

    pub async fn list_in_fetched_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Vec<StoredRetryPolicy>, RetryPolicyError> {
        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewRetryPolicy,
        )?;

        let result = self.list_in_environment_unchecked(environment.id).await?;

        Ok(result)
    }

    pub async fn list_in_environment_unchecked(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<StoredRetryPolicy>, RetryPolicyError> {
        let result = self
            .retry_policy_repo
            .get_for_environment(environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        Ok(result)
    }

    async fn get_with_environment(
        &self,
        retry_policy_id: RetryPolicyId,
        auth: &AuthCtx,
    ) -> Result<(StoredRetryPolicy, Environment), RetryPolicyError> {
        let retry_policy: StoredRetryPolicy = self
            .retry_policy_repo
            .get_by_id(retry_policy_id.0)
            .await?
            .ok_or(RetryPolicyError::RetryPolicyNotFound(retry_policy_id))?
            .try_into()?;

        let environment = self
            .environment_service
            .get(retry_policy.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    RetryPolicyError::RetryPolicyNotFound(retry_policy_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewRetryPolicy,
        )
        .map_err(|_| RetryPolicyError::RetryPolicyNotFound(retry_policy_id))?;

        Ok((retry_policy, environment))
    }
}
