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
use crate::repo::agent_secret::AgentSecretRepo;
use crate::repo::model::agent_secrets::{
    AgentSecretCreationRecord, AgentSecretRepoError, AgentSecretRevisionRecord,
};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use golem_common::model::agent_secret::{
    AgentSecretCreation, AgentSecretId, AgentSecretRevision, AgentSecretUpdate,
    CanonicalAgentSecretPath,
};
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EnvironmentAgentSecretKeyPathPattern,
    EnvironmentAgentSecretKeySegmentPattern, EnvironmentAgentSecretResourcePattern,
    EnvironmentAgentSecretVerb, PermissionTarget,
};
use golem_common::model::environment::{Environment, EnvironmentId};
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_common::schema::validation::{validate_graph, validate_value};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum AgentSecretError {
    #[error("Agent secret value does not match type: [{rendered_errors}]", rendered_errors = errors.join(", "))]
    AgentSecretValueDoesNotMatchType { errors: Vec<String> },
    #[error("Agent secret for path {path} already exists in environment")]
    AgentSecretForPathAlreadyExists { path: CanonicalAgentSecretPath },
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

fn authorize_agent_secret_permission(
    auth: &AuthCtx,
    environment: &Environment,
    key: Option<&CanonicalAgentSecretPath>,
    verb: EnvironmentAgentSecretVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentAgentSecret(
        ClassPermissionTarget {
            verb: Some(verb),
            owner: EnvironmentOwnerPattern::Environment {
                account: environment.owner_account_email.clone(),
                application: environment.application_name.clone(),
                environment: environment.name.clone(),
            },
            resource: key
                .map(|key| {
                    EnvironmentAgentSecretResourcePattern::Key(
                        EnvironmentAgentSecretKeyPathPattern {
                            segments: key
                                .0
                                .iter()
                                .cloned()
                                .map(EnvironmentAgentSecretKeySegmentPattern::Literal)
                                .collect(),
                        },
                    )
                })
                .unwrap_or(EnvironmentAgentSecretResourcePattern::Any),
        },
    ))
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
    registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
}

impl AgentSecretService {
    pub fn new(
        agent_secret_repo: Arc<dyn AgentSecretRepo>,
        environment_service: Arc<EnvironmentService>,
        registry_change_notifier: Arc<dyn RegistryChangeNotifier>,
    ) -> Self {
        Self {
            agent_secret_repo,
            environment_service,
            registry_change_notifier,
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

        let agent_secret_path: CanonicalAgentSecretPath = data.path.into();

        authorize_agent_secret_permission(
            auth,
            &environment,
            Some(&agent_secret_path),
            EnvironmentAgentSecretVerb::Create,
        )?;

        // The REST DTO is schema-native: the secret type arrives as a
        // `SchemaGraph` and the value (if any) as a `SchemaValue`. Validate
        // the graph is well-formed and that the value conforms to it before
        // persisting.
        let secret_type_graph = data.secret_type;
        validate_graph(&secret_type_graph).map_err(|errors| {
            AgentSecretError::AgentSecretValueDoesNotMatchType {
                errors: errors.iter().map(|e| e.to_string()).collect(),
            }
        })?;
        let secret_value = data.secret_value;
        if let Some(sv) = &secret_value {
            validate_value(&secret_type_graph, &secret_type_graph.root, sv).map_err(|errors| {
                AgentSecretError::AgentSecretValueDoesNotMatchType {
                    errors: errors.iter().map(|e| e.to_string()).collect(),
                }
            })?;
        }

        let id = AgentSecretId::new();

        let stored_agent_secret = self
            .agent_secret_repo
            .create(AgentSecretCreationRecord::new(
                id,
                environment_id,
                agent_secret_path.clone(),
                secret_type_graph,
                secret_value,
                auth.actor_account_id(),
            ))
            .await
            .map_err(|err| match err {
                AgentSecretRepoError::SecretViolatesUniqueness => {
                    AgentSecretError::AgentSecretForPathAlreadyExists {
                        path: agent_secret_path,
                    }
                }
                other => other.into(),
            })?
            .signal_new_events_available(&self.registry_change_notifier)
            .try_into()?;

        Ok(stored_agent_secret)
    }

    pub async fn update(
        &self,
        agent_secret_id: AgentSecretId,
        update: AgentSecretUpdate,
        auth: &AuthCtx,
    ) -> Result<AgentSecret, AgentSecretError> {
        let (mut agent_secret, environment) =
            self.get_with_environment(agent_secret_id, auth).await?;

        authorize_agent_secret_permission(
            auth,
            &environment,
            Some(&agent_secret.path),
            EnvironmentAgentSecretVerb::Update,
        )?;

        if update.current_revision != agent_secret.revision {
            return Err(AgentSecretError::ConcurrentModification);
        };

        agent_secret.revision = agent_secret.revision.next()?;

        match update.secret_value {
            OptionalFieldUpdate::NoChange => {}
            OptionalFieldUpdate::Set(new_secret_value) => {
                // The new value is schema-native; validate it against the
                // stored secret's `SchemaGraph` before applying.
                validate_value(
                    &agent_secret.secret_type,
                    &agent_secret.secret_type.root,
                    &new_secret_value,
                )
                .map_err(|errors| {
                    AgentSecretError::AgentSecretValueDoesNotMatchType {
                        errors: errors.iter().map(|e| e.to_string()).collect(),
                    }
                })?;
                agent_secret.secret_value = Some(new_secret_value);
            }
            OptionalFieldUpdate::Unset => {
                agent_secret.secret_value = None;
            }
        }

        let audit = DeletableRevisionAuditFields::new(auth.actor_account_id().0);

        let stored_agent_secret = self
            .agent_secret_repo
            .update(AgentSecretRevisionRecord::from_model(agent_secret, audit))
            .await
            .map_err(|err| match err {
                AgentSecretRepoError::ConcurrentModification => {
                    AgentSecretError::ConcurrentModification
                }
                other => other.into(),
            })?
            .signal_new_events_available(&self.registry_change_notifier)
            .try_into()?;

        Ok(stored_agent_secret)
    }

    pub async fn delete(
        &self,
        agent_secret_id: AgentSecretId,
        current_revision: AgentSecretRevision,
        auth: &AuthCtx,
    ) -> Result<AgentSecret, AgentSecretError> {
        let (mut agent_secret, environment) =
            self.get_with_environment(agent_secret_id, auth).await?;

        authorize_agent_secret_permission(
            auth,
            &environment,
            Some(&agent_secret.path),
            EnvironmentAgentSecretVerb::Delete,
        )?;

        if agent_secret.revision != current_revision {
            return Err(AgentSecretError::ConcurrentModification);
        }

        agent_secret.revision = current_revision.next()?;

        let audit = DeletableRevisionAuditFields::deletion(auth.actor_account_id().0);

        let stored_agent_secret = self
            .agent_secret_repo
            .delete(AgentSecretRevisionRecord::from_model(agent_secret, audit))
            .await
            .map_err(|err| match err {
                AgentSecretRepoError::ConcurrentModification => {
                    AgentSecretError::ConcurrentModification
                }
                other => other.into(),
            })?
            .signal_new_events_available(&self.registry_change_notifier)
            .try_into()?;

        Ok(stored_agent_secret)
    }

    pub async fn get(
        &self,
        agent_secret_id: AgentSecretId,
        auth: &AuthCtx,
    ) -> Result<AgentSecret, AgentSecretError> {
        let (agent_secret, _) = self.get_with_environment(agent_secret_id, auth).await?;
        Ok(agent_secret)
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
        authorize_agent_secret_permission(
            auth,
            environment,
            None,
            EnvironmentAgentSecretVerb::View,
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

        authorize_agent_secret_permission(
            auth,
            &environment,
            Some(&agent_secret.path),
            EnvironmentAgentSecretVerb::View,
        )
        .map_err(|_| AgentSecretError::AgentSecretNotFound(agent_secret_id))?;

        Ok((agent_secret, environment))
    }
}
