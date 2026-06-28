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
use golem_common::schema::SchemaGraph;
use golem_common::schema::agent::reachable_defs;
use golem_common::schema::metadata::TypeId;
use golem_common::schema::schema_type::SchemaType;
use golem_common::schema::validation::{validate_graph, validate_value};
use golem_common::{SafeDisplay, error_forwarding};
use golem_service_base::model::agent_secret::AgentSecret;
use golem_service_base::model::auth::{AuthCtx, AuthorizationError};
use std::collections::HashSet;
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
    key: Option<&CanonicalAgentSecretPath>,
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

fn resolve_schema_ref<'a>(graph: &'a SchemaGraph, mut ty: &'a SchemaType) -> &'a SchemaType {
    let mut seen = std::collections::HashSet::new();
    while let SchemaType::Ref { id, .. } = ty {
        if !seen.insert(id.clone()) {
            break;
        }
        match graph.lookup(id) {
            Some(def) => ty = &def.body,
            None => break,
        }
    }
    ty
}

fn validate_agent_secret_value(
    secret_type_graph: &SchemaGraph,
    secret_value: &golem_common::schema::SchemaValue,
) -> Result<(), AgentSecretError> {
    validate_plaintext_agent_secret_type(secret_type_graph)?;

    validate_value(
        secret_type_graph,
        resolve_schema_ref(secret_type_graph, &secret_type_graph.root),
        secret_value,
    )
    .map_err(
        |errors| AgentSecretError::AgentSecretValueDoesNotMatchType {
            errors: errors.iter().map(|e| e.to_string()).collect(),
        },
    )
}

fn validate_plaintext_agent_secret_type(
    secret_type_graph: &SchemaGraph,
) -> Result<(), AgentSecretError> {
    if schema_contains_host_managed_capability(secret_type_graph) {
        return Err(AgentSecretError::AgentSecretValueDoesNotMatchType {
            errors: vec![
                "agent secret types are stored as plaintext payload schemas; capability types secret<T> and quota-token are not allowed"
                    .to_string(),
            ],
        });
    }

    Ok(())
}

pub(crate) fn schema_contains_host_managed_capability(graph: &SchemaGraph) -> bool {
    let mut visiting = HashSet::new();
    schema_type_contains_host_managed_capability(graph, &graph.root, &mut visiting)
        || graph.defs.iter().any(|def| {
            schema_type_contains_host_managed_capability(graph, &def.body, &mut visiting)
        })
}

fn schema_type_contains_host_managed_capability(
    graph: &SchemaGraph,
    ty: &SchemaType,
    visiting: &mut HashSet<TypeId>,
) -> bool {
    match ty {
        SchemaType::Ref { id, .. } => {
            if !visiting.insert(id.clone()) {
                return false;
            }
            let contains = graph.lookup(id).is_some_and(|def| {
                schema_type_contains_host_managed_capability(graph, &def.body, visiting)
            });
            visiting.remove(id);
            contains
        }
        SchemaType::Secret { .. } | SchemaType::QuotaToken { .. } => true,
        SchemaType::Record { fields, .. } => fields.iter().any(|field| {
            schema_type_contains_host_managed_capability(graph, &field.body, visiting)
        }),
        SchemaType::Variant { cases, .. } => cases.iter().any(|case| {
            case.payload.as_ref().is_some_and(|payload| {
                schema_type_contains_host_managed_capability(graph, payload, visiting)
            })
        }),
        SchemaType::Tuple { elements, .. } => elements
            .iter()
            .any(|element| schema_type_contains_host_managed_capability(graph, element, visiting)),
        SchemaType::List { element, .. }
        | SchemaType::FixedList { element, .. }
        | SchemaType::Option { inner: element, .. } => {
            schema_type_contains_host_managed_capability(graph, element, visiting)
        }
        SchemaType::Map { key, value, .. } => {
            schema_type_contains_host_managed_capability(graph, key, visiting)
                || schema_type_contains_host_managed_capability(graph, value, visiting)
        }
        SchemaType::Result { spec, .. } => {
            spec.ok
                .as_ref()
                .is_some_and(|ok| schema_type_contains_host_managed_capability(graph, ok, visiting))
                || spec.err.as_ref().is_some_and(|err| {
                    schema_type_contains_host_managed_capability(graph, err, visiting)
                })
        }
        SchemaType::Union { spec, .. } => spec.branches.iter().any(|branch| {
            schema_type_contains_host_managed_capability(graph, &branch.body, visiting)
        }),
        SchemaType::Future { inner, .. } | SchemaType::Stream { inner, .. } => {
            inner.as_ref().is_some_and(|inner| {
                schema_type_contains_host_managed_capability(graph, inner, visiting)
            })
        }
        SchemaType::Bool { .. }
        | SchemaType::S8 { .. }
        | SchemaType::S16 { .. }
        | SchemaType::S32 { .. }
        | SchemaType::S64 { .. }
        | SchemaType::U8 { .. }
        | SchemaType::U16 { .. }
        | SchemaType::U32 { .. }
        | SchemaType::U64 { .. }
        | SchemaType::F32 { .. }
        | SchemaType::F64 { .. }
        | SchemaType::Char { .. }
        | SchemaType::String { .. }
        | SchemaType::Enum { .. }
        | SchemaType::Flags { .. }
        | SchemaType::Text { .. }
        | SchemaType::Binary { .. }
        | SchemaType::Path { .. }
        | SchemaType::Url { .. }
        | SchemaType::Datetime { .. }
        | SchemaType::Duration { .. }
        | SchemaType::Quantity { .. } => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::schema::graph::SchemaTypeDef;
    use golem_common::schema::schema_type::{QuotaTokenSpec, SecretSpec};
    use test_r::test;

    #[test]
    fn agent_secret_storage_accepts_plaintext_payload_schema() {
        let schema = SchemaGraph::anonymous(SchemaType::string());

        validate_plaintext_agent_secret_type(&schema).unwrap();
    }

    #[test]
    fn agent_secret_storage_rejects_secret_capability_schema() {
        let schema = SchemaGraph::anonymous(SchemaType::secret(SecretSpec {
            inner: Box::new(SchemaType::string()),
            category: None,
        }));

        assert!(validate_plaintext_agent_secret_type(&schema).is_err());
    }

    #[test]
    fn agent_secret_storage_rejects_nested_quota_token_capability_schema() {
        let schema = SchemaGraph::anonymous(SchemaType::option(SchemaType::quota_token(
            QuotaTokenSpec {
                resource_name: Some("credits".to_string()),
            },
        )));

        assert!(validate_plaintext_agent_secret_type(&schema).is_err());
    }

    #[test]
    fn agent_secret_storage_rejects_unreachable_secret_capability_def() {
        let schema = SchemaGraph {
            defs: vec![SchemaTypeDef {
                id: TypeId::new("api-key-secret"),
                name: None,
                body: SchemaType::secret(SecretSpec {
                    inner: Box::new(SchemaType::string()),
                    category: None,
                }),
            }],
            root: SchemaType::string(),
        };

        assert!(validate_plaintext_agent_secret_type(&schema).is_err());
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
        let secret_type_graph = SchemaGraph {
            defs: reachable_defs(&secret_type_graph, &secret_type_graph.root),
            root: secret_type_graph.root,
        };
        validate_plaintext_agent_secret_type(&secret_type_graph)?;
        let secret_value = data.secret_value;
        if let Some(sv) = &secret_value {
            validate_value(
                &secret_type_graph,
                resolve_schema_ref(&secret_type_graph, &secret_type_graph.root),
                sv,
            )
            .map_err(
                |errors| AgentSecretError::AgentSecretValueDoesNotMatchType {
                    errors: errors.iter().map(|e| e.to_string()).collect(),
                },
            )?;
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
                // The new value is schema-native; validate it against the
                // stored secret's inner type before applying.
                validate_agent_secret_value(&agent_secret.secret_type, &new_secret_value)?;
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
        agent_secret.secret_value = None;

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
                    Some(&agent_secret.path),
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

    pub async fn get_revision_unchecked(
        &self,
        environment_id: EnvironmentId,
        agent_secret_id: AgentSecretId,
        path: CanonicalAgentSecretPath,
        revision: AgentSecretRevision,
    ) -> Result<Option<AgentSecret>, AgentSecretError> {
        self.agent_secret_repo
            .get_revision(environment_id.0, agent_secret_id, path.0, revision)
            .await?
            .map(TryInto::try_into)
            .transpose()
            .map_err(Into::into)
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
