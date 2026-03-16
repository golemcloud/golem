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
use crate::repo::agent_secret::AgentSecretRepo;
use crate::repo::model::agent_secrets::{
    AgentSecretCreationRecord, AgentSecretRepoError, AgentSecretRevisionRecord,
};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use golem_common::model::agent_secret::{
    AgentSecretCreation, AgentSecretId, AgentSecretRevision, AgentSecretUpdate,
};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError, EnvironmentAction};
use golem_wasm::ValueAndType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum AgentSecretError {
    #[error("Agent secret value does not match type: [{rendered_errors}]", rendered_errors = errors.join(", "))]
    AgentSecretValueDoesNotMatchType { errors: Vec<String> },
    #[error("Agent secret for path {rendered_path} already exists in environment", rendered_path = path.join("."))]
    AgentSecretForPathAlreadyExists { path: Vec<String> },
    #[error("Environment {0} not found")]
    ParentEnvironmentNotFound(EnvironmentId),
    #[error("Agent secret {0} not found")]
    AgentSecretNotFound(AgentSecretId),
    #[error("Concurrent update attempt")]
    ConcurrentModification,
    #[error(transparent)]
    Unauthorized(#[from] AuthorizationError),
    #[error(transparent)]
    InternalError(#[from] anyhow::Error),
}

impl SafeDisplay for AgentSecretError {
    fn to_safe_string(&self) -> String {
        match self {
            Self::AgentSecretValueDoesNotMatchType { .. } => self.to_string(),
            Self::AgentSecretForPathAlreadyExists { .. } => self.to_string(),
            Self::ParentEnvironmentNotFound(_) => self.to_string(),
            Self::AgentSecretNotFound(_) => self.to_string(),
            Self::ConcurrentModification => self.to_string(),
            Self::Unauthorized(inner) => inner.to_safe_string(),
            Self::InternalError(_) => "Internal error".to_string(),
        }
    }
}

error_forwarding!(AgentSecretError, EnvironmentError, AgentSecretRepoError);

pub struct AgentSecretService {
    agent_secret_repo: Arc<dyn AgentSecretRepo>,
    environment_service: Arc<EnvironmentService>,
}

impl AgentSecretService {
    pub fn new(
        agent_secret_repo: Arc<dyn AgentSecretRepo>,
        environment_service: Arc<EnvironmentService>,
    ) -> Self {
        Self {
            agent_secret_repo,
            environment_service,
        }
    }

    pub async fn create(
        &self,
        environment_id: EnvironmentId,
        data: AgentSecretCreation,
        auth: &AuthCtx,
    ) -> Result<AgentSecret, AgentSecretError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    AgentSecretError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::CreateAgentSecret,
        )?;

        let secret_value = data
            .secret_value
            .map(|sv| ValueAndType::parse_with_type(&sv, &data.secret_type))
            .transpose()
            .map_err(|errors| AgentSecretError::AgentSecretValueDoesNotMatchType { errors })?
            .map(|vat| vat.value);

        let id = AgentSecretId::new();

        let result = self
            .agent_secret_repo
            .create(AgentSecretCreationRecord::new(
                id,
                environment_id,
                data.path.clone(),
                data.secret_type,
                secret_value,
                auth.account_id(),
            ))
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AgentSecretRepoError::SecretViolatesUniqueness) => {
                Err(AgentSecretError::AgentSecretForPathAlreadyExists { path: data.path })
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn update(
        &self,
        agent_secret_id: AgentSecretId,
        update: AgentSecretUpdate,
        auth: &AuthCtx,
    ) -> Result<AgentSecret, AgentSecretError> {
        let (mut agent_secret, environment) =
            self.get_with_environment(agent_secret_id, auth).await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::UpdateAgentSecret,
        )?;

        if update.current_revision != agent_secret.revision {
            return Err(AgentSecretError::ConcurrentModification);
        };

        agent_secret.revision = agent_secret.revision.next()?;

        match update.secret_value {
            OptionalFieldUpdate::NoChange => {}
            OptionalFieldUpdate::Set(new_secret_value) => {
                let parsed_new_secret_value =
                    ValueAndType::parse_with_type(&new_secret_value, &agent_secret.secret_type)
                        .map_err(
                            |errors| AgentSecretError::AgentSecretValueDoesNotMatchType { errors },
                        )?
                        .value;
                agent_secret.secret_value = Some(parsed_new_secret_value);
            }
            OptionalFieldUpdate::Unset => {
                agent_secret.secret_value = None;
            }
        }

        let audit = DeletableRevisionAuditFields::new(auth.account_id().0);

        let result = self
            .agent_secret_repo
            .update(AgentSecretRevisionRecord::from_model(agent_secret, audit))
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AgentSecretRepoError::ConcurrentModification) => {
                Err(AgentSecretError::ConcurrentModification)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn delete(
        &self,
        agent_secret_id: AgentSecretId,
        current_revision: AgentSecretRevision,
        auth: &AuthCtx,
    ) -> Result<AgentSecret, AgentSecretError> {
        let (mut agent_secret, environment) =
            self.get_with_environment(agent_secret_id, auth).await?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::DeleteShare,
        )?;

        if agent_secret.revision != current_revision {
            return Err(AgentSecretError::ConcurrentModification);
        }

        agent_secret.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.account_id().0);

        let result = self
            .agent_secret_repo
            .delete(AgentSecretRevisionRecord::from_model(agent_secret, audit))
            .await;

        match result {
            Ok(record) => Ok(record.try_into()?),
            Err(AgentSecretRepoError::ConcurrentModification) => {
                Err(AgentSecretError::ConcurrentModification)
            }
            Err(other) => Err(other.into()),
        }
    }

    pub async fn get(
        &self,
        agent_secret_id: AgentSecretId,
        auth: &AuthCtx,
    ) -> Result<AgentSecret, AgentSecretError> {
        let (environment_share, _) = self.get_with_environment(agent_secret_id, auth).await?;
        Ok(environment_share)
    }

    pub async fn list_in_environment(
        &self,
        environment_id: EnvironmentId,
        auth: &AuthCtx,
    ) -> Result<Vec<AgentSecret>, AgentSecretError> {
        let environment = self
            .environment_service
            .get(environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    AgentSecretError::ParentEnvironmentNotFound(environment_id)
                }
                other => other.into(),
            })?;

        self.list_in_fetched_environment(&environment, auth).await
    }

    pub async fn list_in_fetched_environment(
        &self,
        environment: &Environment,
        auth: &AuthCtx,
    ) -> Result<Vec<AgentSecret>, AgentSecretError> {
        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewAgentSecret,
        )?;

        let result = self.list_in_environment_unchecked(environment.id).await?;

        Ok(result)
    }

    // list in environment without checking auth / confirming the environment is not deleted.
    pub async fn list_in_environment_unchecked(
        &self,
        environment_id: EnvironmentId,
    ) -> Result<Vec<AgentSecret>, AgentSecretError> {
        let result = self
            .agent_secret_repo
            .get_for_environment(environment_id.0)
            .await?
            .into_iter()
            .map(|r| r.try_into())
            .collect::<Result<_, _>>()?;

        Ok(result)
    }

    async fn get_with_environment(
        &self,
        agent_secret_id: AgentSecretId,
        auth: &AuthCtx,
    ) -> Result<(AgentSecret, Environment), AgentSecretError> {
        let agent_secret: AgentSecret = self
            .agent_secret_repo
            .get_by_id(agent_secret_id.0)
            .await?
            .ok_or(AgentSecretError::AgentSecretNotFound(agent_secret_id))?
            .try_into()?;

        let environment = self
            .environment_service
            .get(agent_secret.environment_id, false, auth)
            .await
            .map_err(|err| match err {
                EnvironmentError::EnvironmentNotFound(_) => {
                    AgentSecretError::AgentSecretNotFound(agent_secret_id)
                }
                other => other.into(),
            })?;

        auth.authorize_environment_action(
            environment.owner_account_id,
            &environment.roles_from_active_shares,
            EnvironmentAction::ViewAgentSecret,
        )
        .map_err(|_| AgentSecretError::AgentSecretNotFound(agent_secret_id))?;

        Ok((agent_secret, environment))
    }
}
