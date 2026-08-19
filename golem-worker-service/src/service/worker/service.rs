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

use super::WorkerResult;
use super::{
    ConnectWorkerStream, InvocationRequestStream, InvocationResponseStream, WorkerClient,
    WorkerServiceError,
};
use crate::api::agents::{
    AgentInvocationMode, AgentInvocationRequest, AgentInvocationResult, CreateAgentRequest,
    CreateAgentResponse,
};
use crate::service::agent_resolution_cache::AgentResolutionCache;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::component::ComponentService;
use crate::service::limit::LimitService;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use golem_api_grpc::proto::golem::worker::{
    InvocationContext, InvocationRequest, InvocationStart, PublicInvocationStart,
    invocation_request,
};
use golem_common::model::AgentInvocationOutput;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{
    AgentMode, AgentTypeName, GolemUserPrincipal, InvocationFreshnessDisposition, ParsedAgentId,
    Principal, ephemeral_invocation_phantom_id,
};
use golem_common::model::application::ApplicationName;
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use golem_common::model::card::{
    AgentMethodName, AgentResourcePattern, AgentVerb, ClassPermissionTarget, PermissionTarget,
    StoredCard,
};
use golem_common::model::component::{
    CanonicalFilePath, ComponentId, ComponentName, ComponentRevision, PluginPriority,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::oplog::OplogCursor;
use golem_common::model::oplog::OplogIndex;
use golem_common::model::worker::AgentConfigEntryDto;
use golem_common::model::worker::AgentUpdateMode;
use golem_common::model::worker::{AgentMetadataDto, RevertWorkerTarget};
use golem_common::model::{AgentFilter, AgentFingerprint, AgentId, IdempotencyKey, ScanCursor};
use golem_common::schema::json_input_schema_value_to_typed_schema_value;
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::{
    ResultValuePayload, SchemaType, SchemaValue, TypedSchemaValue, UnionValuePayload,
    VariantValuePayload,
};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc};

fn build_public_agent_id(
    component_id: ComponentId,
    agent_type_name: AgentTypeName,
    constructor_parameters: TypedSchemaValue,
    phantom_id: Option<uuid::Uuid>,
    agent_mode: AgentMode,
) -> WorkerResult<AgentId> {
    let agent_id = ParsedAgentId::new_auto_phantom(
        agent_type_name,
        constructor_parameters,
        phantom_id,
        agent_mode,
    )
    .map_err(|err| WorkerServiceError::TypeChecker(format!("Agent ID formatting error: {err}")))?;

    Ok(AgentId {
        component_id,
        agent_id: agent_id.to_string(),
    })
}

fn build_public_invocation_agent_id(
    component_id: ComponentId,
    agent_type_name: AgentTypeName,
    constructor_parameters: TypedSchemaValue,
    phantom_id: Option<uuid::Uuid>,
) -> WorkerResult<AgentId> {
    let agent_id = ParsedAgentId::try_new(agent_type_name, constructor_parameters, phantom_id)
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!("Agent ID formatting error: {err}"))
        })?;

    Ok(AgentId {
        component_id,
        agent_id: agent_id.to_string(),
    })
}

fn validate_one_shot_invocation_is_stream_free(
    component: &Component,
    agent_id: &AgentId,
    method_name: &str,
    method_parameters: &golem_api_grpc::proto::golem::schema::SchemaValue,
) -> WorkerResult<()> {
    let parsed_agent_id = ParsedAgentId::parse(&agent_id.agent_id, &component.metadata)
        .map_err(WorkerServiceError::TypeChecker)?;
    let agent_type = component
        .metadata
        .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
        .ok_or_else(|| {
            WorkerServiceError::TypeChecker(format!(
                "Agent type '{}' not found",
                parsed_agent_id.agent_type
            ))
        })?;
    let method = agent_type
        .methods
        .iter()
        .find(|method| method.name == method_name)
        .ok_or_else(|| {
            WorkerServiceError::TypeChecker(format!(
                "Agent method '{method_name}' not found in agent type '{}'",
                agent_type.type_name
            ))
        })?;
    let input = SchemaValue::try_from(method_parameters.clone())
        .map_err(WorkerServiceError::TypeChecker)?;
    method
        .validate_input(&agent_type.schema, &input)
        .map_err(|error| {
            WorkerServiceError::TypeChecker(format!(
                "Invalid input for agent method '{method_name}': {error}"
            ))
        })?;
    if method.uses_streams(&agent_type.schema) {
        Err(WorkerServiceError::TypeChecker(
            "Streaming agent methods require an attached invocation session".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn decode_public_session_schema_value(
    value: golem_api_grpc::proto::golem::schema::SchemaValue,
) -> Result<SchemaValue, String> {
    decode_public_schema_value(value, true)
}

pub(crate) fn validate_public_session_schema_value(
    value: &golem_api_grpc::proto::golem::schema::SchemaValue,
) -> Result<(), String> {
    decode_public_session_schema_value(value.clone()).map(|_| ())
}

fn decode_public_schema_value(
    value: golem_api_grpc::proto::golem::schema::SchemaValue,
    allow_stream_references: bool,
) -> Result<SchemaValue, String> {
    use golem_api_grpc::proto::golem::schema::{result_value, schema_value};

    let value = value
        .value
        .ok_or_else(|| "Missing field: SchemaValue.value".to_string())?;
    match value {
        schema_value::Value::RecordValue(record) => Ok(SchemaValue::Record {
            fields: record
                .fields
                .into_iter()
                .map(|value| decode_public_schema_value(value, allow_stream_references))
                .collect::<Result<_, _>>()?,
        }),
        schema_value::Value::VariantValue(variant) => {
            Ok(SchemaValue::Variant(VariantValuePayload {
                case: variant.case,
                payload: variant
                    .payload
                    .map(|payload| {
                        decode_public_schema_value(*payload, allow_stream_references).map(Box::new)
                    })
                    .transpose()?,
            }))
        }
        schema_value::Value::TupleValue(tuple) => Ok(SchemaValue::Tuple {
            elements: tuple
                .elements
                .into_iter()
                .map(|value| decode_public_schema_value(value, allow_stream_references))
                .collect::<Result<_, _>>()?,
        }),
        schema_value::Value::ListValue(list) => Ok(SchemaValue::List {
            elements: list
                .elements
                .into_iter()
                .map(|value| decode_public_schema_value(value, allow_stream_references))
                .collect::<Result<_, _>>()?,
        }),
        schema_value::Value::FixedListValue(list) => Ok(SchemaValue::FixedList {
            elements: list
                .elements
                .into_iter()
                .map(|value| decode_public_schema_value(value, allow_stream_references))
                .collect::<Result<_, _>>()?,
        }),
        schema_value::Value::MapValue(map) => Ok(SchemaValue::Map {
            entries: map
                .entries
                .into_iter()
                .map(|entry| {
                    let key = entry
                        .key
                        .ok_or_else(|| "Missing field: MapEntry.key".to_string())?;
                    let value = entry
                        .value
                        .ok_or_else(|| "Missing field: MapEntry.value".to_string())?;
                    Ok((
                        decode_public_schema_value(key, allow_stream_references)?,
                        decode_public_schema_value(value, allow_stream_references)?,
                    ))
                })
                .collect::<Result<_, String>>()?,
        }),
        schema_value::Value::OptionValue(option) => Ok(SchemaValue::Option {
            inner: option
                .inner
                .map(|inner| {
                    decode_public_schema_value(*inner, allow_stream_references).map(Box::new)
                })
                .transpose()?,
        }),
        schema_value::Value::ResultValue(result) => match result.result {
            Some(result_value::Result::Ok(value)) => {
                Ok(SchemaValue::Result(ResultValuePayload::Ok {
                    value: Some(Box::new(decode_public_schema_value(
                        *value,
                        allow_stream_references,
                    )?)),
                }))
            }
            Some(result_value::Result::OkUnit(_)) => {
                Ok(SchemaValue::Result(ResultValuePayload::Ok { value: None }))
            }
            Some(result_value::Result::Err(value)) => {
                Ok(SchemaValue::Result(ResultValuePayload::Err {
                    value: Some(Box::new(decode_public_schema_value(
                        *value,
                        allow_stream_references,
                    )?)),
                }))
            }
            Some(result_value::Result::ErrUnit(_)) => {
                Ok(SchemaValue::Result(ResultValuePayload::Err { value: None }))
            }
            None => Err("Missing field: ResultValue.result".to_string()),
        },
        schema_value::Value::UnionValue(union) => {
            let body = union
                .body
                .ok_or_else(|| "Missing field: UnionValue.body".to_string())?;
            Ok(SchemaValue::Union(UnionValuePayload {
                tag: union.tag,
                body: Box::new(decode_public_schema_value(*body, allow_stream_references)?),
            }))
        }
        schema_value::Value::SecretValue(_) | schema_value::Value::QuotaTokenValue(_) => {
            Err("host-managed capability values cannot cross the public boundary".to_string())
        }
        schema_value::Value::StreamReference(reference) if allow_stream_references => Ok(
            SchemaValue::Stream(SchemaValueStream::from_host_endpoint(reference.stream_id)),
        ),
        schema_value::Value::StreamReference(reference) => Err(format!(
            "stream reference {} is not valid in constructor parameters",
            reference.stream_id
        )),
        value => {
            golem_api_grpc::proto::golem::schema::SchemaValue { value: Some(value) }.try_into()
        }
    }
}

fn normalize_agent_invocation_identity(
    component: &Component,
    agent_id: &AgentId,
    idempotency_key: Option<IdempotencyKey>,
    allow_derived_ephemeral_phantom: bool,
    observation_only: bool,
    freshness_disposition: InvocationFreshnessDisposition,
) -> WorkerResult<(AgentId, IdempotencyKey, InvocationFreshnessDisposition)> {
    let key_was_supplied = idempotency_key.is_some();
    let idempotency_key = idempotency_key.unwrap_or_else(IdempotencyKey::fresh);

    let Ok(parsed_agent_id) = ParsedAgentId::parse(&agent_id.agent_id, &component.metadata) else {
        return Ok((agent_id.clone(), idempotency_key, freshness_disposition));
    };
    let Some(agent_type) = component
        .metadata
        .find_agent_type_by_name_ref(&parsed_agent_id.agent_type)
    else {
        return Ok((agent_id.clone(), idempotency_key, freshness_disposition));
    };

    if agent_type.mode != AgentMode::Ephemeral {
        return Ok((agent_id.clone(), idempotency_key, freshness_disposition));
    }

    if parsed_agent_id.phantom_id.is_some() {
        if observation_only {
            return Ok((
                agent_id.clone(),
                idempotency_key,
                InvocationFreshnessDisposition::MayExist,
            ));
        }
        if !allow_derived_ephemeral_phantom {
            crate::metrics::record_ephemeral_explicit_phantom_invocation_rejection();
            return Err(WorkerServiceError::TypeChecker(
                "An ephemeral invocation cannot select a phantom ID; use the agent ID returned by an invocation only for observation and control operations"
                    .to_string(),
            ));
        }
        // This capability is only valid when a trusted internal caller forwards
        // both an invocation-derived phantom and the supplied key that derived it.
        if !key_was_supplied
            || parsed_agent_id.phantom_id != Some(ephemeral_invocation_phantom_id(&idempotency_key))
        {
            crate::metrics::record_ephemeral_derived_phantom_mismatch_rejection();
            return Err(WorkerServiceError::TypeChecker(
                "The ephemeral invocation phantom ID does not match the identity derived from the invocation's idempotency key"
                    .to_string(),
            ));
        }
    }

    let parsed_agent_id =
        ParsedAgentId::try_new(parsed_agent_id.agent_type, parsed_agent_id.parameters, None)
            .and_then(|logical_agent_id| {
                logical_agent_id.with_ephemeral_invocation_phantom(&idempotency_key)
            })
            .map_err(|err| {
                WorkerServiceError::TypeChecker(format!("Agent ID formatting error: {err}"))
            })?;
    let final_agent_id = AgentId::from_agent_id(agent_id.component_id, &parsed_agent_id)
        .map_err(WorkerServiceError::TypeChecker)?;
    let freshness_disposition =
        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh || !key_was_supplied
        {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };

    Ok((final_agent_id, idempotency_key, freshness_disposition))
}

fn agent_verb_for_invocation_mode(mode: i32) -> AgentVerb {
    if mode == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32 {
        AgentVerb::View
    } else {
        AgentVerb::Invoke
    }
}

fn authorize_agent_permission(
    auth_ctx: &AuthCtx,
    component: &Component,
    agent_id: &AgentId,
    verb: AgentVerb,
    resource: AgentResourcePattern,
) -> WorkerResult<()> {
    auth_ctx
        .authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
            owner: AgentOwnerPattern::Agent {
                account: component.account_email.clone(),
                application: component.application_name.clone(),
                environment: component.environment_name.clone(),
                component: component.component_name.clone(),
                agent: AgentOwnerLeafPattern::Agent(agent_id.agent_id.clone()),
            },
            verb: Some(verb),
            resource,
        }))
        .map_err(AuthServiceError::Unauthorized)?;

    Ok(())
}

fn authorize_component_agents_permission(
    auth_ctx: &AuthCtx,
    component: &Component,
    verb: AgentVerb,
    resource: AgentResourcePattern,
) -> WorkerResult<()> {
    auth_ctx
        .authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
            owner: AgentOwnerPattern::ComponentAgents {
                account: component.account_email.clone(),
                application: component.application_name.clone(),
                environment: component.environment_name.clone(),
                component: component.component_name.clone(),
            },
            verb: Some(verb),
            resource,
        }))
        .map_err(AuthServiceError::Unauthorized)?;

    Ok(())
}

pub struct WorkerService {
    component_service: Arc<dyn ComponentService>,
    _auth_service: Arc<dyn AuthService>,
    limit_service: Arc<dyn LimitService>,
    worker_client: Arc<dyn WorkerClient>,
    agent_resolution_cache: Arc<AgentResolutionCache>,
}

impl WorkerService {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        auth_service: Arc<dyn AuthService>,
        limit_service: Arc<dyn LimitService>,
        worker_client: Arc<dyn WorkerClient>,
        agent_resolution_cache: Arc<AgentResolutionCache>,
    ) -> Self {
        Self {
            component_service,
            _auth_service: auth_service,
            limit_service,
            worker_client,
            agent_resolution_cache,
        }
    }

    pub async fn create(
        &self,
        agent_id: &AgentId,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        principal: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<(ComponentRevision, AgentFingerprint)> {
        let component = self
            .component_service
            .get_current_by_id_uncached(agent_id.component_id)
            .await?;

        self.create_with_component(
            agent_id,
            component,
            environment_variables,
            config,
            ignore_already_existing,
            auth_ctx,
            invocation_context,
            principal,
        )
        .await
    }

    // Like create, but skip fetching the component.
    pub async fn create_with_component(
        &self,
        agent_id: &AgentId,
        component: Component,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        principal: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<(ComponentRevision, AgentFingerprint)> {
        assert!(component.id == agent_id.component_id);

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::Invoke,
            AgentResourcePattern::Any,
        )?;

        let (_, fingerprint) = self
            .worker_client
            .create(
                agent_id,
                environment_variables,
                config,
                ignore_already_existing,
                component.account_id,
                component.environment_id,
                auth_ctx,
                invocation_context,
                principal,
            )
            .await?;

        Ok((component.revision, fingerprint))
    }

    pub async fn connect(
        &self,
        agent_id: &AgentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::View,
            AgentResourcePattern::Any,
        )?;

        let stream = self
            .worker_client
            .connect(
                agent_id,
                component.environment_id,
                component.account_id,
                auth_ctx,
            )
            .await?;

        self.limit_service
            .update_worker_connection_limit(component.account_id, agent_id, true)
            .await?;

        Ok(ConnectWorkerStream::new(
            stream,
            agent_id.clone(),
            component.account_id,
            self.limit_service.clone(),
        ))
    }

    pub async fn delete(&self, agent_id: &AgentId, auth_ctx: AuthCtx) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::Delete,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .delete(agent_id, component.environment_id, auth_ctx)
            .await?;

        Ok(())
    }

    pub async fn complete_promise(
        &self,
        agent_id: &AgentId,
        oplog_id: u64,
        data: Vec<u8>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::Invoke,
            AgentResourcePattern::Any,
        )?;

        let result = self
            .worker_client
            .complete_promise(agent_id, oplog_id, data, component.environment_id, auth_ctx)
            .await?;

        Ok(result)
    }

    pub async fn interrupt(
        &self,
        agent_id: &AgentId,
        recover_immediately: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::Interrupt,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .interrupt(
                agent_id,
                recover_immediately,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn get_metadata(
        &self,
        agent_id: &AgentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<AgentMetadataDto> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::View,
            AgentResourcePattern::Any,
        )?;

        let result = self
            .worker_client
            .get_metadata(agent_id, component.environment_id, auth_ctx)
            .await?;

        Ok(result)
    }

    pub async fn find_metadata(
        &self,
        component_id: ComponentId,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<AgentMetadataDto>)> {
        let component = self
            .component_service
            .get_current_by_id(component_id)
            .await?;

        authorize_component_agents_permission(
            &auth_ctx,
            &component,
            AgentVerb::View,
            AgentResourcePattern::Any,
        )?;

        let result = self
            .worker_client
            .find_metadata(
                component_id,
                filter,
                cursor,
                count,
                precise,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn resume(
        &self,
        agent_id: &AgentId,
        force: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::Resume,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .resume(agent_id, force, component.environment_id, auth_ctx)
            .await?;

        Ok(())
    }

    pub async fn update(
        &self,
        agent_id: &AgentId,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::UpdateRevision,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .update(
                agent_id,
                update_mode,
                target_revision,
                disable_wakeup,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn get_oplog(
        &self,
        agent_id: &AgentId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::View,
            AgentResourcePattern::OplogIndex(from_oplog_index.into()),
        )?;

        let result = self
            .worker_client
            .get_oplog(
                agent_id,
                from_oplog_index,
                cursor,
                count,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn search_oplog(
        &self,
        agent_id: &AgentId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::View,
            AgentResourcePattern::Any,
        )?;

        let result = self
            .worker_client
            .search_oplog(
                agent_id,
                cursor,
                count,
                query,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(result)
    }

    pub async fn get_file_system_node(
        &self,
        agent_id: &AgentId,
        path: CanonicalFilePath,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::View,
            AgentResourcePattern::Any,
        )?;

        let nodes = self
            .worker_client
            .get_file_system_node(
                agent_id,
                path,
                component.environment_id,
                component.account_id,
                auth_ctx,
            )
            .await?;

        Ok(nodes)
    }

    pub async fn get_agent_wallet(
        &self,
        agent_id: &AgentId,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<StoredCard>> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::View,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .get_agent_wallet(
                agent_id,
                component.environment_id,
                component.account_id,
                auth_ctx,
            )
            .await
    }

    pub async fn get_file_contents(
        &self,
        agent_id: &AgentId,
        path: CanonicalFilePath,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::View,
            AgentResourcePattern::Any,
        )?;

        let contents_stream = self
            .worker_client
            .get_file_contents(
                agent_id,
                path,
                component.environment_id,
                component.account_id,
                auth_ctx,
            )
            .await?;

        Ok(contents_stream)
    }

    pub async fn activate_plugin(
        &self,
        agent_id: &AgentId,
        plugin_priority: PluginPriority,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::ActivatePlugin,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .activate_plugin(
                agent_id,
                plugin_priority,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn deactivate_plugin(
        &self,
        agent_id: &AgentId,
        plugin_priority: PluginPriority,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::DeactivatePlugin,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .deactivate_plugin(
                agent_id,
                plugin_priority,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn fork_worker(
        &self,
        source_agent_id: &AgentId,
        target_agent_id: &AgentId,
        oplog_index_cut_off: OplogIndex,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_current_by_id(source_agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            source_agent_id,
            AgentVerb::Fork,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .fork_worker(
                source_agent_id,
                target_agent_id,
                oplog_index_cut_off,
                component.environment_id,
                component.account_id,
                component.account_email,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn revert_worker(
        &self,
        agent_id: &AgentId,
        target: RevertWorkerTarget,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::Revert,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .revert_worker(agent_id, target, component.environment_id, auth_ctx)
            .await?;

        Ok(())
    }

    pub async fn cancel_invocation(
        &self,
        agent_id: &AgentId,
        idempotency_key: &IdempotencyKey,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;

        authorize_agent_permission(
            &auth_ctx,
            &component,
            agent_id,
            AgentVerb::CancelInvocation,
            AgentResourcePattern::Any,
        )?;

        let canceled = self
            .worker_client
            .cancel_invocation(
                agent_id,
                idempotency_key,
                component.environment_id,
                auth_ctx,
            )
            .await?;

        Ok(canceled)
    }

    pub async fn process_oplog_entries(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        component_revision: ComponentRevision,
        idempotency_key: IdempotencyKey,
        _account_id: AccountId,
        config: std::collections::HashMap<String, String>,
        metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let component = self
            .component_service
            .get_revision(target_agent_id.component_id, component_revision)
            .await?;
        authorize_agent_permission(
            &auth_ctx,
            &component,
            target_agent_id,
            AgentVerb::Invoke,
            AgentResourcePattern::Any,
        )?;

        self.worker_client
            .process_oplog_entries(
                target_agent_id,
                environment_id,
                component_revision,
                idempotency_key,
                component.account_id,
                config,
                metadata,
                first_entry_index,
                entries,
                auth_ctx,
            )
            .await
    }

    pub async fn invoke_agent(
        &self,
        agent_id: &AgentId,
        method_name: Option<String>,
        method_parameters: Option<golem_api_grpc::proto::golem::schema::SchemaValue>,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: Option<InvocationContext>,
        allow_derived_ephemeral_phantom: bool,
        freshness_disposition: InvocationFreshnessDisposition,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: AuthCtx,
        principal: golem_api_grpc::proto::golem::component::Principal,
    ) -> WorkerResult<AgentInvocationOutput> {
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;
        let environment_id = component.environment_id;
        let account_id = component.account_id;
        self.dispatch_agent_invocation(
            &component,
            agent_id,
            method_name.clone(),
            method_parameters,
            mode,
            schedule_at,
            idempotency_key,
            invocation_context,
            allow_derived_ephemeral_phantom,
            freshness_disposition,
            config,
            environment_id,
            account_id,
            auth_ctx.clone(),
            principal,
            |final_agent_id| {
                authorize_agent_permission(
                    &auth_ctx,
                    &component,
                    final_agent_id,
                    agent_verb_for_invocation_mode(mode),
                    method_name
                        .as_ref()
                        .map(|method_name| {
                            AgentResourcePattern::Method(AgentMethodName(method_name.clone()))
                        })
                        .unwrap_or(AgentResourcePattern::Any),
                )
            },
        )
        .await
    }

    pub async fn invoke_agent_session(
        &self,
        mut start: InvocationStart,
        tail: InvocationRequestStream,
        allow_derived_ephemeral_phantom: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<InvocationResponseStream> {
        let agent_id: AgentId = start
            .agent_id
            .clone()
            .ok_or_else(|| WorkerServiceError::TypeChecker("agent_id not found".to_string()))?
            .try_into()
            .map_err(WorkerServiceError::TypeChecker)?;
        let component = self
            .component_service
            .get_current_by_id(agent_id.component_id)
            .await?;
        let observation_only =
            start.mode() == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup;
        let freshness_disposition = if start.freshness_disposition()
            == golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
        {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };
        let (agent_id, idempotency_key, mut freshness_disposition) =
            normalize_agent_invocation_identity(
                &component,
                &agent_id,
                start.idempotency_key.clone().map(Into::into),
                allow_derived_ephemeral_phantom,
                observation_only,
                freshness_disposition,
            )?;
        if observation_only {
            freshness_disposition = InvocationFreshnessDisposition::MayExist;
        }
        authorize_agent_permission(
            &auth_ctx,
            &component,
            &agent_id,
            agent_verb_for_invocation_mode(start.mode),
            start
                .method_name
                .as_ref()
                .map(|method_name| {
                    AgentResourcePattern::Method(AgentMethodName(method_name.clone()))
                })
                .unwrap_or(AgentResourcePattern::Any),
        )?;

        start.agent_id = Some(agent_id.clone().into());
        start.idempotency_key = Some(idempotency_key.into());
        start.environment_id = Some(component.environment_id.into());
        start.component_owner_account_id = Some(component.account_id.into());
        start.freshness_disposition = match freshness_disposition {
            InvocationFreshnessDisposition::MayExist => {
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32
            }
            InvocationFreshnessDisposition::KnownFresh => {
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
                    as i32
            }
        };
        let request = stream::once(async move {
            InvocationRequest {
                request: Some(invocation_request::Request::Start(start)),
            }
        })
        .chain(tail);
        self.worker_client
            .invoke_agent_session(&agent_id, Box::pin(request))
            .await
    }

    pub async fn invoke_public_agent_session(
        &self,
        start: PublicInvocationStart,
        tail: InvocationRequestStream,
        auth: AuthCtx,
    ) -> WorkerResult<InvocationResponseStream> {
        let app_name = ApplicationName::try_from(start.application_name)
            .map_err(WorkerServiceError::TypeChecker)?;
        let env_name = EnvironmentName::try_from(start.environment_name)
            .map_err(WorkerServiceError::TypeChecker)?;
        let agent_type_name = AgentTypeName(start.agent_type_name);
        let method_name = start.method_name;
        let constructor_parameters = decode_public_schema_value(
            start.constructor_parameters.ok_or_else(|| {
                WorkerServiceError::TypeChecker(
                    "public invocation has no constructor parameters".to_string(),
                )
            })?,
            false,
        )
        .map_err(|error| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters cannot cross the public boundary: {error}"
            ))
        })?;
        let proto_method_parameters = start.method_parameters.ok_or_else(|| {
            WorkerServiceError::TypeChecker(
                "public invocation has no method parameters".to_string(),
            )
        })?;
        let method_parameters = decode_public_session_schema_value(proto_method_parameters.clone())
            .map_err(|error| {
                WorkerServiceError::TypeChecker(format!(
                    "Agent method parameters cannot cross the public boundary: {error}"
                ))
            })?;
        let phantom_id = start
            .phantom_id
            .map(TryInto::try_into)
            .transpose()
            .map_err(|error| {
                WorkerServiceError::TypeChecker(format!("Invalid phantom id: {error}"))
            })?;
        let idempotency_key: IdempotencyKey = start
            .idempotency_key
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker(
                    "public invocation requires an idempotency key".to_string(),
                )
            })?
            .into();
        let config = start
            .config
            .into_iter()
            .map(AgentConfigEntryDto::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                WorkerServiceError::TypeChecker(format!(
                    "Agent configuration cannot cross the public boundary: {error}"
                ))
            })?;

        let resolved = self
            .agent_resolution_cache
            .resolve(&app_name, &env_name, &agent_type_name, None, &auth)
            .await?;
        let registered_agent_type = &resolved.registered_agent_type;
        let environment_id = resolved.environment_id;
        let component_id = registered_agent_type.implemented_by.component_id;
        let agent_type = &registered_agent_type.agent_type;

        let constructor_parameters = json_input_schema_value_to_typed_schema_value(
            constructor_parameters,
            &agent_type.schema,
            &agent_type.constructor.input_schema,
        )
        .map_err(|error| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters type error: {error}"
            ))
        })?;
        let agent_id = build_public_invocation_agent_id(
            component_id,
            agent_type_name.clone(),
            constructor_parameters,
            phantom_id,
        )?;
        let component = self
            .component_service
            .get_revision(
                component_id,
                registered_agent_type.implemented_by.component_revision,
            )
            .await?;
        let component_owner_account_id = registered_agent_type.implemented_by.account_id;
        let component_name = registered_agent_type.implemented_by.component_name.clone();
        let component_owner_account_email =
            registered_agent_type.implemented_by.account_email.clone();
        let (agent_id, idempotency_key, freshness_disposition, observation_only) = self
            .prepare_agent_invocation_identity(
                &component,
                &agent_id,
                Some(idempotency_key),
                false,
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                InvocationFreshnessDisposition::MayExist,
                |final_agent_id| {
                    auth.authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
                        owner: AgentOwnerPattern::Agent {
                            account: component_owner_account_email,
                            application: app_name,
                            environment: env_name,
                            component: ComponentName(component_name),
                            agent: AgentOwnerLeafPattern::Agent(final_agent_id.agent_id.clone()),
                        },
                        verb: Some(AgentVerb::Invoke),
                        resource: AgentResourcePattern::Method(AgentMethodName(
                            method_name.clone(),
                        )),
                    }))
                    .map_err(AuthServiceError::from)
                    .map_err(WorkerServiceError::from)
                },
            )?;
        debug_assert!(!observation_only);
        let invocation_component = self
            .component_for_invocation(
                &component,
                &agent_id,
                environment_id,
                &auth,
                freshness_disposition,
            )
            .await?;
        let invocation_agent_type = invocation_component
            .metadata
            .find_agent_type_by_name_ref(&agent_type_name)
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent type {agent_type_name} not found in component metadata at revision {}",
                    invocation_component.revision
                ))
            })?;
        let method = invocation_agent_type
            .methods
            .iter()
            .find(|method| method.name == method_name)
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent method {method_name} not found in agent type {agent_type_name}"
                ))
            })?;
        let _validated_method_parameters = json_input_schema_value_to_typed_schema_value(
            method_parameters,
            &invocation_agent_type.schema,
            &method.input_schema,
        )
        .map_err(|error| {
            WorkerServiceError::TypeChecker(format!("Agent method parameters type error: {error}"))
        })?;
        let principal: golem_api_grpc::proto::golem::component::Principal =
            Principal::GolemUser(GolemUserPrincipal {
                account_id: auth.account_id(),
            })
            .into();
        let trusted_start = InvocationStart {
            agent_id: Some(agent_id.clone().into()),
            method_name: Some(method_name),
            input: Some(proto_method_parameters),
            idempotency_key: Some(idempotency_key.into()),
            context: None,
            auth_ctx: Some(auth.into()),
            principal: Some(principal),
            environment_id: Some(environment_id.into()),
            config: config.into_iter().map(Into::into).collect(),
            component_owner_account_id: Some(component_owner_account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            schedule_at: None,
            freshness_disposition: match freshness_disposition {
                InvocationFreshnessDisposition::MayExist => {
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                        as i32
                }
                InvocationFreshnessDisposition::KnownFresh => {
                    golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
                        as i32
                }
            },
        };
        let request = stream::once(async move {
            InvocationRequest {
                request: Some(invocation_request::Request::Start(trusted_start)),
            }
        })
        .chain(tail);
        self.worker_client
            .invoke_agent_session(&agent_id, Box::pin(request))
            .await
    }

    /// Shared invocation-dispatch core: normalizes the invocation identity,
    /// authorizes against the final agent id, dispatches to the executor, and
    /// backfills the final identity into the invocation output.
    #[allow(clippy::too_many_arguments)]
    async fn dispatch_agent_invocation(
        &self,
        component: &Component,
        agent_id: &AgentId,
        method_name: Option<String>,
        method_parameters: Option<golem_api_grpc::proto::golem::schema::SchemaValue>,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        idempotency_key: Option<IdempotencyKey>,
        invocation_context: Option<InvocationContext>,
        allow_derived_ephemeral_phantom: bool,
        freshness_disposition: InvocationFreshnessDisposition,
        config: Vec<AgentConfigEntryDto>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
        principal: golem_api_grpc::proto::golem::component::Principal,
        authorize: impl FnOnce(&AgentId) -> WorkerResult<()>,
    ) -> WorkerResult<AgentInvocationOutput> {
        let (agent_id, idempotency_key, freshness_disposition, observation_only) = self
            .prepare_agent_invocation_identity(
                component,
                agent_id,
                idempotency_key,
                allow_derived_ephemeral_phantom,
                mode,
                freshness_disposition,
                authorize,
            )?;

        let validation_component = if observation_only {
            None
        } else {
            Some(
                self.component_for_invocation(
                    component,
                    &agent_id,
                    environment_id,
                    &auth_ctx,
                    freshness_disposition,
                )
                .await?,
            )
        };

        self.dispatch_prepared_agent_invocation(
            validation_component.as_ref(),
            agent_id,
            method_name,
            method_parameters,
            mode,
            schedule_at,
            idempotency_key,
            invocation_context,
            freshness_disposition,
            config,
            environment_id,
            account_id,
            auth_ctx,
            principal,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare_agent_invocation_identity(
        &self,
        component: &Component,
        agent_id: &AgentId,
        idempotency_key: Option<IdempotencyKey>,
        allow_derived_ephemeral_phantom: bool,
        mode: i32,
        freshness_disposition: InvocationFreshnessDisposition,
        authorize: impl FnOnce(&AgentId) -> WorkerResult<()>,
    ) -> WorkerResult<(
        AgentId,
        IdempotencyKey,
        InvocationFreshnessDisposition,
        bool,
    )> {
        let observation_only =
            mode == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32;
        let (agent_id, idempotency_key, mut freshness_disposition) =
            normalize_agent_invocation_identity(
                component,
                agent_id,
                idempotency_key,
                allow_derived_ephemeral_phantom,
                observation_only,
                freshness_disposition,
            )?;
        if observation_only {
            freshness_disposition = InvocationFreshnessDisposition::MayExist;
        }
        authorize(&agent_id)?;
        Ok((
            agent_id,
            idempotency_key,
            freshness_disposition,
            observation_only,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    async fn dispatch_prepared_agent_invocation(
        &self,
        validation_component: Option<&Component>,
        agent_id: AgentId,
        method_name: Option<String>,
        method_parameters: Option<golem_api_grpc::proto::golem::schema::SchemaValue>,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        idempotency_key: IdempotencyKey,
        invocation_context: Option<InvocationContext>,
        freshness_disposition: InvocationFreshnessDisposition,
        config: Vec<AgentConfigEntryDto>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
        principal: golem_api_grpc::proto::golem::component::Principal,
    ) -> WorkerResult<AgentInvocationOutput> {
        if let Some(validation_component) = validation_component {
            let method_name = method_name.as_deref().ok_or_else(|| {
                WorkerServiceError::TypeChecker(
                    "method_name is required for non-lookup invocations".to_string(),
                )
            })?;
            let method_parameters = method_parameters.as_ref().ok_or_else(|| {
                WorkerServiceError::TypeChecker(
                    "method_parameters are required for non-lookup invocations".to_string(),
                )
            })?;
            validate_one_shot_invocation_is_stream_free(
                validation_component,
                &agent_id,
                method_name,
                method_parameters,
            )?;
        }

        let mut output = self
            .worker_client
            .invoke_agent(
                &agent_id,
                method_name,
                method_parameters,
                mode,
                schedule_at,
                Some(idempotency_key.clone()),
                invocation_context,
                freshness_disposition,
                config,
                environment_id,
                account_id,
                auth_ctx,
                principal,
            )
            .await?;
        output.agent_id.get_or_insert(agent_id);
        output.idempotency_key.get_or_insert(idempotency_key);
        Ok(output)
    }

    async fn component_for_invocation(
        &self,
        fallback: &Component,
        agent_id: &AgentId,
        environment_id: EnvironmentId,
        auth_ctx: &AuthCtx,
        freshness_disposition: InvocationFreshnessDisposition,
    ) -> WorkerResult<Component> {
        if freshness_disposition == InvocationFreshnessDisposition::KnownFresh {
            return Ok(fallback.clone());
        }

        let component_revision = match self
            .worker_client
            .get_metadata(agent_id, environment_id, auth_ctx.clone())
            .await
        {
            Ok(metadata) => metadata.component_revision,
            Err(WorkerServiceError::AgentNotFound(_))
            | Err(WorkerServiceError::GolemError(WorkerExecutorError::AgentNotFound { .. })) => {
                return Ok(fallback.clone());
            }
            Err(error) => return Err(error),
        };

        if component_revision == fallback.revision {
            Ok(fallback.clone())
        } else {
            Ok(self
                .component_service
                .get_revision(fallback.id, component_revision)
                .await?)
        }
    }

    /// REST path: resolves the agent via the registry, validates its parameters, then creates it.
    pub async fn create_agent_rest(
        &self,
        request: CreateAgentRequest,
        auth: AuthCtx,
    ) -> WorkerResult<CreateAgentResponse> {
        let resolved = self
            .agent_resolution_cache
            .resolve(
                &request.app_name,
                &request.env_name,
                &request.agent_type_name,
                None,
                &auth,
            )
            .await?;

        let registered_agent_type = &resolved.registered_agent_type;
        let _environment_id = resolved.environment_id;
        let component_id = registered_agent_type.implemented_by.component_id;
        let agent_type = &registered_agent_type.agent_type;

        let constructor_parameters = json_input_schema_value_to_typed_schema_value(
            request.parameters,
            &agent_type.schema,
            &agent_type.constructor.input_schema,
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters type error: {err}"
            ))
        })?;

        let agent_id = build_public_agent_id(
            component_id,
            request.agent_type_name.clone(),
            constructor_parameters,
            request.phantom_id,
            agent_type.mode,
        )?;

        let component = self
            .component_service
            .get_revision(
                component_id,
                registered_agent_type.implemented_by.component_revision,
            )
            .await?;

        let (component_revision, _created_at) = self
            .create_with_component(
                &agent_id,
                component,
                HashMap::new(),
                request.config,
                true,
                auth,
                None,
                None,
            )
            .await?;

        Ok(CreateAgentResponse {
            agent_id,
            component_revision,
        })
    }

    /// REST path: resolves the agent via the registry, validates its parameters, then delegates.
    pub async fn invoke_agent_rest(
        &self,
        request: AgentInvocationRequest,
        auth: AuthCtx,
    ) -> WorkerResult<AgentInvocationResult> {
        let deployment_revision = request
            .deployment_revision
            .map(|rev| {
                let rev_u64 = u64::try_from(rev).map_err(|_| {
                    WorkerServiceError::Internal(format!(
                        "Invalid deployment revision (must be non-negative): {rev}"
                    ))
                })?;
                DeploymentRevision::new(rev_u64).map_err(|e| {
                    WorkerServiceError::Internal(format!("Invalid deployment revision: {e}"))
                })
            })
            .transpose()?;

        let resolved = match deployment_revision {
            None => {
                self.agent_resolution_cache
                    .resolve(
                        &request.app_name,
                        &request.env_name,
                        &request.agent_type_name,
                        request.owner_account_email.as_deref(),
                        &auth,
                    )
                    .await?
            }
            Some(rev) => {
                self.agent_resolution_cache
                    .resolve_pinned(
                        &request.app_name,
                        &request.env_name,
                        &request.agent_type_name,
                        rev,
                        request.owner_account_email.as_deref(),
                        &auth,
                    )
                    .await?
            }
        };

        let registered_agent_type = &resolved.registered_agent_type;
        let environment_id = resolved.environment_id;
        let component_id = registered_agent_type.implemented_by.component_id;
        let agent_type = &registered_agent_type.agent_type;

        let constructor_parameters = json_input_schema_value_to_typed_schema_value(
            request.parameters,
            &agent_type.schema,
            &agent_type.constructor.input_schema,
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters type error: {err}"
            ))
        })?;

        let agent_id = build_public_invocation_agent_id(
            component_id,
            request.agent_type_name.clone(),
            constructor_parameters,
            request.phantom_id,
        )?;
        let component = self
            .component_service
            .get_revision(
                component_id,
                registered_agent_type.implemented_by.component_revision,
            )
            .await?;
        let component_name = registered_agent_type.implemented_by.component_name.clone();
        let component_owner_account_id = registered_agent_type.implemented_by.account_id;
        let component_owner_account_email =
            registered_agent_type.implemented_by.account_email.clone();
        let method_name = request.method_name.clone();
        let agent_type_name = request.agent_type_name.clone();
        let proto_mode = match request.mode {
            AgentInvocationMode::Await => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32
            }
            AgentInvocationMode::Schedule => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32
            }
        };
        let (agent_id, idempotency_key, freshness_disposition, observation_only) = self
            .prepare_agent_invocation_identity(
                &component,
                &agent_id,
                request.idempotency_key.clone(),
                false,
                proto_mode,
                InvocationFreshnessDisposition::MayExist,
                |final_agent_id| {
                    auth.authorize_permission(&PermissionTarget::Agent(ClassPermissionTarget {
                        owner: AgentOwnerPattern::Agent {
                            account: component_owner_account_email,
                            application: request.app_name,
                            environment: request.env_name,
                            component: ComponentName(component_name),
                            agent: AgentOwnerLeafPattern::Agent(final_agent_id.agent_id.clone()),
                        },
                        verb: Some(AgentVerb::Invoke),
                        resource: AgentResourcePattern::Method(AgentMethodName(
                            method_name.clone(),
                        )),
                    }))
                    .map_err(AuthServiceError::from)
                    .map_err(WorkerServiceError::from)
                },
            )?;
        debug_assert!(!observation_only);
        let invocation_component = self
            .component_for_invocation(
                &component,
                &agent_id,
                environment_id,
                &auth,
                freshness_disposition,
            )
            .await?;

        let invocation_agent_type = invocation_component
            .metadata
            .find_agent_type_by_name_ref(&request.agent_type_name)
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent type {} not found in component metadata at revision {}",
                    request.agent_type_name, invocation_component.revision
                ))
            })?;
        let method = invocation_agent_type
            .methods
            .iter()
            .find(|m| m.name == request.method_name)
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent method {} not found in agent type {}",
                    request.method_name, request.agent_type_name
                ))
            })?;

        let method_parameters = json_input_schema_value_to_typed_schema_value(
            request.method_parameters,
            &invocation_agent_type.schema,
            &method.input_schema,
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!("Agent method parameters type error: {err}"))
        })?
        .into_parts()
        .1;

        let proto_method_parameters: golem_api_grpc::proto::golem::schema::SchemaValue =
            method_parameters.try_into().map_err(|error| {
                WorkerServiceError::TypeChecker(format!(
                    "Agent method parameters cannot cross the worker boundary: {error}"
                ))
            })?;

        let proto_schedule_at = request.schedule_at.map(|dt| ::prost_types::Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        });

        let principal: golem_api_grpc::proto::golem::component::Principal =
            Principal::GolemUser(GolemUserPrincipal {
                account_id: auth.account_id(),
            })
            .into();

        let output = self
            .dispatch_prepared_agent_invocation(
                Some(&invocation_component),
                agent_id.clone(),
                Some(method_name.clone()),
                Some(proto_method_parameters),
                proto_mode,
                proto_schedule_at,
                idempotency_key,
                None,
                freshness_disposition,
                request.config,
                environment_id,
                component_owner_account_id,
                auth.clone(),
                principal,
            )
            .await?;

        let response_agent_id = output.agent_id.clone().unwrap_or_else(|| agent_id.clone());
        let response_idempotency_key = output
            .idempotency_key
            .clone()
            .ok_or_else(|| WorkerServiceError::Internal("Missing idempotency key".to_string()))?;

        match output.result {
            golem_common::model::AgentInvocationResult::AgentMethod {
                output: output_value,
            } => {
                let decode_revision = output
                    .component_revision
                    .unwrap_or(registered_agent_type.implemented_by.component_revision);
                let component_metadata_for_decode = self
                    .component_service
                    .get_revision(component_id, decode_revision)
                    .await?;
                let decode_agent_type = component_metadata_for_decode
                    .metadata
                    .find_agent_type_by_name_ref(&agent_type_name)
                    .ok_or_else(|| {
                        WorkerServiceError::Internal(format!(
                            "Agent type {agent_type_name} not found in component metadata at revision {decode_revision}",
                        ))
                    })?;
                let decode_method = decode_agent_type
                    .methods
                    .iter()
                    .find(|m| m.name == method_name)
                    .ok_or_else(|| {
                        WorkerServiceError::Internal(format!(
                            "Agent method {method_name} not found in agent type {agent_type_name} at revision {decode_revision}",
                        ))
                    })?;
                let mut output_graph = decode_agent_type.schema.clone();
                output_graph.root = decode_method
                    .output_schema
                    .schema()
                    .cloned()
                    .unwrap_or_else(|| SchemaType::tuple(Vec::new()));
                let typed_output = TypedSchemaValue::new(output_graph, output_value);
                Ok(AgentInvocationResult {
                    agent_id: response_agent_id,
                    idempotency_key: response_idempotency_key,
                    result: Some(typed_output),
                    component_revision: Some(decode_revision),
                })
            }
            _ => Ok(AgentInvocationResult {
                agent_id: response_agent_id,
                idempotency_key: response_idempotency_key,
                result: None,
                component_revision: output.component_revision,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        WorkerService, agent_verb_for_invocation_mode, build_public_agent_id,
        build_public_invocation_agent_id, decode_public_schema_value,
        normalize_agent_invocation_identity,
    };
    use crate::api::agents::{AgentInvocationMode, AgentInvocationRequest, CreateAgentRequest};
    use crate::service::agent_resolution_cache::AgentResolutionCache;
    use crate::service::auth::{AuthService, AuthServiceError};
    use crate::service::component::{ComponentService, ComponentServiceError};
    use crate::service::limit::{LimitService, LimitServiceError};
    use crate::service::worker::{WorkerClient, WorkerResult, WorkerServiceError, WorkerStream};
    use async_trait::async_trait;
    use bytes::Bytes;
    use chrono::Utc;
    use futures::{Stream, StreamExt, stream};
    use golem_api_grpc::proto::golem::worker::{
        InvocationContext, InvocationStart, LogEvent, PublicInvocationStart, invocation_request,
    };
    use golem_common::base_model::component_metadata::KnownExports;
    use golem_common::model::AgentInvocationOutput;
    use golem_common::model::Empty;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{
        AgentMode, AgentTypeName, HttpEndpointDetails, InvocationFreshnessDisposition,
        ParsedAgentId, Principal, RegisteredAgentType, RegisteredAgentTypeImplementer,
        ResolvedAgentType, Snapshotting, ephemeral_invocation_phantom_id,
    };
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::card::{AgentVerb, EffectiveSurface, StoredCard};
    use golem_common::model::component::{
        CanonicalFilePath, ComponentId, ComponentName, ComponentRevision, PluginPriority,
    };
    use golem_common::model::component_metadata::ComponentMetadata;
    use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
    use golem_common::model::diff::Hash;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::oplog::{OplogCursor, OplogIndex};
    use golem_common::model::worker::{AgentConfigEntryDto, AgentMetadataDto, RevertWorkerTarget};
    use golem_common::model::{
        AgentFilter, AgentFingerprint, AgentId, AgentStatus, IdempotencyKey, ScanCursor, Timestamp,
    };
    use golem_common::schema::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, InputSchema, NamedField,
        OutputSchema, SchemaGraph, SchemaType, SchemaValue,
    };
    use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
    use golem_service_base::model::auth::AuthCtx;
    use golem_service_base::model::component::Component;
    use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
    use std::collections::{BTreeMap, HashMap, HashSet};
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use test_r::test;
    use uuid::Uuid;

    fn empty_constructor_parameters() -> golem_common::schema::TypedSchemaValue {
        golem_common::schema::TypedSchemaValue::new(
            SchemaGraph::anonymous(golem_common::schema::SchemaType::record(vec![])),
            golem_common::schema::SchemaValue::Record { fields: vec![] },
        )
    }

    struct TestAgentTypeResolver(AgentMode);

    impl golem_common::model::agent::AgentTypeSchemaResolver for TestAgentTypeResolver {
        fn resolve_agent_type_schema_by_name(
            &self,
            name: &AgentTypeName,
        ) -> Result<AgentTypeSchema, String> {
            Ok(test_agent_type(name.clone(), self.0))
        }
    }

    #[test]
    fn public_ephemeral_agent_id_gets_automatic_phantom() {
        let id = build_public_agent_id(
            ComponentId::new(),
            AgentTypeName("test".into()),
            empty_constructor_parameters(),
            None,
            AgentMode::Ephemeral,
        )
        .unwrap();

        assert!(
            ParsedAgentId::parse(&id.agent_id, TestAgentTypeResolver(AgentMode::Ephemeral))
                .unwrap()
                .phantom_id
                .is_some()
        );
    }

    #[test]
    fn public_durable_agent_id_does_not_get_automatic_phantom() {
        let id = build_public_agent_id(
            ComponentId::new(),
            AgentTypeName("test".into()),
            empty_constructor_parameters(),
            None,
            AgentMode::Durable,
        )
        .unwrap();

        assert!(
            ParsedAgentId::parse(&id.agent_id, TestAgentTypeResolver(AgentMode::Durable))
                .unwrap()
                .phantom_id
                .is_none()
        );
    }

    #[test]
    fn public_agent_id_preserves_supplied_phantom() {
        let phantom = Uuid::new_v4();
        let id = build_public_agent_id(
            ComponentId::new(),
            AgentTypeName("test".into()),
            empty_constructor_parameters(),
            Some(phantom),
            AgentMode::Ephemeral,
        )
        .unwrap();

        assert_eq!(
            ParsedAgentId::parse(&id.agent_id, TestAgentTypeResolver(AgentMode::Ephemeral))
                .unwrap()
                .phantom_id,
            Some(phantom)
        );
    }

    #[test]
    fn public_durable_agent_id_preserves_supplied_phantom() {
        let phantom = Uuid::new_v4();
        let id = build_public_agent_id(
            ComponentId::new(),
            AgentTypeName("test".into()),
            empty_constructor_parameters(),
            Some(phantom),
            AgentMode::Durable,
        )
        .unwrap();

        assert_eq!(
            ParsedAgentId::parse(&id.agent_id, TestAgentTypeResolver(AgentMode::Durable))
                .unwrap()
                .phantom_id,
            Some(phantom)
        );
    }

    #[test]
    fn generated_ephemeral_phantom_not_matching_the_invocation_identity_is_rejected() {
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let account_id = AccountId::new();
        let component_revision = ComponentRevision::INITIAL;
        let agent_type_name = AgentTypeName("test".to_string());
        let original_phantom = Uuid::new_v4();
        let agent_id = build_public_agent_id(
            component_id,
            agent_type_name.clone(),
            empty_constructor_parameters(),
            Some(original_phantom),
            AgentMode::Ephemeral,
        )
        .unwrap();
        let component = test_component(
            component_id,
            environment_id,
            account_id,
            component_revision,
            test_agent_type(agent_type_name, AgentMode::Ephemeral),
        );

        let result = normalize_agent_invocation_identity(
            &component,
            &agent_id,
            None,
            true,
            false,
            InvocationFreshnessDisposition::MayExist,
        );

        assert!(matches!(result, Err(WorkerServiceError::TypeChecker(_))));
    }

    #[test]
    fn generated_ephemeral_phantom_matching_the_invocation_identity_is_accepted() {
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let account_id = AccountId::new();
        let component_revision = ComponentRevision::INITIAL;
        let agent_type_name = AgentTypeName("test".to_string());
        let idempotency_key = IdempotencyKey::fresh();
        let derived_phantom = ephemeral_invocation_phantom_id(&idempotency_key);
        let agent_id = build_public_agent_id(
            component_id,
            agent_type_name.clone(),
            empty_constructor_parameters(),
            Some(derived_phantom),
            AgentMode::Ephemeral,
        )
        .unwrap();
        let component = test_component(
            component_id,
            environment_id,
            account_id,
            component_revision,
            test_agent_type(agent_type_name, AgentMode::Ephemeral),
        );

        let (final_agent_id, final_idempotency_key, freshness_disposition) =
            normalize_agent_invocation_identity(
                &component,
                &agent_id,
                Some(idempotency_key.clone()),
                true,
                false,
                InvocationFreshnessDisposition::KnownFresh,
            )
            .unwrap();

        assert_eq!(phantom_id(&final_agent_id), Some(derived_phantom));
        assert_eq!(final_idempotency_key, idempotency_key);
        assert_eq!(
            freshness_disposition,
            InvocationFreshnessDisposition::KnownFresh
        );
    }

    #[test]
    fn matching_ephemeral_phantom_requires_explicit_capability() {
        let component_id = ComponentId::new();
        let environment_id = EnvironmentId::new();
        let account_id = AccountId::new();
        let agent_type_name = AgentTypeName("test".to_string());
        let idempotency_key = IdempotencyKey::fresh();
        let agent_id = build_public_agent_id(
            component_id,
            agent_type_name.clone(),
            empty_constructor_parameters(),
            Some(ephemeral_invocation_phantom_id(&idempotency_key)),
            AgentMode::Ephemeral,
        )
        .unwrap();
        let component = test_component(
            component_id,
            environment_id,
            account_id,
            ComponentRevision::INITIAL,
            test_agent_type(agent_type_name, AgentMode::Ephemeral),
        );

        let result = normalize_agent_invocation_identity(
            &component,
            &agent_id,
            Some(idempotency_key),
            false,
            false,
            InvocationFreshnessDisposition::MayExist,
        );

        assert!(matches!(result, Err(WorkerServiceError::TypeChecker(_))));
    }

    #[derive(Clone)]
    struct TestRegistryService {
        resolved: ResolvedAgentType,
    }

    #[async_trait]
    impl RegistryService for TestRegistryService {
        async fn authenticate_token(
            &self,
            _: &golem_common::model::auth::TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_limits(
            &self,
            _: AccountId,
        ) -> Result<golem_service_base::model::ResourceLimits, RegistryServiceError> {
            unimplemented!()
        }

        async fn update_worker_connection_limit(
            &self,
            _: AccountId,
            _: &golem_common::base_model::AgentId,
            _: bool,
        ) -> Result<(), RegistryServiceError> {
            unimplemented!()
        }

        async fn batch_update_resource_usage(
            &self,
            _: HashMap<AccountId, golem_service_base::clients::registry::ResourceUsageUpdate>,
        ) -> Result<golem_service_base::model::AccountResourceLimits, RegistryServiceError>
        {
            unimplemented!()
        }

        async fn download_component(
            &self,
            _: ComponentId,
            _: ComponentRevision,
        ) -> Result<Vec<u8>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_component_metadata(
            &self,
            _: ComponentId,
            _: ComponentRevision,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_deployed_component_metadata(
            &self,
            _: ComponentId,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_deployed_component_revisions(
            &self,
            _: ComponentId,
        ) -> Result<Vec<Component>, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_component(
            &self,
            _: AccountId,
            _: ApplicationId,
            _: EnvironmentId,
            _: &str,
        ) -> Result<Component, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_all_agent_types(
            &self,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
        ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_agent_type(
            &self,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
            _: &AgentTypeName,
        ) -> Result<RegisteredAgentType, RegistryServiceError> {
            unimplemented!()
        }

        async fn resolve_agent_type_by_names(
            &self,
            _: &ApplicationName,
            _: &EnvironmentName,
            _: &AgentTypeName,
            _: Option<DeploymentRevision>,
            _: Option<&str>,
            _: &AuthCtx,
        ) -> Result<ResolvedAgentType, RegistryServiceError> {
            Ok(self.resolved.clone())
        }

        async fn get_active_routes_for_domain(
            &self,
            _: &golem_common::model::domain_registration::Domain,
        ) -> Result<golem_service_base::custom_api::CompiledRoutes, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_active_compiled_mcps_for_domain(
            &self,
            _: &golem_common::model::domain_registration::Domain,
        ) -> Result<golem_service_base::mcp::CompiledMcp, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_current_environment_state(
            &self,
            _: EnvironmentId,
        ) -> Result<golem_service_base::model::environment::EnvironmentState, RegistryServiceError>
        {
            unimplemented!()
        }

        async fn get_resource_definition_by_id(
            &self,
            _: golem_common::model::quota::ResourceDefinitionId,
        ) -> Result<golem_common::model::quota::ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_definition_by_name(
            &self,
            _: EnvironmentId,
            _: golem_common::model::quota::ResourceName,
        ) -> Result<golem_common::model::quota::ResourceDefinition, RegistryServiceError> {
            unimplemented!()
        }

        async fn subscribe_registry_invalidations(
            &self,
            _: Option<u64>,
        ) -> Result<
            Pin<
                Box<
                    dyn futures::Stream<
                            Item = Result<
                                golem_common::model::agent::RegistryInvalidationEvent,
                                RegistryServiceError,
                            >,
                        > + Send,
                >,
            >,
            RegistryServiceError,
        > {
            unimplemented!()
        }

        async fn run_registry_invalidation_event_subscriber(
            &self,
            _: &'static str,
            _: Option<tokio_util::sync::CancellationToken>,
            _: Arc<dyn golem_service_base::clients::registry::RegistryInvalidationHandler>,
        ) {
            unimplemented!()
        }
    }

    struct StaticComponentService {
        components: Vec<Component>,
    }

    #[async_trait]
    impl ComponentService for StaticComponentService {
        async fn get_current_by_id_in_cache(&self, component_id: ComponentId) -> Option<Component> {
            self.components
                .iter()
                .filter(|component| component.id == component_id)
                .max_by_key(|component| component.revision)
                .cloned()
        }

        async fn get_current_by_id_uncached(
            &self,
            component_id: ComponentId,
        ) -> Result<Component, ComponentServiceError> {
            self.get_current_by_id_in_cache(component_id)
                .await
                .ok_or(ComponentServiceError::ComponentNotFound)
        }

        async fn get_revision(
            &self,
            component_id: ComponentId,
            component_revision: ComponentRevision,
        ) -> Result<Component, ComponentServiceError> {
            self.components
                .iter()
                .find(|component| {
                    component.id == component_id && component.revision == component_revision
                })
                .cloned()
                .ok_or(ComponentServiceError::ComponentNotFound)
        }

        async fn get_all_revisions(
            &self,
            component_id: ComponentId,
        ) -> Result<Vec<Component>, ComponentServiceError> {
            let components = self
                .components
                .iter()
                .filter(|component| component.id == component_id)
                .cloned()
                .collect::<Vec<_>>();
            if components.is_empty() {
                Err(ComponentServiceError::ComponentNotFound)
            } else {
                Ok(components)
            }
        }
    }

    struct AllowAllAuthService;

    #[async_trait]
    impl AuthService for AllowAllAuthService {
        async fn authenticate_token(
            &self,
            _: golem_common::model::auth::TokenSecret,
        ) -> Result<AuthCtx, AuthServiceError> {
            unimplemented!()
        }
    }

    struct NoopLimitService;

    #[async_trait]
    impl LimitService for NoopLimitService {
        async fn update_worker_connection_limit(
            &self,
            _: AccountId,
            _: &AgentId,
            _: bool,
        ) -> Result<(), LimitServiceError> {
            Ok(())
        }
    }

    struct RecordingWorkerClient {
        created_agent_ids: Mutex<Vec<AgentId>>,
        invocations: Mutex<Vec<(AgentId, IdempotencyKey, InvocationFreshnessDisposition)>>,
        invocation_environments: Mutex<Vec<EnvironmentId>>,
        invocation_session_starts: Mutex<Vec<(AgentId, InvocationStart)>>,
        invocation_output: AgentInvocationOutput,
        metadata_component_revision: Option<ComponentRevision>,
    }

    impl RecordingWorkerClient {
        fn new(invocation_output: AgentInvocationOutput) -> Self {
            Self {
                created_agent_ids: Mutex::new(Vec::new()),
                invocations: Mutex::new(Vec::new()),
                invocation_environments: Mutex::new(Vec::new()),
                invocation_session_starts: Mutex::new(Vec::new()),
                invocation_output,
                metadata_component_revision: None,
            }
        }

        fn with_metadata_component_revision(
            invocation_output: AgentInvocationOutput,
            component_revision: ComponentRevision,
        ) -> Self {
            Self {
                created_agent_ids: Mutex::new(Vec::new()),
                invocations: Mutex::new(Vec::new()),
                invocation_environments: Mutex::new(Vec::new()),
                invocation_session_starts: Mutex::new(Vec::new()),
                invocation_output,
                metadata_component_revision: Some(component_revision),
            }
        }

        fn created_agent_id(&self) -> AgentId {
            self.created_agent_ids.lock().unwrap()[0].clone()
        }

        fn invoked_agent_id(&self) -> AgentId {
            self.invocations.lock().unwrap()[0].0.clone()
        }

        fn invocations(&self) -> Vec<(AgentId, IdempotencyKey, InvocationFreshnessDisposition)> {
            self.invocations.lock().unwrap().clone()
        }

        fn invocation_environment(&self) -> EnvironmentId {
            self.invocation_environments.lock().unwrap()[0]
        }

        fn invocation_session_start(&self) -> (AgentId, InvocationStart) {
            self.invocation_session_starts.lock().unwrap()[0].clone()
        }

        fn invocation_session_start_count(&self) -> usize {
            self.invocation_session_starts.lock().unwrap().len()
        }
    }

    #[async_trait]
    impl WorkerClient for RecordingWorkerClient {
        async fn create(
            &self,
            agent_id: &AgentId,
            _: HashMap<String, String>,
            _: Vec<AgentConfigEntryDto>,
            _: bool,
            _: AccountId,
            _: EnvironmentId,
            _: AuthCtx,
            _: Option<InvocationContext>,
            _: Option<golem_api_grpc::proto::golem::component::Principal>,
        ) -> WorkerResult<(AgentId, AgentFingerprint)> {
            self.created_agent_ids
                .lock()
                .unwrap()
                .push(agent_id.clone());
            Ok((agent_id.clone(), AgentFingerprint::new()))
        }

        async fn connect(
            &self,
            _: &AgentId,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<WorkerStream<LogEvent>> {
            unimplemented!()
        }

        async fn delete(&self, _: &AgentId, _: EnvironmentId, _: AuthCtx) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn complete_promise(
            &self,
            _: &AgentId,
            _: u64,
            _: Vec<u8>,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<bool> {
            unimplemented!()
        }

        async fn interrupt(
            &self,
            _: &AgentId,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn get_metadata(
            &self,
            agent_id: &AgentId,
            environment_id: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<AgentMetadataDto> {
            match self.metadata_component_revision {
                Some(component_revision) => Ok(AgentMetadataDto {
                    agent_id: agent_id.clone(),
                    environment_id,
                    created_by: AccountId(Uuid::new_v4()),
                    env: HashMap::new(),
                    config: Vec::new(),
                    status: AgentStatus::Idle,
                    component_revision,
                    retry_count: 0,
                    pending_invocation_count: 0,
                    updates: Vec::new(),
                    created_at: Timestamp::now_utc(),
                    last_error: None,
                    component_size: 0,
                    total_linear_memory_size: 0,
                    exported_resource_instances: Vec::new(),
                    active_plugins: HashSet::new(),
                    skipped_regions: Vec::new(),
                    deleted_regions: Vec::new(),
                    last_oplog_index: OplogIndex::INITIAL,
                    fingerprint: AgentFingerprint(Uuid::new_v4()),
                }),
                None => Err(WorkerServiceError::AgentNotFound(agent_id.clone())),
            }
        }

        async fn find_metadata(
            &self,
            _: ComponentId,
            _: Option<AgentFilter>,
            _: ScanCursor,
            _: u64,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<(Option<ScanCursor>, Vec<AgentMetadataDto>)> {
            unimplemented!()
        }

        async fn resume(
            &self,
            _: &AgentId,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn update(
            &self,
            _: &AgentId,
            _: golem_common::model::worker::AgentUpdateMode,
            _: ComponentRevision,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn get_oplog(
            &self,
            _: &AgentId,
            _: OplogIndex,
            _: Option<OplogCursor>,
            _: u64,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> Result<GetOplogResponse, super::WorkerServiceError> {
            unimplemented!()
        }

        async fn search_oplog(
            &self,
            _: &AgentId,
            _: Option<OplogCursor>,
            _: u64,
            _: String,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> Result<GetOplogResponse, super::WorkerServiceError> {
            unimplemented!()
        }

        async fn get_file_system_node(
            &self,
            _: &AgentId,
            _: CanonicalFilePath,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
            unimplemented!()
        }

        async fn get_agent_wallet(
            &self,
            _: &AgentId,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<Vec<StoredCard>> {
            unimplemented!()
        }

        async fn get_file_contents(
            &self,
            _: &AgentId,
            _: CanonicalFilePath,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>>
        {
            unimplemented!()
        }

        async fn activate_plugin(
            &self,
            _: &AgentId,
            _: PluginPriority,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn deactivate_plugin(
            &self,
            _: &AgentId,
            _: PluginPriority,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn fork_worker(
            &self,
            _: &AgentId,
            _: &AgentId,
            _: OplogIndex,
            _: EnvironmentId,
            _: AccountId,
            _: AccountEmail,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn revert_worker(
            &self,
            _: &AgentId,
            _: RevertWorkerTarget,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }

        async fn cancel_invocation(
            &self,
            _: &AgentId,
            _: &IdempotencyKey,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<bool> {
            unimplemented!()
        }

        async fn invoke_agent(
            &self,
            agent_id: &AgentId,
            _: Option<String>,
            _: Option<golem_api_grpc::proto::golem::schema::SchemaValue>,
            _: i32,
            _: Option<::prost_types::Timestamp>,
            idempotency_key: Option<IdempotencyKey>,
            _: Option<InvocationContext>,
            freshness_disposition: InvocationFreshnessDisposition,
            _: Vec<AgentConfigEntryDto>,
            environment_id: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
            _: golem_api_grpc::proto::golem::component::Principal,
        ) -> WorkerResult<AgentInvocationOutput> {
            self.invocations.lock().unwrap().push((
                agent_id.clone(),
                idempotency_key.expect("worker service should supply an idempotency key"),
                freshness_disposition,
            ));
            self.invocation_environments
                .lock()
                .unwrap()
                .push(environment_id);
            Ok(self.invocation_output.clone())
        }

        async fn invoke_agent_session(
            &self,
            agent_id: &AgentId,
            mut request: super::InvocationRequestStream,
        ) -> WorkerResult<super::InvocationResponseStream> {
            let start = request
                .next()
                .await
                .expect("worker service should send an invocation start");
            let start = match start.request {
                Some(invocation_request::Request::Start(start)) => start,
                other => panic!("expected invocation start, got {other:?}"),
            };
            self.invocation_session_starts
                .lock()
                .unwrap()
                .push((agent_id.clone(), start));
            Ok(Box::pin(stream::empty()))
        }

        async fn process_oplog_entries(
            &self,
            _: &AgentId,
            _: EnvironmentId,
            _: ComponentRevision,
            _: IdempotencyKey,
            _: AccountId,
            _: HashMap<String, String>,
            _: golem_api_grpc::proto::golem::worker::AgentMetadata,
            _: OplogIndex,
            _: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            unimplemented!()
        }
    }

    struct RestHarness {
        worker_service: WorkerService,
        worker_client: Arc<RecordingWorkerClient>,
        agent_type_name: AgentTypeName,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        environment_id: EnvironmentId,
    }

    impl RestHarness {
        fn new(mode: AgentMode) -> Self {
            Self::new_with_method_schema(mode, InputSchema::Parameters(vec![]), OutputSchema::Unit)
        }

        fn new_with_output(mode: AgentMode, output_schema: OutputSchema) -> Self {
            Self::new_with_method_schema(mode, InputSchema::Parameters(vec![]), output_schema)
        }

        fn new_with_input(mode: AgentMode, input_schema: InputSchema) -> Self {
            Self::new_with_method_schema(mode, input_schema, OutputSchema::Unit)
        }

        fn new_with_method_schema(
            mode: AgentMode,
            input_schema: InputSchema,
            output_schema: OutputSchema,
        ) -> Self {
            let component_id = ComponentId(Uuid::new_v4());
            let environment_id = EnvironmentId(Uuid::new_v4());
            let account_id = AccountId(Uuid::new_v4());
            let component_revision = ComponentRevision::INITIAL;
            let agent_type_name = AgentTypeName("weather-agent".to_string());
            let mut agent_type = test_agent_type(agent_type_name.clone(), mode);
            agent_type.methods[0].input_schema = input_schema;
            agent_type.methods[0].output_schema = output_schema;
            let component = test_component(
                component_id,
                environment_id,
                account_id,
                component_revision,
                agent_type.clone(),
            );
            let worker_client = Arc::new(RecordingWorkerClient::new(AgentInvocationOutput {
                result: golem_common::model::AgentInvocationResult::AgentInitialization,
                consumed_fuel: None,
                invocation_status: None,
                component_revision: Some(component_revision),
                agent_id: None,
                idempotency_key: None,
                oplog_index: None,
                agent_fingerprint: None,
            }));
            let registry = Arc::new(TestRegistryService {
                resolved: ResolvedAgentType {
                    registered_agent_type: RegisteredAgentType {
                        agent_type,
                        implemented_by: RegisteredAgentTypeImplementer {
                            component_id,
                            component_revision,
                            component_name: component.component_name.0.clone(),
                            account_id: component.account_id,
                            account_email: component.account_email.clone(),
                        },
                    },
                    environment_id,
                    deployment_revision: DeploymentRevision::INITIAL,
                    current_deployment_revision: Some(CurrentDeploymentRevision::INITIAL),
                },
            });
            let agent_resolution_cache = Arc::new(AgentResolutionCache::new(
                registry,
                1,
                Duration::from_secs(60),
                Duration::from_secs(60),
            ));

            Self {
                worker_service: WorkerService::new(
                    Arc::new(StaticComponentService {
                        components: vec![component],
                    }),
                    Arc::new(AllowAllAuthService),
                    Arc::new(NoopLimitService),
                    worker_client.clone(),
                    agent_resolution_cache,
                ),
                worker_client,
                agent_type_name,
                component_id,
                component_revision,
                environment_id,
            }
        }

        fn new_with_pinned_and_latest_output(
            pinned_output_schema: OutputSchema,
            latest_output_schema: OutputSchema,
        ) -> Self {
            let component_id = ComponentId(Uuid::new_v4());
            let environment_id = EnvironmentId(Uuid::new_v4());
            let account_id = AccountId(Uuid::new_v4());
            let pinned_revision = ComponentRevision::INITIAL;
            let latest_revision = ComponentRevision::new(1).unwrap();
            let agent_type_name = AgentTypeName("weather-agent".to_string());
            let mut pinned_agent_type =
                test_agent_type(agent_type_name.clone(), AgentMode::Durable);
            pinned_agent_type.methods[0].output_schema = pinned_output_schema;
            let mut latest_agent_type =
                test_agent_type(agent_type_name.clone(), AgentMode::Durable);
            latest_agent_type.methods[0].output_schema = latest_output_schema;
            let pinned_component = test_component(
                component_id,
                environment_id,
                account_id,
                pinned_revision,
                pinned_agent_type,
            );
            let latest_component = test_component(
                component_id,
                environment_id,
                account_id,
                latest_revision,
                latest_agent_type.clone(),
            );
            let worker_client = Arc::new(RecordingWorkerClient::with_metadata_component_revision(
                AgentInvocationOutput {
                    result: golem_common::model::AgentInvocationResult::AgentInitialization,
                    consumed_fuel: None,
                    invocation_status: None,
                    component_revision: Some(pinned_revision),
                    agent_id: None,
                    idempotency_key: None,
                    oplog_index: None,
                    agent_fingerprint: None,
                },
                pinned_revision,
            ));
            let registry = Arc::new(TestRegistryService {
                resolved: ResolvedAgentType {
                    registered_agent_type: RegisteredAgentType {
                        agent_type: latest_agent_type,
                        implemented_by: RegisteredAgentTypeImplementer {
                            component_id,
                            component_revision: latest_revision,
                            component_name: latest_component.component_name.0.clone(),
                            account_id: latest_component.account_id,
                            account_email: latest_component.account_email.clone(),
                        },
                    },
                    environment_id,
                    deployment_revision: DeploymentRevision::INITIAL,
                    current_deployment_revision: Some(CurrentDeploymentRevision::INITIAL),
                },
            });
            let agent_resolution_cache = Arc::new(AgentResolutionCache::new(
                registry,
                1,
                Duration::from_secs(60),
                Duration::from_secs(60),
            ));

            Self {
                worker_service: WorkerService::new(
                    Arc::new(StaticComponentService {
                        components: vec![pinned_component, latest_component],
                    }),
                    Arc::new(AllowAllAuthService),
                    Arc::new(NoopLimitService),
                    worker_client.clone(),
                    agent_resolution_cache,
                ),
                worker_client,
                agent_type_name,
                component_id,
                component_revision: latest_revision,
                environment_id,
            }
        }

        fn create_request(&self) -> CreateAgentRequest {
            CreateAgentRequest {
                app_name: ApplicationName::try_from("weather-app".to_string()).unwrap(),
                env_name: EnvironmentName::try_from("prod").unwrap(),
                agent_type_name: self.agent_type_name.clone(),
                parameters: empty_json_tuple(),
                phantom_id: None,
                config: vec![],
            }
        }

        fn invoke_request(&self) -> AgentInvocationRequest {
            AgentInvocationRequest {
                app_name: ApplicationName::try_from("weather-app".to_string()).unwrap(),
                env_name: EnvironmentName::try_from("prod").unwrap(),
                agent_type_name: self.agent_type_name.clone(),
                parameters: empty_json_tuple(),
                phantom_id: None,
                config: vec![],
                method_name: "run".to_string(),
                method_parameters: empty_json_tuple(),
                mode: AgentInvocationMode::Await,
                schedule_at: None,
                idempotency_key: None,
                deployment_revision: None,
                owner_account_email: None,
            }
        }

        fn public_invocation_start(
            &self,
            idempotency_key: IdempotencyKey,
        ) -> PublicInvocationStart {
            PublicInvocationStart {
                application_name: "weather-app".to_string(),
                environment_name: "prod".to_string(),
                agent_type_name: self.agent_type_name.0.clone(),
                constructor_parameters: Some(empty_json_tuple().try_into().unwrap()),
                phantom_id: None,
                config: vec![],
                method_name: "run".to_string(),
                method_parameters: Some(empty_json_tuple().try_into().unwrap()),
                idempotency_key: Some(idempotency_key.into()),
            }
        }
    }

    fn test_agent_type(agent_type_name: AgentTypeName, mode: AgentMode) -> AgentTypeSchema {
        AgentTypeSchema {
            type_name: agent_type_name,
            description: String::new(),
            source_language: String::new(),
            schema: SchemaGraph::empty(),
            constructor: AgentConstructorSchema {
                name: None,
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
            },
            methods: vec![AgentMethodSchema {
                name: "run".to_string(),
                description: String::new(),
                prompt_hint: None,
                input_schema: InputSchema::Parameters(vec![]),
                output_schema: OutputSchema::Unit,
                http_endpoint: vec![HttpEndpointDetails {
                    http_method: golem_common::model::agent::HttpMethod::Get(Empty {}),
                    path_suffix: vec![],
                    header_vars: vec![],
                    query_vars: vec![],
                    auth_details: None,
                    cors_options: golem_common::model::agent::CorsOptions {
                        allowed_patterns: vec![],
                    },
                }],
                read_only: None,
            }],
            dependencies: vec![],
            mode,
            http_mount: None,
            snapshotting: Snapshotting::Disabled(Empty {}),
            config: vec![],
        }
    }

    fn test_component(
        component_id: ComponentId,
        environment_id: EnvironmentId,
        account_id: AccountId,
        component_revision: ComponentRevision,
        agent_type: AgentTypeSchema,
    ) -> Component {
        Component {
            id: component_id,
            revision: component_revision,
            environment_id,
            component_name: ComponentName("weather-component".to_string()),
            hash: Hash::empty(),
            application_id: ApplicationId(Uuid::new_v4()),
            account_id,
            account_email: golem_common::model::account::AccountEmail::new("weather@golem"),
            application_name: ApplicationName::try_from("weather-app".to_string()).unwrap(),
            environment_name: EnvironmentName::try_from("prod").unwrap(),
            component_size: 0,
            metadata: ComponentMetadata::from_parts(
                KnownExports::default(),
                vec![],
                None,
                None,
                vec![agent_type],
                BTreeMap::new(),
            ),
            created_at: Utc::now(),
            wasm_hash: Hash::empty(),
            object_store_key: String::new(),
        }
    }

    fn empty_json_tuple() -> SchemaValue {
        SchemaValue::Record { fields: vec![] }
    }

    fn phantom_id(agent_id: &AgentId) -> Option<Uuid> {
        agent_id
            .agent_id
            .rsplit_once('[')
            .and_then(|(_, phantom_id)| phantom_id.strip_suffix(']'))
            .and_then(|phantom_id| Uuid::parse_str(phantom_id).ok())
    }

    #[test]
    fn lookup_invocation_requires_view_agent_permission() {
        assert_eq!(
            agent_verb_for_invocation_mode(
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32,
            ),
            AgentVerb::View,
        );
    }

    #[test]
    fn non_lookup_invocation_requires_invoke_agent_permission() {
        assert_eq!(
            agent_verb_for_invocation_mode(
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            ),
            AgentVerb::Invoke,
        );
    }

    #[test]
    async fn create_agent_rest_auto_generates_phantom_for_ephemeral_agents() {
        let harness = RestHarness::new(AgentMode::Ephemeral);

        let response = harness
            .worker_service
            .create_agent_rest(harness.create_request(), AuthCtx::system())
            .await
            .unwrap();

        assert_eq!(response.agent_id.component_id, harness.component_id);
        assert_eq!(response.component_revision, harness.component_revision);
        assert_eq!(response.agent_id, harness.worker_client.created_agent_id());
        assert!(phantom_id(&response.agent_id).is_some());
    }

    #[test]
    async fn invoke_agent_rest_auto_generates_phantom_for_ephemeral_agents() {
        let harness = RestHarness::new(AgentMode::Ephemeral);

        let response = harness
            .worker_service
            .invoke_agent_rest(harness.invoke_request(), AuthCtx::system())
            .await
            .unwrap();

        assert_eq!(response.agent_id.component_id, harness.component_id);
        assert_eq!(
            response.component_revision,
            Some(harness.component_revision)
        );
        assert_eq!(response.agent_id, harness.worker_client.invoked_agent_id());
        assert_eq!(
            harness.worker_client.invocation_environment(),
            harness.environment_id
        );
        assert!(phantom_id(&response.agent_id).is_some());
        let invocations = harness.worker_client.invocations();
        assert_eq!(invocations.len(), 1);
        assert_eq!(response.idempotency_key, invocations[0].1);
        assert_eq!(invocations[0].2, InvocationFreshnessDisposition::KnownFresh);
        assert_eq!(
            phantom_id(&invocations[0].0),
            Some(ephemeral_invocation_phantom_id(&invocations[0].1))
        );
    }

    #[test]
    async fn public_invocation_session_resolves_normalizes_and_builds_trusted_start() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        let idempotency_key = IdempotencyKey::new("public-session-key".to_string());

        let _responses = harness
            .worker_service
            .invoke_public_agent_session(
                harness.public_invocation_start(idempotency_key.clone()),
                Box::pin(stream::empty()),
                AuthCtx::system(),
            )
            .await
            .unwrap();

        let (routed_agent_id, start) = harness.worker_client.invocation_session_start();
        let trusted_agent_id: AgentId = start.agent_id.clone().unwrap().try_into().unwrap();
        let trusted_idempotency_key: IdempotencyKey = start.idempotency_key.clone().unwrap().into();

        assert_eq!(routed_agent_id, trusted_agent_id);
        assert_eq!(trusted_agent_id.component_id, harness.component_id);
        assert_eq!(trusted_idempotency_key, idempotency_key);
        assert_eq!(
            phantom_id(&trusted_agent_id),
            Some(ephemeral_invocation_phantom_id(&idempotency_key))
        );
        assert_eq!(start.method_name.as_deref(), Some("run"));
        assert!(start.input.is_some());
        assert!(start.auth_ctx.is_some());
        assert!(start.principal.is_some());
        assert_eq!(
            EnvironmentId::try_from(start.environment_id.unwrap()).unwrap(),
            harness.environment_id
        );
        assert!(start.component_owner_account_id.is_some());
        assert_eq!(
            start.mode(),
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await
        );
        assert!(start.schedule_at.is_none());
        assert_eq!(
            start.freshness_disposition(),
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
        );
    }

    #[test]
    async fn public_invocation_session_validates_and_preserves_live_stream_references() {
        use golem_api_grpc::proto::golem::schema::{
            RecordValue, SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, schema_value,
        };

        let harness = RestHarness::new_with_input(
            AgentMode::Durable,
            InputSchema::Parameters(vec![NamedField::user_supplied(
                "input",
                SchemaType::stream(Some(SchemaType::u32())),
            )]),
        );
        let method_parameters = ProtoSchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![ProtoSchemaValue {
                    value: Some(schema_value::Value::StreamReference(
                        SchemaValueStreamReference { stream_id: 1 },
                    )),
                }],
            })),
        };
        let mut start = harness.public_invocation_start(IdempotencyKey::fresh());
        start.method_parameters = Some(method_parameters.clone());

        let _responses = harness
            .worker_service
            .invoke_public_agent_session(start, Box::pin(stream::empty()), AuthCtx::system())
            .await
            .unwrap();

        let (_, trusted_start) = harness.worker_client.invocation_session_start();
        assert_eq!(trusted_start.input, Some(method_parameters));
    }

    #[test]
    fn public_invocation_values_reject_capabilities_and_constructor_streams_recursively() {
        use golem_api_grpc::proto::golem::schema::{
            RecordValue, SchemaValue as ProtoSchemaValue, SchemaValueStreamReference, SecretValue,
            schema_value,
        };

        let nested = |value| ProtoSchemaValue {
            value: Some(schema_value::Value::RecordValue(RecordValue {
                fields: vec![ProtoSchemaValue { value: Some(value) }],
            })),
        };

        let secret = nested(schema_value::Value::SecretValue(SecretValue::default()));
        assert!(
            decode_public_schema_value(secret, true)
                .unwrap_err()
                .contains("host-managed capability")
        );

        let constructor_stream = nested(schema_value::Value::StreamReference(
            SchemaValueStreamReference { stream_id: 7 },
        ));
        assert!(
            decode_public_schema_value(constructor_stream, false)
                .unwrap_err()
                .contains("not valid in constructor parameters")
        );
    }

    #[test]
    async fn unauthorized_public_invocation_session_never_reaches_worker_dispatch() {
        let harness = RestHarness::new(AgentMode::Durable);
        let idempotency_key = IdempotencyKey::fresh();
        let auth = AuthCtx::agent_with_effective_surface(
            AccountId(Uuid::new_v4()),
            AccountEmail::new("unauthorized@golem"),
            EffectiveSurface {
                source_card_ids: vec![],
                lower: vec![],
                upper: vec![],
            },
        );

        let error = match harness
            .worker_service
            .invoke_public_agent_session(
                harness.public_invocation_start(idempotency_key),
                Box::pin(stream::empty()),
                auth,
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("an empty permission surface must reject the resolved agent selector"),
        };

        assert!(matches!(error, WorkerServiceError::AuthError(_)));
        assert_eq!(harness.worker_client.invocation_session_start_count(), 0);
    }

    #[test]
    async fn non_attached_streaming_modes_are_rejected_before_worker_dispatch() {
        let harness = RestHarness::new_with_output(
            AgentMode::Durable,
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u8())))),
        );

        for (mode, schedule_at) in [
            (AgentInvocationMode::Await, None),
            (AgentInvocationMode::Schedule, None),
            (AgentInvocationMode::Schedule, Some(Utc::now())),
        ] {
            let mut request = harness.invoke_request();
            request.mode = mode;
            request.schedule_at = schedule_at;
            let error = harness
                .worker_service
                .invoke_agent_rest(request, AuthCtx::system())
                .await
                .expect_err("non-attached invocation must reject streaming methods");

            assert!(
                error
                    .to_string()
                    .contains("require an attached invocation session"),
                "unexpected error: {error}"
            );
        }
        assert!(
            harness.worker_client.invocations().is_empty(),
            "rejection must happen before scheduling, executor dispatch, enqueue, or result storage"
        );
    }

    #[test]
    async fn non_attached_stream_free_modes_still_dispatch() {
        let harness = RestHarness::new(AgentMode::Durable);

        for (mode, schedule_at) in [
            (AgentInvocationMode::Await, None),
            (AgentInvocationMode::Schedule, None),
            (AgentInvocationMode::Schedule, Some(Utc::now())),
        ] {
            let mut request = harness.invoke_request();
            request.mode = mode;
            request.schedule_at = schedule_at;
            harness
                .worker_service
                .invoke_agent_rest(request, AuthCtx::system())
                .await
                .expect("stream-free one-shot invocation must still dispatch");
        }

        assert_eq!(harness.worker_client.invocations().len(), 3);
    }

    #[test]
    async fn non_attached_classification_uses_the_existing_workers_component_revision() {
        let stream_output =
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u8()))));
        let pinned_streaming = RestHarness::new_with_pinned_and_latest_output(
            stream_output.clone(),
            OutputSchema::Unit,
        );

        let error = pinned_streaming
            .worker_service
            .invoke_agent_rest(pinned_streaming.invoke_request(), AuthCtx::system())
            .await
            .expect_err("the pinned streaming revision must be rejected");
        assert!(
            error
                .to_string()
                .contains("require an attached invocation session"),
            "unexpected error: {error}"
        );
        assert!(pinned_streaming.worker_client.invocations().is_empty());

        let pinned_stream_free =
            RestHarness::new_with_pinned_and_latest_output(OutputSchema::Unit, stream_output);
        pinned_stream_free
            .worker_service
            .invoke_agent_rest(pinned_stream_free.invoke_request(), AuthCtx::system())
            .await
            .expect("the pinned stream-free revision must remain dispatchable");
        assert_eq!(pinned_stream_free.worker_client.invocations().len(), 1);
    }

    #[test]
    async fn ephemeral_rest_invocations_get_fresh_identities_per_request() {
        let harness = RestHarness::new(AgentMode::Ephemeral);

        let first = harness
            .worker_service
            .invoke_agent_rest(harness.invoke_request(), AuthCtx::system())
            .await
            .unwrap();
        let second = harness
            .worker_service
            .invoke_agent_rest(harness.invoke_request(), AuthCtx::system())
            .await
            .unwrap();

        let invocations = harness.worker_client.invocations();
        assert_eq!(invocations.len(), 2);
        assert_ne!(invocations[0].1, invocations[1].1);
        assert_ne!(invocations[0].0, invocations[1].0);
        assert_eq!(first.agent_id, invocations[0].0);
        assert_eq!(first.idempotency_key, invocations[0].1);
        assert_eq!(second.agent_id, invocations[1].0);
        assert_eq!(second.idempotency_key, invocations[1].1);
        assert!(
            invocations
                .iter()
                .all(|invocation| invocation.2 == InvocationFreshnessDisposition::KnownFresh)
        );
    }

    #[test]
    async fn caller_supplied_key_derives_stable_ephemeral_identity_conservatively() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        let idempotency_key = IdempotencyKey::new("caller-selected-key".to_string());
        let mut request = harness.invoke_request();
        request.idempotency_key = Some(idempotency_key.clone());

        harness
            .worker_service
            .invoke_agent_rest(request.clone(), AuthCtx::system())
            .await
            .unwrap();
        harness
            .worker_service
            .invoke_agent_rest(request, AuthCtx::system())
            .await
            .unwrap();

        let invocations = harness.worker_client.invocations();
        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].0, invocations[1].0);
        assert_eq!(invocations[0].1, idempotency_key);
        assert_eq!(invocations[1].1, idempotency_key);
        assert!(
            invocations
                .iter()
                .all(|invocation| invocation.2 == InvocationFreshnessDisposition::MayExist)
        );
    }

    #[test]
    async fn explicit_ephemeral_phantom_is_rejected_for_invocation() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        let explicit_phantom = Uuid::new_v4();
        let mut request = harness.invoke_request();
        request.phantom_id = Some(explicit_phantom);

        let result = harness
            .worker_service
            .invoke_agent_rest(request, AuthCtx::system())
            .await;

        assert!(matches!(
            result,
            Err(WorkerServiceError::TypeChecker(message))
                if message.starts_with("An ephemeral invocation cannot select a phantom ID")
        ));
        assert!(harness.worker_client.invocations().is_empty());
    }

    #[test]
    async fn ephemeral_lookup_accepts_the_final_invocation_identity() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        let final_phantom_id = Uuid::new_v4();
        let idempotency_key = IdempotencyKey::fresh();
        let agent_id = build_public_invocation_agent_id(
            harness.component_id,
            harness.agent_type_name.clone(),
            empty_constructor_parameters(),
            Some(final_phantom_id),
        )
        .unwrap();

        harness
            .worker_service
            .invoke_agent(
                &agent_id,
                None,
                None,
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32,
                None,
                Some(idempotency_key.clone()),
                None,
                false,
                InvocationFreshnessDisposition::MayExist,
                Vec::new(),
                AuthCtx::system(),
                Principal::anonymous().into(),
            )
            .await
            .unwrap();

        let invocations = harness.worker_client.invocations();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].0, agent_id);
        assert_eq!(invocations[0].1, idempotency_key);
        assert_eq!(invocations[0].2, InvocationFreshnessDisposition::MayExist);
    }

    #[test]
    async fn durable_rest_paths_keep_non_phantom_agent_ids() {
        let harness = RestHarness::new(AgentMode::Durable);

        let create_response = harness
            .worker_service
            .create_agent_rest(harness.create_request(), AuthCtx::system())
            .await
            .unwrap();
        let invoke_response = harness
            .worker_service
            .invoke_agent_rest(harness.invoke_request(), AuthCtx::system())
            .await
            .unwrap();

        assert_eq!(
            create_response.agent_id,
            harness.worker_client.created_agent_id()
        );
        assert_eq!(
            invoke_response.agent_id,
            harness.worker_client.invoked_agent_id()
        );
        assert!(phantom_id(&create_response.agent_id).is_none());
        assert!(phantom_id(&invoke_response.agent_id).is_none());
    }
}
