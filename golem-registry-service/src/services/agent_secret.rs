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
    AgentSecretAuthExtRevisionRecord, AgentSecretCreationRecord, AgentSecretRepoError,
    AgentSecretRevisionRecord,
};
use crate::repo::model::audit::DeletableRevisionAuditFields;
use golem_common::model::account::AccountEmail;
use golem_common::model::agent_secret::{
    AgentSecretCreation, AgentSecretId, AgentSecretRevision, AgentSecretUpdate,
    CanonicalAgentSecretPath,
};
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::EnvironmentOwnerPattern;
use golem_common::model::card::{
    ClassPermissionTarget, EnvironmentAgentSecretKeyPathPattern,
    EnvironmentAgentSecretKeySegmentPattern, EnvironmentAgentSecretResourcePattern,
    EnvironmentAgentSecretVerb, PermissionTarget,
};
use golem_common::model::environment::{Environment, EnvironmentId, EnvironmentName};
use golem_common::model::optional_field_update::OptionalFieldUpdate;
use golem_common::schema::adapters::analysed_type::{
    analysed_type_to_schema_graph, schema_graph_to_analysed_type,
};
use golem_common::schema::adapters::value::value_to_schema_value;
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use golem_wasm::ValueAndType;
use golem_wasm::json::ValueAndTypeJsonExtensions;
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
    authorize_agent_secret_permission_for_owner(
        auth,
        EnvironmentOwnerPattern::Environment {
            account: environment.owner_account_email.clone(),
            application: environment.application_name.clone(),
            environment: environment.name.clone(),
        },
        key,
        verb,
    )
}

fn authorize_agent_secret_permission_for_owner(
    auth: &AuthCtx,
    owner: EnvironmentOwnerPattern,
    key: Option<&[String]>,
    verb: EnvironmentAgentSecretVerb,
) -> Result<(), AuthorizationError> {
    auth.authorize_permission(&PermissionTarget::EnvironmentAgentSecret(
        ClassPermissionTarget {
            verb: Some(verb),
            owner,
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

fn environment_owner_from_agent_secret(
    agent_secret: &AgentSecretAuthExtRevisionRecord,
) -> EnvironmentOwnerPattern {
    EnvironmentOwnerPattern::Environment {
        account: AccountEmail::new(agent_secret.owner_account_email.clone()),
        application: ApplicationName(agent_secret.application_name.clone()),
        environment: EnvironmentName(agent_secret.environment_name.clone()),
    }
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

        // The REST DTO carries an `AnalysedType` + legacy-shaped JSON, so
        // parse the JSON against the `AnalysedType` first and then promote
        // the resulting `Value` into the schema layer for in-memory + repo
        // use. This preserves the legacy wire shape (numeric `char`,
        // `{"Case": null}` for unit variants, lenient optional record
        // fields, etc.).
        let secret_type_graph = analysed_type_to_schema_graph(&data.secret_type).map_err(|e| {
            AgentSecretError::AgentSecretValueDoesNotMatchType {
                errors: vec![format!("Invalid secret type: {e}")],
            }
        })?;
        let secret_value = data
            .secret_value
            .as_ref()
            .map(|sv| {
                let vat =
                    ValueAndType::parse_with_type(sv, &data.secret_type).map_err(|errors| {
                        AgentSecretError::AgentSecretValueDoesNotMatchType { errors }
                    })?;
                value_to_schema_value(&vat.value, &data.secret_type).map_err(|e| {
                    AgentSecretError::AgentSecretValueDoesNotMatchType {
                        errors: vec![format!(
                            "Failed to promote secret value to schema layer: {e}"
                        )],
                    }
                })
            })
            .transpose()?;

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
        let (mut agent_secret, owner) = self.get_with_environment(agent_secret_id, auth).await?;

        authorize_agent_secret_permission_for_owner(
            auth,
            owner,
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
                // See `create` above for the JSON-shape rationale. Project
                // the stored `SchemaGraph` back to `AnalysedType` so the
                // legacy JSON parser can be used, then promote the resulting
                // `Value` into the schema layer.
                let legacy_type = schema_graph_to_analysed_type(&agent_secret.secret_type)
                    .map_err(|e| AgentSecretError::AgentSecretValueDoesNotMatchType {
                        errors: vec![format!(
                            "Failed to project stored secret schema to AnalysedType: {e}"
                        )],
                    })?;
                let vat = ValueAndType::parse_with_type(&new_secret_value, &legacy_type).map_err(
                    |errors| AgentSecretError::AgentSecretValueDoesNotMatchType { errors },
                )?;
                let parsed_new_secret_value = value_to_schema_value(&vat.value, &legacy_type)
                    .map_err(|e| AgentSecretError::AgentSecretValueDoesNotMatchType {
                        errors: vec![format!(
                            "Failed to promote secret value to schema layer: {e}"
                        )],
                    })?;
                agent_secret.secret_value = Some(parsed_new_secret_value);
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
        let (mut agent_secret, owner) = self.get_with_environment(agent_secret_id, auth).await?;

        authorize_agent_secret_permission_for_owner(
            auth,
            owner,
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
        let result = self.list_in_environment_unchecked(environment.id).await?;

        Ok(result
            .into_iter()
            .filter(|agent_secret| {
                authorize_agent_secret_permission(
                    auth,
                    environment,
                    Some(&agent_secret.path.0),
                    EnvironmentAgentSecretVerb::View,
                )
                .is_ok()
            })
            .collect())
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
    ) -> Result<(AgentSecret, EnvironmentOwnerPattern), AgentSecretError> {
        let record = self
            .agent_secret_repo
            .get_by_id(agent_secret_id.0)
            .await?
            .ok_or(AgentSecretError::AgentSecretNotFound(agent_secret_id))?;

        let owner = environment_owner_from_agent_secret(&record);
        let agent_secret: AgentSecret = record.agent_secret.try_into()?;

        authorize_agent_secret_permission_for_owner(
            auth,
            owner.clone(),
            Some(&agent_secret.path),
            EnvironmentAgentSecretVerb::View,
        )
        .map_err(|_| AgentSecretError::AgentSecretNotFound(agent_secret_id))?;

        Ok((agent_secret, owner))
    }
}
