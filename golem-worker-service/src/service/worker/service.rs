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
    ConnectWorkerStream, InvocationRequestStream, InvocationResponseStream, OplogRangeRead,
    WorkerClient, WorkerServiceError,
};
use crate::api::agents::{
    AgentInvocationMode, AgentInvocationRequest, AgentInvocationResult, AgentKeyRequest,
    AgentReference, CreateAgentRequest, NativeToolDefinition, NativeToolDescribeRequest,
    NativeToolFailure, NativeToolInvocationMode, NativeToolInvocationRequest,
    NativeToolInvocationResponse, NativeToolResult, NativeToolSuccess,
};
use crate::invocation_session_token::{
    SessionAgentIdentity, SessionInvocationTarget, SessionTokenPayload,
};
use crate::service::agent_resolution_cache::AgentResolutionCache;
use crate::service::agent_types::AgentTypeResolver;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::component::ComponentService;
use crate::service::limit::LimitService;
use crate::service::worker::RoutedTarget;
use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use golem_api_grpc::invocation_session_protocol::InvocationSessionState;
use golem_api_grpc::proto::golem::worker::invocation_request;
use golem_api_grpc::proto::golem::worker::{
    ExternalToolInvocation, InvocationContext, InvocationRequest, InvocationStart, ResumeAttach,
};
use golem_api_grpc::proto::golem::workerexecutor::v1::{
    CreateStreamSessionRequest, CreateStreamSessionSuccess, DurableStreamAttachmentControlRequest,
    ExportStreamControlResult, ReadStreamSlotRequest, ReadStreamSlotSuccess,
};
use golem_common::base_model::json::NormalizedJsonValue;
use golem_common::model::AgentInvocationOutput;
use golem_common::model::account::AccountId;
use golem_common::model::agent::AgentConfigEntryDto;
use golem_common::model::agent::AgentUpdateMode;
use golem_common::model::agent::RegisteredAgentType;
use golem_common::model::agent::{
    AgentConfigSource, AgentMode, AgentTypeName, InvocationFreshnessDisposition, OwnerKind,
    ParsedAgentKey,
};
use golem_common::model::agent::{
    AgentMetadataDto, AgentTarget, ResolvedRevert, RevertWorkerTarget, ScheduleId,
};
use golem_common::model::application::ApplicationName;
use golem_common::model::caller::{
    AccountAuthentication, AccountPrincipal, Caller, ClaimVisibility, Principal,
};
use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
use golem_common::model::card::{
    AgentInvocationIdPattern, AgentMethodName, AgentPluginName, AgentResourcePattern, AgentVerb,
    CardId, ClassPermissionTarget, FilesystemPathPattern, FilesystemResourcePattern,
    FilesystemVerb, OplogResourcePattern, OplogVerb, PermissionTarget, StoredCard,
};
use golem_common::model::component::{
    CanonicalFilePath, ComponentId, ComponentRevision, PluginPriority,
};
use golem_common::model::deployment::DeploymentRevision;
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::filesystem::{
    FileByteSelection, FileReadError, FileReadHead, validate_file_read_path,
};
use golem_common::model::golem_issuer::GolemIssuer;
use golem_common::model::invocation_session_public::{
    InvocationSelector, JsonMethodInput, PublicAgentTarget, PublicConfigEntry, PublicErrorCode,
    PublicNativeToolTarget, PublicTypedValue,
};
use golem_common::model::oplog::OplogCursor;
use golem_common::model::oplog::{OplogIndex, PublicOplogEntryWithIndex};
use golem_common::model::tool::{ToolBindingOwner, ToolName};
use golem_common::model::{
    AgentFilter, AgentFingerprint, AgentId, IdempotencyKey, InvocationRequestKey, OwnedAgentId,
    PromiseId, ScanCursor,
};
use golem_common::schema::ExternalSchemaValue;
use golem_common::schema::agent::AgentConfigDeclarationSchema;
use golem_common::schema::json_input_schema_value_to_typed_schema_value;
use golem_common::schema::public_json::{
    PublicSchemaValueError, PublicStreamReference, PublicStreamReferencePolicy,
    decode_public_schema_value as decode_public_json_schema_value,
};
use golem_common::schema::stream::SchemaValueStream;
use golem_common::schema::{
    FieldSource, InputSchema, NamedFieldType, ResultValuePayload, SchemaGraph, SchemaType,
    SchemaValue, TypedSchemaValue, UnionValuePayload, VariantValuePayload,
};
use golem_service_base::clients::registry::RegistryServiceError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::model::auth::AuthCtx;
use golem_service_base::model::component::Component;
use golem_service_base::model::{ComponentFileSystemNode, FileReadResponse, GetOplogResponse};
use std::pin::Pin;
use std::{collections::HashMap, sync::Arc};

fn public_input_graph(graph: &SchemaGraph, input_schema: &InputSchema) -> SchemaGraph {
    let fields = input_schema
        .fields()
        .iter()
        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
        .map(|field| NamedFieldType {
            name: field.name.clone(),
            body: field.schema.clone(),
            metadata: field.metadata.clone(),
        })
        .collect();
    SchemaGraph {
        defs: graph.defs.clone(),
        root: SchemaType::record(fields),
    }
}

fn decode_public_agent_config(
    graph: &SchemaGraph,
    declarations: &[AgentConfigDeclarationSchema],
    entries: Vec<PublicConfigEntry>,
) -> Result<Vec<AgentConfigEntryDto>, PublicSchemaValueError> {
    entries
        .into_iter()
        .map(|entry| {
            let declaration = declarations
                .iter()
                .find(|declaration| {
                    declaration.source == AgentConfigSource::Local && declaration.path == entry.path
                })
                .ok_or_else(|| {
                    PublicSchemaValueError::new(
                        PublicErrorCode::ValidationError,
                        format!(
                            "agent type does not declare local config {}",
                            entry.path.join(".")
                        ),
                    )
                })?;
            let value = golem_common::schema::render::from_json_value(
                graph,
                &declaration.value_type,
                &entry.value,
            )
            .map_err(|error| {
                PublicSchemaValueError::new(
                    PublicErrorCode::ValidationError,
                    format!(
                        "config value for path {} does not match its schema: {error}",
                        entry.path.join(".")
                    ),
                )
            })?;
            let value =
                golem_schema::schema::render::to_json_value(graph, &declaration.value_type, &value)
                    .map_err(|error| {
                        PublicSchemaValueError::new(
                            PublicErrorCode::ValidationError,
                            format!(
                                "config value for path {} cannot be encoded canonically: {error}",
                                entry.path.join(".")
                            ),
                        )
                    })?;
            Ok(AgentConfigEntryDto {
                path: entry.path,
                value: NormalizedJsonValue::new(value),
            })
        })
        .collect()
}

/// Checks a trusted resume request built from a public session against the invocation session
/// protocol before it is dispatched, so a malformed request produced by the public adapter
/// fails here with its protocol error instead of reaching the executor.
fn validate_trusted_resume_request(request: &InvocationRequest) -> Result<(), WorkerServiceError> {
    InvocationSessionState::default()
        .validate_trusted_request(request)
        .map_err(|error| {
            WorkerServiceError::Internal(format!(
                "public resume produced an invalid trusted request: {error}"
            ))
        })
}

pub(crate) fn decode_public_session_schema_value(
    value: golem_schema::proto::golem::schema::SchemaValue,
) -> Result<SchemaValue, String> {
    decode_public_schema_value(value, true)
}

fn decode_public_schema_value(
    value: golem_schema::proto::golem::schema::SchemaValue,
    allow_stream_references: bool,
) -> Result<SchemaValue, String> {
    use golem_schema::proto::golem::schema::{result_value, schema_value};

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
        value => golem_schema::proto::golem::schema::SchemaValue { value: Some(value) }.try_into(),
    }
}

fn start_environment_id(start: &InvocationStart) -> WorkerResult<EnvironmentId> {
    start
        .environment_id
        .ok_or_else(|| WorkerServiceError::TypeChecker("environment_id not found".to_string()))?
        .try_into()
        .map_err(WorkerServiceError::TypeChecker)
}

fn start_target(start: &InvocationStart) -> WorkerResult<AgentTarget> {
    start
        .target
        .clone()
        .ok_or_else(|| WorkerServiceError::TypeChecker("target not found".to_string()))?
        .try_into()
        .map_err(WorkerServiceError::TypeChecker)
}

fn proto_freshness_disposition(freshness_disposition: InvocationFreshnessDisposition) -> i32 {
    match freshness_disposition {
        InvocationFreshnessDisposition::MayExist => {
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist as i32
        }
        InvocationFreshnessDisposition::KnownFresh => {
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh as i32
        }
    }
}

fn parse_final_invocation_id(key: &str) -> WorkerResult<golem_common::model::InvocationId> {
    key.parse().map_err(|_| {
        WorkerExecutorError::invalid_request("Invalid invocation ID: expected a UUID").into()
    })
}

fn agent_verb_for_invocation_mode(mode: i32) -> AgentVerb {
    if mode == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32 {
        AgentVerb::View
    } else {
        AgentVerb::Invoke
    }
}

/// A permission that a request to an agent needs. Its owner is the agent's, which is known
/// only once the request knows the agent's component.
#[derive(Clone, Debug)]
pub(crate) enum AgentPermission {
    Agent(AgentVerb, AgentResourcePattern),
    Oplog(OplogResourcePattern),
    Filesystem(FilesystemVerb, FilesystemResourcePattern),
}

impl AgentPermission {
    fn filesystem(verb: FilesystemVerb, path: &CanonicalFilePath) -> WorkerResult<Self> {
        let resource = FilesystemPathPattern::parse(path.as_abs_str())
            .map(FilesystemResourcePattern::Path)
            .map_err(WorkerServiceError::TypeChecker)?;
        Ok(Self::Filesystem(verb, resource))
    }

    fn target(&self, owner: AgentOwnerPattern) -> PermissionTarget {
        match self {
            Self::Agent(verb, resource) => PermissionTarget::Agent(ClassPermissionTarget {
                owner,
                verb: Some(*verb),
                resource: resource.clone(),
            }),
            Self::Oplog(resource) => PermissionTarget::Oplog(ClassPermissionTarget {
                owner,
                verb: Some(OplogVerb::Read),
                resource: resource.clone(),
            }),
            Self::Filesystem(verb, resource) => {
                PermissionTarget::Filesystem(ClassPermissionTarget {
                    owner,
                    verb: Some(*verb),
                    resource: resource.clone(),
                })
            }
        }
    }
}

/// The owner of an agent of `component`. `agent_type` is the trusted name of the agent's
/// type, from its Create entry or from the request that creates it.
pub(crate) fn agent_owner(
    component: &Component,
    agent_id: AgentId,
    agent_type: Option<AgentTypeName>,
) -> AgentOwnerPattern {
    AgentOwnerPattern::Agent {
        account: component.account_email.clone(),
        application: component.application_name.clone(),
        environment: component.environment_name.clone(),
        component: component.component_name.clone(),
        agent: AgentOwnerLeafPattern::agent(agent_id, agent_type),
    }
}

fn authorize_permissions(
    auth_ctx: &AuthCtx,
    owner: &AgentOwnerPattern,
    permissions: &[AgentPermission],
) -> WorkerResult<()> {
    for permission in permissions {
        auth_ctx
            .authorize_permission(&permission.target(owner.clone()))
            .map_err(AuthServiceError::Unauthorized)?;
    }
    Ok(())
}

/// Authorizes a request that names the agent by its key or creates it, against the
/// component that implements its type.
fn authorize_target(
    auth_ctx: &AuthCtx,
    component: &Component,
    target: &RoutedTarget,
    verb: AgentVerb,
    resource: AgentResourcePattern,
) -> WorkerResult<()> {
    authorize_permissions(
        auth_ctx,
        &agent_owner(component, target.agent_id, target.agent_type.clone()),
        &[AgentPermission::Agent(verb, resource)],
    )
}

/// Authorizes a request to every agent of a type, such as a listing.
fn authorize_agent_type(
    auth_ctx: &AuthCtx,
    component: &Component,
    agent_type: &AgentTypeName,
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
                agent: AgentOwnerLeafPattern::AgentTypeWildcard(agent_type.clone()),
            },
            verb: Some(verb),
            resource,
        }))
        .map_err(AuthServiceError::Unauthorized)?;

    Ok(())
}

/// Authorizes a request to every agent of a component, such as describing its baseline tools.
fn authorize_component_agents(
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

fn is_agent_not_found(error: &WorkerServiceError) -> bool {
    matches!(
        error,
        WorkerServiceError::AgentNotFound(_)
            | WorkerServiceError::GolemError(WorkerExecutorError::AgentNotFound { .. })
    )
}

/// The agent that an internal caller, such as an HTTP route, selects beside its constructor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentBinding {
    /// The keyed agent of the constructor. An ephemeral invocation runs on an instance of its
    /// own.
    Constructor,
    /// A new durable agent without a key for each invocation. An ephemeral type behaves as
    /// with `Constructor`.
    Fresh,
    /// The agent that serves one Durable Streams session of a route.
    StreamSession {
        route_scope: String,
        session: String,
    },
    /// The existing agent with this ID. It is never created; a missing agent is not found.
    Id(AgentId),
}

/// An existing agent that a request names by its ID, with the component of its Create entry.
pub struct ManagedAgent {
    pub agent_id: OwnedAgentId,
    pub component: Component,
    pub metadata: AgentMetadataDto,
}

impl ManagedAgent {
    fn owner(&self) -> AgentOwnerPattern {
        agent_owner(
            &self.component,
            self.agent_id.agent_id,
            self.metadata.agent_type_name.clone(),
        )
    }

    /// Authorizes a request to the agent. A denial has the same text as the denial of a
    /// missing agent.
    fn authorize(&self, auth_ctx: &AuthCtx, permissions: &[AgentPermission]) -> WorkerResult<()> {
        authorize_permissions(auth_ctx, &self.owner(), permissions)
            .map_err(|_| WorkerServiceError::AgentAccessDenied(self.agent_id.agent_id))
    }

    fn routed(&self) -> RoutedTarget {
        RoutedTarget::existing(
            self.agent_id.agent_id,
            self.metadata.agent_type_name.clone(),
        )
    }
}

pub struct WorkerService {
    component_service: Arc<dyn ComponentService>,
    _auth_service: Arc<dyn AuthService>,
    limit_service: Arc<dyn LimitService>,
    worker_client: Arc<dyn WorkerClient>,
    agent_resolution_cache: Arc<AgentResolutionCache>,
    agent_types: Arc<AgentTypeResolver>,
    golem_issuer: GolemIssuer,
}

pub struct PublicAgentSessionStart {
    pub environment_id: EnvironmentId,
    pub selector: InvocationSelector,
    pub config: Vec<PublicConfigEntry>,
    pub idempotency_key: String,
    pub attempt_id: uuid::Uuid,
    pub method_parameters: JsonMethodInput,
}

pub struct PublicToolSessionStart {
    pub environment_id: EnvironmentId,
    pub application: String,
    pub environment: String,
    pub target: PublicNativeToolTarget,
    pub tool_name: String,
    pub command_path: Vec<String>,
    pub input: PublicTypedValue,
    pub stdin: bool,
    pub stdout: bool,
    pub stderr: bool,
    pub idempotency_key: String,
    pub attempt_id: uuid::Uuid,
    pub expected_deployment_revision: Option<DeploymentRevision>,
}

pub struct StartedPublicAgentSession {
    pub responses: InvocationResponseStream,
    pub initial_request: InvocationRequest,
    pub schema: SchemaGraph,
    pub output_schema: Option<SchemaType>,
    pub agent_id: AgentId,
    pub component_id: ComponentId,
    pub application: ApplicationName,
    pub environment: EnvironmentName,
    pub target: SessionInvocationTarget,
    pub component_revision: ComponentRevision,
}

pub struct PublicAgentSessionResume {
    pub environment_id: EnvironmentId,
    pub identity: SessionAgentIdentity,
    pub session: SessionTokenPayload,
    pub attempt_id: uuid::Uuid,
    pub operation: golem_api_grpc::proto::golem::worker::ResumeOperation,
    pub cursors: Vec<golem_api_grpc::proto::golem::worker::StreamCursor>,
}

#[derive(Debug)]
pub enum PublicAgentSessionStartError {
    Protocol(PublicSchemaValueError),
    Worker(WorkerServiceError),
}

impl From<PublicSchemaValueError> for PublicAgentSessionStartError {
    fn from(value: PublicSchemaValueError) -> Self {
        Self::Protocol(value)
    }
}

impl From<WorkerServiceError> for PublicAgentSessionStartError {
    fn from(value: WorkerServiceError) -> Self {
        Self::Worker(value)
    }
}

impl WorkerService {
    pub fn new(
        component_service: Arc<dyn ComponentService>,
        auth_service: Arc<dyn AuthService>,
        limit_service: Arc<dyn LimitService>,
        worker_client: Arc<dyn WorkerClient>,
        agent_resolution_cache: Arc<AgentResolutionCache>,
        agent_types: Arc<AgentTypeResolver>,
        golem_issuer: GolemIssuer,
    ) -> Self {
        Self {
            component_service,
            _auth_service: auth_service,
            limit_service,
            worker_client,
            agent_resolution_cache,
            agent_types,
            golem_issuer,
        }
    }

    /// The caller of a request that an account token authenticated: the account in both roles.
    pub fn rest_caller(&self, auth: &AuthCtx) -> Caller {
        Caller::entry(Principal::Account(AccountPrincipal {
            issuer: self.golem_issuer.to_string(),
            account_id: auth.account_id(),
            authentication: AccountAuthentication::ApiToken,
        }))
    }

    /// Reads the agent that a request names by its ID; this never creates the agent. When it
    /// is missing, a caller that `permissions` would allow on the type's current implementer
    /// gets not found, and every other caller gets the denial that an existing agent gives, so
    /// the answer never tells an unauthorized caller whether the agent exists. A caller that
    /// expects another incarnation gets not found.
    async fn read_agent(
        &self,
        agent_id: &OwnedAgentId,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: &AuthCtx,
        permissions: &[AgentPermission],
    ) -> WorkerResult<ManagedAgent> {
        match self
            .worker_client
            .get_metadata(
                &agent_id.agent_id,
                agent_id.environment_id,
                auth_ctx.clone(),
            )
            .await
        {
            Ok(existing) => {
                let component = self
                    .component_service
                    .get_revision(existing.component_id, existing.metadata.component_revision)
                    .await?;
                let agent = ManagedAgent {
                    agent_id: agent_id.clone(),
                    component,
                    metadata: existing.metadata,
                };
                if expected_fingerprint
                    .is_some_and(|expected| expected != agent.metadata.fingerprint)
                {
                    agent.authorize(auth_ctx, permissions)?;
                    return Err(WorkerServiceError::AgentNotFound(agent_id.agent_id));
                }
                Ok(agent)
            }
            Err(error) if is_agent_not_found(&error) => {
                self.authorize_missing(agent_id, auth_ctx, permissions)
                    .await?;
                Err(WorkerServiceError::AgentNotFound(agent_id.agent_id))
            }
            Err(error) => Err(error),
        }
    }

    /// Reads the agent, then authorizes `permissions` against its Create entry.
    async fn managed_agent(
        &self,
        agent_id: &OwnedAgentId,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: &AuthCtx,
        permissions: &[AgentPermission],
    ) -> WorkerResult<ManagedAgent> {
        let agent = self
            .read_agent(agent_id, expected_fingerprint, auth_ctx, permissions)
            .await?;
        agent.authorize(auth_ctx, permissions)?;
        Ok(agent)
    }

    async fn authorize_missing(
        &self,
        agent_id: &OwnedAgentId,
        auth_ctx: &AuthCtx,
        permissions: &[AgentPermission],
    ) -> WorkerResult<()> {
        if auth_ctx.is_system() {
            return Ok(());
        }
        let denied = || WorkerServiceError::AgentAccessDenied(agent_id.agent_id);
        self.agent_types
            .authorize_environment(agent_id.environment_id, auth_ctx)
            .await
            .map_err(|error| match error {
                RegistryServiceError::Unauthorized(_)
                | RegistryServiceError::NotFound(_)
                | RegistryServiceError::CouldNotAuthenticate(_) => denied(),
                other => other.into(),
            })?;
        let Some(tagged) = self
            .agent_types
            .tagged(agent_id.environment_id, agent_id.agent_id)
            .await?
        else {
            return Ok(());
        };
        let Some(implementer) = tagged.implemented_by else {
            return Ok(());
        };
        let component = self
            .component_service
            .get_revision(implementer.component_id, implementer.component_revision)
            .await?;
        if component.environment_id != agent_id.environment_id {
            return Err(denied());
        }
        authorize_permissions(
            auth_ctx,
            &agent_owner(&component, agent_id.agent_id, Some(tagged.name)),
            permissions,
        )
        .map_err(|_| denied())
    }

    /// The agent type of the environment's current deployment that has the name, with the
    /// component revision that implements it. A caller without the permission to view the
    /// type's agents gets not found, as from the registry's lookup by names.
    pub async fn deployed_agent_type(
        &self,
        environment_id: EnvironmentId,
        agent_type_name: &AgentTypeName,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(Arc<RegisteredAgentType>, Component)> {
        let not_found = || {
            WorkerServiceError::RegistryServiceError(RegistryServiceError::NotFound(format!(
                "Agent type {agent_type_name} not found"
            )))
        };
        let registered = self
            .agent_types
            .deployed(environment_id, agent_type_name)
            .await?
            .ok_or_else(not_found)?;
        let component = self
            .component_service
            .get_revision(
                registered.implemented_by.component_id,
                registered.implemented_by.component_revision,
            )
            .await?;
        if component.environment_id != environment_id {
            return Err(not_found());
        }
        authorize_agent_type(
            auth_ctx,
            &component,
            agent_type_name,
            AgentVerb::View,
            AgentResourcePattern::Empty,
        )
        .map_err(|_| not_found())?;
        Ok((registered, component))
    }

    /// Authorizes the retained namespace before scanning agents, without requiring its
    /// current implementer. Every returned agent still needs its recorded owner's permission.
    async fn retained_agent_types(
        &self,
        environment_id: EnvironmentId,
        name: &AgentTypeName,
        auth: &AuthCtx,
    ) -> WorkerResult<Vec<(RegisteredAgentType, Component)>> {
        let mut allowed = Vec::new();
        for registered in self.agent_types.retained(environment_id, name).await? {
            let component = self
                .component_service
                .get_revision(
                    registered.implemented_by.component_id,
                    registered.implemented_by.component_revision,
                )
                .await?;
            if authorize_agent_type(
                auth,
                &component,
                name,
                AgentVerb::View,
                AgentResourcePattern::Empty,
            )
            .is_ok()
            {
                allowed.push((registered, component));
            }
        }
        if allowed.is_empty() {
            return Err(
                RegistryServiceError::NotFound(format!("Agent type {name} not found")).into(),
            );
        }
        Ok(allowed)
    }

    /// The fingerprint of the incarnation a streaming invocation reaches. An expected
    /// fingerprint must match the existing agent; otherwise the agent is created or joined.
    #[allow(clippy::too_many_arguments)]
    async fn resolve_streaming_callee_fingerprint(
        &self,
        component: &Component,
        target: &RoutedTarget,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: AuthCtx,
        invocation_context: Option<InvocationContext>,
        caller: golem_api_grpc::proto::golem::component::Caller,
        expected: Option<golem_schema::proto::golem::common::Uuid>,
        invocation: InvocationStart,
    ) -> WorkerResult<AgentFingerprint> {
        if expected.is_some() || !target.creates() {
            let expected = expected.map(|expected| AgentFingerprint(uuid::Uuid::from(expected)));
            return match self
                .worker_client
                .get_metadata(&target.agent_id, component.environment_id, auth_ctx)
                .await
            {
                Ok(existing)
                    if expected
                        .is_none_or(|expected| existing.metadata.fingerprint == expected) =>
                {
                    Ok(existing.metadata.fingerprint)
                }
                Ok(_) => Err(WorkerServiceError::AgentNotFound(target.agent_id)),
                Err(error) if is_agent_not_found(&error) => {
                    Err(WorkerServiceError::AgentNotFound(target.agent_id))
                }
                Err(error) => Err(error),
            };
        }
        self.worker_client
            .create(
                target,
                environment_variables,
                config,
                true,
                component.account_id,
                component.environment_id,
                auth_ctx,
                invocation_context,
                caller,
                None,
                Some(invocation),
            )
            .await
            .map(|(_, fingerprint)| fingerprint)
    }

    /// Creates the agent of `target`, which `component` implements, or joins it when
    /// `ignore_already_existing` is set. The caller needs to be allowed to invoke `resource`
    /// on the agent.
    #[allow(clippy::too_many_arguments)]
    pub async fn create(
        &self,
        target: &RoutedTarget,
        component: &Component,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        caller: golem_api_grpc::proto::golem::component::Caller,
        idempotency_key: Option<IdempotencyKey>,
        resource: AgentResourcePattern,
    ) -> WorkerResult<(AgentId, ComponentRevision, AgentFingerprint)> {
        authorize_target(&auth_ctx, component, target, AgentVerb::Invoke, resource)?;

        let (agent_id, fingerprint) = self
            .worker_client
            .create(
                target,
                environment_variables,
                config,
                ignore_already_existing,
                component.account_id,
                component.environment_id,
                auth_ctx,
                invocation_context,
                caller,
                idempotency_key,
                None,
            )
            .await?;

        Ok((agent_id, component.revision, fingerprint))
    }

    /// Creates or joins the agent of a create request from the gRPC API: the keyed agent of
    /// `constructor`, or, from an internal caller, the agent without a key at `unkeyed_id`.
    /// With `prepare`, the agent is made ready for an invocation of `method_name`.
    #[allow(clippy::too_many_arguments)]
    pub async fn launch_agent(
        &self,
        component_id: ComponentId,
        constructor: ParsedAgentKey,
        unkeyed_id: Option<AgentId>,
        method_name: Option<String>,
        prepare: bool,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        caller: golem_api_grpc::proto::golem::component::Caller,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        invocation: Option<InvocationStart>,
    ) -> WorkerResult<(AgentId, ComponentRevision, AgentFingerprint)> {
        let component = self
            .component_service
            .get_current_by_id(component_id)
            .await?;
        let trusted_internal_caller = matches!(&auth_ctx, AuthCtx::System | AuthCtx::Agent(_));
        let (target, constructor) = match unkeyed_id {
            None => (
                AgentTarget::Key(
                    constructor
                        .canonical_key(&component.metadata)
                        .map_err(WorkerServiceError::TypeChecker)?,
                ),
                None,
            ),
            Some(agent_id) => (AgentTarget::Id(agent_id), Some(constructor)),
        };
        let resource = method_name
            .clone()
            .map(|method_name| AgentResourcePattern::Method(AgentMethodName(method_name)))
            .unwrap_or(AgentResourcePattern::Any);
        let (target, component) = self
            .resolve_requested_target(
                component.environment_id,
                target,
                constructor,
                Some(component_id),
                trusted_internal_caller,
                None,
                &auth_ctx,
                AgentPermission::Agent(AgentVerb::Invoke, resource.clone()),
            )
            .await?;
        if prepare {
            let method_name = method_name.ok_or_else(|| {
                WorkerServiceError::TypeChecker("Missing method name".to_string())
            })?;
            self.prepare_for_invocation(
                &target,
                &component,
                method_name,
                environment_variables,
                config,
                ignore_already_existing,
                auth_ctx,
                invocation_context,
                caller,
                idempotency_key,
                freshness_disposition,
                invocation,
            )
            .await
        } else {
            self.create(
                &target,
                &component,
                environment_variables,
                config,
                ignore_already_existing,
                auth_ctx,
                invocation_context,
                caller,
                idempotency_key,
                resource,
            )
            .await
        }
    }

    /// Creates or joins the agent of `target` for an invocation of `method_name`.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_for_invocation(
        &self,
        target: &RoutedTarget,
        component: &Component,
        method_name: String,
        environment_variables: HashMap<String, String>,
        config: Vec<AgentConfigEntryDto>,
        ignore_already_existing: bool,
        auth_ctx: AuthCtx,
        invocation_context: Option<golem_api_grpc::proto::golem::worker::InvocationContext>,
        caller: golem_api_grpc::proto::golem::component::Caller,
        idempotency_key: Option<IdempotencyKey>,
        freshness_disposition: InvocationFreshnessDisposition,
        invocation: Option<InvocationStart>,
    ) -> WorkerResult<(AgentId, ComponentRevision, AgentFingerprint)> {
        if invocation
            .as_ref()
            .is_some_and(|start| start.method_name.as_deref() != Some(method_name.as_str()))
        {
            return Err(WorkerServiceError::TypeChecker(
                "creation context does not match the prepared method".into(),
            ));
        }
        authorize_target(
            &auth_ctx,
            component,
            target,
            AgentVerb::Invoke,
            AgentResourcePattern::Method(AgentMethodName(method_name)),
        )?;

        let (agent_id, fingerprint) = self
            .worker_client
            .prepare(
                target,
                environment_variables,
                config,
                ignore_already_existing,
                component.account_id,
                component.environment_id,
                auth_ctx,
                invocation_context,
                caller,
                idempotency_key,
                freshness_disposition,
                invocation,
            )
            .await?;

        Ok((agent_id, component.revision, fingerprint))
    }

    pub async fn connect(
        &self,
        agent_id: &OwnedAgentId,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<ConnectWorkerStream> {
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::View,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;
        let account_id = agent.component.account_id;

        let stream = self
            .worker_client
            .connect(
                &agent_id.agent_id,
                agent_id.environment_id,
                account_id,
                auth_ctx,
            )
            .await?;

        self.limit_service
            .update_worker_connection_limit(account_id, &agent_id.agent_id, true)
            .await?;

        Ok(ConnectWorkerStream::new(
            stream,
            agent_id.agent_id,
            account_id,
            self.limit_service.clone(),
        ))
    }

    pub async fn delete(
        &self,
        agent_id: &OwnedAgentId,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        // Deleting an agent that does not exist is an error, and that answer has to survive
        // the routing layer's retries. A retried delete cannot tell "I already deleted it"
        // from "it was never here", but a read can, so the read settles whether the agent
        // was there when the caller asked, before any delete goes out.
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::Delete,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;

        self.worker_client
            .delete(
                &agent_id.agent_id,
                agent_id.environment_id,
                agent.metadata.fingerprint,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn complete_promise(
        &self,
        environment_id: EnvironmentId,
        promise_id: PromiseId,
        data: Vec<u8>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let agent_id = OwnedAgentId::new(environment_id, &promise_id.agent_id);
        self.managed_agent(
            &agent_id,
            None,
            &auth_ctx,
            &[AgentPermission::Agent(
                AgentVerb::Invoke,
                AgentResourcePattern::Any,
            )],
        )
        .await?;

        let result = self
            .worker_client
            .complete_promise(&promise_id, data, environment_id, auth_ctx)
            .await?;

        Ok(result)
    }

    pub async fn interrupt(
        &self,
        agent_id: &OwnedAgentId,
        recover_immediately: bool,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::Interrupt,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;

        self.worker_client
            .interrupt(
                &agent_id.agent_id,
                recover_immediately,
                agent_id.environment_id,
                agent.metadata.fingerprint,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn get_metadata(
        &self,
        agent_id: &OwnedAgentId,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<AgentMetadataDto> {
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::View,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;
        Ok(agent.metadata)
    }

    /// Lists existing agents of a retained type, including removed declarations.
    #[allow(clippy::too_many_arguments)]
    pub async fn find_metadata(
        &self,
        environment_id: EnvironmentId,
        agent_type_name: &AgentTypeName,
        component_name: Option<&str>,
        filter: Option<AgentFilter>,
        cursor: ScanCursor,
        count: u64,
        precise: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<(Option<ScanCursor>, Vec<AgentMetadataDto>)> {
        let registered = self
            .retained_agent_types(environment_id, agent_type_name, &auth_ctx)
            .await?;

        let (cursor, agents) = self
            .worker_client
            .find_metadata(
                registered[0].0.type_id,
                filter,
                cursor,
                count,
                precise,
                environment_id,
                auth_ctx.clone(),
            )
            .await?;
        let mut metadata = Vec::with_capacity(agents.len());
        for agent in agents {
            let component = self
                .component_service
                .get_revision(agent.component_id, agent.metadata.component_revision)
                .await?;
            if component_name.is_some_and(|name| name != component.component_name.0) {
                continue;
            }
            if authorize_agent_type(
                &auth_ctx,
                &component,
                agent_type_name,
                AgentVerb::View,
                AgentResourcePattern::Empty,
            )
            .is_err()
            {
                continue;
            }
            metadata.push(agent.metadata);
        }
        Ok((cursor, metadata))
    }

    pub async fn resume(
        &self,
        agent_id: &OwnedAgentId,
        force: bool,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::Resume,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;

        self.worker_client
            .resume(
                &agent_id.agent_id,
                force,
                agent_id.environment_id,
                agent.metadata.fingerprint,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    /// `requester` is who asked for the update, which the agent records; `None` records the
    /// account of `auth_ctx`.
    #[allow(clippy::too_many_arguments)]
    pub async fn update(
        &self,
        agent_id: &OwnedAgentId,
        update_mode: AgentUpdateMode,
        target_revision: ComponentRevision,
        disable_wakeup: bool,
        expected_fingerprint: Option<AgentFingerprint>,
        requester: Option<Caller>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<OplogIndex> {
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::UpdateRevision,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;

        let requester = requester.unwrap_or_else(|| self.rest_caller(&auth_ctx));
        self.worker_client
            .update(
                &agent_id.agent_id,
                update_mode,
                target_revision,
                disable_wakeup,
                requester,
                agent_id.environment_id,
                agent.metadata.fingerprint,
                auth_ctx,
            )
            .await
    }

    /// Reads an oplog page. With `range`, the read is a range read: it reads no entry after
    /// `range.to`, and the response accounts its scan. `claims` asks for the users' verified
    /// claims among the callers; it reaches the executor only after the oplog read check.
    pub async fn get_oplog(
        &self,
        agent_id: &OwnedAgentId,
        from_oplog_index: OplogIndex,
        cursor: Option<OplogCursor>,
        count: u64,
        expected_fingerprint: Option<AgentFingerprint>,
        range: Option<OplogRangeRead>,
        claims: ClaimVisibility,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        let start = cursor
            .as_ref()
            .map(|cursor| cursor.next_oplog_index)
            .unwrap_or_else(|| from_oplog_index.into());
        let end = start
            .checked_add(count.saturating_sub(1))
            .map(|end| match range {
                Some(range) => end.min(range.to.into()),
                None => end,
            });
        self.managed_agent(
            agent_id,
            expected_fingerprint,
            &auth_ctx,
            &[AgentPermission::Oplog(OplogResourcePattern::range(
                Some(start),
                end,
            ))],
        )
        .await?;

        self.worker_client
            .get_oplog(
                &agent_id.agent_id,
                from_oplog_index,
                cursor,
                count,
                range,
                claims,
                agent_id.environment_id,
                auth_ctx,
            )
            .await
    }

    /// Reads one oplog entry in full. Oplog pages omit the entries that are too large for them;
    /// this returns such an entry. `component_revision` is the revision that the page reported
    /// for the omitted entry; without it, the executor scans the oplog for it. `claims`
    /// reaches the executor only after the oplog read check.
    pub async fn get_oplog_entry(
        &self,
        agent_id: &OwnedAgentId,
        oplog_index: OplogIndex,
        component_revision: Option<ComponentRevision>,
        expected_fingerprint: Option<AgentFingerprint>,
        claims: ClaimVisibility,
        auth_ctx: AuthCtx,
    ) -> Result<PublicOplogEntryWithIndex, WorkerServiceError> {
        self.managed_agent(
            agent_id,
            expected_fingerprint,
            &auth_ctx,
            &[AgentPermission::Oplog(OplogResourcePattern::range(
                Some(oplog_index.into()),
                Some(oplog_index.into()),
            ))],
        )
        .await?;

        self.worker_client
            .get_oplog_entry(
                &agent_id.agent_id,
                oplog_index,
                component_revision,
                claims,
                agent_id.environment_id,
                auth_ctx,
            )
            .await
    }

    /// Searches the oplog. `claims` reaches the executor only after the oplog read
    /// check.
    pub async fn search_oplog(
        &self,
        agent_id: &OwnedAgentId,
        cursor: Option<OplogCursor>,
        count: u64,
        query: String,
        expected_fingerprint: Option<AgentFingerprint>,
        claims: ClaimVisibility,
        auth_ctx: AuthCtx,
    ) -> Result<GetOplogResponse, WorkerServiceError> {
        self.managed_agent(
            agent_id,
            expected_fingerprint,
            &auth_ctx,
            &[AgentPermission::Oplog(OplogResourcePattern::Any)],
        )
        .await?;

        self.worker_client
            .search_oplog(
                &agent_id.agent_id,
                cursor,
                count,
                query,
                claims,
                agent_id.environment_id,
                auth_ctx,
            )
            .await
    }

    pub async fn get_file_system_node(
        &self,
        agent_id: &OwnedAgentId,
        path: CanonicalFilePath,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[
                    AgentPermission::filesystem(FilesystemVerb::Stat, &path)?,
                    AgentPermission::filesystem(FilesystemVerb::List, &path)?,
                ],
            )
            .await?;

        self.worker_client
            .get_file_system_node(
                &agent_id.agent_id,
                path,
                agent_id.environment_id,
                agent.component.account_id,
                auth_ctx,
            )
            .await
    }

    pub async fn get_agent_wallet(
        &self,
        agent_id: &OwnedAgentId,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<StoredCard>> {
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::View,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;

        self.worker_client
            .get_agent_wallet(
                &agent_id.agent_id,
                agent_id.environment_id,
                agent.component.account_id,
                auth_ctx,
            )
            .await
    }

    /// Reads a path from an existing agent selected by an authorized HTTP filesystem mount.
    /// Key selection verifies ownership before reading and never creates the agent.
    pub(crate) async fn read_mounted_file(
        &self,
        component_id: ComponentId,
        agent_type: &AgentTypeName,
        constructor: Option<&ParsedAgentKey>,
        binding: AgentBinding,
        path: &str,
        selection: FileByteSelection,
        environment_id: EnvironmentId,
        account_id: AccountId,
    ) -> WorkerResult<FileReadResponse> {
        validate_file_read_path(path)?;
        let path =
            CanonicalFilePath::from_abs_str(path).map_err(|_| FileReadError::InvalidTarget)?;
        let (registered, component) = self
            .implementing_component(component_id, agent_type)
            .await?;
        if component.environment_id != environment_id {
            return Err(WorkerServiceError::TypeChecker(
                "The component does not belong to the route's environment".to_string(),
            ));
        }
        let (target, key) = match (binding, constructor) {
            (AgentBinding::Constructor, Some(constructor)) => {
                let key = constructor
                    .canonical_key(&component.metadata)
                    .map_err(WorkerServiceError::TypeChecker)?;
                (
                    RoutedTarget::keyed(
                        environment_id,
                        registered.type_id,
                        &component,
                        constructor,
                    )?,
                    Some(key),
                )
            }
            (AgentBinding::Id(agent_id), None) => (
                RoutedTarget::existing(agent_id, Some(agent_type.clone())),
                None,
            ),
            _ => {
                return Err(WorkerServiceError::TypeChecker(
                    "Mounted files require a key or ID selection".into(),
                ));
            }
        };
        let agent_id = target.agent_id;
        if agent_id.type_id() != Some(registered.type_id) {
            return Err(WorkerServiceError::AgentNotFound(agent_id));
        }
        let agent = self
            .managed_agent(
                &OwnedAgentId::new(environment_id, &agent_id),
                None,
                &AuthCtx::System,
                &[AgentPermission::filesystem(FilesystemVerb::Read, &path)?],
            )
            .await
            .map_err(|error| {
                if is_agent_not_found(&error) {
                    WorkerServiceError::AgentNotFound(agent_id)
                } else {
                    error
                }
            })?;
        if agent.metadata.agent_type_name.as_ref() != Some(agent_type) {
            return Err(WorkerServiceError::AgentNotFound(agent_id));
        }
        if let Some(key) = &key
            && agent.metadata.canonical_key.as_deref() != Some(key.to_string().as_str())
        {
            return Err(WorkerExecutorError::AgentKeyCollision {
                agent_id,
                requested_key: key.to_string(),
            }
            .into());
        }
        self.worker_client
            .get_file_contents(
                &target,
                path,
                selection,
                environment_id,
                account_id,
                AuthCtx::System,
            )
            .await
            .map_err(|error| {
                if is_agent_not_found(&error) {
                    WorkerServiceError::AgentNotFound(agent_id)
                } else {
                    error
                }
            })
    }

    pub async fn get_file_contents(
        &self,
        agent_id: &OwnedAgentId,
        path: CanonicalFilePath,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::filesystem(FilesystemVerb::Read, &path)?],
            )
            .await?;

        let contents_stream = self
            .worker_client
            .get_file_contents(
                &RoutedTarget::existing(agent_id.agent_id, agent.metadata.agent_type_name.clone()),
                path.clone(),
                FileByteSelection::Full,
                agent_id.environment_id,
                agent.component.account_id,
                auth_ctx,
            )
            .await?;
        match contents_stream.head {
            FileReadHead::File(_) => Ok(Box::pin(
                contents_stream
                    .body
                    .map(|item| item.map_err(WorkerServiceError::FileRead)),
            )),
            FileReadHead::Absent => Err(WorkerServiceError::FileNotFound(path)),
            FileReadHead::NotRegular | FileReadHead::Symlink => {
                Err(WorkerServiceError::BadFileType(path))
            }
            FileReadHead::PermissionDenied => {
                Err(WorkerExecutorError::permission_denied("filesystem read denied").into())
            }
        }
    }

    /// The name of the plugin that the agent's type installs at the priority, from the
    /// component revision the agent runs.
    fn agent_plugin_name(
        agent: &ManagedAgent,
        plugin_priority: PluginPriority,
    ) -> WorkerResult<String> {
        let agent_type = agent.metadata.agent_type_name.clone().ok_or_else(|| {
            WorkerServiceError::TypeChecker("The agent has no agent type".to_string())
        })?;
        agent
            .component
            .metadata
            .agent_type_plugins(&agent_type)
            .and_then(|plugins| {
                plugins
                    .iter()
                    .find(|plugin| plugin.priority == plugin_priority)
            })
            .map(|plugin| plugin.plugin_name.clone())
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker(format!(
                    "Plugin priority {} is not installed for agent type {}",
                    plugin_priority.0, agent_type.0
                ))
            })
    }

    async fn plugin_agent(
        &self,
        agent_id: &OwnedAgentId,
        plugin_priority: PluginPriority,
        verb: AgentVerb,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<AgentFingerprint> {
        let agent = self
            .read_agent(
                agent_id,
                expected_fingerprint,
                auth_ctx,
                &[AgentPermission::Agent(verb, AgentResourcePattern::Any)],
            )
            .await?;
        let plugin_name = Self::agent_plugin_name(&agent, plugin_priority);
        let plugin_name = match plugin_name {
            Ok(plugin_name) => plugin_name,
            Err(error) => {
                agent.authorize(
                    auth_ctx,
                    &[AgentPermission::Agent(verb, AgentResourcePattern::Any)],
                )?;
                return Err(error);
            }
        };
        agent.authorize(
            auth_ctx,
            &[AgentPermission::Agent(
                verb,
                AgentResourcePattern::PluginName(AgentPluginName(plugin_name)),
            )],
        )?;
        Ok(agent.metadata.fingerprint)
    }

    pub async fn activate_plugin(
        &self,
        agent_id: &OwnedAgentId,
        plugin_priority: PluginPriority,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let fingerprint = self
            .plugin_agent(
                agent_id,
                plugin_priority,
                AgentVerb::ActivatePlugin,
                expected_fingerprint,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .activate_plugin(
                &agent_id.agent_id,
                plugin_priority,
                agent_id.environment_id,
                fingerprint,
                auth_ctx,
            )
            .await
    }

    pub async fn deactivate_plugin(
        &self,
        agent_id: &OwnedAgentId,
        plugin_priority: PluginPriority,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let fingerprint = self
            .plugin_agent(
                agent_id,
                plugin_priority,
                AgentVerb::DeactivatePlugin,
                expected_fingerprint,
                &auth_ctx,
            )
            .await?;

        self.worker_client
            .deactivate_plugin(
                &agent_id.agent_id,
                plugin_priority,
                agent_id.environment_id,
                fingerprint,
                auth_ctx,
            )
            .await
    }

    /// Forks the source agent into `target_agent_id`, or, without one, into a new ID that
    /// Golem issues with the source's type tag. Returns the fork's ID. `caller` is the
    /// forking caller the target records; `None` records the account of `auth_ctx`.
    pub async fn fork_worker(
        &self,
        source_agent_id: &OwnedAgentId,
        target_agent_id: Option<AgentId>,
        oplog_index_cut_off: OplogIndex,
        caller: Option<Caller>,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<AgentId> {
        let agent = self
            .managed_agent(
                source_agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::Fork,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;

        let target_agent_id = match target_agent_id {
            Some(target_agent_id) => target_agent_id,
            None => {
                let type_id = source_agent_id.agent_id.type_id().ok_or_else(|| {
                    WorkerServiceError::TypeChecker(
                        "An agent without an agent type cannot be forked".to_string(),
                    )
                })?;
                AgentId::random(type_id)
            }
        };
        let caller = caller.unwrap_or_else(|| self.rest_caller(&auth_ctx));
        self.worker_client
            .fork_worker(
                &source_agent_id.agent_id,
                &target_agent_id,
                oplog_index_cut_off,
                source_agent_id.environment_id,
                agent.component.account_id,
                agent.component.account_email.clone(),
                caller,
                agent.metadata.fingerprint,
                auth_ctx,
            )
            .await?;

        Ok(target_agent_id)
    }

    pub async fn revert_worker(
        &self,
        agent_id: &OwnedAgentId,
        target: RevertWorkerTarget,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        self.revert_worker_prepared(agent_id, target, None, expected_fingerprint, auth_ctx)
            .await
    }

    pub async fn revert_worker_prepared(
        &self,
        agent_id: &OwnedAgentId,
        target: RevertWorkerTarget,
        prepared_revert: Option<ResolvedRevert>,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        let agent = self
            .read_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::Revert,
                    AgentResourcePattern::Any,
                )],
            )
            .await?;

        let resolved_revert = match &target {
            RevertWorkerTarget::RevertToOplogIndex(_) => {
                if prepared_revert.is_some() {
                    return Err(WorkerExecutorError::invalid_request(
                        "Resolved revert must only be supplied for a count-based revert",
                    )
                    .into());
                }
                None
            }
            RevertWorkerTarget::RevertLastInvocations(target) => match prepared_revert {
                Some(resolved) => Some(resolved),
                None => match self
                    .worker_client
                    .resolve_revert_last_invocations(
                        &agent_id.agent_id,
                        target.number_of_invocations,
                        agent_id.environment_id,
                        auth_ctx.clone(),
                    )
                    .await
                {
                    Ok(resolved) => Some(resolved),
                    // The resolution failure describes the agent's history, so only a caller
                    // that may revert the agent at any cutoff sees it.
                    Err(error) => {
                        agent.authorize(
                            &auth_ctx,
                            &[AgentPermission::Agent(
                                AgentVerb::Revert,
                                AgentResourcePattern::Any,
                            )],
                        )?;
                        return Err(error);
                    }
                },
            },
        };
        let last_oplog_index = resolved_revert
            .as_ref()
            .map(|resolved| resolved.last_oplog_index)
            .unwrap_or_else(|| match &target {
                RevertWorkerTarget::RevertToOplogIndex(target) => target.last_oplog_index,
                RevertWorkerTarget::RevertLastInvocations(_) => unreachable!(),
            });
        agent.authorize(
            &auth_ctx,
            &[AgentPermission::Agent(
                AgentVerb::Revert,
                AgentResourcePattern::OplogIndex(last_oplog_index.into()),
            )],
        )?;

        self.worker_client
            .revert_worker(
                &agent_id.agent_id,
                target,
                resolved_revert,
                agent_id.environment_id,
                agent.metadata.fingerprint,
                auth_ctx,
            )
            .await?;

        Ok(())
    }

    pub async fn cancel_invocation(
        &self,
        agent_id: &OwnedAgentId,
        idempotency_key: &IdempotencyKey,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        let accepted_id = parse_final_invocation_id(&idempotency_key.value)?;
        let invocation_id = AgentInvocationIdPattern::Uuid(accepted_id.0);
        let agent = self
            .managed_agent(
                agent_id,
                expected_fingerprint,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::CancelInvocation,
                    AgentResourcePattern::InvocationId(invocation_id),
                )],
            )
            .await?;

        self.worker_client
            .cancel_invocation(
                &agent_id.agent_id,
                &accepted_id,
                agent_id.environment_id,
                auth_ctx,
                None,
                Some(agent.metadata.fingerprint),
            )
            .await
    }

    /// Internal token control uses the recorded scheduling caller, not operator permission: the
    /// target cancels only when `canceller` is the immediate caller recorded for the call.
    pub async fn cancel_scheduled_invocation(
        &self,
        target: &OwnedAgentId,
        key: &golem_common::model::InvocationId,
        receipt: ScheduleId,
        canceller: &golem_common::model::caller::Principal,
        auth: AuthCtx,
    ) -> WorkerResult<bool> {
        if !matches!(&auth, AuthCtx::System | AuthCtx::Agent(_)) {
            return Err(WorkerExecutorError::invalid_request(
                "schedule cancellation requires an authenticated internal caller",
            )
            .into());
        }
        self.worker_client
            .cancel_invocation(
                &target.agent_id(),
                key,
                target.environment_id,
                auth,
                Some(
                    golem_api_grpc::proto::golem::worker::ScheduledInvocationCancellation {
                        schedule_id: Some(receipt.id.into()),
                        target_environment_id: Some(target.environment_id.into()),
                        canceller: Some(canceller.clone().into()),
                    },
                ),
                None,
            )
            .await
    }

    pub async fn process_oplog_entries(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        idempotency_key: golem_common::model::InvocationId,
        _account_id: AccountId,
        config: std::collections::HashMap<String, String>,
        metadata: golem_api_grpc::proto::golem::worker::AgentMetadata,
        first_entry_index: OplogIndex,
        entries: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
        auth_ctx: AuthCtx,
        caller: golem_api_grpc::proto::golem::component::Caller,
    ) -> WorkerResult<()> {
        auth_ctx
            .authorize_system_only("process oplog entries")
            .map_err(AuthServiceError::Unauthorized)?;

        let component = self
            .component_service
            .get_revision(component_id, component_revision)
            .await?;

        self.worker_client
            .process_oplog_entries(
                target_agent_id,
                environment_id,
                component_id,
                component_revision,
                idempotency_key,
                component.account_id,
                config,
                metadata,
                first_entry_index,
                entries,
                auth_ctx,
                caller,
            )
            .await
    }

    pub async fn control_durable_stream_attachment(
        &self,
        producer_agent_id: &AgentId,
        producer_environment_id: EnvironmentId,
        consumer_agent_id: &AgentId,
        consumer_environment_id: EnvironmentId,
        expected_consumer_fingerprint: AgentFingerprint,
        payload: Vec<u8>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<bool> {
        self.managed_agent(
            &OwnedAgentId::new(producer_environment_id, producer_agent_id),
            None,
            &auth_ctx,
            &[AgentPermission::Agent(
                AgentVerb::Invoke,
                AgentResourcePattern::Any,
            )],
        )
        .await?;
        self.worker_client
            .control_durable_stream_attachment(
                producer_agent_id,
                producer_environment_id,
                consumer_agent_id,
                consumer_environment_id,
                expected_consumer_fingerprint,
                payload,
                auth_ctx,
            )
            .await
    }

    pub async fn read_durable_stream_segment(
        &self,
        producer_agent_id: &AgentId,
        producer_environment_id: EnvironmentId,
        consumer_agent_id: Option<&AgentId>,
        consumer_environment_id: Option<EnvironmentId>,
        expected_consumer_fingerprint: Option<AgentFingerprint>,
        payload: Vec<u8>,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<Vec<u8>> {
        let read: golem_common::model::durable_stream::DurableStreamReadRequest =
            golem_common::serialization::deserialize(&payload)
                .map_err(|error| WorkerServiceError::Internal(error.to_string()))?;
        match read {
            golem_common::model::durable_stream::DurableStreamReadRequest::AttachedConsumer(_) => {
                self.managed_agent(
                    &OwnedAgentId::new(producer_environment_id, producer_agent_id),
                    None,
                    &auth_ctx,
                    &[AgentPermission::Agent(
                        AgentVerb::View,
                        AgentResourcePattern::Any,
                    )],
                )
                .await?;
            }
            golem_common::model::durable_stream::DurableStreamReadRequest::AuthorizedExport(_) => {
                auth_ctx
                    .authorize_system_only("read authorized durable stream export")
                    .map_err(AuthServiceError::Unauthorized)?;
            }
        }
        self.worker_client
            .read_durable_stream_segment(
                producer_agent_id,
                producer_environment_id,
                consumer_agent_id,
                consumer_environment_id,
                expected_consumer_fingerprint,
                payload,
                auth_ctx,
            )
            .await
    }

    pub async fn deliver_card_transfer(
        &self,
        target_agent_id: &AgentId,
        environment_id: EnvironmentId,
        transfer_id: uuid::Uuid,
        source_card_id: CardId,
        card: StoredCard,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<()> {
        auth_ctx
            .authorize_system_only("deliver permission card transfer")
            .map_err(AuthServiceError::Unauthorized)?;

        self.worker_client
            .deliver_card_transfer(
                target_agent_id,
                environment_id,
                transfer_id,
                source_card_id,
                card,
                auth_ctx,
            )
            .await
    }

    /// Resolves a target that an internal caller sends. A key target, or an ID target with a
    /// constructor, may create the agent, which `component_id` implements; only a trusted
    /// internal caller may name the ID of an agent to create. An ID target alone names an
    /// existing agent, and the request is authorized against its Create entry.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn resolve_requested_target(
        &self,
        environment_id: EnvironmentId,
        target: AgentTarget,
        constructor: Option<ParsedAgentKey>,
        component_id: Option<ComponentId>,
        trusted_internal_caller: bool,
        expected_fingerprint: Option<AgentFingerprint>,
        auth_ctx: &AuthCtx,
        permission: AgentPermission,
    ) -> WorkerResult<(RoutedTarget, Component)> {
        match (target, constructor) {
            (AgentTarget::Key(key), None) => {
                let component = self
                    .creating_component(environment_id, component_id)
                    .await?;
                let registered = self
                    .agent_types
                    .deployed(environment_id, key.agent_type())
                    .await?
                    .ok_or_else(|| {
                        WorkerServiceError::TypeChecker(format!(
                            "Agent type {} is not deployed",
                            key.agent_type()
                        ))
                    })?;
                let target = RoutedTarget::keyed(
                    environment_id,
                    registered.type_id,
                    &component,
                    &ParsedAgentKey::from(key),
                )?;
                authorize_permissions(
                    auth_ctx,
                    &agent_owner(&component, target.agent_id, target.agent_type.clone()),
                    &[permission],
                )?;
                Ok((target, component))
            }
            (AgentTarget::Id(agent_id), Some(constructor)) => {
                if !trusted_internal_caller {
                    return Err(WorkerServiceError::TypeChecker(
                        "Only an internal caller may create an agent at a given ID".to_string(),
                    ));
                }
                let component = self
                    .creating_component(environment_id, component_id)
                    .await?;
                let target = RoutedTarget::unkeyed(agent_id, &component, &constructor)?;
                authorize_permissions(
                    auth_ctx,
                    &agent_owner(&component, target.agent_id, target.agent_type.clone()),
                    &[permission],
                )?;
                Ok((target, component))
            }
            (AgentTarget::Id(agent_id), None) => {
                let agent = self
                    .managed_agent(
                        &OwnedAgentId::new(environment_id, &agent_id),
                        expected_fingerprint,
                        auth_ctx,
                        &[permission],
                    )
                    .await?;
                Ok((agent.routed(), agent.component))
            }
            (AgentTarget::Key(_), Some(_)) => Err(WorkerServiceError::TypeChecker(
                "A key target has no separate constructor".to_string(),
            )),
        }
    }

    /// The component that implements the agent type of a request that creates an agent.
    async fn creating_component(
        &self,
        environment_id: EnvironmentId,
        component_id: Option<ComponentId>,
    ) -> WorkerResult<Component> {
        let component_id = component_id.ok_or_else(|| {
            WorkerServiceError::TypeChecker(
                "component_id is required for a request that creates".to_string(),
            )
        })?;
        let component = self
            .component_service
            .get_current_by_id(component_id)
            .await?;
        if component.environment_id != environment_id {
            return Err(WorkerServiceError::TypeChecker(
                "The component does not belong to the agent's environment".to_string(),
            ));
        }
        Ok(component)
    }

    /// The target of an invocation of `operation` on the agent of `constructor`, of the
    /// deployed agent type `registered` that `component` implements. A durable agent is the
    /// keyed agent of the constructor. An ephemeral invocation runs on an instance of its own,
    /// whose ID derives from the invocation's owner key, so a retry reaches the same instance;
    /// an invocation without an idempotency key reaches a fresh instance.
    pub(crate) fn invocation_target(
        environment_id: EnvironmentId,
        registered: &RegisteredAgentType,
        component: &Component,
        constructor: &ParsedAgentKey,
        request_key: &InvocationRequestKey,
        operation: &str,
    ) -> WorkerResult<(RoutedTarget, InvocationFreshnessDisposition)> {
        match registered.agent_type.mode {
            AgentMode::Durable => Ok((
                RoutedTarget::keyed(environment_id, registered.type_id, component, constructor)?,
                InvocationFreshnessDisposition::MayExist,
            )),
            AgentMode::Ephemeral => {
                let agent_id = AgentId::ephemeral_invocation(
                    environment_id,
                    registered.type_id,
                    &request_key.owner_key(operation),
                );
                let freshness_disposition =
                    if matches!(request_key, InvocationRequestKey::Unkeyed(_)) {
                        InvocationFreshnessDisposition::KnownFresh
                    } else {
                        InvocationFreshnessDisposition::MayExist
                    };
                Ok((
                    RoutedTarget::unkeyed(agent_id, component, constructor)?,
                    freshness_disposition,
                ))
            }
        }
    }

    /// The deployed agent type `agent_type` of the environment of `component_id`, which that
    /// component must implement, with the deployed revision of the component.
    async fn implementing_component(
        &self,
        component_id: ComponentId,
        agent_type: &AgentTypeName,
    ) -> WorkerResult<(Arc<RegisteredAgentType>, Component)> {
        let current = self
            .component_service
            .get_current_by_id(component_id)
            .await?;
        let registered = self
            .agent_types
            .deployed(current.environment_id, agent_type)
            .await?
            .filter(|registered| registered.implemented_by.component_id == component_id)
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker(format!(
                    "Agent type {agent_type} is not deployed on the component"
                ))
            })?;
        let component = self
            .component_service
            .get_revision(component_id, registered.implemented_by.component_revision)
            .await?;
        Ok((registered, component))
    }

    /// The target of an invocation of `operation` from an internal caller that names the agent
    /// by the component that implements its type and by its constructor, such as an HTTP route
    /// or an MCP tool, with the agent that `binding` selects. Returns the target, the deployed
    /// revision of the component, and the freshness of the target.
    #[allow(clippy::too_many_arguments)]
    pub async fn bound_invocation_target(
        &self,
        component_id: ComponentId,
        agent_type: &AgentTypeName,
        constructor: Option<&ParsedAgentKey>,
        binding: AgentBinding,
        request_key: &InvocationRequestKey,
        operation: &str,
        auth_ctx: &AuthCtx,
    ) -> WorkerResult<(RoutedTarget, Component, InvocationFreshnessDisposition)> {
        let (registered, component) = self
            .implementing_component(component_id, agent_type)
            .await?;
        let environment_id = component.environment_id;
        let (target, freshness_disposition) = match (binding, constructor) {
            (AgentBinding::Constructor, Some(constructor)) => Self::invocation_target(
                environment_id,
                &registered,
                &component,
                constructor,
                request_key,
                operation,
            )?,
            (AgentBinding::Fresh, Some(constructor))
                if registered.agent_type.mode == AgentMode::Durable =>
            {
                (
                    RoutedTarget::unkeyed(
                        AgentId::random(registered.type_id),
                        &component,
                        constructor,
                    )?,
                    InvocationFreshnessDisposition::MayExist,
                )
            }
            (AgentBinding::Fresh, Some(constructor)) => Self::invocation_target(
                environment_id,
                &registered,
                &component,
                constructor,
                request_key,
                operation,
            )?,
            (
                AgentBinding::StreamSession {
                    route_scope,
                    session,
                },
                Some(constructor),
            ) => (
                RoutedTarget::unkeyed(
                    AgentId::durable_stream_session(
                        environment_id,
                        registered.type_id,
                        &route_scope,
                        &session,
                    ),
                    &component,
                    constructor,
                )?,
                InvocationFreshnessDisposition::MayExist,
            ),
            (AgentBinding::Id(agent_id), _) => {
                if agent_id.type_id() != Some(registered.type_id) {
                    return Err(WorkerServiceError::AgentNotFound(agent_id));
                }
                (
                    RoutedTarget::existing(agent_id, Some(agent_type.clone())),
                    InvocationFreshnessDisposition::MayExist,
                )
            }
            _ => {
                return Err(WorkerServiceError::TypeChecker(
                    "A key or fresh target requires constructor parameters".into(),
                ));
            }
        };
        authorize_target(
            auth_ctx,
            &component,
            &target,
            AgentVerb::Invoke,
            AgentResourcePattern::Method(AgentMethodName(operation.to_string())),
        )?;
        Ok((target, component, freshness_disposition))
    }

    /// A one-shot invocation from an internal caller.
    #[allow(clippy::too_many_arguments)]
    pub async fn invoke_agent(
        &self,
        environment_id: EnvironmentId,
        target: AgentTarget,
        constructor: Option<ParsedAgentKey>,
        component_id: Option<ComponentId>,
        method_name: Option<String>,
        method_parameters: Option<golem_schema::proto::golem::schema::SchemaValue>,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        request_key: InvocationRequestKey,
        invocation_context: Option<InvocationContext>,
        trusted_internal_caller: bool,
        freshness_disposition: InvocationFreshnessDisposition,
        config: Vec<AgentConfigEntryDto>,
        auth_ctx: AuthCtx,
        caller: golem_api_grpc::proto::golem::component::Caller,
        scope_card: Option<golem_api_grpc::proto::golem::worker::EncodedScopeCard>,
        expected_fingerprint: Option<AgentFingerprint>,
    ) -> WorkerResult<AgentInvocationOutput> {
        let resource = method_name
            .as_ref()
            .map(|method_name| AgentResourcePattern::Method(AgentMethodName(method_name.clone())))
            .unwrap_or(AgentResourcePattern::Any);
        let (target, component) = self
            .resolve_requested_target(
                environment_id,
                target,
                constructor,
                component_id,
                trusted_internal_caller,
                expected_fingerprint,
                &auth_ctx,
                AgentPermission::Agent(agent_verb_for_invocation_mode(mode), resource),
            )
            .await?;
        let freshness_disposition = if trusted_internal_caller {
            freshness_disposition
        } else {
            InvocationFreshnessDisposition::MayExist
        };
        self.dispatch_agent_invocation(
            &component,
            &target,
            method_name,
            method_parameters
                .map(|input| {
                    golem_common::model::agent::NativeMethodInput::capture(input, Vec::new()).map(
                        |(input, _)| golem_common::model::agent::AgentMethodInput::Native(input),
                    )
                })
                .transpose()
                .map_err(WorkerServiceError::TypeChecker)?,
            mode,
            schedule_at,
            request_key,
            None,
            invocation_context,
            freshness_disposition,
            config,
            environment_id,
            component.account_id,
            auth_ctx,
            caller,
            scope_card,
            expected_fingerprint,
        )
        .await
    }

    pub async fn lookup_agent_invocation(
        &self,
        mut start: InvocationStart,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<AgentInvocationOutput> {
        let environment_id = start_environment_id(&start)?;
        let AgentTarget::Id(agent_id) = start_target(&start)? else {
            return Err(WorkerServiceError::TypeChecker(
                "a lookup names the agent by its ID".to_string(),
            ));
        };
        if start.idempotency_key.is_none() {
            return Err(WorkerServiceError::TypeChecker(
                "lookup requires a final invocation key".to_string(),
            ));
        }
        parse_final_invocation_id(&start.idempotency_key.as_ref().unwrap().value)?;
        let agent = self
            .managed_agent(
                &OwnedAgentId::new(environment_id, &agent_id),
                None,
                &auth_ctx,
                &[AgentPermission::Agent(
                    AgentVerb::View,
                    AgentResourcePattern::Any,
                )],
            )
            .await?;
        agent.routed().apply(&mut start);
        start.mode = golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32;
        start.freshness_disposition =
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist as i32;
        start.component_owner_account_id = Some(agent.component.account_id.into());
        start.auth_ctx = Some(auth_ctx.into());
        self.worker_client
            .invoke_agent_session_one_shot(&agent_id, start)
            .await
    }

    pub async fn invoke_agent_session(
        &self,
        mut start: InvocationStart,
        tail: InvocationRequestStream,
        trusted_internal_caller: bool,
        auth_ctx: AuthCtx,
    ) -> WorkerResult<InvocationResponseStream> {
        let environment_id = start_environment_id(&start)?;
        let observation_only =
            start.mode() == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup;
        let raw_key = start.idempotency_key.clone().map(Into::into);
        if observation_only && raw_key.is_none() {
            return Err(WorkerServiceError::TypeChecker(
                "lookup requires a final invocation key".into(),
            ));
        }
        let request_key = start
            .request_key
            .clone()
            .map(InvocationRequestKey::try_from)
            .transpose()
            .map_err(WorkerServiceError::TypeChecker)?
            .unwrap_or_else(|| {
                InvocationRequestKey::external(auth_ctx.idempotency_scope(), raw_key.as_ref())
            });
        let expected_fingerprint = start
            .expected_callee_fingerprint
            .map(|fingerprint| AgentFingerprint(fingerprint.into()));
        let component_id = start
            .component_id
            .map(ComponentId::try_from)
            .transpose()
            .map_err(WorkerServiceError::TypeChecker)?;
        let fresh_tool_owner = start
            .external_tool
            .as_ref()
            .is_some_and(|tool| tool.fresh_owner);
        let resource = start
            .method_name
            .as_ref()
            .map(|method_name| AgentResourcePattern::Method(AgentMethodName(method_name.clone())))
            .unwrap_or(AgentResourcePattern::Any);
        let permission =
            AgentPermission::Agent(agent_verb_for_invocation_mode(start.mode), resource);
        let (target, component) = if fresh_tool_owner {
            let Some(tool) = start.external_tool.as_ref() else {
                unreachable!("a fresh owner is an external tool's");
            };
            let component = self
                .creating_component(environment_id, component_id)
                .await?;
            let command_path = self
                .canonical_tool_command_path(
                    &component,
                    &ToolBindingOwner::ComponentBaseline {
                        component_id: component.id,
                    },
                    &tool.tool_name,
                    &tool.command_path,
                )
                .await?;
            let owner_key =
                request_key.owner_key(&golem_common::model::InvocationOperation::tool_operation(
                    &tool.tool_name,
                    &command_path,
                ));
            let target = RoutedTarget::fresh_tool_owner(
                AgentId::external_tool_owner(environment_id, component.id, &owner_key),
                component.id,
            );
            authorize_permissions(
                &auth_ctx,
                &agent_owner(&component, target.agent_id, None),
                &[permission],
            )?;
            if let Some(tool) = start.external_tool.as_mut() {
                tool.command_path = command_path;
            }
            (target, component)
        } else {
            let constructor = start
                .constructor
                .clone()
                .map(ParsedAgentKey::try_from)
                .transpose()
                .map_err(WorkerServiceError::TypeChecker)?;
            self.resolve_requested_target(
                environment_id,
                start_target(&start)?,
                constructor,
                component_id,
                trusted_internal_caller,
                expected_fingerprint,
                &auth_ctx,
                permission,
            )
            .await?
        };
        let mut freshness_disposition = if observation_only || !trusted_internal_caller {
            InvocationFreshnessDisposition::MayExist
        } else if start.freshness_disposition()
            == golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh
        {
            InvocationFreshnessDisposition::KnownFresh
        } else {
            InvocationFreshnessDisposition::MayExist
        };

        if !trusted_internal_caller {
            start.captured_method_input = None;
            start.allow_streams = true;
        }
        if start.mode() == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await
            && start.method_name.is_some()
            && start.allow_streams
        {
            let config = start
                .config
                .iter()
                .cloned()
                .map(AgentConfigEntryDto::try_from)
                .collect::<Result<Vec<_>, _>>()
                .map_err(WorkerServiceError::TypeChecker)?;
            let environment_variables = start
                .context
                .as_ref()
                .map(|context| context.env.clone())
                .unwrap_or_default();
            let fingerprint = self
                .resolve_streaming_callee_fingerprint(
                    &component,
                    &target,
                    environment_variables,
                    config,
                    auth_ctx.clone(),
                    start.context.clone(),
                    start.caller.clone().ok_or_else(|| {
                        WorkerServiceError::TypeChecker("caller is missing".to_string())
                    })?,
                    start.expected_callee_fingerprint,
                    start.clone(),
                )
                .await?;
            // The resolution created or found the incarnation, so the start names an
            // existing one, which the executor does not accept as KnownFresh.
            start.expected_callee_fingerprint = Some(fingerprint.0.into());
            freshness_disposition = InvocationFreshnessDisposition::MayExist;
        }

        target.apply(&mut start);
        start.request_key = Some(request_key.into());
        start.environment_id = Some(environment_id.into());
        start.component_owner_account_id = Some(component.account_id.into());
        start.freshness_disposition = proto_freshness_disposition(freshness_disposition);
        let request = stream::once(async move {
            InvocationRequest {
                request: Some(invocation_request::Request::Start(start)),
            }
        })
        .chain(tail);
        self.worker_client
            .invoke_agent_session(&target.agent_id, Box::pin(request))
            .await
    }

    pub async fn create_stream_session(
        &self,
        agent_id: &AgentId,
        request: CreateStreamSessionRequest,
    ) -> WorkerResult<CreateStreamSessionSuccess> {
        let auth_ctx: AuthCtx = request
            .invocation
            .as_ref()
            .ok_or_else(|| WorkerExecutorError::invalid_request("invocation not found"))?
            .auth_ctx
            .clone()
            .ok_or_else(|| WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        auth_ctx
            .authorize_system_only("create authorized Durable Streams session")
            .map_err(AuthServiceError::Unauthorized)?;

        self.worker_client
            .create_stream_session(agent_id, request)
            .await
    }

    pub async fn control_export_stream(
        &self,
        agent_id: &AgentId,
        request: DurableStreamAttachmentControlRequest,
    ) -> WorkerResult<ExportStreamControlResult> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or_else(|| WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        auth_ctx
            .authorize_system_only("control authorized Durable Streams export")
            .map_err(AuthServiceError::Unauthorized)?;
        self.worker_client
            .control_export_stream(agent_id, request)
            .await
    }

    pub async fn read_stream_slot(
        &self,
        agent_id: &AgentId,
        request: ReadStreamSlotRequest,
    ) -> WorkerResult<Option<ReadStreamSlotSuccess>> {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or_else(|| WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        auth_ctx
            .authorize_system_only("access authorized Durable Streams slot")
            .map_err(AuthServiceError::Unauthorized)?;

        self.worker_client.read_stream_slot(agent_id, request).await
    }

    pub async fn fork_stream_slot(
        &self,
        agent_id: &AgentId,
        request: golem_api_grpc::proto::golem::workerexecutor::v1::ForkStreamSlotRequest,
    ) -> WorkerResult<
        golem_api_grpc::proto::golem::workerexecutor::v1::fork_stream_slot_response::Result,
    > {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or_else(|| WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        auth_ctx
            .authorize_system_only("create authorized Durable Streams fork")
            .map_err(AuthServiceError::Unauthorized)?;
        self.worker_client.fork_stream_slot(agent_id, request).await
    }

    pub async fn append_to_stream_slot(
        &self,
        agent_id: &AgentId,
        request: golem_api_grpc::proto::golem::workerexecutor::v1::AppendToStreamSlotRequest,
    ) -> WorkerResult<golem_api_grpc::proto::golem::workerexecutor::v1::AppendToStreamSlotResponse>
    {
        let auth_ctx: AuthCtx = request
            .auth_ctx
            .clone()
            .ok_or_else(|| WorkerExecutorError::invalid_request("auth_ctx not found"))?
            .try_into()
            .map_err(WorkerExecutorError::invalid_request)?;
        auth_ctx
            .authorize_system_only("append to authorized Durable Streams slot")
            .map_err(AuthServiceError::Unauthorized)?;
        self.worker_client
            .append_to_stream_slot(agent_id, request)
            .await
    }

    pub async fn invoke_public_agent_session_v1(
        &self,
        start: PublicAgentSessionStart,
        tail: InvocationRequestStream,
        auth: AuthCtx,
        _resolve_stream: impl FnMut(
            PublicStreamReference,
            Option<&SchemaType>,
        ) -> Result<SchemaValueStream, PublicSchemaValueError>,
    ) -> Result<StartedPublicAgentSession, PublicAgentSessionStartError> {
        let app_name = ApplicationName::try_from(start.selector.application)
            .map_err(WorkerServiceError::TypeChecker)?;
        let env_name = EnvironmentName::try_from(start.selector.environment)
            .map_err(WorkerServiceError::TypeChecker)?;
        let agent_type_name = AgentTypeName(start.selector.agent_type);
        let method_name = start.selector.method;
        let idempotency_key = IdempotencyKey::new(start.idempotency_key);
        let request_key =
            InvocationRequestKey::external(auth.idempotency_scope(), Some(&idempotency_key));
        let resolved = self
            .agent_resolution_cache
            .resolve(&app_name, &env_name, &agent_type_name, None, &auth)
            .await
            .map_err(WorkerServiceError::from)?;
        let registered_agent_type = &resolved.registered_agent_type;
        let environment_id = resolved.environment_id;
        if environment_id != start.environment_id {
            return Err(WorkerServiceError::TypeChecker(
                "the selector names another environment than the request path".to_string(),
            )
            .into());
        }
        let component_id = registered_agent_type.implemented_by.component_id;
        let agent_type = &registered_agent_type.agent_type;
        if !start.config.is_empty()
            && (agent_type.mode == AgentMode::Durable
                || matches!(start.selector.target, PublicAgentTarget::Id { .. }))
        {
            return Err(WorkerServiceError::TypeChecker(
                "Configuration overrides are only accepted by create".to_string(),
            )
            .into());
        }
        let config =
            decode_public_agent_config(&agent_type.schema, &agent_type.config, start.config)?;
        let component_owner_account_id = registered_agent_type.implemented_by.account_id;
        let (target, component, expected_fingerprint) = match start.selector.target {
            PublicAgentTarget::Key {
                constructor_parameters,
            } => {
                let constructor_graph =
                    public_input_graph(&agent_type.schema, &agent_type.constructor.input_schema);
                let constructor_parameters = decode_public_json_schema_value(
                    &constructor_graph,
                    &constructor_graph.root,
                    &constructor_parameters,
                    PublicStreamReferencePolicy::None,
                    |_, _| unreachable!("constructor stream references are disabled by policy"),
                )?;
                let constructor_parameters = json_input_schema_value_to_typed_schema_value(
                    constructor_parameters,
                    &agent_type.schema,
                    &agent_type.constructor.input_schema,
                )
                .map_err(WorkerServiceError::TypeChecker)?;
                let component = self
                    .component_service
                    .get_revision(
                        component_id,
                        registered_agent_type.implemented_by.component_revision,
                    )
                    .await
                    .map_err(WorkerServiceError::from)?;
                let constructor =
                    ParsedAgentKey::new(agent_type_name.clone(), constructor_parameters);
                let (target, _) = Self::invocation_target(
                    environment_id,
                    registered_agent_type,
                    &component,
                    &constructor,
                    &request_key,
                    &method_name,
                )?;
                (target, component, None)
            }
            PublicAgentTarget::Id {
                agent_id,
                expected_fingerprint,
            } => {
                if agent_id.type_id() != Some(registered_agent_type.type_id) {
                    return Err(WorkerServiceError::AgentNotFound(agent_id).into());
                }
                let (target, component) = self
                    .resolve_requested_target(
                        environment_id,
                        AgentTarget::Id(agent_id),
                        None,
                        None,
                        false,
                        expected_fingerprint,
                        &auth,
                        AgentPermission::Agent(
                            AgentVerb::Invoke,
                            AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
                        ),
                    )
                    .await?;
                if target.agent_type.as_ref() != Some(&agent_type_name) {
                    return Err(WorkerServiceError::AgentNotFound(agent_id).into());
                }
                (target, component, expected_fingerprint)
            }
        };
        authorize_target(
            &auth,
            &component,
            &target,
            AgentVerb::Invoke,
            AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
        )?;
        let invocation_component = component;
        let caller: golem_api_grpc::proto::golem::component::Caller =
            self.rest_caller(&auth).into();
        let expected_callee_fingerprint = {
            let fingerprint = self
                .resolve_streaming_callee_fingerprint(
                    &invocation_component,
                    &target,
                    HashMap::new(),
                    config.clone(),
                    auth.clone(),
                    None,
                    caller.clone(),
                    expected_fingerprint.map(|fingerprint| fingerprint.0.into()),
                    InvocationStart {
                        method_name: Some(method_name.clone()),
                        captured_method_input: Some(
                            golem_common::serialization::serialize(
                                &golem_common::model::agent::AgentMethodInput::PublicJson(
                                    start.method_parameters.clone(),
                                ),
                            )
                            .map_err(WorkerServiceError::Internal)?,
                        ),
                        allow_streams: true,
                        ..Default::default()
                    },
                )
                .await?;
            Some(fingerprint.0.into())
        };
        // Resolution created or found this incarnation, so the start cannot be KnownFresh.
        let freshness_disposition = InvocationFreshnessDisposition::MayExist;
        let output_schema = None;
        let schema = SchemaGraph {
            defs: Default::default(),
            root: SchemaType::tuple(Vec::new()),
        };
        let component_revision = invocation_component.revision;
        let mut trusted_start = InvocationStart {
            target: None,
            constructor: None,
            component_id: None,
            method_name: Some(method_name.clone()),
            input: None,
            captured_method_input: Some(
                golem_common::serialization::serialize(
                    &golem_common::model::agent::AgentMethodInput::PublicJson(
                        start.method_parameters,
                    ),
                )
                .map_err(WorkerServiceError::Internal)?,
            ),
            allow_streams: true,
            request_key: Some(request_key.into()),
            idempotency_key: Some(idempotency_key.into()),
            context: None,
            auth_ctx: Some(auth.into()),
            caller: Some(caller),
            environment_id: Some(environment_id.into()),
            config: config.into_iter().map(Into::into).collect(),
            component_owner_account_id: Some(component_owner_account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            schedule_at: None,
            freshness_disposition: proto_freshness_disposition(freshness_disposition),
            attempt_id: Some(start.attempt_id.into()),
            expected_callee_fingerprint,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: None,
        };
        target.apply(&mut trusted_start);
        let initial_request = InvocationRequest {
            request: Some(invocation_request::Request::Start(trusted_start)),
        };
        let request = stream::once(std::future::ready(initial_request.clone())).chain(tail);
        let responses = self
            .worker_client
            .invoke_agent_session(&target.agent_id, Box::pin(request))
            .await?;

        Ok(StartedPublicAgentSession {
            responses,
            initial_request,
            schema,
            output_schema,
            agent_id: target.agent_id,
            component_id: invocation_component.id,
            application: app_name,
            environment: env_name,
            target: SessionInvocationTarget::Method {
                agent_type: agent_type_name.0,
                method: method_name,
            },
            component_revision,
        })
    }

    pub async fn resume_public_agent_session(
        &self,
        mut resume: ResumeAttach,
        tail: InvocationRequestStream,
        auth: AuthCtx,
    ) -> WorkerResult<InvocationResponseStream> {
        let caller: golem_api_grpc::proto::golem::component::Caller =
            self.rest_caller(&auth).into();
        resume.caller = Some(caller);
        self.resume_agent_session(resume, tail, auth).await
    }

    pub(crate) async fn selected_public_method_schema(
        &self,
        identity: &SessionAgentIdentity,
        revision: ComponentRevision,
    ) -> WorkerResult<(SchemaGraph, Option<SchemaType>)> {
        let component = self
            .component_service
            .get_revision(ComponentId(identity.component_id), revision)
            .await?;
        let SessionInvocationTarget::Method { agent_type, method } = &identity.target else {
            return Err(WorkerServiceError::Internal(
                "selected method schema requested for a tool".into(),
            ));
        };
        let agent = component
            .metadata
            .find_agent_type_by_name_ref(&AgentTypeName(agent_type.clone()))
            .ok_or_else(|| WorkerServiceError::Internal("selected agent type is missing".into()))?;
        let method = agent
            .methods
            .iter()
            .find(|candidate| &candidate.name == method)
            .ok_or_else(|| {
                WorkerServiceError::Internal("selected agent method is missing".into())
            })?;
        Ok((
            SchemaGraph {
                defs: agent.schema.defs.clone(),
                root: SchemaType::record(
                    method
                        .input_schema
                        .fields()
                        .iter()
                        .filter(|field| matches!(field.source, FieldSource::UserSupplied))
                        .map(|field| NamedFieldType {
                            name: field.name.clone(),
                            body: field.schema.clone(),
                            metadata: field.metadata.clone(),
                        })
                        .collect(),
                ),
            },
            method.output_schema.schema().cloned(),
        ))
    }

    pub async fn resume_public_agent_session_v1(
        &self,
        resume: PublicAgentSessionResume,
        tail: InvocationRequestStream,
        auth: AuthCtx,
    ) -> Result<StartedPublicAgentSession, PublicAgentSessionStartError> {
        let app_name = ApplicationName::try_from(resume.session.application.clone())
            .map_err(WorkerServiceError::TypeChecker)?;
        let env_name = EnvironmentName::try_from(resume.session.environment.clone())
            .map_err(WorkerServiceError::TypeChecker)?;
        let SessionInvocationTarget::Method {
            agent_type,
            method: method_name,
        } = &resume.identity.target
        else {
            return self.resume_public_tool_session_v1(resume, tail, auth).await;
        };
        let agent_type_name = AgentTypeName(agent_type.clone());
        let resolved = self
            .agent_resolution_cache
            .resolve(&app_name, &env_name, &agent_type_name, None, &auth)
            .await
            .map_err(WorkerServiceError::from)?;
        let environment_id = resolved.environment_id;
        if environment_id != resume.environment_id {
            return Err(WorkerServiceError::TypeChecker(
                "the session names another environment than the request path".to_string(),
            )
            .into());
        }
        let component_id = ComponentId(resume.identity.component_id);
        let component = match resume.identity.component_revision {
            Some(revision) => self
                .component_service
                .get_revision(
                    component_id,
                    ComponentRevision::new(revision)
                        .map_err(|error| WorkerServiceError::TypeChecker(error.to_string()))?,
                )
                .await
                .map_err(WorkerServiceError::from)?,
            None => self
                .component_service
                .get_current_by_id_uncached(component_id)
                .await
                .map_err(WorkerServiceError::from)?,
        };
        let component_revision = component.revision;
        if component.environment_id != environment_id {
            return Err(WorkerServiceError::TypeChecker(
                "session token environment does not own the pinned component".to_string(),
            )
            .into());
        }
        let agent_id = resume.identity.agent_id;
        self.managed_agent(
            &OwnedAgentId::new(environment_id, &agent_id),
            None,
            &auth,
            &[AgentPermission::Agent(
                AgentVerb::Invoke,
                AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
            )],
        )
        .await?;
        let output_schema = None;
        let schema = SchemaGraph {
            defs: Default::default(),
            root: SchemaType::tuple(Vec::new()),
        };
        let caller: golem_api_grpc::proto::golem::component::Caller =
            self.rest_caller(&auth).into();
        let trusted_resume = ResumeAttach {
            idempotency_key: Some(
                parse_final_invocation_id(&resume.session.idempotency_key)
                    .map_err(PublicAgentSessionStartError::Worker)?
                    .into(),
            ),
            agent_id: Some(agent_id.into()),
            environment_id: Some(environment_id.into()),
            attachment_id: Some(resume.session.attachment_id.into()),
            attempt_id: Some(resume.attempt_id.into()),
            expected_callee_fingerprint: Some(resume.session.callee_incarnation.into()),
            expected_epoch: resume.session.expected_attachment_generation,
            operation: resume.operation as i32,
            cursors: resume.cursors,
            auth_ctx: Some(auth.into()),
            caller: Some(caller),
        };
        let initial_request = InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(trusted_resume)),
        };
        validate_trusted_resume_request(&initial_request)?;
        let request = stream::once(std::future::ready(initial_request.clone())).chain(tail);
        let responses = self
            .worker_client
            .invoke_agent_session(&agent_id, Box::pin(request))
            .await?;

        Ok(StartedPublicAgentSession {
            responses,
            initial_request,
            schema,
            output_schema,
            agent_id,
            component_id,
            application: app_name,
            environment: env_name,
            target: resume.identity.target,
            component_revision,
        })
    }

    pub async fn invoke_public_tool_session_v1(
        &self,
        start: PublicToolSessionStart,
        tail: InvocationRequestStream,
        auth: AuthCtx,
        validate_identity: impl FnOnce(&SessionAgentIdentity) -> Result<(), PublicSchemaValueError>,
    ) -> Result<StartedPublicAgentSession, PublicAgentSessionStartError> {
        let caller = self.rest_caller(&auth);
        let request_key = InvocationRequestKey::external(
            auth.idempotency_scope(),
            Some(&IdempotencyKey::new(start.idempotency_key.clone())),
        );
        self.invoke_internal_tool_session_v1(
            start,
            tail,
            auth,
            caller,
            request_key,
            validate_identity,
        )
        .await
    }

    pub async fn invoke_internal_tool_session_v1(
        &self,
        start: PublicToolSessionStart,
        tail: InvocationRequestStream,
        auth: AuthCtx,
        caller: Caller,
        request_key: InvocationRequestKey,
        validate_identity: impl FnOnce(&SessionAgentIdentity) -> Result<(), PublicSchemaValueError>,
    ) -> Result<StartedPublicAgentSession, PublicAgentSessionStartError> {
        let app_name = ApplicationName::try_from(start.application)
            .map_err(WorkerServiceError::TypeChecker)?;
        let env_name = EnvironmentName::try_from(start.environment)
            .map_err(WorkerServiceError::TypeChecker)?;
        let idempotency_key = IdempotencyKey::new(start.idempotency_key);
        let (agent_id, component_id) = match start.target {
            PublicNativeToolTarget::Agent { agent_id } => (Some(agent_id), None),
            PublicNativeToolTarget::Component { component_id } => {
                (None, Some(ComponentId(component_id)))
            }
        };
        let (routed, component, expected_callee_fingerprint, fresh_owner, command_path) = self
            .tool_target(
                start.environment_id,
                agent_id,
                component_id,
                &request_key,
                &start.tool_name,
                &start.command_path,
                false,
                AgentVerb::Invoke,
                &auth,
            )
            .await?;
        if component.application_name != app_name || component.environment_name != env_name {
            return Err(WorkerServiceError::TypeChecker(
                "component is not deployed in the requested application and environment"
                    .to_string(),
            )
            .into());
        }
        let agent_id = routed.agent_id;
        let component_id = component.id;
        let expected_callee_fingerprint =
            expected_callee_fingerprint.map(|fingerprint| fingerprint.0.into());
        let input_schema = start.input.graph;
        let input = decode_public_json_schema_value(
            &input_schema,
            &input_schema.root,
            &start.input.value,
            PublicStreamReferencePolicy::None,
            |_, _| unreachable!("stream references are rejected by the public value codec"),
        )?;
        let input = golem_schema::proto::golem::schema::TypedSchemaValue {
            graph: Some(input_schema.clone().into()),
            value: Some(input.try_into().map_err(WorkerServiceError::TypeChecker)?),
        };
        let target = SessionInvocationTarget::ExternalTool {
            tool_name: start.tool_name.clone(),
            command_path: command_path.clone(),
        };
        let identity = SessionAgentIdentity {
            component_id: component_id.0,
            component_revision: Some(component.revision.get()),
            agent_id,
            target: target.clone(),
        };
        validate_identity(&identity)?;
        let caller: golem_api_grpc::proto::golem::component::Caller = caller.into();
        let mut trusted_start = InvocationStart {
            target: None,
            constructor: None,
            component_id: None,
            method_name: None,
            captured_method_input: None,
            allow_streams: false,
            input: None,
            request_key: Some(request_key.into()),
            idempotency_key: Some(idempotency_key.into()),
            context: None,
            auth_ctx: Some(auth.into()),
            caller: Some(caller),
            environment_id: Some(component.environment_id.into()),
            config: Vec::new(),
            component_owner_account_id: Some(component.account_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            schedule_at: None,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(start.attempt_id.into()),
            expected_callee_fingerprint,
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: Some(ExternalToolInvocation {
                tool_name: start.tool_name,
                command_path,
                input: Some(input),
                stdin: start.stdin,
                stdout: start.stdout,
                stderr: start.stderr,
                fresh_owner,
                expected_deployment_revision: start
                    .expected_deployment_revision
                    .map(DeploymentRevision::get),
            }),
        };
        routed.apply(&mut trusted_start);
        let initial_request = InvocationRequest {
            request: Some(invocation_request::Request::Start(trusted_start)),
        };
        let request = stream::once(std::future::ready(initial_request.clone())).chain(tail);
        let responses = self
            .worker_client
            .invoke_agent_session(&agent_id, Box::pin(request))
            .await?;
        Ok(StartedPublicAgentSession {
            responses,
            initial_request,
            schema: input_schema,
            output_schema: None,
            agent_id,
            component_id,
            application: app_name,
            environment: env_name,
            target,
            component_revision: component.revision,
        })
    }

    async fn resume_public_tool_session_v1(
        &self,
        resume: PublicAgentSessionResume,
        tail: InvocationRequestStream,
        auth: AuthCtx,
    ) -> Result<StartedPublicAgentSession, PublicAgentSessionStartError> {
        let app_name = ApplicationName::try_from(resume.session.application.clone())
            .map_err(WorkerServiceError::TypeChecker)?;
        let env_name = EnvironmentName::try_from(resume.session.environment.clone())
            .map_err(WorkerServiceError::TypeChecker)?;
        let component_id = ComponentId(resume.identity.component_id);
        let component_revision =
            ComponentRevision::new(resume.identity.component_revision.ok_or_else(|| {
                WorkerServiceError::TypeChecker("tool session has no selected revision".into())
            })?)
            .map_err(|error| WorkerServiceError::TypeChecker(error.to_string()))?;
        let current_component = self
            .component_service
            .get_current_by_id_uncached(component_id)
            .await
            .map_err(WorkerServiceError::from)?;
        if current_component.application_name != app_name
            || current_component.environment_name != env_name
        {
            return Err(WorkerServiceError::TypeChecker(
                "session target is no longer deployed in the requested environment".to_string(),
            )
            .into());
        }
        let component = self
            .component_service
            .get_revision(component_id, component_revision)
            .await
            .map_err(WorkerServiceError::from)?;
        if component.application_name != app_name
            || component.environment_name != env_name
            || component.environment_id != resume.environment_id
        {
            return Err(WorkerServiceError::TypeChecker(
                "session token environment does not own the pinned component".to_string(),
            )
            .into());
        }
        let agent_id = resume.identity.agent_id;
        self.managed_agent(
            &OwnedAgentId::new(component.environment_id, &agent_id),
            None,
            &auth,
            &[AgentPermission::Agent(
                AgentVerb::Invoke,
                AgentResourcePattern::Any,
            )],
        )
        .await?;
        let caller: golem_api_grpc::proto::golem::component::Caller =
            self.rest_caller(&auth).into();
        let trusted_resume = ResumeAttach {
            idempotency_key: Some(
                parse_final_invocation_id(&resume.session.idempotency_key)
                    .map_err(PublicAgentSessionStartError::Worker)?
                    .into(),
            ),
            agent_id: Some(agent_id.into()),
            environment_id: Some(component.environment_id.into()),
            attachment_id: Some(resume.session.attachment_id.into()),
            attempt_id: Some(resume.attempt_id.into()),
            expected_callee_fingerprint: Some(resume.session.callee_incarnation.into()),
            expected_epoch: resume.session.expected_attachment_generation,
            operation: resume.operation as i32,
            cursors: resume.cursors,
            auth_ctx: Some(auth.into()),
            caller: Some(caller),
        };
        let initial_request = InvocationRequest {
            request: Some(invocation_request::Request::ResumeAttach(trusted_resume)),
        };
        validate_trusted_resume_request(&initial_request)?;
        let request = stream::once(std::future::ready(initial_request.clone())).chain(tail);
        let responses = self
            .worker_client
            .invoke_agent_session(&agent_id, Box::pin(request))
            .await?;
        Ok(StartedPublicAgentSession {
            responses,
            initial_request,
            schema: SchemaGraph::empty(),
            output_schema: None,
            agent_id,
            component_id,
            application: app_name,
            environment: env_name,
            target: resume.identity.target,
            component_revision,
        })
    }

    pub async fn resume_agent_session(
        &self,
        mut resume: ResumeAttach,
        tail: InvocationRequestStream,
        auth: AuthCtx,
    ) -> WorkerResult<InvocationResponseStream> {
        let agent_id: AgentId = resume
            .agent_id
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker("resume agent_id not found".to_string())
            })?
            .try_into()
            .map_err(WorkerServiceError::TypeChecker)?;
        let environment_id: EnvironmentId = resume
            .environment_id
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker("resume environment_id not found".to_string())
            })?
            .try_into()
            .map_err(WorkerServiceError::TypeChecker)?;
        self.managed_agent(
            &OwnedAgentId::new(environment_id, &agent_id),
            None,
            &auth,
            &[AgentPermission::Agent(
                AgentVerb::Invoke,
                AgentResourcePattern::Any,
            )],
        )
        .await?;
        resume.auth_ctx = Some(auth.into());
        let request = stream::once(async move {
            InvocationRequest {
                request: Some(invocation_request::Request::ResumeAttach(resume)),
            }
        })
        .chain(tail);
        self.worker_client
            .invoke_agent_session(&agent_id, Box::pin(request))
            .await
    }

    /// Dispatches one-shot work without choosing its execution schema. The caller authorizes
    /// the request; the loaded executor validates the selected method and input.
    #[allow(clippy::too_many_arguments)]
    pub async fn dispatch_agent_invocation(
        &self,
        _component: &Component,
        target: &RoutedTarget,
        method_name: Option<String>,
        method_parameters: Option<golem_common::model::agent::AgentMethodInput>,
        mode: i32,
        schedule_at: Option<::prost_types::Timestamp>,
        request_key: InvocationRequestKey,
        supplied_key: Option<IdempotencyKey>,
        invocation_context: Option<InvocationContext>,
        freshness_disposition: InvocationFreshnessDisposition,
        config: Vec<AgentConfigEntryDto>,
        environment_id: EnvironmentId,
        account_id: AccountId,
        auth_ctx: AuthCtx,
        caller: golem_api_grpc::proto::golem::component::Caller,
        scope_card: Option<golem_api_grpc::proto::golem::worker::EncodedScopeCard>,
        expected_fingerprint: Option<AgentFingerprint>,
    ) -> WorkerResult<AgentInvocationOutput> {
        let observation_only =
            mode == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32;
        if observation_only && matches!(request_key, InvocationRequestKey::Unkeyed(_)) {
            return Err(WorkerServiceError::TypeChecker(
                "lookup requires a final invocation key".to_string(),
            ));
        }
        let freshness_disposition = if observation_only || expected_fingerprint.is_some() {
            InvocationFreshnessDisposition::MayExist
        } else {
            freshness_disposition
        };

        if !observation_only && (method_name.is_none() || method_parameters.is_none()) {
            return Err(WorkerServiceError::TypeChecker(
                "method_name and method_parameters are required for non-lookup invocations".into(),
            ));
        }

        let mut output = self
            .worker_client
            .invoke_agent(
                target,
                method_name,
                method_parameters,
                mode,
                schedule_at,
                request_key,
                supplied_key,
                invocation_context,
                freshness_disposition,
                config,
                environment_id,
                account_id,
                auth_ctx,
                caller,
                scope_card,
                expected_fingerprint,
            )
            .await?;
        output.agent_id.get_or_insert(target.agent_id);
        Ok(output)
    }

    /// The constructor of an agent of the deployed type, from the parameters of a REST request.
    fn rest_constructor(
        registered: &RegisteredAgentType,
        parameters: ExternalSchemaValue,
    ) -> WorkerResult<ParsedAgentKey> {
        let agent_type = &registered.agent_type;
        let parameters = json_input_schema_value_to_typed_schema_value(
            parameters.into_inner(),
            &agent_type.schema,
            &agent_type.constructor.input_schema,
        )
        .map_err(|err| {
            WorkerServiceError::TypeChecker(format!(
                "Agent constructor parameters type error: {err}"
            ))
        })?;
        Ok(ParsedAgentKey::new(
            agent_type.type_name.clone(),
            parameters,
        ))
    }

    /// Creates an unkeyed agent for an authenticated mount using the shared REST create policy.
    /// HTTP route authentication does not provide an idempotency scope.
    pub(crate) async fn create_mounted_agent(
        &self,
        component_id: ComponentId,
        environment_id: EnvironmentId,
        agent_type: &AgentTypeName,
        request: CreateAgentRequest,
        idempotency_key: Option<IdempotencyKey>,
        caller: Caller,
    ) -> WorkerResult<AgentReference> {
        if request.keyed {
            return Err(WorkerServiceError::TypeChecker(
                "HTTP create routes require an unkeyed create".into(),
            ));
        }
        let (registered, component) = self
            .implementing_component(component_id, agent_type)
            .await?;
        if component.environment_id != environment_id {
            return Err(WorkerServiceError::TypeChecker(
                "The component does not belong to the route's environment".into(),
            ));
        }
        self.create_resolved_agent(
            &registered,
            &component,
            request,
            idempotency_key,
            AuthCtx::System,
            caller,
        )
        .await
    }

    /// REST create of an agent of a durable type. A keyed create makes the keyed agent of the
    /// parameters and fails with a conflict when that key is current. A create without a key
    /// makes an agent at a new ID;
    /// with an idempotency key from a caller with a caller scope, the ID derives from the scope
    /// and the key, so a retry reaches the agent the first attempt made.
    pub async fn create_agent_rest(
        &self,
        environment_id: EnvironmentId,
        agent_type_name: &AgentTypeName,
        request: CreateAgentRequest,
        idempotency_key: Option<IdempotencyKey>,
        auth: AuthCtx,
    ) -> WorkerResult<AgentReference> {
        let (registered, component) = self
            .deployed_agent_type(environment_id, agent_type_name, &auth)
            .await?;
        let caller = self.rest_caller(&auth);
        self.create_resolved_agent(
            &registered,
            &component,
            request,
            idempotency_key,
            auth,
            caller,
        )
        .await
    }

    async fn create_resolved_agent(
        &self,
        registered: &RegisteredAgentType,
        component: &Component,
        request: CreateAgentRequest,
        idempotency_key: Option<IdempotencyKey>,
        auth: AuthCtx,
        caller: Caller,
    ) -> WorkerResult<AgentReference> {
        let environment_id = component.environment_id;
        if registered.agent_type.mode == AgentMode::Ephemeral {
            return Err(WorkerServiceError::TypeChecker(format!(
                "Create needs a durable agent type, and {} is ephemeral",
                registered.agent_type.type_name
            )));
        }
        let constructor = Self::rest_constructor(registered, request.parameters)?;
        let request_key =
            InvocationRequestKey::external(auth.idempotency_scope(), idempotency_key.as_ref());
        let target = if request.keyed {
            RoutedTarget::keyed(environment_id, registered.type_id, component, &constructor)?
        } else {
            let agent_id = match &request_key {
                InvocationRequestKey::External { scope, key } => AgentId::idempotent_create(
                    environment_id,
                    registered.type_id,
                    &scope.to_string(),
                    key,
                ),
                _ => AgentId::random(registered.type_id),
            };
            RoutedTarget::unkeyed(agent_id, component, &constructor)?
        };

        let (agent_id, _, agent_fingerprint) = self
            .create(
                &target,
                component,
                request.env,
                request.config,
                false,
                auth,
                None,
                caller.into(),
                Some(request_key.owner_key("create")),
                AgentResourcePattern::Any,
            )
            .await?;

        Ok(AgentReference {
            agent_id,
            agent_fingerprint,
        })
    }

    /// REST upsert: the keyed agent of the parameters, created when it is missing.
    pub async fn upsert_agent_rest(
        &self,
        environment_id: EnvironmentId,
        agent_type_name: &AgentTypeName,
        request: AgentKeyRequest,
        auth: AuthCtx,
    ) -> WorkerResult<AgentReference> {
        let (registered, component) = self
            .deployed_agent_type(environment_id, agent_type_name, &auth)
            .await?;
        if registered.agent_type.mode == AgentMode::Ephemeral {
            return Err(WorkerServiceError::TypeChecker(
                "An ephemeral agent has no key".to_string(),
            ));
        }
        let constructor = Self::rest_constructor(&registered, request.parameters)?;
        let target =
            RoutedTarget::keyed(environment_id, registered.type_id, &component, &constructor)?;

        let caller = self.rest_caller(&auth).into();
        let (agent_id, _, agent_fingerprint) = self
            .create(
                &target,
                &component,
                HashMap::new(),
                Vec::new(),
                true,
                auth,
                None,
                caller,
                None,
                AgentResourcePattern::Any,
            )
            .await?;

        Ok(AgentReference {
            agent_id,
            agent_fingerprint,
        })
    }

    /// REST lookup: the keyed agent of the parameters, which this never creates. An occupied
    /// address with another key returns a collision after authorization.
    pub async fn lookup_agent_rest(
        &self,
        environment_id: EnvironmentId,
        agent_type_name: &AgentTypeName,
        request: AgentKeyRequest,
        auth: AuthCtx,
    ) -> WorkerResult<AgentReference> {
        let mut retained = self
            .retained_agent_types(environment_id, agent_type_name, &auth)
            .await?;
        let (registered, component) = match self
            .agent_types
            .deployed(environment_id, agent_type_name)
            .await?
        {
            Some(registered) => {
                let component = self
                    .component_service
                    .get_revision(
                        registered.implemented_by.component_id,
                        registered.implemented_by.component_revision,
                    )
                    .await?;
                (registered.as_ref().clone(), component)
            }
            None => retained.remove(0),
        };
        if registered.agent_type.mode == AgentMode::Ephemeral {
            return Err(WorkerServiceError::TypeChecker(
                "An ephemeral agent has no key".to_string(),
            ));
        }
        let constructor = Self::rest_constructor(&registered, request.parameters)?;
        let key = constructor
            .canonical_key(&component.metadata)
            .map_err(WorkerServiceError::TypeChecker)?;
        let agent_id = AgentId::keyed(environment_id, registered.type_id, &key);
        let agent = self
            .managed_agent(
                &OwnedAgentId::new(environment_id, &agent_id),
                None,
                &auth,
                &[AgentPermission::Agent(
                    AgentVerb::View,
                    AgentResourcePattern::Empty,
                )],
            )
            .await?;
        if agent.metadata.canonical_key.as_deref() != Some(key.to_string().as_str()) {
            return Err(WorkerExecutorError::AgentKeyCollision {
                agent_id,
                requested_key: key.to_string(),
            }
            .into());
        }
        Ok(AgentReference {
            agent_id,
            agent_fingerprint: agent.metadata.fingerprint,
        })
    }

    /// The target of a tool call: an existing agent, or a fresh owner on a component of the
    /// environment. Returns the target, the component, the expected fingerprint of an
    /// existing agent, whether the owner is fresh, and the canonical form of the command path,
    /// from which the invocation ID and a fresh owner's agent ID are derived.
    #[allow(clippy::too_many_arguments)]
    async fn tool_target(
        &self,
        environment_id: EnvironmentId,
        agent_id: Option<AgentId>,
        component_id: Option<ComponentId>,
        request_key: &InvocationRequestKey,
        tool_name: &str,
        command_path: &[String],
        lookup: bool,
        verb: AgentVerb,
        auth: &AuthCtx,
    ) -> WorkerResult<(
        RoutedTarget,
        Component,
        Option<AgentFingerprint>,
        bool,
        Vec<String>,
    )> {
        match (agent_id, component_id) {
            (Some(agent_id), None) => {
                let agent = self
                    .managed_agent(
                        &OwnedAgentId::new(environment_id, &agent_id),
                        None,
                        auth,
                        &[AgentPermission::Agent(verb, AgentResourcePattern::Any)],
                    )
                    .await?;
                if !lookup && agent.metadata.owner_kind != OwnerKind::ComponentAgent {
                    return Err(WorkerServiceError::TypeChecker(
                        "existing external-tool target is not a real component agent"
                            .to_string(),
                    ));
                }
                let owner = match (&agent.metadata.owner_kind, &agent.metadata.agent_type_name) {
                    (OwnerKind::ComponentAgent, Some(agent_type_name)) => {
                        ToolBindingOwner::AgentType {
                            agent_type_name: agent_type_name.clone(),
                        }
                    }
                    _ => ToolBindingOwner::ComponentBaseline {
                        component_id: agent.component.id,
                    },
                };
                let command_path = self
                    .canonical_tool_command_path(&agent.component, &owner, tool_name, command_path)
                    .await?;
                let fingerprint = agent.metadata.fingerprint;
                Ok((
                    agent.routed(),
                    agent.component,
                    Some(fingerprint),
                    false,
                    command_path,
                ))
            }
            (None, Some(component_id)) if !lookup => {
                let component = self
                    .creating_component(environment_id, Some(component_id))
                    .await?;
                let command_path = self
                    .canonical_tool_command_path(
                        &component,
                        &ToolBindingOwner::ComponentBaseline { component_id },
                        tool_name,
                        command_path,
                    )
                    .await?;
                let operation =
                    golem_common::model::InvocationOperation::tool_operation(&tool_name, &command_path);
                let target = RoutedTarget::fresh_tool_owner(
                    AgentId::external_tool_owner(
                        environment_id,
                        component_id,
                        &request_key.owner_key(&operation),
                    ),
                    component_id,
                );
                authorize_permissions(
                    auth,
                    &agent_owner(&component, target.agent_id, None),
                    &[AgentPermission::Agent(verb, AgentResourcePattern::Any)],
                )?;
                Ok((target, component, None, true, command_path))
            }
            _ => Err(WorkerServiceError::TypeChecker(
                "exactly one of agent_id and component_id is required, and a lookup names the agent"
                    .to_string(),
            )),
        }
    }

    pub async fn invoke_tool_rest(
        &self,
        environment_id: EnvironmentId,
        request: NativeToolInvocationRequest,
        idempotency_key: Option<IdempotencyKey>,
        auth: AuthCtx,
    ) -> WorkerResult<NativeToolInvocationResponse> {
        let lookup = matches!(request.mode, NativeToolInvocationMode::Lookup);
        if lookup
            && (idempotency_key.is_none()
                || request.input.is_some()
                || request.schedule_at.is_some()
                || request.agent_id.is_none())
        {
            return Err(WorkerServiceError::TypeChecker(
                "lookup requires the final agent ID and invocation key and forbids input and scheduling".to_string(),
            ));
        }
        if !lookup && request.input.is_none() {
            return Err(WorkerServiceError::TypeChecker(
                "external tool input is required".to_string(),
            ));
        }
        if request.schedule_at.is_some()
            && !matches!(request.mode, NativeToolInvocationMode::Schedule)
        {
            return Err(WorkerServiceError::TypeChecker(
                "schedule_at requires schedule mode".to_string(),
            ));
        }
        if lookup {
            parse_final_invocation_id(
                &idempotency_key
                    .as_ref()
                    .expect("lookup key validated")
                    .value,
            )?;
        }
        let request_key =
            InvocationRequestKey::external(auth.idempotency_scope(), idempotency_key.as_ref());
        let proto_mode = match request.mode {
            NativeToolInvocationMode::Await => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32
            }
            NativeToolInvocationMode::Schedule => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32
            }
            NativeToolInvocationMode::Lookup => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32
            }
        };
        let (target, component, expected_callee_fingerprint, fresh_owner, command_path) = self
            .tool_target(
                environment_id,
                request.agent_id,
                request.component_id,
                &request_key,
                &request.tool_name,
                &request.command_path,
                lookup,
                agent_verb_for_invocation_mode(proto_mode),
                &auth,
            )
            .await?;

        let input = request
            .input
            .map(|input| input.into_inner().try_into())
            .transpose()
            .map_err(|error| {
                WorkerServiceError::TypeChecker(format!(
                    "external tool input cannot cross the worker boundary: {error}"
                ))
            })?;
        let caller: golem_api_grpc::proto::golem::component::Caller =
            self.rest_caller(&auth).into();
        let mut start = InvocationStart {
            target: None,
            constructor: None,
            component_id: None,
            method_name: None,
            captured_method_input: None,
            allow_streams: false,
            input: None,
            request_key: Some(request_key.into()),
            idempotency_key: idempotency_key.map(Into::into),
            context: None,
            auth_ctx: Some(auth.into()),
            caller: Some(caller),
            environment_id: Some(environment_id.into()),
            config: Vec::new(),
            component_owner_account_id: Some(component.account_id.into()),
            mode: proto_mode,
            schedule_at: request.schedule_at.map(|at| ::prost_types::Timestamp {
                seconds: at.timestamp(),
                nanos: at.timestamp_subsec_nanos() as i32,
            }),
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(uuid::Uuid::new_v4().into()),
            expected_callee_fingerprint: expected_callee_fingerprint
                .map(|fingerprint| fingerprint.0.into()),
            durable_input_mappings: Vec::new(),
            scope_card: None,
            origin_invocation: None,
            external_tool: Some(ExternalToolInvocation {
                tool_name: request.tool_name,
                command_path,
                input,
                stdin: false,
                stdout: false,
                stderr: false,
                fresh_owner,
                expected_deployment_revision: None,
            }),
        };
        target.apply(&mut start);
        let mut output = self
            .worker_client
            .invoke_agent_session_one_shot(&target.agent_id, start)
            .await?;
        let response_agent_id = output.agent_id.take().unwrap_or(target.agent_id);
        let response_key = output
            .idempotency_key
            .take()
            .ok_or_else(|| WorkerServiceError::Internal("Missing idempotency key".to_string()))?;
        let result = match output.result {
            golem_common::model::AgentInvocationResult::ExternalTool { result } => {
                Some(native_tool_result(result)?)
            }
            _ if proto_mode
                != golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32 =>
            {
                None
            }
            _ => {
                return Err(WorkerServiceError::Internal(
                    "native external-tool invocation returned a non-tool result".to_string(),
                ));
            }
        };
        Ok(NativeToolInvocationResponse {
            agent_id: response_agent_id,
            idempotency_key: response_key,
            result,
            status: output.invocation_status,
            component_revision: output.component_revision,
            agent_fingerprint: output.agent_fingerprint,
            oplog_index: output.oplog_index,
        })
    }

    pub async fn describe_tool_rest(
        &self,
        environment_id: EnvironmentId,
        request: NativeToolDescribeRequest,
        auth: AuthCtx,
    ) -> WorkerResult<NativeToolDefinition> {
        let (component, owner) = match (request.agent_id, request.component_id) {
            (Some(agent_id), None) => {
                let agent = self
                    .managed_agent(
                        &OwnedAgentId::new(environment_id, &agent_id),
                        None,
                        &auth,
                        &[AgentPermission::Agent(
                            AgentVerb::View,
                            AgentResourcePattern::Any,
                        )],
                    )
                    .await?;
                let agent_type_name =
                    match (&agent.metadata.owner_kind, &agent.metadata.agent_type_name) {
                        (OwnerKind::ComponentAgent, Some(agent_type_name)) => {
                            agent_type_name.clone()
                        }
                        _ => {
                            return Err(WorkerServiceError::TypeChecker(
                                "existing external-tool target is not a real component agent"
                                    .to_string(),
                            ));
                        }
                    };
                (
                    agent.component,
                    ToolBindingOwner::AgentType { agent_type_name },
                )
            }
            (None, Some(component_id)) => {
                let component = self
                    .creating_component(environment_id, Some(component_id))
                    .await?;
                authorize_component_agents(
                    &auth,
                    &component,
                    AgentVerb::View,
                    AgentResourcePattern::Any,
                )?;
                (
                    component,
                    ToolBindingOwner::ComponentBaseline { component_id },
                )
            }
            _ => {
                return Err(WorkerServiceError::TypeChecker(
                    "exactly one of agent_id and component_id is required".to_string(),
                ));
            }
        };
        let (definition, deployment_revision) = self
            .effective_tool_definition(&component, &owner, &request.tool_name)
            .await?;
        Ok(NativeToolDefinition {
            definition,
            component_revision: component.revision,
            deployment_revision,
        })
    }

    /// The definition of the tool named `tool_name` as `owner` of `component` sees it, with the
    /// deployment revision that selected it.
    async fn effective_tool_definition(
        &self,
        component: &Component,
        owner: &ToolBindingOwner,
        tool_name: &str,
    ) -> WorkerResult<(golem_common::schema::tool::Tool, DeploymentRevision)> {
        let state = self
            .component_service
            .get_tool_deployment_state(component.environment_id, component.id, component.revision)
            .await?
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker(
                    "no tool deployment is active for the target".to_string(),
                )
            })?;
        let tool_name = ToolName::try_from(tool_name).map_err(WorkerServiceError::TypeChecker)?;
        let registered = state.registered_tools.get(&tool_name).ok_or_else(|| {
            WorkerServiceError::TypeChecker(format!("tool '{tool_name}' is not registered"))
        })?;
        if !state
            .tool_bindings
            .get(owner)
            .is_some_and(|bindings| bindings.contains_key(&tool_name))
        {
            return Err(WorkerServiceError::TypeChecker(format!(
                "tool '{tool_name}' is not bound to the target owner"
            )));
        }
        let definition = state
            .tool_middleware_chains
            .get(owner)
            .and_then(|chains| chains.get(&tool_name))
            .map(|chain| chain.effective_definition.clone())
            .unwrap_or_else(|| registered.registration.definition.clone());
        Ok((definition, state.deployment_revision))
    }

    /// The canonical form of `command_path` in the tool named `tool_name` as `owner` of
    /// `component` sees it. Invocation IDs are derived from the canonical form, so an alias and
    /// the name of a command give the same ID.
    async fn canonical_tool_command_path(
        &self,
        component: &Component,
        owner: &ToolBindingOwner,
        tool_name: &str,
        command_path: &[String],
    ) -> WorkerResult<Vec<String>> {
        let (definition, _) = self
            .effective_tool_definition(component, owner, tool_name)
            .await?;
        definition
            .canonical_command_path(command_path)
            .ok_or_else(|| {
                WorkerServiceError::TypeChecker(format!(
                    "command path {command_path:?} does not name a command of tool '{tool_name}'"
                ))
            })
    }

    /// REST invoke: on the agent with `agentId`, which must have the type, or on the agent of
    /// `parameters`, which upserts a durable agent and reaches an instance of its own for an
    /// ephemeral type.
    pub async fn invoke_agent_rest(
        &self,
        environment_id: EnvironmentId,
        agent_type_name: &AgentTypeName,
        request: AgentInvocationRequest,
        idempotency_key: Option<IdempotencyKey>,
        auth: AuthCtx,
    ) -> WorkerResult<AgentInvocationResult> {
        let method_name = request.method.clone();
        let proto_mode = match request.mode {
            AgentInvocationMode::Await => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32
            }
            AgentInvocationMode::Schedule => {
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32
            }
        };
        let request_key =
            InvocationRequestKey::external(auth.idempotency_scope(), idempotency_key.as_ref());
        let permission = AgentPermission::Agent(
            AgentVerb::Invoke,
            AgentResourcePattern::Method(AgentMethodName(method_name.clone())),
        );
        let expected_fingerprint = request.expected_fingerprint;
        let (target, component, freshness_disposition) =
            match (request.agent_id, request.parameters) {
                (Some(agent_id), None) => {
                    let retained = self
                        .retained_agent_types(environment_id, agent_type_name, &auth)
                        .await
                        .map_err(|error| match error {
                            WorkerServiceError::RegistryServiceError(
                                RegistryServiceError::NotFound(_),
                            ) => WorkerServiceError::AgentNotFound(agent_id),
                            other => other,
                        })?;
                    if !retained
                        .iter()
                        .any(|(registered, _)| agent_id.type_id() == Some(registered.type_id))
                    {
                        return Err(WorkerServiceError::AgentNotFound(agent_id));
                    }
                    let agent = self
                        .managed_agent(
                            &OwnedAgentId::new(environment_id, &agent_id),
                            expected_fingerprint,
                            &auth,
                            &[permission],
                        )
                        .await?;
                    if agent.metadata.agent_type_name.as_ref() != Some(agent_type_name) {
                        return Err(WorkerServiceError::AgentNotFound(agent_id));
                    }
                    (
                        agent.routed(),
                        agent.component,
                        InvocationFreshnessDisposition::MayExist,
                    )
                }
                (None, Some(parameters)) => {
                    let (registered, component) = self
                        .deployed_agent_type(environment_id, agent_type_name, &auth)
                        .await?;
                    let constructor = Self::rest_constructor(&registered, parameters)?;
                    let (target, freshness_disposition) = Self::invocation_target(
                        environment_id,
                        &registered,
                        &component,
                        &constructor,
                        &request_key,
                        &method_name,
                    )?;
                    authorize_permissions(
                        &auth,
                        &agent_owner(&component, target.agent_id, target.agent_type.clone()),
                        &[permission],
                    )?;
                    let freshness_disposition = if expected_fingerprint.is_some() {
                        InvocationFreshnessDisposition::MayExist
                    } else {
                        freshness_disposition
                    };
                    (target, component, freshness_disposition)
                }
                _ => {
                    return Err(WorkerServiceError::TypeChecker(
                        "exactly one of agentId and parameters is required".to_string(),
                    ));
                }
            };
        let method_parameters = request.method_parameters.into_inner();

        let proto_method_parameters: golem_schema::proto::golem::schema::SchemaValue =
            method_parameters.try_into().map_err(|error| {
                WorkerServiceError::TypeChecker(format!(
                    "Agent method parameters cannot cross the worker boundary: {error}"
                ))
            })?;
        let (method_parameters, _) = golem_common::model::agent::NativeMethodInput::capture(
            proto_method_parameters,
            Vec::new(),
        )
        .map_err(WorkerServiceError::TypeChecker)?;

        let proto_schedule_at = request.schedule_at.map(|dt| ::prost_types::Timestamp {
            seconds: dt.timestamp(),
            nanos: dt.timestamp_subsec_nanos() as i32,
        });

        let caller: golem_api_grpc::proto::golem::component::Caller =
            self.rest_caller(&auth).into();

        let mut output = self
            .worker_client
            .invoke_agent(
                &target,
                Some(method_name.clone()),
                Some(golem_common::model::agent::AgentMethodInput::Native(
                    method_parameters.for_external_rest(),
                )),
                proto_mode,
                proto_schedule_at,
                request_key,
                None,
                None,
                freshness_disposition,
                Vec::new(),
                environment_id,
                component.account_id,
                auth.clone(),
                caller,
                None,
                expected_fingerprint,
            )
            .await?;

        let response_agent_id = output.agent_id.take().unwrap_or(target.agent_id);
        let response_idempotency_key = output
            .idempotency_key
            .ok_or_else(|| WorkerServiceError::Internal("Missing idempotency key".to_string()))?;
        let background_failure = output
            .background_failure
            .take()
            .map(invocations::background_failure);

        match output.result {
            golem_common::model::AgentInvocationResult::AgentMethod {
                output: output_value,
            } => {
                let decode_revision = output.component_revision.ok_or_else(|| {
                    WorkerServiceError::Internal("method result has no execution revision".into())
                })?;
                let typed_output = self
                    .agent_method_output_to_external(
                        component.id,
                        decode_revision,
                        agent_type_name,
                        &method_name,
                        output_value,
                    )
                    .await?;
                Ok(AgentInvocationResult {
                    agent_id: response_agent_id,
                    idempotency_key: response_idempotency_key,
                    result: Some(typed_output),
                    component_revision: Some(decode_revision),
                    background_failure,
                })
            }
            _ => Ok(AgentInvocationResult {
                agent_id: response_agent_id,
                idempotency_key: response_idempotency_key,
                result: None,
                component_revision: output.component_revision,
                background_failure,
            }),
        }
    }

    /// Decodes an agent method's result with the method's output schema at the revision that
    /// produced it, so a method that a later revision removed still decodes.
    async fn agent_method_output_to_external(
        &self,
        component_id: ComponentId,
        revision: ComponentRevision,
        agent_type_name: &AgentTypeName,
        method_name: &str,
        output_value: golem_common::schema::SchemaValue,
    ) -> WorkerResult<golem_common::schema::ExternalTypedSchemaValue> {
        let component = self
            .component_service
            .get_revision(component_id, revision)
            .await?;
        let agent_type = component
            .metadata
            .find_agent_type_by_name_ref(agent_type_name)
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent type {agent_type_name} not found in component metadata at revision {revision}",
                ))
            })?;
        let method = agent_type
            .methods
            .iter()
            .find(|m| m.name == method_name)
            .ok_or_else(|| {
                WorkerServiceError::Internal(format!(
                    "Agent method {method_name} not found in agent type {agent_type_name} at revision {revision}",
                ))
            })?;
        let mut output_graph = agent_type.schema.clone();
        output_graph.root = method
            .output_schema
            .schema()
            .cloned()
            .unwrap_or_else(|| SchemaType::tuple(Vec::new()));
        TypedSchemaValue::new(output_graph, output_value)
            .try_into()
            .map_err(|error| {
                WorkerServiceError::Internal(format!(
                    "Agent method result cannot cross the external JSON boundary: {error}"
                ))
            })
    }
}

/// Converts a tool invocation's result to its REST form.
fn native_tool_result(
    result: Result<
        golem_common::model::tool::SerializableToolInvocationResult,
        golem_common::model::tool::SerializableToolRpcError,
    >,
) -> WorkerResult<NativeToolResult> {
    Ok(match result {
        Ok(success) => NativeToolResult::Success(NativeToolSuccess {
            result: success
                .result
                .map(|value| (*value).try_into().map(Box::new))
                .transpose()
                .map_err(|error| {
                    WorkerServiceError::Internal(format!(
                        "external tool result cannot cross the external JSON boundary: {error}"
                    ))
                })?,
        }),
        Err(error) => {
            if let golem_common::model::tool::SerializableToolRpcError::RemoteToolError(tool_error) =
                &error
                && let golem_common::model::tool::SerializableToolError::CustomError(value) =
                    tool_error.as_ref()
            {
                golem_common::schema::ExternalTypedSchemaValue::try_from(value.payload.clone())
                    .map_err(|error| {
                        WorkerServiceError::Internal(format!(
                            "external tool error cannot cross the external JSON boundary: {error}"
                        ))
                    })?;
            }
            NativeToolResult::Failure(NativeToolFailure { error })
        }
    })
}

mod invocations;

pub use invocations::InvocationListQuery;

#[cfg(test)]
mod tests {
    use golem_common::model::caller::ClaimVisibility;

    fn test_golem_issuer() -> GolemIssuer {
        "https://golem.test".parse().unwrap()
    }

    use super::{
        AgentBinding, PublicAgentSessionStart, PublicAgentSessionStartError, WorkerService,
        agent_verb_for_invocation_mode, decode_public_agent_config, decode_public_schema_value,
    };
    use crate::api::agents::{
        AgentInvocationMode, AgentInvocationRequest, AgentKeyRequest, CreateAgentRequest,
        NativeToolDescribeRequest, NativeToolInvocationMode, NativeToolInvocationRequest,
        NativeToolResult,
    };
    use crate::service::agent_resolution_cache::AgentResolutionCache;
    use crate::service::agent_types::AgentTypeResolver;
    use crate::service::auth::{AuthService, AuthServiceError};
    use crate::service::component::{ComponentService, ComponentServiceError};
    use crate::service::limit::{LimitService, LimitServiceError};
    use crate::service::worker::{
        ExistingAgent, RoutedTarget, WorkerClient, WorkerResult, WorkerServiceError, WorkerStream,
    };
    use async_trait::async_trait;
    use chrono::Utc;
    use futures::{StreamExt, stream};
    use golem_api_grpc::proto::golem::worker::{
        InvocationContext, InvocationStart, LogEvent, ResumeAttach, ResumeOperation,
        invocation_request,
    };
    use golem_common::base_model::component_metadata::KnownExports;
    use golem_common::base_model::environment_plugin_grant::EnvironmentPluginGrantId;
    use golem_common::base_model::plugin_registration::PluginRegistrationId;
    use golem_common::model::AgentInvocationOutput;
    use golem_common::model::account::{AccountEmail, AccountId};
    use golem_common::model::agent::{
        AgentConfigEntryDto, AgentMetadataDto, AgentTarget, AgentUpdateMode, ResolvedRevert,
        RevertLastInvocations, RevertToOplogIndex, RevertWorkerTarget,
    };
    use golem_common::model::agent::{
        AgentConfigSource, AgentKey, AgentMode, AgentTypeName, HttpEndpointDetails,
        InvocationFreshnessDisposition, OwnerKind, ParsedAgentKey, RegisteredAgentType,
        RegisteredAgentTypeImplementer, ResolvedAgentType, Snapshotting,
    };
    use golem_common::model::application::{ApplicationId, ApplicationName};
    use golem_common::model::caller::{Caller, Principal};
    use golem_common::model::card::owner::{AgentOwnerLeafPattern, AgentOwnerPattern};
    use golem_common::model::card::{
        AgentInvocationIdPattern, AgentMethodName, AgentPluginName, AgentResourcePattern,
        AgentVerb, Card, CardId, ClassPermissionTarget, EffectiveSurface, FilesystemPathPattern,
        FilesystemResourcePattern, FilesystemVerb, GrantSurface, OplogResourcePattern, OplogVerb,
        PermissionPattern, PermissionTarget, PolymorphicCard, StoredCard,
    };
    use golem_common::model::component::{
        CanonicalFilePath, ComponentId, ComponentName, ComponentRevision, InstalledPlugin,
        PluginPriority,
    };
    use golem_common::model::component_metadata::{AgentTypeProvisionConfig, ComponentMetadata};
    use golem_common::model::deployment::{CurrentDeploymentRevision, DeploymentRevision};
    use golem_common::model::diff::Hash;
    use golem_common::model::environment::{EnvironmentId, EnvironmentName};
    use golem_common::model::filesystem::{FileByteSelection, FileReadHead};
    use golem_common::model::golem_issuer::GolemIssuer;
    use golem_common::model::invocation_session_public::{
        InvocationSelector, PublicAgentTarget, PublicConfigEntry,
    };
    use golem_common::model::json::NormalizedJsonValue;
    use golem_common::model::oplog::{OplogCursor, OplogIndex};
    use golem_common::model::tool::{ToolBindingOwner, ToolName};
    use golem_common::model::type_id::{TypeId, TypeIdKind};
    use golem_common::model::{
        AgentFilter, AgentFingerprint, AgentId, AgentStatus, IdempotencyKey, InvocationRequestKey,
        OwnedAgentId, ScanCursor, Timestamp,
    };
    use golem_common::schema::agent::AgentConfigDeclarationSchema;
    use golem_common::schema::{
        AgentConstructorSchema, AgentMethodSchema, AgentTypeSchema, ExternalSchemaValue,
        InputSchema, NamedField, OutputSchema, SchemaGraph, SchemaType, SchemaValue,
    };
    use golem_schema::Empty;
    use golem_service_base::clients::registry::{
        RegistryService, RegistryServiceError, ResolvedTypeId,
    };
    use golem_service_base::error::worker_executor::WorkerExecutorError;
    use golem_service_base::model::auth::AuthCtx;
    use golem_service_base::model::component::Component;
    use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
    use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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

    #[test]
    fn public_agent_config_is_transcoded_to_canonical_json() {
        let graph = SchemaGraph::anonymous(SchemaType::string());
        let declarations = vec![
            AgentConfigDeclarationSchema {
                source: AgentConfigSource::Local,
                path: vec!["optional".to_string()],
                value_type: SchemaType::option(SchemaType::string()),
            },
            AgentConfigDeclarationSchema {
                source: AgentConfigSource::Local,
                path: vec!["result".to_string()],
                value_type: SchemaType::result(golem_common::schema::ResultSpec {
                    ok: Some(Box::new(SchemaType::string())),
                    err: None,
                }),
            },
            AgentConfigDeclarationSchema {
                source: AgentConfigSource::Local,
                path: vec!["variant".to_string()],
                value_type: SchemaType::variant(vec![golem_common::schema::VariantCaseType {
                    name: "payload".to_string(),
                    payload: Some(SchemaType::string()),
                    metadata: Default::default(),
                }]),
            },
            AgentConfigDeclarationSchema {
                source: AgentConfigSource::Local,
                path: vec!["union".to_string()],
                value_type: SchemaType::union(golem_common::schema::UnionSpec {
                    branches: vec![golem_common::schema::UnionBranch {
                        tag: "command".to_string(),
                        body: SchemaType::string(),
                        discriminator: golem_common::schema::DiscriminatorRule::Prefix {
                            prefix: "cmd:".to_string(),
                        },
                        metadata: Default::default(),
                    }],
                }),
            },
        ];
        let config = decode_public_agent_config(
            &graph,
            &declarations,
            vec![
                PublicConfigEntry {
                    path: vec!["optional".to_string()],
                    value: serde_json::json!("west"),
                },
                PublicConfigEntry {
                    path: vec!["result".to_string()],
                    value: serde_json::json!({"ok": "ready"}),
                },
                PublicConfigEntry {
                    path: vec!["variant".to_string()],
                    value: serde_json::json!({"payload": "value"}),
                },
                PublicConfigEntry {
                    path: vec!["union".to_string()],
                    value: serde_json::json!("cmd:run"),
                },
            ],
        )
        .unwrap();

        assert_eq!(config[0].value.0, serde_json::json!("west"));
        assert_eq!(config[1].value.0, serde_json::json!({"ok": "ready"}));
        assert_eq!(config[2].value.0, serde_json::json!({"payload": "value"}));
        assert_eq!(config[3].value.0, serde_json::json!("cmd:run"));
    }

    #[derive(Clone)]
    struct TestRegistryService {
        resolved: ResolvedAgentType,
        removed: bool,
    }

    #[async_trait]
    impl RegistryService for TestRegistryService {
        async fn resolve_mcp_import(
            &self,
            _: &golem_common::model::mcp_import::McpImportSource,
            _: &AuthCtx,
            _: bool,
        ) -> Result<golem_service_base::model::mcp_import::McpImportObservation, RegistryServiceError>
        {
            panic!("unexpected MCP discovery")
        }
        async fn get_mcp_runtime_credential(
            &self,
            _: &golem_common::model::mcp_import::McpImportSource,
            _: &AuthCtx,
        ) -> Result<golem_service_base::clients::registry::McpRuntimeCredential, RegistryServiceError>
        {
            panic!("unexpected MCP credential request")
        }
        async fn report_mcp_resource_unauthorized(
            &self,
            _: &golem_common::model::mcp_import::McpImportSource,
            _: &AuthCtx,
            _: Option<uuid::Uuid>,
        ) -> Result<(), RegistryServiceError> {
            panic!("unexpected MCP feedback")
        }
        async fn authenticate_token(
            &self,
            _: &golem_common::model::auth::TokenSecret,
        ) -> Result<AuthCtx, RegistryServiceError> {
            unimplemented!()
        }

        async fn get_resource_limits(
            &self,
            _: AccountId,
            _metering: golem_service_base::clients::registry::ResourceUsageMetering,
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

        async fn authorize_environment(
            &self,
            environment_id: EnvironmentId,
            auth_ctx: &AuthCtx,
        ) -> Result<(), RegistryServiceError> {
            use golem_common::model::card::owner::EnvironmentOwnerPattern;
            use golem_common::model::card::{EnvironmentResourcePattern, EnvironmentVerb};
            if environment_id != self.resolved.environment_id {
                return Err(RegistryServiceError::NotFound("environment".into()));
            }
            auth_ctx
                .authorize_permission(&PermissionTarget::Environment(ClassPermissionTarget {
                    owner: EnvironmentOwnerPattern::Environment {
                        account: AccountEmail::new("weather@golem"),
                        application: ApplicationName::try_from("weather-app").unwrap(),
                        environment: EnvironmentName::try_from("prod").unwrap(),
                    },
                    verb: Some(EnvironmentVerb::View),
                    resource: EnvironmentResourcePattern::Any,
                }))
                .map_err(|error| RegistryServiceError::Unauthorized(error.to_string()))
        }

        async fn resolve_type_id(
            &self,
            environment_id: EnvironmentId,
            type_id: TypeId,
        ) -> Result<Option<ResolvedTypeId>, RegistryServiceError> {
            if environment_id == self.resolved.environment_id && type_id.get() == 43 {
                return Ok(Some(ResolvedTypeId {
                    kind: TypeIdKind::Tool,
                    name: "test-tool".into(),
                    implemented_by: None,
                }));
            }
            let registered = &self.resolved.registered_agent_type;
            Ok(
                (environment_id == self.resolved.environment_id && type_id == registered.type_id)
                    .then(|| ResolvedTypeId {
                        kind: TypeIdKind::AgentType,
                        name: registered.agent_type.type_name.0.clone(),
                        implemented_by: Some(registered.implemented_by.clone()),
                    }),
            )
        }

        async fn resolve_agent_type_name(
            &self,
            environment_id: EnvironmentId,
            name: &AgentTypeName,
        ) -> Result<Vec<RegisteredAgentType>, RegistryServiceError> {
            let registered = &self.resolved.registered_agent_type;
            Ok((environment_id == self.resolved.environment_id
                && *name == registered.agent_type.type_name)
                .then(|| registered.clone())
                .into_iter()
                .collect())
        }

        async fn get_deployed_agent_type(
            &self,
            environment_id: EnvironmentId,
            agent_type_name: &AgentTypeName,
        ) -> Result<Option<RegisteredAgentType>, RegistryServiceError> {
            let registered = &self.resolved.registered_agent_type;
            Ok((!self.removed
                && environment_id == self.resolved.environment_id
                && *agent_type_name == registered.agent_type.type_name)
                .then(|| registered.clone()))
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
        tool_state: Option<golem_common::model::tool::ToolDeploymentState>,
        tool_queries: Mutex<Vec<(EnvironmentId, ComponentId, ComponentRevision)>>,
        revision_gate: Mutex<Option<Arc<crate::service::component::tests::LookupGate>>>,
    }

    #[async_trait]
    #[golem_async_trait::region_simple]
    impl ComponentService for StaticComponentService {
        async fn invalidate_current(&self) {}

        async fn get_tool_deployment_state(
            &self,
            environment_id: EnvironmentId,
            component_id: ComponentId,
            revision: ComponentRevision,
        ) -> Result<
            Option<Arc<golem_common::model::tool::ToolDeploymentState>>,
            ComponentServiceError,
        > {
            self.tool_queries
                .lock()
                .unwrap()
                .push((environment_id, component_id, revision));
            Ok(self.tool_state.clone().map(Arc::new))
        }

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
            let gate = self.revision_gate.lock().unwrap().take();
            if let Some(gate) = gate {
                gate.started.add_permits(1);
                gate.release.acquire().await.unwrap().forget();
            }
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

    type RecordedCardTransfer = (AgentId, EnvironmentId, Uuid, CardId, StoredCard, AuthCtx);

    struct RecordingWorkerClient {
        component_id: ComponentId,
        metadata_agent_type_name: Mutex<Option<AgentTypeName>>,
        metadata_canonical_key: Mutex<Option<String>>,
        created_targets: Mutex<Vec<RoutedTarget>>,
        invoked_targets: Mutex<Vec<RoutedTarget>>,
        created_agent_ids: Mutex<Vec<AgentId>>,
        created_callers: Mutex<Vec<Caller>>,
        forked_callers: Mutex<Vec<Caller>>,
        updated_callers: Mutex<Vec<Caller>>,
        creating_idempotency_keys: Mutex<Vec<Option<IdempotencyKey>>>,
        creating_ignore_already_existing: Mutex<Vec<bool>>,
        metadata_environment_id: Mutex<Option<EnvironmentId>>,
        prepared_agent_ids: Mutex<Vec<AgentId>>,
        delivered_card_transfers: Mutex<Vec<RecordedCardTransfer>>,
        invocations: Mutex<Vec<(AgentId, IdempotencyKey, InvocationFreshnessDisposition)>>,
        invocation_inputs: Mutex<Vec<Option<golem_common::model::agent::AgentMethodInput>>>,
        invocation_modes_and_schedules: Mutex<Vec<(i32, Option<::prost_types::Timestamp>)>>,
        invocation_environments: Mutex<Vec<EnvironmentId>>,
        invocation_expected_fingerprints: Mutex<Vec<Option<AgentFingerprint>>>,
        invocation_session_starts: Mutex<Vec<(AgentId, InvocationStart)>>,
        invocation_session_resumes: Mutex<Vec<(AgentId, ResumeAttach)>>,
        effects: Mutex<Vec<&'static str>>,
        invocation_output: Mutex<AgentInvocationOutput>,
        metadata_component_revision: Mutex<Option<ComponentRevision>>,
        listed_agents: Mutex<(Option<ScanCursor>, Vec<ExistingAgent>)>,
        file_read_missing: Mutex<bool>,
        deleted_agent_ids: Mutex<Vec<AgentId>>,
        fingerprint: AgentFingerprint,
        oplog_entry_reads: Mutex<Vec<(OplogIndex, Option<ComponentRevision>, ClaimVisibility)>>,
        invocation_summary:
            Mutex<Option<golem_api_grpc::proto::golem::workerexecutor::v1::InvocationSummary>>,
        revert_resolution_error: Mutex<Option<String>>,
    }

    impl RecordingWorkerClient {
        fn new(invocation_output: AgentInvocationOutput, component_id: ComponentId) -> Self {
            Self {
                component_id,
                metadata_agent_type_name: Mutex::new(Some(AgentTypeName(
                    "weather-agent".to_string(),
                ))),
                metadata_canonical_key: Mutex::new(None),
                created_targets: Mutex::new(Vec::new()),
                invoked_targets: Mutex::new(Vec::new()),
                created_agent_ids: Mutex::new(Vec::new()),
                created_callers: Mutex::new(Vec::new()),
                forked_callers: Mutex::new(Vec::new()),
                updated_callers: Mutex::new(Vec::new()),
                creating_idempotency_keys: Mutex::new(Vec::new()),
                creating_ignore_already_existing: Mutex::new(Vec::new()),
                metadata_environment_id: Mutex::new(None),
                prepared_agent_ids: Mutex::new(Vec::new()),
                delivered_card_transfers: Mutex::new(Vec::new()),
                invocations: Mutex::new(Vec::new()),
                invocation_inputs: Mutex::new(Vec::new()),
                invocation_modes_and_schedules: Mutex::new(Vec::new()),
                invocation_environments: Mutex::new(Vec::new()),
                invocation_expected_fingerprints: Mutex::new(Vec::new()),
                invocation_session_starts: Mutex::new(Vec::new()),
                invocation_session_resumes: Mutex::new(Vec::new()),
                effects: Mutex::new(Vec::new()),
                invocation_output: Mutex::new(invocation_output),
                metadata_component_revision: Mutex::new(None),
                listed_agents: Mutex::new((None, Vec::new())),
                file_read_missing: Mutex::new(false),
                deleted_agent_ids: Mutex::new(Vec::new()),
                fingerprint: AgentFingerprint::new(),
                oplog_entry_reads: Mutex::new(Vec::new()),
                invocation_summary: Mutex::new(None),
                revert_resolution_error: Mutex::new(None),
            }
        }

        fn with_metadata_component_revision(
            invocation_output: AgentInvocationOutput,
            component_id: ComponentId,
            component_revision: ComponentRevision,
        ) -> Self {
            let client = Self::new(invocation_output, component_id);
            client.set_metadata_component_revision(component_revision);
            client
        }

        fn set_metadata_component_revision(&self, component_revision: ComponentRevision) {
            *self.metadata_component_revision.lock().unwrap() = Some(component_revision);
        }

        fn set_invocation_output(&self, output: AgentInvocationOutput) {
            *self.invocation_output.lock().unwrap() = output;
        }

        fn created_agent_id(&self) -> AgentId {
            self.created_agent_ids.lock().unwrap()[0]
        }

        fn created_targets(&self) -> Vec<RoutedTarget> {
            self.created_targets.lock().unwrap().clone()
        }

        fn invoked_agent_id(&self) -> AgentId {
            self.invocations.lock().unwrap()[0].0
        }

        fn invoked_targets(&self) -> Vec<RoutedTarget> {
            self.invoked_targets.lock().unwrap().clone()
        }

        fn invocations(&self) -> Vec<(AgentId, IdempotencyKey, InvocationFreshnessDisposition)> {
            self.invocations.lock().unwrap().clone()
        }

        fn invocation_modes_and_schedules(&self) -> Vec<(i32, Option<::prost_types::Timestamp>)> {
            self.invocation_modes_and_schedules.lock().unwrap().clone()
        }

        fn invocation_expected_fingerprints(&self) -> Vec<Option<AgentFingerprint>> {
            self.invocation_expected_fingerprints
                .lock()
                .unwrap()
                .clone()
        }

        fn invocation_environment(&self) -> EnvironmentId {
            self.invocation_environments.lock().unwrap()[0]
        }

        fn invocation_session_start(&self) -> (AgentId, InvocationStart) {
            self.invocation_session_starts.lock().unwrap()[0].clone()
        }

        fn invocation_session_starts(&self) -> Vec<(AgentId, InvocationStart)> {
            self.invocation_session_starts.lock().unwrap().clone()
        }

        fn invocation_session_start_count(&self) -> usize {
            self.invocation_session_starts.lock().unwrap().len()
        }

        fn invocation_session_resume(&self) -> (AgentId, ResumeAttach) {
            self.invocation_session_resumes.lock().unwrap()[0].clone()
        }

        fn invocation_session_resume_count(&self) -> usize {
            self.invocation_session_resumes.lock().unwrap().len()
        }

        fn delivered_card_transfers(&self) -> Vec<RecordedCardTransfer> {
            self.delivered_card_transfers.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl WorkerClient for RecordingWorkerClient {
        async fn prepare(
            &self,
            target: &RoutedTarget,
            _: HashMap<String, String>,
            _: Vec<AgentConfigEntryDto>,
            _: bool,
            _: AccountId,
            _: EnvironmentId,
            _: AuthCtx,
            _: Option<InvocationContext>,
            _: golem_api_grpc::proto::golem::component::Caller,
            _: Option<IdempotencyKey>,
            _: InvocationFreshnessDisposition,
            _: Option<InvocationStart>,
        ) -> WorkerResult<(AgentId, AgentFingerprint)> {
            self.prepared_agent_ids
                .lock()
                .unwrap()
                .push(target.agent_id);
            Ok((target.agent_id, self.fingerprint))
        }

        async fn create(
            &self,
            target: &RoutedTarget,
            _: HashMap<String, String>,
            _: Vec<AgentConfigEntryDto>,
            ignore_already_existing: bool,
            _: AccountId,
            _: EnvironmentId,
            _: AuthCtx,
            _: Option<InvocationContext>,
            caller: golem_api_grpc::proto::golem::component::Caller,
            idempotency_key: Option<IdempotencyKey>,
            _: Option<InvocationStart>,
        ) -> WorkerResult<(AgentId, AgentFingerprint)> {
            self.created_callers
                .lock()
                .unwrap()
                .push(Caller::try_from(caller).unwrap());
            self.creating_ignore_already_existing
                .lock()
                .unwrap()
                .push(ignore_already_existing);
            self.created_agent_ids.lock().unwrap().push(target.agent_id);
            self.created_targets.lock().unwrap().push(target.clone());
            self.creating_idempotency_keys
                .lock()
                .unwrap()
                .push(idempotency_key);
            Ok((target.agent_id, self.fingerprint))
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

        async fn delete(
            &self,
            agent_id: &AgentId,
            _: EnvironmentId,
            expected_fingerprint: AgentFingerprint,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            assert_eq!(expected_fingerprint, self.fingerprint);
            self.effects.lock().unwrap().push("delete");
            self.deleted_agent_ids.lock().unwrap().push(*agent_id);
            Ok(())
        }

        async fn complete_promise(
            &self,
            _: &golem_common::model::PromiseId,
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
            expected_fingerprint: AgentFingerprint,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            assert_eq!(expected_fingerprint, self.fingerprint);
            self.effects.lock().unwrap().push("interrupt");
            Ok(())
        }

        async fn get_metadata(
            &self,
            agent_id: &AgentId,
            environment_id: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<ExistingAgent> {
            self.effects.lock().unwrap().push("metadata");
            if self
                .metadata_environment_id
                .lock()
                .unwrap()
                .is_some_and(|owner| owner != environment_id)
            {
                return Err(WorkerServiceError::AgentNotFound(*agent_id));
            }
            match *self.metadata_component_revision.lock().unwrap() {
                Some(component_revision) => Ok(ExistingAgent {
                    component_id: self.component_id,
                    metadata: AgentMetadataDto {
                        agent_id: *agent_id,
                        environment_id,
                        created_by: AccountId(Uuid::new_v4()),
                        env: HashMap::new(),
                        config: Vec::new(),
                        status: AgentStatus::Idle,
                        underlying_status: None,
                        component_revision,
                        retry_count: 0,
                        pending_invocation_count: 0,
                        updates: Vec::new(),
                        created_at: Timestamp::now_utc(),
                        last_error: None,
                        last_error_kind: None,
                        component_size: 0,
                        total_linear_memory_size: 0,
                        exported_resource_instances: Vec::new(),
                        active_plugins: HashSet::new(),
                        skipped_regions: Vec::new(),
                        deleted_regions: Vec::new(),
                        last_oplog_index: OplogIndex::INITIAL,
                        fingerprint: self.fingerprint,
                        owner_kind: if agent_id.type_id().is_some() {
                            OwnerKind::ComponentAgent
                        } else {
                            OwnerKind::EphemeralExternalTool
                        },
                        agent_type_name: agent_id
                            .type_id()
                            .and(self.metadata_agent_type_name.lock().unwrap().clone()),
                        canonical_key: self.metadata_canonical_key.lock().unwrap().clone(),
                        constructor_parameters: None,
                    },
                }),
                None => Err(WorkerServiceError::AgentNotFound(*agent_id)),
            }
        }

        async fn find_metadata(
            &self,
            _: TypeId,
            _: Option<AgentFilter>,
            _: ScanCursor,
            _: u64,
            _: bool,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<(Option<ScanCursor>, Vec<ExistingAgent>)> {
            Ok(self.listed_agents.lock().unwrap().clone())
        }

        async fn resume(
            &self,
            _: &AgentId,
            _: bool,
            _: EnvironmentId,
            expected_fingerprint: AgentFingerprint,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            assert_eq!(expected_fingerprint, self.fingerprint);
            self.effects.lock().unwrap().push("resume");
            Ok(())
        }

        async fn update(
            &self,
            _: &AgentId,
            _: golem_common::model::agent::AgentUpdateMode,
            _: ComponentRevision,
            _: bool,
            requester: Caller,
            _: EnvironmentId,
            expected_fingerprint: AgentFingerprint,
            _: AuthCtx,
        ) -> WorkerResult<OplogIndex> {
            assert_eq!(expected_fingerprint, self.fingerprint);
            self.effects.lock().unwrap().push("update");
            self.updated_callers.lock().unwrap().push(requester);
            Ok(OplogIndex::INITIAL)
        }

        async fn get_oplog(
            &self,
            _: &AgentId,
            _: OplogIndex,
            _: Option<OplogCursor>,
            _: u64,
            _: Option<super::OplogRangeRead>,
            claims: ClaimVisibility,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> Result<GetOplogResponse, super::WorkerServiceError> {
            self.effects.lock().unwrap().push(if claims.is_shown() {
                "get-oplog-with-claims"
            } else {
                "get-oplog"
            });
            Ok(GetOplogResponse {
                entries: Vec::new(),
                next: None,
                first_index_in_chunk: 0,
                last_index: 0,
                scan: None,
            })
        }

        async fn get_oplog_entry(
            &self,
            _: &AgentId,
            oplog_index: OplogIndex,
            component_revision: Option<ComponentRevision>,
            claims: ClaimVisibility,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> Result<golem_common::model::oplog::PublicOplogEntryWithIndex, super::WorkerServiceError>
        {
            self.oplog_entry_reads
                .lock()
                .unwrap()
                .push((oplog_index, component_revision, claims));
            Ok(golem_common::model::oplog::PublicOplogEntryWithIndex {
                oplog_index,
                attribution: golem_common::model::oplog::PublicOplogEntryAttribution::agent(),
                entry: golem_common::model::oplog::PublicOplogEntry::Log(
                    golem_common::model::oplog::public_oplog_entry::LogParams {
                        timestamp: golem_common::model::Timestamp::now_utc(),
                        level: golem_common::model::oplog::LogLevel::Info,
                        context: String::new(),
                        message: String::new(),
                        trace_context: None,
                    },
                ),
            })
        }

        async fn search_oplog(
            &self,
            _: &AgentId,
            _: Option<OplogCursor>,
            _: u64,
            _: String,
            claims: ClaimVisibility,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> Result<GetOplogResponse, super::WorkerServiceError> {
            self.effects.lock().unwrap().push(if claims.is_shown() {
                "search-oplog-with-claims"
            } else {
                "search-oplog"
            });
            Ok(GetOplogResponse {
                entries: Vec::new(),
                next: None,
                first_index_in_chunk: 0,
                last_index: 0,
                scan: None,
            })
        }

        async fn get_file_system_node(
            &self,
            _: &AgentId,
            _: CanonicalFilePath,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<Vec<ComponentFileSystemNode>> {
            self.effects.lock().unwrap().push("filesystem-node");
            Ok(Vec::new())
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
            target: &RoutedTarget,
            _: CanonicalFilePath,
            _: FileByteSelection,
            _: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
        ) -> WorkerResult<golem_service_base::model::FileReadResponse> {
            self.effects.lock().unwrap().push("file-contents");
            if *self.file_read_missing.lock().unwrap() {
                return Err(WorkerExecutorError::worker_not_found(target.agent_id).into());
            }
            Ok(golem_service_base::model::FileReadResponse {
                head: FileReadHead::File(golem_common::model::filesystem::FileReadMetadata {
                    total_size: 0,
                    selection: golem_common::model::filesystem::FileReadExtent::Selected {
                        offset: 0,
                        length: 0,
                    },
                    modified_at: None,
                }),
                body: Box::pin(futures::stream::empty()),
            })
        }

        async fn activate_plugin(
            &self,
            _: &AgentId,
            _: PluginPriority,
            _: EnvironmentId,
            expected_fingerprint: AgentFingerprint,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            assert_eq!(expected_fingerprint, self.fingerprint);
            self.effects.lock().unwrap().push("activate-plugin");
            Ok(())
        }

        async fn deactivate_plugin(
            &self,
            _: &AgentId,
            _: PluginPriority,
            _: EnvironmentId,
            expected_fingerprint: AgentFingerprint,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            assert_eq!(expected_fingerprint, self.fingerprint);
            self.effects.lock().unwrap().push("deactivate-plugin");
            Ok(())
        }

        async fn fork_worker(
            &self,
            _: &AgentId,
            _: &AgentId,
            _: OplogIndex,
            _: EnvironmentId,
            _: AccountId,
            _: AccountEmail,
            caller: Caller,
            expected_fingerprint: AgentFingerprint,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            self.forked_callers.lock().unwrap().push(caller);
            assert_eq!(expected_fingerprint, self.fingerprint);
            self.effects.lock().unwrap().push("fork");
            Ok(())
        }

        async fn revert_worker(
            &self,
            _: &AgentId,
            _: RevertWorkerTarget,
            _: Option<ResolvedRevert>,
            _: EnvironmentId,
            expected_fingerprint: AgentFingerprint,
            _: AuthCtx,
        ) -> WorkerResult<()> {
            assert_eq!(expected_fingerprint, self.fingerprint);
            self.effects.lock().unwrap().push("revert");
            Ok(())
        }

        async fn resolve_revert_last_invocations(
            &self,
            _: &AgentId,
            _: u64,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<ResolvedRevert> {
            if let Some(error) = self.revert_resolution_error.lock().unwrap().clone() {
                return Err(WorkerExecutorError::invalid_request(error).into());
            }
            Ok(ResolvedRevert {
                last_oplog_index: OplogIndex::from_u64(42),
            })
        }

        async fn cancel_invocation(
            &self,
            _: &AgentId,
            _: &golem_common::model::InvocationId,
            _: EnvironmentId,
            _: AuthCtx,
            _: Option<golem_api_grpc::proto::golem::worker::ScheduledInvocationCancellation>,
            expected_fingerprint: Option<AgentFingerprint>,
        ) -> WorkerResult<bool> {
            assert_eq!(expected_fingerprint, Some(self.fingerprint));
            self.effects.lock().unwrap().push("cancel-invocation");
            Ok(true)
        }

        async fn get_invocation(
            &self,
            _: &AgentId,
            _: &golem_common::model::InvocationId,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<Option<golem_api_grpc::proto::golem::workerexecutor::v1::InvocationSummary>>
        {
            Ok(self.invocation_summary.lock().unwrap().clone())
        }

        async fn list_invocations(
            &self,
            _: &AgentId,
            _: golem_api_grpc::proto::golem::workerexecutor::v1::ListInvocationsRequest,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<golem_api_grpc::proto::golem::workerexecutor::v1::ListInvocationsSuccess>
        {
            unimplemented!()
        }

        async fn lookup_invocations(
            &self,
            _: &AgentId,
            _: &IdempotencyKey,
            _: Option<String>,
            _: EnvironmentId,
            _: AuthCtx,
        ) -> WorkerResult<Vec<golem_api_grpc::proto::golem::workerexecutor::v1::InvocationSummary>>
        {
            unimplemented!()
        }

        async fn invoke_agent(
            &self,
            target: &RoutedTarget,
            method_name: Option<String>,
            input: Option<golem_common::model::agent::AgentMethodInput>,
            mode: i32,
            schedule_at: Option<::prost_types::Timestamp>,
            request_key: InvocationRequestKey,
            _: Option<IdempotencyKey>,
            _: Option<InvocationContext>,
            freshness_disposition: InvocationFreshnessDisposition,
            _: Vec<AgentConfigEntryDto>,
            environment_id: EnvironmentId,
            _: AccountId,
            _: AuthCtx,
            _: golem_api_grpc::proto::golem::component::Caller,
            _: Option<golem_api_grpc::proto::golem::worker::EncodedScopeCard>,
            expected_fingerprint: Option<AgentFingerprint>,
        ) -> WorkerResult<AgentInvocationOutput> {
            let agent_id = target.agent_id;
            self.invocation_inputs.lock().unwrap().push(input);
            self.invoked_targets.lock().unwrap().push(target.clone());
            self.invocation_expected_fingerprints
                .lock()
                .unwrap()
                .push(expected_fingerprint);
            let idempotency_key = if mode
                == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32
            {
                match &request_key {
                    InvocationRequestKey::External { key, .. }
                    | InvocationRequestKey::Internal(key)
                    | InvocationRequestKey::RouteSession { key, .. } => key.clone(),
                    InvocationRequestKey::Unkeyed(_) => panic!("lookup requires a final key"),
                }
            } else {
                request_key.owner_key(method_name.as_deref().unwrap_or_default())
            };
            self.invocations.lock().unwrap().push((
                agent_id,
                idempotency_key.clone(),
                freshness_disposition,
            ));
            self.invocation_modes_and_schedules
                .lock()
                .unwrap()
                .push((mode, schedule_at));
            self.invocation_environments
                .lock()
                .unwrap()
                .push(environment_id);
            let mut output = self.invocation_output.lock().unwrap().clone();
            output.agent_id = Some(agent_id);
            output
                .idempotency_key
                .get_or_insert(idempotency_key.value.parse().unwrap());
            Ok(output)
        }

        async fn invoke_agent_session(
            &self,
            agent_id: &AgentId,
            mut request: super::InvocationRequestStream,
        ) -> WorkerResult<super::InvocationResponseStream> {
            let first = request
                .next()
                .await
                .expect("worker service should send an invocation control request");
            match first.request {
                Some(invocation_request::Request::Start(start)) => self
                    .invocation_session_starts
                    .lock()
                    .unwrap()
                    .push((*agent_id, start)),
                Some(invocation_request::Request::ResumeAttach(resume)) => self
                    .invocation_session_resumes
                    .lock()
                    .unwrap()
                    .push((*agent_id, resume)),
                other => panic!("expected invocation start or resume, got {other:?}"),
            }
            Ok(Box::pin(stream::empty()))
        }

        async fn invoke_agent_session_one_shot(
            &self,
            agent_id: &AgentId,
            start: InvocationStart,
        ) -> WorkerResult<AgentInvocationOutput> {
            let key: golem_common::model::InvocationId = if start.mode()
                == golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup
            {
                start
                    .idempotency_key
                    .as_ref()
                    .unwrap()
                    .value
                    .parse()
                    .unwrap()
            } else {
                let prepared =
                    InvocationRequestKey::try_from(start.request_key.clone().unwrap()).unwrap();
                let operation = start
                    .external_tool
                    .as_ref()
                    .map(|tool| {
                        golem_common::model::InvocationOperation::tool_operation(
                            &tool.tool_name,
                            &tool.command_path,
                        )
                    })
                    .unwrap_or_else(|| start.method_name.clone().unwrap_or_default());
                prepared
                    .finalize(*agent_id, self.fingerprint, &operation)
                    .id()
            };
            self.invocation_session_starts
                .lock()
                .unwrap()
                .push((*agent_id, start));
            let mut output = self.invocation_output.lock().unwrap().clone();
            output.agent_id = Some(*agent_id);
            output.idempotency_key.get_or_insert(key);
            Ok(output)
        }

        async fn deliver_card_transfer(
            &self,
            target_agent_id: &AgentId,
            environment_id: EnvironmentId,
            transfer_id: uuid::Uuid,
            source_card_id: CardId,
            card: StoredCard,
            auth_ctx: AuthCtx,
        ) -> WorkerResult<()> {
            self.delivered_card_transfers.lock().unwrap().push((
                *target_agent_id,
                environment_id,
                transfer_id,
                source_card_id,
                card,
                auth_ctx,
            ));
            Ok(())
        }

        async fn process_oplog_entries(
            &self,
            _: &AgentId,
            _: EnvironmentId,
            _: ComponentId,
            _: ComponentRevision,
            _: golem_common::model::InvocationId,
            _: AccountId,
            _: HashMap<String, String>,
            _: golem_api_grpc::proto::golem::worker::AgentMetadata,
            _: OplogIndex,
            _: Vec<golem_api_grpc::proto::golem::worker::RawOplogEntry>,
            _: AuthCtx,
            _: golem_api_grpc::proto::golem::component::Caller,
        ) -> WorkerResult<()> {
            self.effects.lock().unwrap().push("process-oplog-entries");
            Ok(())
        }
    }

    struct RestHarness {
        worker_service: WorkerService,
        worker_client: Arc<RecordingWorkerClient>,
        agent_type_name: AgentTypeName,
        component_id: ComponentId,
        component_revision: ComponentRevision,
        environment_id: EnvironmentId,
        type_id: TypeId,
    }

    /// The type ID that the test registry assigns to the harness's agent type.
    fn test_type_id() -> TypeId {
        TypeId::try_from(42u32).unwrap()
    }

    /// The deployed-agent-type resolver of a registry.
    fn agent_type_resolver(registry: Arc<TestRegistryService>) -> Arc<AgentTypeResolver> {
        Arc::new(AgentTypeResolver::new(
            registry,
            1,
            Duration::from_secs(60),
            Duration::from_secs(60),
        ))
    }

    /// The harness's tool `weather`: one subcommand `current`, with the alias `now`.
    fn weather_tool_definition() -> golem_common::schema::tool::Tool {
        use golem_common::schema::tool::{
            CommandBody, CommandIndex, CommandNode, CommandTree, Doc, Tool,
        };
        let body = CommandBody {
            positionals: Default::default(),
            options: vec![],
            flags: vec![],
            constraints: vec![],
            stdin: None,
            stdout: None,
            stderr: None,
            result: None,
            errors: vec![],
            annotations: None,
        };
        Tool {
            version: "1.0.0".into(),
            requires_filesystem: false,
            http_mount: None,
            schema: SchemaGraph::empty(),
            commands: CommandTree {
                nodes: vec![
                    CommandNode {
                        name: "weather".into(),
                        aliases: vec![],
                        doc: Doc::default(),
                        http: None,
                        globals: Default::default(),
                        subcommands: vec![CommandIndex(1)],
                        body: None,
                    },
                    CommandNode {
                        name: "current".into(),
                        aliases: vec!["now".into()],
                        doc: Doc::default(),
                        http: None,
                        globals: Default::default(),
                        subcommands: vec![],
                        body: Some(body),
                    },
                ],
            },
        }
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
            let worker_client = Arc::new(RecordingWorkerClient::new(
                AgentInvocationOutput {
                    result: golem_common::model::AgentInvocationResult::AgentInitialization,
                    consumed_fuel: None,
                    invocation_status: None,
                    component_revision: Some(component_revision),
                    agent_id: None,
                    idempotency_key: None,
                    oplog_index: None,
                    agent_fingerprint: None,
                    background_failure: None,
                },
                component_id,
            ));
            *worker_client.metadata_environment_id.lock().unwrap() = Some(environment_id);
            let registry = Arc::new(TestRegistryService {
                removed: false,
                resolved: ResolvedAgentType {
                    registered_agent_type: RegisteredAgentType {
                        type_id: test_type_id(),
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
                registry.clone(),
                1,
                Duration::from_secs(60),
                Duration::from_secs(60),
            ));
            let agent_types = agent_type_resolver(registry);
            let tool_state = crate::service::component::tests::tool_deployment_state(
                &component,
                "weather",
                weather_tool_definition(),
                [
                    ToolBindingOwner::ComponentBaseline { component_id },
                    ToolBindingOwner::AgentType {
                        agent_type_name: agent_type_name.clone(),
                    },
                ],
            );

            Self {
                worker_service: WorkerService::new(
                    Arc::new(StaticComponentService {
                        components: vec![component],
                        tool_state: Some(tool_state),
                        tool_queries: Mutex::new(vec![]),
                        revision_gate: Mutex::new(None),
                    }),
                    Arc::new(AllowAllAuthService),
                    Arc::new(NoopLimitService),
                    worker_client.clone(),
                    agent_resolution_cache,
                    agent_types,
                    test_golem_issuer(),
                ),
                worker_client,
                agent_type_name,
                component_id,
                component_revision,
                environment_id,
                type_id: test_type_id(),
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
                    background_failure: None,
                },
                component_id,
                pinned_revision,
            ));
            *worker_client.metadata_environment_id.lock().unwrap() = Some(environment_id);
            let registry = Arc::new(TestRegistryService {
                removed: false,
                resolved: ResolvedAgentType {
                    registered_agent_type: RegisteredAgentType {
                        type_id: test_type_id(),
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
                registry.clone(),
                1,
                Duration::from_secs(60),
                Duration::from_secs(60),
            ));
            let agent_types = agent_type_resolver(registry);

            Self {
                worker_service: WorkerService::new(
                    Arc::new(StaticComponentService {
                        components: vec![pinned_component, latest_component],
                        tool_state: None,
                        tool_queries: Mutex::new(vec![]),
                        revision_gate: Mutex::new(None),
                    }),
                    Arc::new(AllowAllAuthService),
                    Arc::new(NoopLimitService),
                    worker_client.clone(),
                    agent_resolution_cache,
                    agent_types,
                    test_golem_issuer(),
                ),
                worker_client,
                agent_type_name,
                component_id,
                component_revision: latest_revision,
                environment_id,
                type_id: test_type_id(),
            }
        }

        /// The constructor `weather-agent()` of the harness's agent type.
        fn constructor(&self) -> ParsedAgentKey {
            ParsedAgentKey::from(self.agent_key())
        }

        fn agent_key(&self) -> AgentKey {
            AgentKey::new(self.agent_type_name.clone(), empty_constructor_parameters()).unwrap()
        }

        /// The ID of the keyed agent `weather-agent()`, derived as the executor derives it.
        fn keyed_agent_id(&self) -> AgentId {
            AgentId::keyed(self.environment_id, self.type_id, &self.agent_key())
        }

        /// The keyed agent `weather-agent()` in the harness's environment.
        fn some_agent_id(&self) -> OwnedAgentId {
            OwnedAgentId::new(self.environment_id, &self.keyed_agent_id())
        }

        /// An agent of the harness's environment with this ID.
        fn owned(&self, agent_id: &AgentId) -> OwnedAgentId {
            OwnedAgentId::new(self.environment_id, agent_id)
        }

        fn create_request(&self, keyed: bool) -> CreateAgentRequest {
            CreateAgentRequest {
                parameters: empty_json_tuple(),
                config: vec![],
                env: HashMap::new(),
                keyed,
            }
        }

        fn invoke_request(&self) -> AgentInvocationRequest {
            AgentInvocationRequest {
                agent_id: None,
                parameters: Some(empty_json_tuple()),
                method: "run".to_string(),
                method_parameters: empty_json_tuple(),
                mode: AgentInvocationMode::Await,
                schedule_at: None,
                expected_fingerprint: None,
            }
        }

        async fn invoke_rest(
            &self,
            request: AgentInvocationRequest,
            idempotency_key: Option<IdempotencyKey>,
            auth: AuthCtx,
        ) -> WorkerResult<crate::api::agents::AgentInvocationResult> {
            self.worker_service
                .invoke_agent_rest(
                    self.environment_id,
                    &self.agent_type_name,
                    request,
                    idempotency_key,
                    auth,
                )
                .await
        }

        fn tool_request(&self) -> NativeToolInvocationRequest {
            NativeToolInvocationRequest {
                agent_id: None,
                component_id: Some(self.component_id),
                tool_name: "weather".to_string(),
                command_path: vec!["current".to_string()],
                input: Some(empty_constructor_parameters().try_into().unwrap()),
                mode: NativeToolInvocationMode::Await,
                schedule_at: None,
            }
        }

        fn tool_key() -> IdempotencyKey {
            IdempotencyKey::new("native-tool-key".to_string())
        }

        fn public_invocation_start(
            &self,
            idempotency_key: IdempotencyKey,
        ) -> PublicAgentSessionStart {
            PublicAgentSessionStart {
                environment_id: self.environment_id,
                selector: InvocationSelector {
                    application: "weather-app".to_string(),
                    environment: "prod".to_string(),
                    agent_type: self.agent_type_name.0.clone(),
                    target: PublicAgentTarget::Key {
                        constructor_parameters: serde_json::json!({
                            "kind": "record",
                            "value": {"fields": []}
                        }),
                    },
                    method: "run".to_string(),
                },
                config: vec![],
                method_parameters: serde_json::json!({
                    "kind": "record",
                    "value": {"fields": []}
                })
                .try_into()
                .unwrap(),
                idempotency_key: idempotency_key.value,
                attempt_id: Uuid::new_v4(),
            }
        }

        fn public_resume(&self) -> ResumeAttach {
            let agent_id = self.keyed_agent_id();
            ResumeAttach {
                idempotency_key: Some(golem_common::model::InvocationId::new().into()),
                agent_id: Some(agent_id.into()),
                environment_id: Some(self.environment_id.into()),
                attachment_id: Some(Uuid::new_v4().into()),
                attempt_id: Some(Uuid::new_v4().into()),
                expected_callee_fingerprint: Some(self.worker_client.fingerprint.0.into()),
                expected_epoch: 3,
                operation: ResumeOperation::Resume as i32,
                cursors: Vec::new(),
                auth_ctx: Some(AuthCtx::system().into()),
                caller: Some(Default::default()),
            }
        }
    }

    fn test_agent_type(agent_type_name: AgentTypeName, mode: AgentMode) -> AgentTypeSchema {
        AgentTypeSchema {
            kind: golem_common::schema::agent::AgentTypeKind::Regular,
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
                    http_method: golem_common::model::agent::HttpMethod::Method(
                        golem_schema::http::HttpMethod::Get(Default::default()),
                    ),
                    path_suffix: vec![],
                    header_vars: vec![],
                    query_vars: vec![],
                    auth_details: None,
                    cors_options: golem_common::model::agent::CorsOptions {
                        allowed_patterns: vec![],
                    },
                    mode: None,
                    durable_streams: None,
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
        let provision_config = AgentTypeProvisionConfig {
            initial_permissions: PolymorphicCard {
                card_id: CardId::new(),
                parent_ids: Vec::new(),
                lower_positive: Vec::new(),
                lower_negative: Vec::new(),
                upper_positive: Vec::new(),
                upper_negative: Vec::new(),
                created_at: Utc::now(),
                expires_at: None,
                system_card: false,
            },
            env: BTreeMap::new(),
            config: Vec::new(),
            plugins: vec![InstalledPlugin {
                environment_plugin_grant_id: EnvironmentPluginGrantId::new(),
                priority: PluginPriority(7),
                parameters: BTreeMap::new(),
                plugin_registration_id: PluginRegistrationId::new(),
                plugin_name: "audit-plugin".to_string(),
                plugin_version: "1.0.0".to_string(),
                oplog_processor_component_id: None,
                oplog_processor_component_revision: None,
                installed_by: golem_common::model::account::AccountId::new(),
            }],
            files: Vec::new(),
        };
        let agent_type_name = agent_type.type_name.clone();
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
                vec![agent_type],
                BTreeMap::from([(agent_type_name, provision_config)]),
            ),
            created_at: Utc::now(),
            wasm_hash: Hash::empty(),
            package_metadata: None,
            object_store_key: String::new(),
        }
    }

    fn empty_json_tuple() -> ExternalSchemaValue {
        ExternalSchemaValue::try_from(SchemaValue::Record { fields: vec![] }).unwrap()
    }

    fn test_card() -> StoredCard {
        StoredCard::Concrete(Card {
            card_id: CardId::new(),
            parent_ids: vec![],
            lower_positive: vec![],
            lower_negative: vec![],
            upper_positive: vec![],
            upper_negative: vec![],
            created_at: Utc::now(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        })
    }

    fn agent_target(
        agent_id: &AgentId,
        verb: AgentVerb,
        resource: AgentResourcePattern,
    ) -> PermissionTarget {
        PermissionTarget::Agent(ClassPermissionTarget {
            owner: AgentOwnerPattern::Agent {
                account: AccountEmail::new("weather@golem"),
                application: ApplicationName::try_from("weather-app").unwrap(),
                environment: EnvironmentName::try_from("prod").unwrap(),
                component: ComponentName("weather-component".to_string()),
                agent: AgentOwnerLeafPattern::agent(*agent_id, None),
            },
            verb: Some(verb),
            resource,
        })
    }

    fn auth_ctx_with_permissions(positive: Vec<PermissionTarget>) -> AuthCtx {
        AuthCtx::agent_with_effective_surface(
            AccountId::new(),
            AccountEmail::new("caller@golem"),
            EffectiveSurface {
                source_card_ids: Vec::new(),
                lower: vec![GrantSurface {
                    positive,
                    negative: Vec::new(),
                }],
                upper: Vec::new(),
            },
        )
    }

    fn scoped_invocation_auth() -> AuthCtx {
        auth_ctx_with_permissions(vec![PermissionTarget::Agent(ClassPermissionTarget {
            owner: AgentOwnerPattern::AnyAgents,
            verb: None,
            resource: AgentResourcePattern::Any,
        })])
    }

    fn oplog_target(agent_id: &AgentId, resource: OplogResourcePattern) -> PermissionTarget {
        PermissionTarget::Oplog(ClassPermissionTarget {
            owner: match agent_target(agent_id, AgentVerb::View, AgentResourcePattern::Empty) {
                PermissionTarget::Agent(target) => target.owner,
                _ => unreachable!(),
            },
            verb: Some(OplogVerb::Read),
            resource,
        })
    }

    fn filesystem_target(agent_id: &AgentId, verb: FilesystemVerb, path: &str) -> PermissionTarget {
        PermissionTarget::Filesystem(ClassPermissionTarget {
            owner: match agent_target(agent_id, AgentVerb::View, AgentResourcePattern::Empty) {
                PermissionTarget::Agent(target) => target.owner,
                _ => unreachable!(),
            },
            verb: Some(verb),
            resource: FilesystemResourcePattern::Path(FilesystemPathPattern::parse(path).unwrap()),
        })
    }

    #[test]
    async fn removed_type_lookup_and_listing_use_retained_declarations() {
        let mut harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        *harness.worker_client.metadata_canonical_key.lock().unwrap() =
            Some(harness.agent_key().to_string());
        let component = harness
            .worker_service
            .component_service
            .get_revision(harness.component_id, harness.component_revision)
            .await
            .unwrap();
        harness.worker_service.agent_types = agent_type_resolver(Arc::new(TestRegistryService {
            removed: true,
            resolved: ResolvedAgentType {
                registered_agent_type: RegisteredAgentType {
                    type_id: harness.type_id,
                    agent_type: component.metadata.agent_types()[0].clone(),
                    implemented_by: RegisteredAgentTypeImplementer {
                        component_id: component.id,
                        component_revision: component.revision,
                        component_name: component.component_name.0.clone(),
                        account_id: component.account_id,
                        account_email: component.account_email.clone(),
                    },
                },
                environment_id: harness.environment_id,
                deployment_revision: DeploymentRevision::INITIAL,
                current_deployment_revision: None,
            },
        }));
        let reference = harness
            .worker_service
            .lookup_agent_rest(
                harness.environment_id,
                &harness.agent_type_name,
                AgentKeyRequest {
                    parameters: empty_json_tuple(),
                },
                AuthCtx::system(),
            )
            .await
            .unwrap();
        assert_eq!(reference.agent_id, harness.keyed_agent_id());
        let existing = harness
            .worker_client
            .get_metadata(
                &reference.agent_id,
                harness.environment_id,
                AuthCtx::system(),
            )
            .await
            .unwrap();
        *harness.worker_client.listed_agents.lock().unwrap() = (None, vec![existing]);
        let (_, agents) = harness
            .worker_service
            .find_metadata(
                harness.environment_id,
                &harness.agent_type_name,
                None,
                None,
                ScanCursor::default(),
                10,
                false,
                AuthCtx::system(),
            )
            .await
            .unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].agent_id, reference.agent_id);
        let mut request = harness.invoke_request();
        request.agent_id = Some(reference.agent_id);
        request.parameters = None;
        let invocation = harness
            .invoke_rest(request.clone(), None, AuthCtx::system())
            .await
            .unwrap();
        assert_eq!(invocation.agent_id, reference.agent_id);
        let wrong_name = harness
            .worker_service
            .invoke_agent_rest(
                harness.environment_id,
                &AgentTypeName("other-agent".into()),
                request,
                None,
                AuthCtx::system(),
            )
            .await;
        assert!(
            matches!(wrong_name, Err(WorkerServiceError::AgentNotFound(id)) if id == reference.agent_id)
        );
        assert!(
            harness
                .worker_service
                .upsert_agent_rest(
                    harness.environment_id,
                    &harness.agent_type_name,
                    AgentKeyRequest {
                        parameters: empty_json_tuple()
                    },
                    AuthCtx::system(),
                )
                .await
                .is_err(),
            "creates still require a deployed type"
        );
    }

    #[test]
    async fn component_selection_uses_each_agents_recorded_owner_and_preserves_the_cursor() {
        let mut harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let current_id = AgentId::from_parts(harness.type_id.get(), 1);
        let current = harness
            .worker_client
            .get_metadata(&current_id, harness.environment_id, AuthCtx::system())
            .await
            .unwrap();
        let current_component = harness
            .worker_service
            .component_service
            .get_revision(harness.component_id, harness.component_revision)
            .await
            .unwrap();
        let mut previous_component = current_component.clone();
        previous_component.id = ComponentId::new();
        previous_component.component_name = ComponentName("previous-component".into());
        let mut previous = current.clone();
        previous.component_id = previous_component.id;
        previous.metadata.agent_id = AgentId::from_parts(harness.type_id.get(), 2);
        let previous_id = previous.metadata.agent_id;
        harness.worker_service.component_service = Arc::new(StaticComponentService {
            components: vec![current_component, previous_component],
            tool_state: None,
            tool_queries: Mutex::new(vec![]),
            revision_gate: Mutex::new(None),
        });
        let cursor = Some(ScanCursor::new("next-page".into()));
        *harness.worker_client.listed_agents.lock().unwrap() =
            (cursor.clone(), vec![previous, current]);

        for (filter, expected) in [
            (None, vec![previous_id, current_id]),
            (Some("weather-component"), vec![current_id]),
            (Some("previous-component"), vec![previous_id]),
            (Some("another-component"), vec![]),
        ] {
            let (next, agents) = harness
                .worker_service
                .find_metadata(
                    harness.environment_id,
                    &harness.agent_type_name,
                    filter,
                    None,
                    ScanCursor::default(),
                    10,
                    false,
                    AuthCtx::system(),
                )
                .await
                .unwrap();
            assert_eq!(next, cursor);
            assert_eq!(
                agents
                    .into_iter()
                    .map(|agent| agent.agent_id)
                    .collect::<Vec<_>>(),
                expected
            );
        }
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
    fn key_limit_is_not_a_constructor_limit() {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let name = AgentTypeName("Doc".into());
            let mut ty = test_agent_type(name.clone(), mode);
            ty.constructor.input_schema = InputSchema::Parameters(vec![NamedField::user_supplied(
                "body",
                SchemaType::string(),
            )]);
            let env = EnvironmentId::new();
            let component = test_component(
                ComponentId::new(),
                env,
                AccountId::new(),
                ComponentRevision::INITIAL,
                ty.clone(),
            );
            for body in ["a".repeat(10), "a".repeat(520), "é".repeat(260)] {
                let constructor = ParsedAgentKey::new(
                    name.clone(),
                    golem_common::model::agent::typed_constructor_parameters(
                        &ty,
                        SchemaValue::Record {
                            fields: vec![SchemaValue::String(body.clone())],
                        },
                    ),
                );
                if mode == AgentMode::Durable {
                    let keyed = RoutedTarget::keyed(env, test_type_id(), &component, &constructor);
                    assert_eq!(keyed.is_ok(), body.len() == 10);
                    if let Err(error) = keyed {
                        assert!(error.to_string().contains("Agent key is too long"));
                    }
                }
                let routed = RoutedTarget::unkeyed(
                    AgentId::random(test_type_id()),
                    &component,
                    &constructor,
                )
                .expect("a constructor is not a key");
                let (_, wire_constructor, _) = routed.invoke_fields();
                let decoded = ParsedAgentKey::try_from(wire_constructor.unwrap()).unwrap();
                assert_eq!(decoded, constructor);
            }
        }
    }

    #[test]
    async fn explicit_create_requires_any_invoke_grant_and_preserves_the_creating_key() {
        let harness = RestHarness::new(AgentMode::Durable);
        let agent_id = harness.keyed_agent_id();
        let key = IdempotencyKey::fresh();
        for (resource, allowed) in [
            (
                AgentResourcePattern::Method(AgentMethodName("run".to_string())),
                false,
            ),
            (AgentResourcePattern::Any, true),
        ] {
            let result = harness
                .worker_service
                .launch_agent(
                    harness.component_id,
                    harness.agent_key().into(),
                    None,
                    None,
                    false,
                    HashMap::new(),
                    Vec::new(),
                    true,
                    auth_ctx_with_permissions(vec![agent_target(
                        &agent_id,
                        AgentVerb::Invoke,
                        resource,
                    )]),
                    None,
                    Caller::anonymous().into(),
                    Some(key.clone()),
                    InvocationFreshnessDisposition::MayExist,
                    None,
                )
                .await;
            if allowed {
                let (created, _, fingerprint) = result.unwrap();
                assert_eq!(created, agent_id);
                assert_eq!(fingerprint, harness.worker_client.fingerprint);
            } else {
                assert!(result.is_err());
            }
        }
        assert_eq!(
            *harness
                .worker_client
                .creating_idempotency_keys
                .lock()
                .unwrap(),
            vec![Some(key)],
        );
    }

    #[test]
    async fn preparation_rejects_a_different_creation_method_before_dispatch() {
        let harness = RestHarness::new(AgentMode::Durable);
        let result = harness
            .worker_service
            .launch_agent(
                harness.component_id,
                harness.constructor(),
                None,
                Some("WRONG".into()),
                true,
                HashMap::new(),
                Vec::new(),
                true,
                AuthCtx::system(),
                None,
                Caller::anonymous().into(),
                None,
                InvocationFreshnessDisposition::MayExist,
                Some(InvocationStart {
                    method_name: Some("run".into()),
                    input: Some(SchemaValue::Record { fields: vec![] }.try_into().unwrap()),
                    ..Default::default()
                }),
            )
            .await;
        assert!(
            matches!(result, Err(WorkerServiceError::TypeChecker(details)) if details.contains("prepared method"))
        );
        assert!(
            harness
                .worker_client
                .prepared_agent_ids
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(
            harness
                .worker_client
                .created_agent_ids
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    async fn invocation_target_activation_accepts_the_concrete_method_grant_only() {
        let harness = RestHarness::new(AgentMode::Durable);
        let agent_id = harness.keyed_agent_id();
        let run_permission = agent_target(
            &agent_id,
            AgentVerb::Invoke,
            AgentResourcePattern::Method(AgentMethodName("run".to_string())),
        );
        let other_method_permission = agent_target(
            &agent_id,
            AgentVerb::Invoke,
            AgentResourcePattern::Method(AgentMethodName("other".to_string())),
        );
        let key = IdempotencyKey::fresh();
        let launch = |prepare: bool, permission: PermissionTarget, key: Option<IdempotencyKey>| {
            harness.worker_service.launch_agent(
                harness.component_id,
                harness.agent_key().into(),
                None,
                Some("run".to_string()),
                prepare,
                HashMap::new(),
                Vec::new(),
                true,
                auth_ctx_with_permissions(vec![permission]),
                None,
                Caller::anonymous().into(),
                key,
                InvocationFreshnessDisposition::MayExist,
                None,
            )
        };

        launch(false, run_permission.clone(), Some(key.clone()))
            .await
            .unwrap();
        launch(true, run_permission, None).await.unwrap();
        assert!(
            launch(false, other_method_permission.clone(), None)
                .await
                .is_err()
        );
        assert!(launch(true, other_method_permission, None).await.is_err());

        assert_eq!(
            harness
                .worker_client
                .created_agent_ids
                .lock()
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&agent_id),
            "denied target activation reached the worker client"
        );
        assert_eq!(
            *harness
                .worker_client
                .creating_idempotency_keys
                .lock()
                .unwrap(),
            vec![Some(key)],
        );
        assert_eq!(
            harness
                .worker_client
                .prepared_agent_ids
                .lock()
                .unwrap()
                .as_slice(),
            std::slice::from_ref(&agent_id),
            "denied prepare reached the worker client"
        );
    }

    #[test]
    async fn cancel_invocation_rejects_raw_request_key() {
        let harness = RestHarness::new(AgentMode::Durable);
        let agent_id = harness.keyed_agent_id();
        let result = harness
            .worker_service
            .cancel_invocation(
                &harness.owned(&agent_id),
                &IdempotencyKey::from("raw-request-key"),
                None,
                AuthCtx::System,
            )
            .await;
        assert!(result.unwrap_err().to_string().contains("expected a UUID"));
        assert!(harness.worker_client.effects.lock().unwrap().is_empty());
    }

    #[test]
    async fn service_operations_use_exact_agent_oplog_and_filesystem_resources() {
        let harness = RestHarness::new(AgentMode::Durable);
        // `delete` settles existence with a read before dispatching anything,
        // so the agent has to be there for the delete leg to run at all.
        harness
            .worker_client
            .set_metadata_component_revision(ComponentRevision::INITIAL);
        let agent_id = harness.keyed_agent_id();
        let path = CanonicalFilePath::from_abs_str("/data/report.txt").unwrap();
        let invocation_id = "00000000-0000-0000-0000-000000000001";
        let permissions = vec![
            agent_target(&agent_id, AgentVerb::Delete, AgentResourcePattern::Empty),
            agent_target(&agent_id, AgentVerb::Interrupt, AgentResourcePattern::Empty),
            agent_target(&agent_id, AgentVerb::Resume, AgentResourcePattern::Empty),
            agent_target(
                &agent_id,
                AgentVerb::UpdateRevision,
                AgentResourcePattern::Empty,
            ),
            agent_target(&agent_id, AgentVerb::Fork, AgentResourcePattern::Empty),
            oplog_target(&agent_id, OplogResourcePattern::range(Some(5), Some(7))),
            filesystem_target(&agent_id, FilesystemVerb::Stat, path.as_abs_str()),
            filesystem_target(&agent_id, FilesystemVerb::List, path.as_abs_str()),
            filesystem_target(&agent_id, FilesystemVerb::Read, path.as_abs_str()),
            agent_target(
                &agent_id,
                AgentVerb::CancelInvocation,
                AgentResourcePattern::InvocationId(AgentInvocationIdPattern::Uuid(
                    uuid::Uuid::parse_str(invocation_id).unwrap(),
                )),
            ),
        ];
        let auth_ctx = auth_ctx_with_permissions(permissions);
        let owned = harness.owned(&agent_id);

        harness
            .worker_service
            .delete(&owned, None, auth_ctx.clone())
            .await
            .unwrap();
        harness
            .worker_service
            .interrupt(&owned, false, None, auth_ctx.clone())
            .await
            .unwrap();
        harness
            .worker_service
            .resume(&owned, false, None, auth_ctx.clone())
            .await
            .unwrap();
        harness
            .worker_service
            .update(
                &owned,
                AgentUpdateMode::Automatic,
                ComponentRevision::new(1).unwrap(),
                false,
                None,
                None,
                auth_ctx.clone(),
            )
            .await
            .unwrap();
        let fork_target = AgentId::random(harness.type_id);
        assert_eq!(
            harness
                .worker_service
                .fork_worker(
                    &owned,
                    Some(fork_target),
                    OplogIndex::from_u64(4),
                    None,
                    None,
                    auth_ctx.clone(),
                )
                .await
                .unwrap(),
            fork_target
        );
        harness
            .worker_service
            .get_oplog(
                &owned,
                OplogIndex::from_u64(5),
                None,
                3,
                None,
                None,
                ClaimVisibility::Hidden,
                auth_ctx.clone(),
            )
            .await
            .unwrap();
        harness
            .worker_service
            .get_file_system_node(&owned, path.clone(), None, auth_ctx.clone())
            .await
            .unwrap();
        let _contents = harness
            .worker_service
            .get_file_contents(&owned, path, None, auth_ctx.clone())
            .await
            .unwrap();
        assert!(
            harness
                .worker_service
                .cancel_invocation(
                    &owned,
                    &IdempotencyKey::new(invocation_id.to_string()),
                    None,
                    auth_ctx,
                )
                .await
                .unwrap()
        );

        assert_eq!(
            *harness.worker_client.effects.lock().unwrap(),
            vec![
                "metadata",
                "delete",
                "metadata",
                "interrupt",
                "metadata",
                "resume",
                "metadata",
                "update",
                "metadata",
                "fork",
                "metadata",
                "get-oplog",
                "metadata",
                "filesystem-node",
                "metadata",
                "file-contents",
                "metadata",
                "cancel-invocation",
            ]
        );
    }

    #[test]
    async fn oplog_entry_reads_require_permission_for_the_exact_oplog_index() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let agent_id = harness.keyed_agent_id();
        let owned = harness.owned(&agent_id);
        let auth_ctx = auth_ctx_with_permissions(vec![oplog_target(
            &agent_id,
            OplogResourcePattern::range(Some(6), Some(6)),
        )]);

        let entry = harness
            .worker_service
            .get_oplog_entry(
                &owned,
                OplogIndex::from_u64(6),
                Some(ComponentRevision::new(3).unwrap()),
                None,
                ClaimVisibility::Hidden,
                auth_ctx.clone(),
            )
            .await
            .unwrap();
        assert_eq!(entry.oplog_index, OplogIndex::from_u64(6));
        harness
            .worker_service
            .get_oplog_entry(
                &owned,
                OplogIndex::from_u64(6),
                None,
                None,
                ClaimVisibility::Shown,
                auth_ctx.clone(),
            )
            .await
            .unwrap();

        for claims in [ClaimVisibility::Hidden, ClaimVisibility::Shown] {
            assert!(
                harness
                    .worker_service
                    .get_oplog_entry(
                        &owned,
                        OplogIndex::from_u64(7),
                        None,
                        None,
                        claims,
                        auth_ctx.clone(),
                    )
                    .await
                    .is_err(),
                "the permission covers only oplog index 6"
            );
        }
        assert_eq!(
            *harness.worker_client.oplog_entry_reads.lock().unwrap(),
            vec![
                (
                    OplogIndex::from_u64(6),
                    Some(ComponentRevision::new(3).unwrap()),
                    ClaimVisibility::Hidden
                ),
                (OplogIndex::from_u64(6), None, ClaimVisibility::Shown),
            ],
            "a denied oplog entry read reached the worker client, or the claims request was lost"
        );
    }

    #[test]
    async fn oplog_page_reads_pass_the_claims_request_after_the_permission_check() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let agent_id = harness.keyed_agent_id();
        let owned = harness.owned(&agent_id);
        let allowed =
            auth_ctx_with_permissions(vec![oplog_target(&agent_id, OplogResourcePattern::Any)]);
        let denied = auth_ctx_with_permissions(Vec::new());

        for claims in [ClaimVisibility::Hidden, ClaimVisibility::Shown] {
            harness
                .worker_service
                .get_oplog(
                    &owned,
                    OplogIndex::INITIAL,
                    None,
                    10,
                    None,
                    None,
                    claims,
                    allowed.clone(),
                )
                .await
                .unwrap();
            harness
                .worker_service
                .search_oplog(
                    &owned,
                    None,
                    10,
                    "bob".to_string(),
                    None,
                    claims,
                    allowed.clone(),
                )
                .await
                .unwrap();
        }
        for claims in [ClaimVisibility::Hidden, ClaimVisibility::Shown] {
            assert!(
                harness
                    .worker_service
                    .get_oplog(
                        &owned,
                        OplogIndex::INITIAL,
                        None,
                        10,
                        None,
                        None,
                        claims,
                        denied.clone(),
                    )
                    .await
                    .is_err()
            );
            assert!(
                harness
                    .worker_service
                    .search_oplog(
                        &owned,
                        None,
                        10,
                        "bob".to_string(),
                        None,
                        claims,
                        denied.clone(),
                    )
                    .await
                    .is_err()
            );
        }

        // The worker service also reads the agent's metadata; only the oplog reads count here.
        let oplog_reads = harness
            .worker_client
            .effects
            .lock()
            .unwrap()
            .iter()
            .copied()
            .filter(|effect| effect.contains("oplog"))
            .collect::<Vec<_>>();
        assert_eq!(
            oplog_reads,
            vec![
                "get-oplog",
                "search-oplog",
                "get-oplog-with-claims",
                "search-oplog-with-claims",
            ],
            "a denied oplog read reached the worker client, or the claims request was lost"
        );
    }

    #[test]
    async fn plugin_lifecycle_resolves_and_authorizes_the_concrete_plugin_name() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let agent_id = harness.keyed_agent_id();
        let owned = harness.owned(&agent_id);
        let resource =
            AgentResourcePattern::PluginName(AgentPluginName("audit-plugin".to_string()));
        let permissions = vec![
            agent_target(&agent_id, AgentVerb::ActivatePlugin, resource.clone()),
            agent_target(&agent_id, AgentVerb::DeactivatePlugin, resource),
        ];
        let auth_ctx = auth_ctx_with_permissions(permissions);

        harness
            .worker_service
            .activate_plugin(&owned, PluginPriority(7), None, auth_ctx.clone())
            .await
            .unwrap();
        harness
            .worker_service
            .deactivate_plugin(&owned, PluginPriority(7), None, auth_ctx)
            .await
            .unwrap();
        assert_eq!(
            *harness.worker_client.effects.lock().unwrap(),
            vec![
                "metadata",
                "activate-plugin",
                "metadata",
                "deactivate-plugin"
            ]
        );

        let wrong_permission = agent_target(
            &agent_id,
            AgentVerb::ActivatePlugin,
            AgentResourcePattern::PluginName(AgentPluginName("other-plugin".to_string())),
        );
        assert!(
            harness
                .worker_service
                .activate_plugin(
                    &owned,
                    PluginPriority(7),
                    None,
                    auth_ctx_with_permissions(vec![wrong_permission]),
                )
                .await
                .is_err()
        );
        assert_eq!(
            *harness.worker_client.effects.lock().unwrap(),
            vec![
                "metadata",
                "activate-plugin",
                "metadata",
                "deactivate-plugin",
                "metadata"
            ],
            "denied plugin activation reached the activation backend"
        );
    }

    #[test]
    async fn revert_uses_the_concrete_cutoff_for_index_and_count_targets() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let agent_id = harness.keyed_agent_id();
        let owned = harness.owned(&agent_id);
        let cutoff = OplogIndex::from_u64(42);
        let exact_permission = agent_target(
            &agent_id,
            AgentVerb::Revert,
            AgentResourcePattern::OplogIndex(cutoff.into()),
        );

        harness
            .worker_service
            .revert_worker(
                &owned,
                RevertWorkerTarget::RevertToOplogIndex(RevertToOplogIndex {
                    last_oplog_index: cutoff,
                }),
                None,
                auth_ctx_with_permissions(vec![exact_permission.clone()]),
            )
            .await
            .unwrap();
        harness
            .worker_service
            .revert_worker(
                &owned,
                RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                    number_of_invocations: 1,
                }),
                None,
                auth_ctx_with_permissions(vec![exact_permission]),
            )
            .await
            .unwrap();

        assert!(
            harness
                .worker_service
                .revert_worker(
                    &owned,
                    RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                        number_of_invocations: 1,
                    }),
                    None,
                    auth_ctx_with_permissions(vec![agent_target(
                        &agent_id,
                        AgentVerb::Revert,
                        AgentResourcePattern::OplogIndex(OplogIndex::from_u64(41).into()),
                    )]),
                )
                .await
                .is_err(),
            "count-based revert authorization used a target other than the resolved cutoff"
        );

        assert_eq!(
            *harness.worker_client.effects.lock().unwrap(),
            vec!["metadata", "revert", "metadata", "revert", "metadata"]
        );
    }

    #[test]
    async fn a_failed_count_revert_resolution_is_denied_to_an_unauthorized_caller() {
        let harness = RestHarness::new(AgentMode::Durable);
        let agent_id = harness.keyed_agent_id();
        let owned = harness.owned(&agent_id);
        let unauthorized = auth_ctx_with_permissions(vec![]);
        let target = || {
            RevertWorkerTarget::RevertLastInvocations(RevertLastInvocations {
                number_of_invocations: 1000,
            })
        };

        let missing = harness
            .worker_service
            .revert_worker(&owned, target(), None, unauthorized.clone())
            .await;
        assert!(
            matches!(missing, Err(WorkerServiceError::AgentAccessDenied(_))),
            "missing agent: {missing:?}"
        );

        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        *harness
            .worker_client
            .revert_resolution_error
            .lock()
            .unwrap() = Some("Could not find 1000 invocations to revert".to_string());
        let existing = harness
            .worker_service
            .revert_worker(&owned, target(), None, unauthorized)
            .await;
        assert!(
            matches!(existing, Err(WorkerServiceError::AgentAccessDenied(_))),
            "an unauthorized caller learns that the agent exists: {existing:?}"
        );

        let authorized = harness
            .worker_service
            .revert_worker(&owned, target(), None, AuthCtx::system())
            .await;
        assert!(
            matches!(authorized, Err(WorkerServiceError::GolemError(_))),
            "an authorized caller sees the resolution failure: {authorized:?}"
        );
    }

    #[test]
    async fn card_transfer_delivery_is_system_only_and_preserves_retry_identity() {
        let harness = RestHarness::new(AgentMode::Durable);
        let target_agent_id = AgentId::random(harness.type_id);
        let transfer_id = Uuid::new_v4();
        let source_card_id = CardId::new();
        let card = test_card();

        for _ in 0..2 {
            harness
                .worker_service
                .deliver_card_transfer(
                    &target_agent_id,
                    harness.environment_id,
                    transfer_id,
                    source_card_id,
                    card.clone(),
                    AuthCtx::System,
                )
                .await
                .unwrap();
        }

        assert_eq!(
            harness.worker_client.delivered_card_transfers(),
            vec![
                (
                    target_agent_id,
                    harness.environment_id,
                    transfer_id,
                    source_card_id,
                    card.clone(),
                    AuthCtx::System,
                ),
                (
                    target_agent_id,
                    harness.environment_id,
                    transfer_id,
                    source_card_id,
                    card.clone(),
                    AuthCtx::System,
                ),
            ]
        );

        let non_system_auth = AuthCtx::agent_with_effective_surface(
            AccountId::new(),
            AccountEmail::new("caller@golem"),
            EffectiveSurface::default(),
        );
        assert!(
            harness
                .worker_service
                .deliver_card_transfer(
                    &target_agent_id,
                    harness.environment_id,
                    transfer_id,
                    source_card_id,
                    card,
                    non_system_auth,
                )
                .await
                .is_err()
        );
        assert_eq!(
            harness.worker_client.delivered_card_transfers().len(),
            2,
            "a non-system request must not reach the executor client"
        );
    }

    #[test]
    async fn oplog_processor_dispatch_is_system_only() {
        let harness = RestHarness::new(AgentMode::Durable);
        let target_agent_id = AgentId::random_reserved();

        harness
            .worker_service
            .process_oplog_entries(
                &target_agent_id,
                harness.environment_id,
                harness.component_id,
                harness.component_revision,
                golem_common::model::InvocationId::new(),
                AccountId::new(),
                HashMap::new(),
                golem_api_grpc::proto::golem::worker::AgentMetadata::default(),
                OplogIndex::INITIAL,
                Vec::new(),
                AuthCtx::System,
                Caller::anonymous().into(),
            )
            .await
            .unwrap();

        let non_system_auth = AuthCtx::agent_with_effective_surface(
            AccountId::new(),
            AccountEmail::new("caller@golem"),
            EffectiveSurface::default(),
        );
        assert!(
            harness
                .worker_service
                .process_oplog_entries(
                    &target_agent_id,
                    harness.environment_id,
                    harness.component_id,
                    harness.component_revision,
                    golem_common::model::InvocationId::new(),
                    AccountId::new(),
                    HashMap::new(),
                    golem_api_grpc::proto::golem::worker::AgentMetadata::default(),
                    OplogIndex::INITIAL,
                    Vec::new(),
                    non_system_auth,
                    Caller::anonymous().into(),
                )
                .await
                .is_err()
        );
        assert_eq!(
            *harness.worker_client.effects.lock().unwrap(),
            vec!["process-oplog-entries"],
            "a non-system request must not reach the executor client"
        );
    }

    #[test]
    async fn mounted_files_lookup_existing_key_and_id_without_creation() {
        for id_binding in [false, true] {
            for state in [
                "existing",
                "retained-owner",
                "missing",
                "deleted-during-read",
                "wrong-environment",
                "wrong-type",
                "key-collision",
            ] {
                let mut harness = RestHarness::new(AgentMode::Durable);
                let mut route_component_id = harness.component_id;
                if state == "retained-owner" {
                    let original = harness
                        .worker_service
                        .component_service
                        .get_current_by_id(harness.component_id)
                        .await
                        .unwrap();
                    let mut current = original.clone();
                    current.id = ComponentId::new();
                    route_component_id = current.id;
                    harness.worker_service.agent_types =
                        agent_type_resolver(Arc::new(TestRegistryService {
                            removed: false,
                            resolved: ResolvedAgentType {
                                registered_agent_type: RegisteredAgentType {
                                    type_id: harness.type_id,
                                    agent_type: current.metadata.agent_types()[0].clone(),
                                    implemented_by: RegisteredAgentTypeImplementer {
                                        component_id: current.id,
                                        component_revision: current.revision,
                                        component_name: current.component_name.0.clone(),
                                        account_id: current.account_id,
                                        account_email: current.account_email.clone(),
                                    },
                                },
                                environment_id: harness.environment_id,
                                deployment_revision: DeploymentRevision::INITIAL,
                                current_deployment_revision: Some(
                                    CurrentDeploymentRevision::INITIAL,
                                ),
                            },
                        }));
                    harness.worker_service.component_service = Arc::new(StaticComponentService {
                        components: vec![original, current],
                        tool_state: None,
                        tool_queries: Mutex::new(vec![]),
                        revision_gate: Mutex::new(None),
                    });
                }
                if state != "missing" {
                    harness
                        .worker_client
                        .set_metadata_component_revision(harness.component_revision);
                }
                if state == "wrong-environment" {
                    *harness
                        .worker_client
                        .metadata_environment_id
                        .lock()
                        .unwrap() = Some(EnvironmentId::new());
                }
                if state == "wrong-type" {
                    *harness
                        .worker_client
                        .metadata_agent_type_name
                        .lock()
                        .unwrap() = Some(AgentTypeName("other".into()));
                }
                *harness.worker_client.file_read_missing.lock().unwrap() =
                    state == "deleted-during-read";
                *harness.worker_client.metadata_canonical_key.lock().unwrap() =
                    Some(if matches!(state, "key-collision" | "wrong-type") {
                        "another-key".into()
                    } else {
                        harness.agent_key().to_string()
                    });
                let constructor = harness.constructor();
                let result = harness
                    .worker_service
                    .read_mounted_file(
                        route_component_id,
                        &harness.agent_type_name,
                        (!id_binding).then_some(&constructor),
                        if id_binding {
                            AgentBinding::Id(harness.keyed_agent_id())
                        } else {
                            AgentBinding::Constructor
                        },
                        "/public/file.txt",
                        FileByteSelection::Full,
                        harness.environment_id,
                        AccountId::new(),
                    )
                    .await;
                let readable = matches!(state, "existing" | "retained-owner")
                    || (id_binding && state == "key-collision");
                assert_eq!(
                    result.is_ok(),
                    readable,
                    "{state}, ID binding: {id_binding}"
                );
                if state == "key-collision" && !id_binding {
                    assert!(matches!(
                        &result,
                        Err(WorkerServiceError::GolemError(WorkerExecutorError::AgentKeyCollision {
                            agent_id,
                            requested_key,
                        })) if *agent_id == harness.keyed_agent_id()
                            && requested_key == &harness.agent_key().to_string()
                    ));
                } else if !readable {
                    assert!(
                        matches!(result, Err(WorkerServiceError::AgentNotFound(_))),
                        "{state}"
                    );
                }
                assert_eq!(
                    harness
                        .worker_client
                        .effects
                        .lock()
                        .unwrap()
                        .contains(&"file-contents"),
                    readable || state == "deleted-during-read"
                );
                assert!(harness.worker_client.created_targets().is_empty());
                assert!(harness.worker_client.invoked_targets().is_empty());
            }
        }
    }

    #[test]
    #[test_r::timeout("30s")]
    async fn mounted_create_keeps_its_resolved_component_across_a_deployment_change() {
        use crate::service::component::tests::{LookupGate, TestRegistry};

        let mut harness = RestHarness::new(AgentMode::Durable);
        let original = harness
            .worker_service
            .component_service
            .get_current_by_id(harness.component_id)
            .await
            .unwrap();
        let mut replacement = original.clone();
        replacement.id = ComponentId::new();
        let registered = |component: &Component| RegisteredAgentType {
            type_id: harness.type_id,
            agent_type: component.metadata.agent_types()[0].clone(),
            implemented_by: RegisteredAgentTypeImplementer {
                component_id: component.id,
                component_revision: component.revision,
                component_name: component.component_name.0.clone(),
                account_id: component.account_id,
                account_email: component.account_email.clone(),
            },
        };
        let registry = Arc::new(TestRegistry::new(
            vec![original.clone(), replacement.clone()],
            Some(original.clone()),
        ));
        *registry.deployed_agent_type.lock().unwrap() = Some(registered(&original));
        let resolver = Arc::new(AgentTypeResolver::new(
            registry.clone(),
            1,
            Duration::from_secs(60),
            Duration::from_secs(60),
        ));
        harness.worker_service.agent_types = resolver.clone();
        let gate = LookupGate::new();
        harness.worker_service.component_service = Arc::new(StaticComponentService {
            components: vec![original.clone(), replacement.clone()],
            tool_state: None,
            tool_queries: Mutex::new(Vec::new()),
            revision_gate: Mutex::new(Some(gate.clone())),
        });
        let creation = harness.worker_service.create_mounted_agent(
            harness.component_id,
            harness.environment_id,
            &harness.agent_type_name,
            harness.create_request(false),
            None,
            Caller::anonymous(),
        );
        let deploy = async {
            gate.started.acquire().await.unwrap().forget();
            *registry.deployed_agent_type.lock().unwrap() = Some(registered(&replacement));
            resolver
                .invalidate_environment(harness.environment_id)
                .await;
            gate.release.add_permits(1);
        };
        let (result, ()) = tokio::join!(creation, deploy);
        let reference = result.unwrap();
        let targets = harness.worker_client.created_targets();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].component_id(), Some(original.id));
        assert_eq!(targets[0].agent_id, reference.agent_id);
        assert_ne!(targets[0].component_id(), Some(replacement.id));
        assert_eq!(
            *harness.worker_client.created_callers.lock().unwrap(),
            vec![Caller::anonymous()]
        );
    }

    #[test]
    fn card_transfer_wire_payload_preserves_authority_resource_kind() {
        let agent_id = AgentId::from_parts(7, 1);
        let permission: PermissionPattern = format!(
            "agent(acme/shop/prod/cart/{agent_id}) @ acme/shop/prod/cart/{agent_id} : activate-plugin : plugin-a"
        )
        .parse()
        .unwrap();

        let card = StoredCard::Concrete(Card {
            card_id: CardId::new(),
            parent_ids: vec![],
            lower_positive: vec![permission],
            lower_negative: vec![],
            upper_positive: vec![],
            upper_negative: vec![],
            created_at: Utc::now(),
            expires_at: None,
            system_card: false,
            managed_by: None,
        });

        let wire_payload = desert_rust::serialize_to_byte_vec(&card).unwrap();
        let decoded: StoredCard = desert_rust::deserialize(&wire_payload).unwrap();

        assert_eq!(decoded, card, "the transfer payload must preserve the card");
    }

    #[test]
    async fn rest_fork_defaults_to_the_account_caller() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let auth = AuthCtx::system();
        let source = harness.owned(&harness.keyed_agent_id());
        let target = AgentId::random(harness.type_id);

        harness
            .worker_service
            .fork_worker(
                &source,
                Some(target),
                OplogIndex::from_u64(4),
                None,
                None,
                auth.clone(),
            )
            .await
            .unwrap();
        let explicit = Caller::anonymous();
        harness
            .worker_service
            .fork_worker(
                &source,
                Some(target),
                OplogIndex::from_u64(4),
                Some(explicit.clone()),
                None,
                auth.clone(),
            )
            .await
            .unwrap();

        let callers = harness.worker_client.forked_callers.lock().unwrap().clone();
        assert_eq!(
            callers,
            vec![harness.worker_service.rest_caller(&auth), explicit]
        );
        assert!(matches!(callers[0].original, Principal::Account(_)));
    }

    #[test]
    async fn rest_update_sends_the_account_caller() {
        let harness = RestHarness::new(AgentMode::Durable);
        let auth = AuthCtx::system();
        // The executor knows the agent, so the update reaches it.
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let agent_id = harness.owned(&harness.keyed_agent_id());

        for mode in [AgentUpdateMode::Automatic, AgentUpdateMode::Manual] {
            harness
                .worker_service
                .update(
                    &agent_id,
                    mode,
                    ComponentRevision::new(1).unwrap(),
                    false,
                    None,
                    None,
                    auth.clone(),
                )
                .await
                .unwrap();
        }

        let requesters = harness
            .worker_client
            .updated_callers
            .lock()
            .unwrap()
            .clone();
        let account = harness.worker_service.rest_caller(&auth);
        assert_eq!(requesters, vec![account.clone(), account]);
        assert!(matches!(requesters[0].original, Principal::Account(_)));
    }

    #[test]
    async fn update_keeps_the_given_requester() {
        let harness = RestHarness::new(AgentMode::Durable);
        // The executor knows the agent, so the update reaches it.
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let agent_id = harness.owned(&harness.keyed_agent_id());
        let requester = harness.worker_service.rest_caller(&AuthCtx::system());
        let requester = Caller {
            original: requester.original,
            immediate: Principal::Anonymous,
            tokens: Vec::new(),
        };

        harness
            .worker_service
            .update(
                &agent_id,
                AgentUpdateMode::Automatic,
                ComponentRevision::new(1).unwrap(),
                false,
                None,
                Some(requester.clone()),
                AuthCtx::system(),
            )
            .await
            .unwrap();

        assert_eq!(
            harness
                .worker_client
                .updated_callers
                .lock()
                .unwrap()
                .clone(),
            vec![requester]
        );
    }

    #[test]
    async fn rest_create_records_the_account_caller() {
        let harness = RestHarness::new(AgentMode::Durable);
        let auth = AuthCtx::system();
        harness
            .worker_service
            .create_agent_rest(
                harness.environment_id,
                &harness.agent_type_name,
                harness.create_request(true),
                None,
                auth.clone(),
            )
            .await
            .unwrap();

        let callers = harness
            .worker_client
            .created_callers
            .lock()
            .unwrap()
            .clone();
        assert_eq!(callers, vec![harness.worker_service.rest_caller(&auth)]);
        assert_eq!(
            callers[0].original.pair().unwrap().to_string(),
            format!(
                "(\"{}\",\"account:{}\")",
                test_golem_issuer(),
                auth.account_id().0.hyphenated()
            )
        );
    }

    #[test]
    async fn create_agent_rest_keyed_create_is_exclusive_at_the_keyed_id() {
        let harness = RestHarness::new(AgentMode::Durable);

        let response = harness
            .worker_service
            .create_agent_rest(
                harness.environment_id,
                &harness.agent_type_name,
                harness.create_request(true),
                None,
                AuthCtx::system(),
            )
            .await
            .unwrap();

        let expected = AgentId::keyed(harness.environment_id, test_type_id(), &harness.agent_key());
        assert_eq!(response.agent_id, expected);
        assert_eq!(
            response.agent_fingerprint,
            harness.worker_client.fingerprint
        );
        assert_eq!(harness.worker_client.created_agent_id(), expected);
        assert_eq!(
            *harness
                .worker_client
                .creating_ignore_already_existing
                .lock()
                .unwrap(),
            vec![false],
            "a keyed create must fail when the key is current"
        );
    }

    #[test]
    async fn create_agent_rest_rejects_an_ephemeral_type() {
        let harness = RestHarness::new(AgentMode::Ephemeral);

        for keyed in [true, false] {
            let error = harness
                .worker_service
                .create_agent_rest(
                    harness.environment_id,
                    &harness.agent_type_name,
                    harness.create_request(keyed),
                    None,
                    AuthCtx::system(),
                )
                .await
                .unwrap_err();

            assert!(
                matches!(error, WorkerServiceError::TypeChecker(_)),
                "keyed: {keyed}, error: {error:?}"
            );
        }
        assert!(harness.worker_client.created_targets().is_empty());
    }

    #[test]
    async fn create_agent_rest_without_a_key_derives_a_retry_stable_id_from_the_idempotency_key() {
        let harness = RestHarness::new(AgentMode::Durable);
        let auth = scoped_invocation_auth();
        let scope = auth.idempotency_scope().unwrap();
        let key = IdempotencyKey::new("create-key".to_string());

        let mut created = Vec::new();
        for _ in 0..2 {
            created.push(
                harness
                    .worker_service
                    .create_agent_rest(
                        harness.environment_id,
                        &harness.agent_type_name,
                        harness.create_request(false),
                        Some(key.clone()),
                        auth.clone(),
                    )
                    .await
                    .unwrap()
                    .agent_id,
            );
        }
        let expected = AgentId::idempotent_create(
            harness.environment_id,
            test_type_id(),
            &scope.to_string(),
            &key,
        );
        assert_eq!(created, vec![expected, expected]);
        assert_ne!(expected, harness.keyed_agent_id());

        let mut unkeyed = Vec::new();
        for _ in 0..2 {
            unkeyed.push(
                harness
                    .worker_service
                    .create_agent_rest(
                        harness.environment_id,
                        &harness.agent_type_name,
                        harness.create_request(false),
                        None,
                        auth.clone(),
                    )
                    .await
                    .unwrap()
                    .agent_id,
            );
        }
        assert_ne!(unkeyed[0], unkeyed[1]);
        assert!(
            unkeyed
                .iter()
                .all(|agent_id| agent_id.type_id() == Some(test_type_id()))
        );
        assert!(
            harness
                .worker_client
                .created_targets()
                .iter()
                .all(RoutedTarget::creates)
        );
    }

    #[test]
    async fn lookup_distinguishes_an_authorized_key_collision_from_absence() {
        let harness = RestHarness::new(AgentMode::Durable);
        let lookup = |auth| {
            harness.worker_service.lookup_agent_rest(
                harness.environment_id,
                &harness.agent_type_name,
                AgentKeyRequest {
                    parameters: empty_json_tuple(),
                },
                auth,
            )
        };
        assert!(matches!(
            lookup(AuthCtx::system()).await,
            Err(WorkerServiceError::AgentNotFound(_))
        ));
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        *harness.worker_client.metadata_canonical_key.lock().unwrap() =
            Some(harness.agent_key().to_string());
        let reference = lookup(AuthCtx::system()).await.unwrap();
        assert_eq!(reference.agent_id, harness.keyed_agent_id());
        assert_eq!(
            reference.agent_fingerprint,
            harness.worker_client.fingerprint
        );
        let denied = lookup(auth_ctx_with_permissions(vec![])).await.unwrap_err();

        *harness.worker_client.metadata_canonical_key.lock().unwrap() =
            Some("weather-agent(1)".into());
        assert!(matches!(lookup(AuthCtx::system()).await,
            Err(WorkerServiceError::GolemError(WorkerExecutorError::AgentKeyCollision { agent_id, .. }))
            if agent_id == harness.keyed_agent_id()));
        let denied_collision = lookup(auth_ctx_with_permissions(vec![])).await.unwrap_err();
        assert_eq!(
            denied_collision.to_string(),
            denied.to_string(),
            "denied callers must not learn about a collision"
        );
        assert!(
            harness.worker_client.created_targets().is_empty(),
            "lookup must not create"
        );
    }

    #[test]
    async fn create_agent_rest_derives_the_id_of_an_idempotent_create_from_the_caller_scope() {
        let harness = RestHarness::new(AgentMode::Durable);
        let key = IdempotencyKey::new("create-key".to_string());
        let create = |auth: AuthCtx| {
            let (harness, key) = (&harness, key.clone());
            async move {
                harness
                    .worker_service
                    .create_agent_rest(
                        harness.environment_id,
                        &harness.agent_type_name,
                        harness.create_request(false),
                        Some(key),
                        auth,
                    )
                    .await
                    .unwrap()
                    .agent_id
            }
        };

        let first_caller = scoped_invocation_auth();
        let second_caller = scoped_invocation_auth();
        let scoped = [
            create(first_caller.clone()).await,
            create(first_caller).await,
            create(second_caller).await,
        ];
        let system = [
            create(AuthCtx::system()).await,
            create(AuthCtx::system()).await,
        ];

        assert_eq!(scoped[0], scoped[1], "one caller with one key");
        assert_ne!(scoped[0], scoped[2], "two callers with one key");
        assert_ne!(system[0], system[1], "a system caller has no caller scope");
        assert!(
            !scoped.contains(&system[0]) && !scoped.contains(&system[1]),
            "a system caller shares no ID with a scoped caller"
        );
    }

    #[test]
    async fn invoke_agent_rest_runs_an_ephemeral_invocation_on_an_instance_of_its_own() {
        let harness = RestHarness::new(AgentMode::Ephemeral);

        let response = harness
            .invoke_rest(harness.invoke_request(), None, AuthCtx::system())
            .await
            .unwrap();

        assert_eq!(
            response.component_revision,
            Some(harness.component_revision)
        );
        assert_eq!(response.agent_id, harness.worker_client.invoked_agent_id());
        assert_eq!(
            harness.worker_client.invocation_environment(),
            harness.environment_id
        );
        let invocations = harness.worker_client.invocations();
        assert_eq!(invocations.len(), 1);
        assert_eq!(response.idempotency_key.to_string(), invocations[0].1.value);
        assert_eq!(invocations[0].2, InvocationFreshnessDisposition::KnownFresh);
        assert_eq!(
            invocations[0].0,
            AgentId::ephemeral_invocation(
                harness.environment_id,
                test_type_id(),
                &invocations[0].1
            )
        );
        assert_ne!(invocations[0].0, harness.keyed_agent_id());
        assert!(harness.worker_client.invoked_targets()[0].creates());
    }

    #[test]
    async fn public_invocation_session_resolves_normalizes_and_builds_trusted_start() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        let idempotency_key = IdempotencyKey::new("public-session-key".to_string());

        let started = harness
            .worker_service
            .invoke_public_agent_session_v1(
                harness.public_invocation_start(idempotency_key.clone()),
                Box::pin(stream::empty()),
                AuthCtx::system(),
                |_, _| panic!("stream resolver called for a stream-free input"),
            )
            .await
            .unwrap();

        let (routed_agent_id, start) = harness.worker_client.invocation_session_start();
        let trusted_agent_id: AgentId = start
            .target_agent_id()
            .cloned()
            .unwrap()
            .try_into()
            .unwrap();
        let trusted_idempotency_key: IdempotencyKey = start.idempotency_key.clone().unwrap().into();
        let request_key =
            InvocationRequestKey::try_from(start.request_key.clone().unwrap()).unwrap();
        assert!(matches!(request_key, InvocationRequestKey::Unkeyed(_)));

        assert_eq!(routed_agent_id, trusted_agent_id);
        assert_eq!(started.agent_id, trusted_agent_id);
        assert_eq!(started.component_id, harness.component_id);
        assert_eq!(trusted_idempotency_key, idempotency_key);
        assert_eq!(
            trusted_agent_id,
            AgentId::ephemeral_invocation(
                harness.environment_id,
                test_type_id(),
                &request_key.owner_key("run")
            )
        );
        assert!(start.constructor.is_some());
        assert_eq!(
            start
                .component_id
                .map(|id| ComponentId::try_from(id).unwrap()),
            Some(harness.component_id)
        );
        assert_eq!(start.method_name.as_deref(), Some("run"));
        assert!(start.input.is_none());
        assert!(start.captured_method_input.is_some());
        assert!(start.allow_streams);
        assert!(start.auth_ctx.is_some());
        let expected_caller = harness.worker_service.rest_caller(&AuthCtx::system());
        let caller = Caller::try_from(start.caller.clone().unwrap()).unwrap();
        assert_eq!(caller, expected_caller);
        assert!(caller.original.equals(&caller.immediate));
        assert!(caller.tokens.is_empty());
        let Principal::Account(account) = &caller.original else {
            panic!("expected an account caller, got {:?}", caller.original);
        };
        assert_eq!(account.account_id, AuthCtx::system().account_id());
        assert_eq!(
            caller.original.pair().unwrap().to_string(),
            format!(
                "(\"{}\",\"account:{}\")",
                test_golem_issuer(),
                AuthCtx::system().account_id().0.hyphenated()
            )
        );
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
        assert!(start.attempt_id.is_some());
        assert!(start.expected_callee_fingerprint.is_some());
    }

    #[test]
    async fn public_invocation_session_id_never_creates_and_pins_the_incarnation() {
        for case in [
            "missing",
            "wrong-type",
            "other-environment",
            "stale",
            "existing",
        ] {
            let harness = RestHarness::new_with_output(
                AgentMode::Durable,
                OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u32())))),
            );
            if case != "missing" {
                harness
                    .worker_client
                    .set_metadata_component_revision(harness.component_revision);
            }
            if case == "other-environment" {
                *harness
                    .worker_client
                    .metadata_environment_id
                    .lock()
                    .unwrap() = Some(EnvironmentId::new());
            }
            let mut start = harness.public_invocation_start(IdempotencyKey::fresh());
            let agent_id = if case == "wrong-type" {
                AgentId::keyed(
                    harness.environment_id,
                    TypeId::try_from(43u32).unwrap(),
                    &harness.agent_key(),
                )
            } else {
                harness.keyed_agent_id()
            };
            start.selector.target = PublicAgentTarget::Id {
                agent_id,
                expected_fingerprint: Some(if case == "stale" {
                    AgentFingerprint::new()
                } else {
                    harness.worker_client.fingerprint
                }),
            };
            let result = harness
                .worker_service
                .invoke_public_agent_session_v1(
                    start,
                    Box::pin(stream::empty()),
                    AuthCtx::system(),
                    |_, _| panic!("stream-free input"),
                )
                .await;
            if case == "existing" {
                assert_eq!(result.unwrap().agent_id, agent_id);
                let (_, trusted) = harness.worker_client.invocation_session_start();
                assert!(trusted.constructor.is_none());
                assert_eq!(
                    trusted.expected_callee_fingerprint.map(Uuid::from),
                    Some(harness.worker_client.fingerprint.0)
                );
            } else {
                assert!(
                    matches!(
                        result,
                        Err(PublicAgentSessionStartError::Worker(
                            WorkerServiceError::AgentNotFound(_)
                        ))
                    ),
                    "{case}: {:?}",
                    result.err()
                );
                assert_eq!(
                    harness.worker_client.invocation_session_start_count(),
                    0,
                    "{case}"
                );
            }
            assert!(
                harness
                    .worker_client
                    .created_agent_ids
                    .lock()
                    .unwrap()
                    .is_empty(),
                "{case} must not create"
            );
        }
    }

    #[test]
    async fn public_invocation_session_rejects_create_only_overrides() {
        let harness = RestHarness::new(AgentMode::Durable);
        for target in [
            harness
                .public_invocation_start(IdempotencyKey::fresh())
                .selector
                .target,
            PublicAgentTarget::Id {
                agent_id: harness.keyed_agent_id(),
                expected_fingerprint: None,
            },
        ] {
            let mut start = harness.public_invocation_start(IdempotencyKey::fresh());
            start.selector.target = target;
            start.config.push(PublicConfigEntry {
                path: vec!["limit".to_string()],
                value: serde_json::json!({"kind": "u32", "value": 3}),
            });
            let result = harness
                .worker_service
                .invoke_public_agent_session_v1(
                    start,
                    Box::pin(stream::empty()),
                    AuthCtx::system(),
                    |_, _| panic!("rejected input"),
                )
                .await;
            assert!(matches!(
                result,
                Err(PublicAgentSessionStartError::Worker(
                    WorkerServiceError::TypeChecker(_)
                ))
            ));
        }
        assert_eq!(harness.worker_client.invocation_session_start_count(), 0);
        assert!(
            harness
                .worker_client
                .created_agent_ids
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    async fn public_invocation_session_defers_output_schema_until_execution_is_ready() {
        let harness = RestHarness::new_with_pinned_and_latest_output(
            OutputSchema::Single(Box::new(SchemaType::string())),
            OutputSchema::Single(Box::new(SchemaType::u32())),
        );
        let mut start = harness.public_invocation_start(IdempotencyKey::fresh());
        start.selector.target = PublicAgentTarget::Id {
            agent_id: harness.keyed_agent_id(),
            expected_fingerprint: None,
        };
        let started = harness
            .worker_service
            .invoke_public_agent_session_v1(
                start,
                Box::pin(stream::empty()),
                AuthCtx::system(),
                |_, _| panic!("stream-free input"),
            )
            .await
            .unwrap();
        assert_eq!(started.component_revision, ComponentRevision::INITIAL);
        assert_eq!(started.output_schema, None);
    }

    #[test]
    async fn public_invocation_session_rejects_a_selector_of_another_environment() {
        let harness = RestHarness::new(AgentMode::Durable);
        let mut start = harness.public_invocation_start(IdempotencyKey::fresh());
        start.environment_id = EnvironmentId::new();

        let result = harness
            .worker_service
            .invoke_public_agent_session_v1(
                start,
                Box::pin(stream::empty()),
                AuthCtx::system(),
                |_, _| panic!("stream resolver called for a rejected start"),
            )
            .await;

        assert!(matches!(
            result,
            Err(PublicAgentSessionStartError::Worker(
                WorkerServiceError::TypeChecker(_)
            ))
        ));
        assert_eq!(harness.worker_client.invocation_session_start_count(), 0);
    }

    #[test]
    async fn public_invocation_session_preserves_raw_live_stream_references() {
        let harness = RestHarness::new_with_input(
            AgentMode::Durable,
            InputSchema::Parameters(vec![NamedField::user_supplied(
                "input",
                SchemaType::stream(Some(SchemaType::u32())),
            )]),
        );
        let mut start = harness.public_invocation_start(IdempotencyKey::fresh());
        let provisional_ref = Uuid::new_v4();
        start.method_parameters = serde_json::json!({
            "kind": "record",
            "value": {"fields": [{
                "kind": "stream",
                "value": {"provisionalRef": provisional_ref}
            }]}
        })
        .try_into()
        .unwrap();
        let expected_input = golem_common::model::agent::AgentMethodInput::PublicJson(
            start.method_parameters.clone(),
        );

        let _responses = harness
            .worker_service
            .invoke_public_agent_session_v1(
                start,
                Box::pin(stream::empty()),
                AuthCtx::system(),
                |_, _| panic!("stream bindings must wait for the selected execution schema"),
            )
            .await
            .unwrap();

        let (_, trusted_start) = harness.worker_client.invocation_session_start();
        assert!(trusted_start.input.is_none());
        let captured: golem_common::model::agent::AgentMethodInput =
            golem_common::serialization::deserialize(
                trusted_start.captured_method_input.as_ref().unwrap(),
            )
            .unwrap();
        assert_eq!(captured, expected_input);
        assert_eq!(
            trusted_start.expected_callee_fingerprint.map(Uuid::from),
            Some(harness.worker_client.fingerprint.0)
        );
        assert_eq!(
            harness.worker_client.created_agent_id(),
            harness.keyed_agent_id()
        );
        assert!(
            trusted_start.target_agent_id().is_none(),
            "a durable agent of a constructor is addressed by its key"
        );
    }

    #[test]
    async fn public_streaming_invocation_pins_the_resolved_callee_fingerprint() {
        let harness = RestHarness::new_with_output(
            AgentMode::Durable,
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u32())))),
        );

        harness
            .worker_service
            .invoke_public_agent_session_v1(
                harness.public_invocation_start(IdempotencyKey::fresh()),
                Box::pin(stream::empty()),
                AuthCtx::system(),
                |_, _| panic!("stream resolver called for a stream-free input"),
            )
            .await
            .unwrap();

        let (_, trusted_start) = harness.worker_client.invocation_session_start();
        assert_eq!(
            trusted_start.expected_callee_fingerprint.map(Uuid::from),
            Some(harness.worker_client.fingerprint.0)
        );
    }

    // The executor's admission refuses KnownFresh together with an expected callee fingerprint,
    // so no start the worker service sends may combine the two.
    #[test]
    async fn public_ephemeral_streaming_start_is_not_known_fresh() {
        let harness = RestHarness::new_with_output(
            AgentMode::Ephemeral,
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u32())))),
        );

        harness
            .worker_service
            .invoke_public_agent_session_v1(
                harness.public_invocation_start(IdempotencyKey::fresh()),
                Box::pin(stream::empty()),
                AuthCtx::system(),
                |_, _| panic!("stream resolver called for a stream-free input"),
            )
            .await
            .unwrap();

        let (_, trusted_start) = harness.worker_client.invocation_session_start();
        assert!(
            !(trusted_start.expected_callee_fingerprint.is_some()
                && trusted_start.freshness_disposition()
                    == golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh),
            "the start combines KnownFresh with an expected callee fingerprint, which the executor refuses"
        );
    }

    #[test]
    async fn session_start_with_expected_fingerprint_is_not_upgraded_to_known_fresh() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let agent_id = AgentId::ephemeral_invocation(
            harness.environment_id,
            test_type_id(),
            &IdempotencyKey::fresh(),
        );
        let start = InvocationStart {
            target: Some(AgentTarget::Id(agent_id).try_into().unwrap()),
            environment_id: Some(harness.environment_id.into()),
            method_name: Some("run".to_string()),
            request_key: Some(InvocationRequestKey::Unkeyed(Uuid::new_v4()).into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(harness.worker_client.fingerprint.0.into()),
            ..Default::default()
        };

        let _responses = harness
            .worker_service
            .invoke_agent_session(start, Box::pin(stream::empty()), true, AuthCtx::system())
            .await
            .unwrap();

        let (_, admitted) = harness.worker_client.invocation_session_start();
        assert!(admitted.expected_callee_fingerprint.is_some());
        assert_eq!(
            admitted.freshness_disposition(),
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist,
            "a start that expects an existing incarnation must reach the executor's guard, which refuses KnownFresh"
        );
    }

    #[test]
    async fn public_resume_reauthorizes_and_preserves_attachment_identity() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let mut resume = harness.public_resume();
        let requested = resume.clone();
        let auth = AuthCtx::system();
        let expected_auth = auth.clone().into();
        let expected_caller: golem_api_grpc::proto::golem::component::Caller =
            harness.worker_service.rest_caller(&auth).into();

        resume.auth_ctx = None;
        resume.caller = Some(Default::default());
        let stale_caller = resume.caller.clone();
        let _responses = harness
            .worker_service
            .resume_public_agent_session(resume, Box::pin(stream::empty()), auth)
            .await
            .unwrap();

        let (routed_agent_id, routed) = harness.worker_client.invocation_session_resume();
        assert_eq!(
            routed_agent_id,
            AgentId::try_from(requested.agent_id.unwrap()).unwrap()
        );
        assert_eq!(routed.idempotency_key, requested.idempotency_key);
        assert_eq!(routed.agent_id, requested.agent_id);
        assert_eq!(routed.environment_id, requested.environment_id);
        assert_eq!(routed.attachment_id, requested.attachment_id);
        assert_eq!(routed.attempt_id, requested.attempt_id);
        assert_eq!(
            routed.expected_callee_fingerprint,
            requested.expected_callee_fingerprint
        );
        assert_eq!(routed.expected_epoch, requested.expected_epoch);
        assert_eq!(routed.operation, requested.operation);
        assert_eq!(routed.cursors, requested.cursors);
        assert_eq!(routed.auth_ctx, Some(expected_auth));
        assert_eq!(routed.caller, Some(expected_caller));
        assert_ne!(routed.caller, stale_caller);
    }

    #[test]
    async fn public_takeover_preserves_the_requested_operation() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let mut takeover = harness.public_resume();
        takeover.operation = ResumeOperation::Takeover as i32;

        let _responses = harness
            .worker_service
            .resume_public_agent_session(
                takeover.clone(),
                Box::pin(stream::empty()),
                AuthCtx::system(),
            )
            .await
            .unwrap();

        let (_, routed) = harness.worker_client.invocation_session_resume();
        assert_eq!(routed.operation, ResumeOperation::Takeover as i32);
        assert_eq!(routed.expected_epoch, takeover.expected_epoch);
        assert_eq!(routed.attachment_id, takeover.attachment_id);
        assert_eq!(routed.attempt_id, takeover.attempt_id);
    }

    #[test]
    async fn trusted_resume_preserves_the_pinned_caller() {
        use golem_common::model::caller::{
            AgentPrincipal, Claims, Pair, TokenProvenance, UserPrincipal,
        };

        // A user's call that an agent made on the user's behalf, with a token: both roles and the
        // token must reach the executor as recorded, the claims byte for byte.
        let claims = r#"{"sub":"user-1", "nested":{"list":[1,2.50,{"deep":null}]},"n":1e3}"#;
        let pinned = Caller {
            original: Principal::User(UserPrincipal {
                pair: Pair::new("https://idp.example/", "user-1"),
                claims: Claims::from_json(claims.to_string()).unwrap(),
                authenticated_at: "2026-01-02T03:04:05.678Z".parse().unwrap(),
            }),
            immediate: Principal::Agent(AgentPrincipal::of_agent(
                "https://golem.example".to_string(),
                AgentId::from_parts(7, 0xc0),
                golem_common::model::AgentFingerprint(Uuid::from_u128(0xf1)),
                AgentTypeName("Relay".to_string()),
                EnvironmentId(Uuid::from_u128(0xe0)),
            )),
            tokens: vec![TokenProvenance {
                id: Uuid::from_u128(0x70c1),
                maker: Some(golem_common::model::caller::PrincipalRef::new(Pair::new(
                    "https://golem.example",
                    "account:1",
                ))),
            }],
        };
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let mut resume = harness.public_resume();
        let pinned_caller = Some(pinned.clone().into());
        resume.caller = pinned_caller.clone();

        let _responses = harness
            .worker_service
            .resume_agent_session(resume, Box::pin(stream::empty()), AuthCtx::system())
            .await
            .unwrap();

        let (_, routed) = harness.worker_client.invocation_session_resume();
        assert_eq!(routed.caller, pinned_caller);
        let routed = Caller::try_from(routed.caller.unwrap()).unwrap();
        assert_eq!(routed, pinned);
        match &routed.original {
            Principal::User(user) => assert_eq!(user.claims.as_json(), claims),
            other => panic!("the original caller is not the user: {other:?}"),
        }
    }

    #[test]
    async fn public_resume_of_an_agent_in_another_environment_is_not_found_before_dispatch() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let mut resume = harness.public_resume();
        resume.environment_id = Some(EnvironmentId::new().into());

        let error = match harness
            .worker_service
            .resume_public_agent_session(resume, Box::pin(stream::empty()), AuthCtx::system())
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("resume reached an agent of another environment"),
        };

        assert!(matches!(error, WorkerServiceError::AgentNotFound(_)));
        assert_eq!(harness.worker_client.invocation_session_resume_count(), 0);
    }

    #[test]
    async fn unauthorized_public_resume_never_reaches_worker_dispatch() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let auth = AuthCtx::agent_with_effective_surface(
            AccountId(Uuid::new_v4()),
            AccountEmail::new("unauthorized-resume@golem"),
            EffectiveSurface {
                source_card_ids: vec![],
                lower: vec![],
                upper: vec![],
            },
        );

        let error = match harness
            .worker_service
            .resume_public_agent_session(harness.public_resume(), Box::pin(stream::empty()), auth)
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("an empty permission surface must reject resume"),
        };

        assert!(matches!(error, WorkerServiceError::AgentAccessDenied(_)));
        assert_eq!(harness.worker_client.invocation_session_resume_count(), 0);
    }

    #[test]
    fn public_invocation_values_reject_capabilities_and_constructor_streams_recursively() {
        use golem_schema::proto::golem::schema::{
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
            .invoke_public_agent_session_v1(
                harness.public_invocation_start(idempotency_key),
                Box::pin(stream::empty()),
                auth,
                |_, _| panic!("stream resolver called before authorization"),
            )
            .await
        {
            Err(error) => error,
            Ok(_) => panic!("an empty permission surface must reject the resolved agent selector"),
        };

        assert!(matches!(
            error,
            PublicAgentSessionStartError::Worker(WorkerServiceError::AuthError(_))
        ));
        assert_eq!(harness.worker_client.invocation_session_start_count(), 0);
    }

    #[test]
    async fn non_attached_modes_defer_streaming_classification_to_execution() {
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
            harness
                .invoke_rest(request, None, AuthCtx::system())
                .await
                .expect("the executor classifies the selected method");
        }
        assert_eq!(harness.worker_client.invocations().len(), 3);
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
                .invoke_rest(request, None, AuthCtx::system())
                .await
                .expect("stream-free one-shot invocation must still dispatch");
        }

        assert_eq!(harness.worker_client.invocations().len(), 3);
    }

    #[test]
    async fn invoke_agent_forwards_the_expected_fingerprint_without_known_fresh() {
        for mode in [AgentMode::Durable, AgentMode::Ephemeral] {
            let harness = RestHarness::new(mode);
            harness
                .worker_client
                .set_metadata_component_revision(harness.component_revision);
            let agent_id = match mode {
                AgentMode::Durable => harness.keyed_agent_id(),
                AgentMode::Ephemeral => AgentId::ephemeral_invocation(
                    harness.environment_id,
                    test_type_id(),
                    &IdempotencyKey::fresh(),
                ),
            };
            let expected = harness.worker_client.fingerprint;
            for fingerprint in [Some(expected), None] {
                harness
                    .worker_service
                    .invoke_agent(
                        harness.environment_id,
                        AgentTarget::Id(agent_id),
                        None,
                        None,
                        Some("run".to_string()),
                        Some(SchemaValue::Record { fields: vec![] }.try_into().unwrap()),
                        golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                        None,
                        InvocationRequestKey::external(None, None),
                        None,
                        true,
                        InvocationFreshnessDisposition::KnownFresh,
                        Vec::new(),
                        AuthCtx::system(),
                        Caller::anonymous().into(),
                        None,
                        fingerprint,
                    )
                    .await
                    .unwrap();
            }
            assert_eq!(
                harness.worker_client.invocation_expected_fingerprints(),
                vec![Some(expected), None],
                "{mode:?}"
            );
            assert!(
                harness
                    .worker_client
                    .invocations()
                    .iter()
                    .all(|invocation| invocation.0 == agent_id),
                "{mode:?}"
            );
            // A trusted caller may declare the agent fresh, but a caller that expects an
            // existing incarnation must reach the executor's existing-only guard.
            let freshness: Vec<_> = harness
                .worker_client
                .invocations()
                .into_iter()
                .map(|invocation| invocation.2)
                .collect();
            assert_eq!(
                freshness,
                vec![
                    InvocationFreshnessDisposition::MayExist,
                    InvocationFreshnessDisposition::KnownFresh
                ],
                "{mode:?}"
            );
        }
    }

    // A caller that names an incarnation is answered for that incarnation before its input is
    // validated, so a method that only the incarnation's own revision may have is not reported
    // as invalid input when the incarnation is gone or was replaced.
    #[test]
    async fn invoke_agent_with_expected_fingerprint_checks_the_incarnation_before_the_method() {
        let harness = RestHarness::new(AgentMode::Durable);
        let agent_id = harness.keyed_agent_id();
        let invoke = |fingerprint| {
            harness.worker_service.invoke_agent(
                harness.environment_id,
                AgentTarget::Id(agent_id),
                None,
                None,
                Some("no-such-method".to_string()),
                Some(SchemaValue::Record { fields: vec![] }.try_into().unwrap()),
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                None,
                InvocationRequestKey::external(None, None),
                None,
                true,
                InvocationFreshnessDisposition::MayExist,
                Vec::new(),
                AuthCtx::system(),
                Caller::anonymous().into(),
                None,
                fingerprint,
            )
        };

        let missing = invoke(Some(harness.worker_client.fingerprint)).await;
        assert!(
            matches!(missing, Err(WorkerServiceError::AgentNotFound(_))),
            "{missing:?}"
        );

        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let replaced = invoke(Some(AgentFingerprint::new())).await;
        assert!(
            matches!(replaced, Err(WorkerServiceError::AgentNotFound(_))),
            "{replaced:?}"
        );

        let unpinned = invoke(None).await;
        assert!(unpinned.is_ok(), "{unpinned:?}");
        assert_eq!(harness.worker_client.invocations().len(), 1);
    }

    // A session start that names an incarnation is answered for that incarnation before its
    // method is looked up, so a method missing from the deployed component is not reported as a
    // validation failure when the incarnation is gone or was replaced.
    #[test]
    async fn session_start_with_expected_fingerprint_checks_the_incarnation_before_the_method() {
        let harness = RestHarness::new(AgentMode::Durable);
        let start = |fingerprint: AgentFingerprint| InvocationStart {
            target: Some(
                AgentTarget::Id(harness.keyed_agent_id())
                    .try_into()
                    .unwrap(),
            ),
            environment_id: Some(harness.environment_id.into()),
            method_name: Some("no-such-method".to_string()),
            request_key: Some(InvocationRequestKey::external(None, None).into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(Uuid::new_v4().into()),
            expected_callee_fingerprint: Some(fingerprint.0.into()),
            ..Default::default()
        };

        let missing = harness
            .worker_service
            .invoke_agent_session(
                start(harness.worker_client.fingerprint),
                Box::pin(stream::empty()),
                true,
                AuthCtx::system(),
            )
            .await;
        assert!(
            matches!(missing, Err(WorkerServiceError::AgentNotFound(_))),
            "{:?}",
            missing.err()
        );

        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let replaced = harness
            .worker_service
            .invoke_agent_session(
                start(AgentFingerprint::new()),
                Box::pin(stream::empty()),
                true,
                AuthCtx::system(),
            )
            .await;
        assert!(
            matches!(replaced, Err(WorkerServiceError::AgentNotFound(_))),
            "{:?}",
            replaced.err()
        );
        assert_eq!(harness.worker_client.invocation_session_start_count(), 0);
    }

    #[test]
    async fn one_shot_native_and_protobuf_inputs_reach_execution_without_schema_validation() {
        let harness = RestHarness::new_with_input(
            AgentMode::Durable,
            InputSchema::Parameters(vec![NamedField::user_supplied(
                "bytes",
                SchemaType::list(SchemaType::u8()),
            )]),
        );
        for mode in [AgentInvocationMode::Await, AgentInvocationMode::Schedule] {
            let mut request = harness.invoke_request();
            request.mode = mode;
            request.method_parameters =
                golem_common::schema::ExternalSchemaValue::try_from(SchemaValue::Record {
                    fields: vec![SchemaValue::List {
                        elements: vec![SchemaValue::U8(3), SchemaValue::String("bad".into())],
                    }],
                })
                .unwrap();
            harness
                .invoke_rest(request, None, AuthCtx::system())
                .await
                .expect("input validation belongs to the selected execution");
        }
        for input in [
            SchemaValue::Record {
                fields: vec![SchemaValue::List {
                    elements: (0..10_000)
                        .map(|i| SchemaValue::U8((i % 251) as u8))
                        .collect(),
                }],
            },
            SchemaValue::Record {
                fields: vec![SchemaValue::List {
                    elements: vec![SchemaValue::U8(3), SchemaValue::String("bad".into())],
                }],
            },
        ] {
            let captured: golem_common::model::agent::AgentMethodInput =
                input.clone().try_into().unwrap();
            let result = harness
                .worker_service
                .invoke_agent(
                    harness.environment_id,
                    AgentTarget::Key(harness.agent_key()),
                    None,
                    Some(harness.component_id),
                    Some("run".to_string()),
                    Some(input.try_into().unwrap()),
                    golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
                    None,
                    InvocationRequestKey::external(None, None),
                    None,
                    false,
                    InvocationFreshnessDisposition::MayExist,
                    Vec::new(),
                    AuthCtx::system(),
                    Caller::anonymous().into(),
                    None,
                    None,
                )
                .await;
            assert!(result.is_ok(), "unexpected result: {result:?}");
            assert_eq!(
                harness
                    .worker_client
                    .invocation_inputs
                    .lock()
                    .unwrap()
                    .last(),
                Some(&Some(captured))
            );
        }
        assert_eq!(harness.worker_client.invocations().len(), 4);
    }

    #[test]
    async fn new_invocations_resolve_the_deployed_type_and_existing_inputs_reach_execution() {
        use crate::config::ComponentServiceConfig;
        use crate::custom_api::call_agent::CallAgentHandler;
        use crate::custom_api::route_resolver::ResolvedRouteEntry;
        use crate::custom_api::{
            ResponseBody, RichCompiledRoute, RichRequest, RichRouteBehaviour, RichRouteSecurity,
        };
        use crate::service::component::RemoteComponentService;
        use crate::service::component::tests::TestRegistry;
        use golem_common::model::domain_registration::Domain;
        use golem_schema::http::HttpRequestTarget;
        use golem_service_base::custom_api::{
            AgentRouteMode, AgentSelection, CallAgentBehaviour, CompiledInputSchema,
            CompiledOutputSchema, MethodParameter, PathSegmentType, QueryOrHeaderType,
            RequestBodySchema,
        };

        for new_type in [true, false] {
            let harness = RestHarness::new(AgentMode::Durable);
            let a = harness
                .worker_service
                .component_service
                .get_current_by_id(harness.component_id)
                .await
                .unwrap();
            let type_name = if new_type {
                AgentTypeName("Greeter".into())
            } else {
                harness.agent_type_name.clone()
            };
            let mut agent_type = test_agent_type(type_name.clone(), AgentMode::Durable);
            agent_type.methods[0].input_schema =
                InputSchema::Parameters(vec![NamedField::user_supplied(
                    "message",
                    SchemaType::string(),
                )]);
            agent_type.methods[0].output_schema =
                OutputSchema::Single(Box::new(SchemaType::string()));
            let method_input = agent_type.methods[0].input_schema.clone();
            let b = test_component(
                a.id,
                a.environment_id,
                a.account_id,
                ComponentRevision::new(1).unwrap(),
                agent_type.clone(),
            );
            let registry = Arc::new(TestRegistry::new(
                vec![a.clone(), b.clone()],
                Some(a.clone()),
            ));
            let components = Arc::new(RemoteComponentService::new(
                registry.clone(),
                &ComponentServiceConfig::default(),
            ));
            components.get_current_by_id(a.id).await.unwrap();
            let agent_types = agent_type_resolver(Arc::new(TestRegistryService {
                removed: false,
                resolved: ResolvedAgentType {
                    registered_agent_type: RegisteredAgentType {
                        type_id: test_type_id(),
                        agent_type,
                        implemented_by: RegisteredAgentTypeImplementer {
                            component_id: b.id,
                            component_revision: b.revision,
                            component_name: b.component_name.0.clone(),
                            account_id: b.account_id,
                            account_email: b.account_email.clone(),
                        },
                    },
                    environment_id: b.environment_id,
                    deployment_revision: DeploymentRevision::new(1).unwrap(),
                    current_deployment_revision: None,
                },
            }));
            let agent_id = AgentId::keyed(
                a.environment_id,
                test_type_id(),
                &AgentKey::new(type_name.clone(), empty_constructor_parameters()).unwrap(),
            );
            let input = SchemaValue::Record {
                fields: vec![SchemaValue::String("new schema".into())],
            };
            let behaviour = CallAgentBehaviour {
                route_mode: AgentRouteMode::Plain,
                base_path_variables: 0,
                durable_streams: None,
                component_id: a.id,
                component_revision: b.revision,
                agent_type: type_name.clone(),
                agent_mode: AgentMode::Durable,
                constructor_input: CompiledInputSchema {
                    graph: SchemaGraph::anonymous(SchemaType::record(vec![])),
                    input_schema: InputSchema::Parameters(vec![]),
                },
                constructor_parameters: vec![],
                agent_selection: AgentSelection::Key,
                method_name: "run".into(),
                method_input: CompiledInputSchema {
                    graph: SchemaGraph::anonymous(SchemaType::record(vec![
                        golem_common::schema::NamedFieldType {
                            name: "message".into(),
                            body: SchemaType::string(),
                            metadata: Default::default(),
                        },
                    ])),
                    input_schema: method_input,
                },
                body: RequestBodySchema::Unused,
                method_parameters: vec![MethodParameter::Query {
                    query_parameter_name: "message".into(),
                    parameter_type: QueryOrHeaderType::Primitive(PathSegmentType::Str),
                }],
                expected_agent_response: CompiledOutputSchema {
                    graph: SchemaGraph::anonymous(SchemaType::string()),
                    output_schema: OutputSchema::Single(Box::new(SchemaType::string())),
                },
                method_description: None,
                read_only: None,
            };
            let resolved = ResolvedRouteEntry {
                domain: Domain("example.com".into()),
                public_scheme: "http".into(),
                public_authority: "example.com".into(),
                route: Arc::new(RichCompiledRoute {
                    account_id: a.account_id,
                    account_email: a.account_email.clone(),
                    environment_id: a.environment_id,
                    deployment_revision: DeploymentRevision::new(1).unwrap(),
                    route_id: 1,
                    route_match: golem_common::model::agent::HttpMethod::Method(
                        golem_schema::http::HttpMethod::Get(Default::default()),
                    )
                    .into(),
                    path: vec![],
                    behavior: RichRouteBehaviour::CallAgent(behaviour.clone()),
                    security: RichRouteSecurity::None,
                    cors: golem_service_base::custom_api::CorsOptions {
                        allowed_patterns: vec![],
                    },
                }),
                captured_path_parameters: vec![],
                request_target: HttpRequestTarget::parse("/").unwrap(),
                openapi_inputs: None,
            };
            let mut output = harness
                .worker_client
                .invocation_output
                .lock()
                .unwrap()
                .clone();
            output.result = golem_common::model::AgentInvocationResult::AgentMethod {
                output: SchemaValue::String("hello from deployed agent".into()),
            };
            harness.worker_client.set_invocation_output(output);
            let worker_service = Arc::new(WorkerService::new(
                components.clone(),
                harness.worker_service._auth_service.clone(),
                harness.worker_service.limit_service.clone(),
                harness.worker_client.clone(),
                harness.worker_service.agent_resolution_cache.clone(),
                agent_types,
                test_golem_issuer(),
            ));
            let handler = CallAgentHandler::new(worker_service.clone());
            let mut request = RichRequest::new(
                poem::Request::builder()
                    .uri("/?message=new%20schema".parse().unwrap())
                    .header("host", "example.com")
                    .finish(),
            );
            let response = handler
                .handle_call_agent_behaviour(&mut request, &resolved, &behaviour)
                .await
                .expect("a new HTTP invocation must use the deployed revision of its agent type");
            assert_eq!(response.status, http::StatusCode::OK);
            let ResponseBody::ComponentModelJsonBody { body } = response.body else {
                panic!("expected JSON HTTP response")
            };
            assert_eq!(
                body.value(),
                &SchemaValue::String("hello from deployed agent".into())
            );
            assert_eq!(harness.worker_client.invoked_agent_id(), agent_id);
            if new_type {
                continue;
            }
            *harness
                .worker_client
                .metadata_component_revision
                .lock()
                .unwrap() = Some(a.revision);
            let invoke = |input: SchemaValue| {
                worker_service.invoke_agent(
                    a.environment_id,
                    AgentTarget::Id(agent_id),
                    None,
                    None,
                    Some("run".into()),
                    Some(input.try_into().unwrap()),
                    golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
                    None,
                    InvocationRequestKey::external(None, None),
                    None,
                    false,
                    InvocationFreshnessDisposition::MayExist,
                    vec![],
                    AuthCtx::system(),
                    Caller::anonymous().into(),
                    None,
                    None,
                )
            };
            invoke(input)
                .await
                .expect("the executor selects the existing agent's input schema");
            invoke(SchemaValue::Record { fields: vec![] })
                .await
                .expect("existing agent must still accept its recorded revision's input");
        }
    }

    #[test]
    async fn rest_output_capability_restrictions_reach_selected_execution() {
        for capability in [
            SchemaType::secret(Default::default()),
            SchemaType::quota_token(Default::default()),
            SchemaType::permission_card(Default::default()),
        ] {
            for output in [capability.clone(), SchemaType::option(capability)] {
                let harness = RestHarness::new_with_output(
                    AgentMode::Durable,
                    OutputSchema::Single(Box::new(output)),
                );
                for (mode, schedule_at) in [
                    (AgentInvocationMode::Await, None),
                    (AgentInvocationMode::Schedule, None),
                    (AgentInvocationMode::Schedule, Some(Utc::now())),
                ] {
                    let mut request = harness.invoke_request();
                    request.mode = mode;
                    request.schedule_at = schedule_at;
                    harness
                        .invoke_rest(request, None, AuthCtx::system())
                        .await
                        .expect("the selected execution checks output capabilities");
                }
                let inputs = harness.worker_client.invocation_inputs.lock().unwrap();
                assert_eq!(inputs.len(), 3);
                assert!(inputs.iter().all(|input| matches!(input,
                    Some(golem_common::model::agent::AgentMethodInput::Native(input)) if input.is_external_rest()
                )));
            }
        }
    }

    #[test]
    async fn rest_output_capability_classification_does_not_use_metadata_revision() {
        let capability_output =
            OutputSchema::Single(Box::new(SchemaType::secret(Default::default())));
        let pinned_capability = RestHarness::new_with_pinned_and_latest_output(
            capability_output.clone(),
            OutputSchema::Unit,
        );
        pinned_capability
            .invoke_rest(pinned_capability.invoke_request(), None, AuthCtx::system())
            .await
            .expect("the executor selects the output schema");
        assert_eq!(pinned_capability.worker_client.invocations().len(), 1);

        let pinned_plain =
            RestHarness::new_with_pinned_and_latest_output(OutputSchema::Unit, capability_output);
        pinned_plain
            .invoke_rest(pinned_plain.invoke_request(), None, AuthCtx::system())
            .await
            .expect("the pinned ordinary output must remain dispatchable");
        assert_eq!(pinned_plain.worker_client.invocations().len(), 1);
    }

    #[test]
    async fn non_attached_classification_does_not_use_metadata_revision() {
        let stream_output =
            OutputSchema::Single(Box::new(SchemaType::stream(Some(SchemaType::u8()))));
        let pinned_streaming = RestHarness::new_with_pinned_and_latest_output(
            stream_output.clone(),
            OutputSchema::Unit,
        );

        pinned_streaming
            .invoke_rest(pinned_streaming.invoke_request(), None, AuthCtx::system())
            .await
            .expect("the executor selects the streaming policy");
        assert_eq!(pinned_streaming.worker_client.invocations().len(), 1);

        let pinned_stream_free =
            RestHarness::new_with_pinned_and_latest_output(OutputSchema::Unit, stream_output);
        pinned_stream_free
            .invoke_rest(pinned_stream_free.invoke_request(), None, AuthCtx::system())
            .await
            .expect("the pinned stream-free revision must remain dispatchable");
        assert_eq!(pinned_stream_free.worker_client.invocations().len(), 1);
    }

    #[test]
    async fn ephemeral_rest_invocations_get_fresh_identities_per_request() {
        let harness = RestHarness::new(AgentMode::Ephemeral);

        let first = harness
            .invoke_rest(harness.invoke_request(), None, AuthCtx::system())
            .await
            .unwrap();
        let second = harness
            .invoke_rest(harness.invoke_request(), None, AuthCtx::system())
            .await
            .unwrap();

        let invocations = harness.worker_client.invocations();
        assert_eq!(invocations.len(), 2);
        assert_ne!(invocations[0].1, invocations[1].1);
        assert_ne!(invocations[0].0, invocations[1].0);
        assert_eq!(first.agent_id, invocations[0].0);
        assert_eq!(first.idempotency_key.to_string(), invocations[0].1.value);
        assert_eq!(second.agent_id, invocations[1].0);
        assert_eq!(second.idempotency_key.to_string(), invocations[1].1.value);
        for (agent_id, owner_key, _) in &invocations {
            assert_eq!(
                *agent_id,
                AgentId::ephemeral_invocation(harness.environment_id, test_type_id(), owner_key)
            );
        }
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
        let auth = scoped_invocation_auth();
        let seed = InvocationRequestKey::external(auth.idempotency_scope(), Some(&idempotency_key))
            .owner_key("run");
        let request = harness.invoke_request();

        harness
            .invoke_rest(request.clone(), Some(idempotency_key.clone()), auth.clone())
            .await
            .unwrap();
        harness
            .invoke_rest(request, Some(idempotency_key), auth)
            .await
            .unwrap();

        let invocations = harness.worker_client.invocations();
        assert_eq!(invocations.len(), 2);
        assert_eq!(invocations[0].0, invocations[1].0);
        assert_eq!(
            invocations[0].0,
            AgentId::ephemeral_invocation(harness.environment_id, test_type_id(), &seed)
        );
        assert_eq!(invocations[0].1, seed);
        assert_eq!(invocations[1].1, seed);
        assert!(
            invocations
                .iter()
                .all(|invocation| invocation.2 == InvocationFreshnessDisposition::MayExist)
        );
    }

    #[test]
    async fn scheduled_ephemeral_invocation_preserves_supplied_identity_key_and_time() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        let idempotency_key = IdempotencyKey::new("scheduled-tool-call".to_string());
        let auth = scoped_invocation_auth();
        let seed = InvocationRequestKey::external(auth.idempotency_scope(), Some(&idempotency_key))
            .owner_key("run");
        let schedule_at = Utc::now();
        let expected_schedule_at = ::prost_types::Timestamp {
            seconds: schedule_at.timestamp(),
            nanos: schedule_at.timestamp_subsec_nanos() as i32,
        };
        let mut request = harness.invoke_request();
        request.mode = AgentInvocationMode::Schedule;
        request.schedule_at = Some(schedule_at);

        let response = harness
            .invoke_rest(request, Some(idempotency_key), auth)
            .await
            .unwrap();

        let invocations = harness.worker_client.invocations();
        assert_eq!(invocations.len(), 1);
        assert_eq!(response.agent_id, invocations[0].0);
        assert_eq!(response.idempotency_key.to_string(), seed.value);
        assert_eq!(invocations[0].1, seed);
        assert_eq!(
            invocations[0].0,
            AgentId::ephemeral_invocation(harness.environment_id, test_type_id(), &seed)
        );
        assert_eq!(invocations[0].2, InvocationFreshnessDisposition::MayExist);
        assert_eq!(
            harness.worker_client.invocation_modes_and_schedules(),
            vec![(
                golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
                Some(expected_schedule_at),
            )]
        );
    }

    #[test]
    async fn fresh_durable_binding_uses_distinct_targets_without_ephemeral_freshness() {
        let harness = RestHarness::new(AgentMode::Durable);
        let request_key = InvocationRequestKey::external(None, None);
        let mut targets = HashSet::new();
        for _ in 0..2 {
            let (target, _, freshness) = harness
                .worker_service
                .bound_invocation_target(
                    harness.component_id,
                    &harness.agent_type_name,
                    Some(&harness.constructor()),
                    AgentBinding::Fresh,
                    &request_key,
                    "run",
                    &AuthCtx::system(),
                )
                .await
                .unwrap();
            assert!(target.creates());
            assert_eq!(freshness, InvocationFreshnessDisposition::MayExist);
            assert!(targets.insert(target.agent_id));
        }
    }

    #[test]
    async fn ephemeral_session_entry_is_stable_and_lookup_is_observation_only() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        let idempotency_key = IdempotencyKey::new("session-tool-call".to_string());
        let request_key = InvocationRequestKey::external(
            scoped_invocation_auth().idempotency_scope(),
            Some(&idempotency_key),
        );
        let seed = request_key.owner_key("run");
        let expected_agent_id =
            AgentId::ephemeral_invocation(harness.environment_id, test_type_id(), &seed);

        let mut targets = Vec::new();
        for _ in 0..2 {
            let (target, _, freshness_disposition) = harness
                .worker_service
                .bound_invocation_target(
                    harness.component_id,
                    &harness.agent_type_name,
                    Some(&harness.constructor()),
                    AgentBinding::Constructor,
                    &request_key,
                    "run",
                    &AuthCtx::system(),
                )
                .await
                .unwrap();
            assert_eq!(
                freshness_disposition,
                InvocationFreshnessDisposition::MayExist
            );
            assert!(target.creates());
            targets.push(target);
        }
        assert_eq!(targets[0].agent_id, expected_agent_id);
        assert_eq!(targets[1].agent_id, expected_agent_id);

        let mut start = InvocationStart {
            method_name: Some("run".to_string()),
            request_key: Some(request_key.into()),
            idempotency_key: Some(idempotency_key.clone().into()),
            caller: Some(Default::default()),
            environment_id: Some(harness.environment_id.into()),
            mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32,
            freshness_disposition:
                golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
                    as i32,
            attempt_id: Some(Uuid::new_v4().into()),
            ..Default::default()
        };
        targets[0].apply(&mut start);

        let _responses = harness
            .worker_service
            .invoke_agent_session(
                start.clone(),
                Box::pin(stream::empty()),
                true,
                AuthCtx::system(),
            )
            .await
            .unwrap();

        let starts = harness.worker_client.invocation_session_starts();
        let (routed_agent_id, admitted_start) = &starts[0];
        let admitted_agent_id: AgentId = admitted_start
            .target_agent_id()
            .cloned()
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(routed_agent_id, &admitted_agent_id);
        assert_eq!(admitted_agent_id, expected_agent_id);
        assert_eq!(
            admitted_start.mode,
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Await as i32
        );
        assert_eq!(
            admitted_start.idempotency_key.clone().map(Into::into),
            Some(idempotency_key.clone())
        );
        assert_eq!(
            admitted_start.freshness_disposition(),
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
        );

        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        RoutedTarget::existing(admitted_agent_id, Some(harness.agent_type_name.clone()))
            .apply(&mut start);
        let final_key = IdempotencyKey::from_uuid(
            InvocationRequestKey::try_from(start.request_key.clone().unwrap())
                .unwrap()
                .finalize(admitted_agent_id, harness.worker_client.fingerprint, "run")
                .id()
                .0,
        );
        start.idempotency_key = Some(final_key.clone().into());
        start.mode = golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32;
        start.freshness_disposition =
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::KnownFresh as i32;
        let _responses = harness
            .worker_service
            .invoke_agent_session(start, Box::pin(stream::empty()), false, AuthCtx::system())
            .await
            .unwrap();

        let starts = harness.worker_client.invocation_session_starts();
        assert_eq!(starts.len(), 2);
        let (lookup_routed_agent_id, lookup_start) = &starts[1];
        assert_eq!(lookup_routed_agent_id, &admitted_agent_id);
        assert_eq!(
            lookup_start.target_agent_id(),
            Some(&admitted_agent_id.into())
        );
        assert!(lookup_start.constructor.is_none());
        assert!(lookup_start.component_id.is_none());
        assert_eq!(
            lookup_start.mode,
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32
        );
        assert_eq!(
            lookup_start.idempotency_key.clone().map(Into::into),
            Some(final_key)
        );
        assert_eq!(
            lookup_start.freshness_disposition(),
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
        );
    }

    #[test]
    async fn rest_invocation_requires_exactly_one_of_agent_id_and_parameters() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let mut both = harness.invoke_request();
        both.agent_id = Some(harness.keyed_agent_id());
        let mut neither = harness.invoke_request();
        neither.parameters = None;

        for request in [both, neither] {
            let result = harness.invoke_rest(request, None, AuthCtx::system()).await;
            assert!(
                matches!(result, Err(WorkerServiceError::TypeChecker(_))),
                "{result:?}"
            );
        }
        assert!(harness.worker_client.invocations().is_empty());
    }

    #[test]
    async fn rest_invocation_by_agent_id_never_creates_the_agent() {
        let harness = RestHarness::new(AgentMode::Durable);
        let by_id = |agent_id: AgentId| {
            let mut request = harness.invoke_request();
            request.agent_id = Some(agent_id);
            request.parameters = None;
            request
        };

        let missing = harness
            .invoke_rest(by_id(harness.keyed_agent_id()), None, AuthCtx::system())
            .await;
        assert!(
            matches!(missing, Err(WorkerServiceError::AgentNotFound(_))),
            "{missing:?}"
        );

        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let other_type = AgentId::from_parts(test_type_id().get() + 1, 1);
        let mistyped = harness
            .invoke_rest(by_id(other_type), None, AuthCtx::system())
            .await;
        assert!(
            matches!(mistyped, Err(WorkerServiceError::AgentNotFound(_))),
            "{mistyped:?}"
        );
        assert!(harness.worker_client.invocations().is_empty());

        let response = harness
            .invoke_rest(by_id(harness.keyed_agent_id()), None, AuthCtx::system())
            .await
            .unwrap();
        assert_eq!(response.agent_id, harness.keyed_agent_id());
        let targets = harness.worker_client.invoked_targets();
        assert_eq!(targets.len(), 1);
        assert!(
            !targets[0].creates(),
            "an ID-bound invocation must not create"
        );
        assert!(harness.worker_client.created_targets().is_empty());
    }

    #[test]
    async fn prepared_session_owners_are_scoped_by_caller_and_operation_and_survive_retry() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        let raw = IdempotencyKey::from("shared-raw-key");
        let account = AccountId::new();
        let requests = [
            (
                InvocationRequestKey::External {
                    scope: golem_common::model::IdempotencyScope::User(account),
                    key: raw.clone(),
                },
                "run",
            ),
            (
                InvocationRequestKey::External {
                    scope: golem_common::model::IdempotencyScope::User(AccountId::new()),
                    key: raw.clone(),
                },
                "run",
            ),
            (
                InvocationRequestKey::External {
                    scope: golem_common::model::IdempotencyScope::User(account),
                    key: raw.clone(),
                },
                "other",
            ),
            (InvocationRequestKey::external(None, Some(&raw)), "run"),
            (InvocationRequestKey::external(None, Some(&raw)), "run"),
        ];
        let mut owners = BTreeSet::new();
        for (prepared, method) in requests {
            let (target, _, _) = harness
                .worker_service
                .bound_invocation_target(
                    harness.component_id,
                    &harness.agent_type_name,
                    Some(&harness.constructor()),
                    AgentBinding::Constructor,
                    &prepared,
                    method,
                    &AuthCtx::System,
                )
                .await
                .unwrap();
            let mut start = InvocationStart {
                method_name: Some(method.into()),
                idempotency_key: Some(raw.clone().into()),
                request_key: Some(prepared.clone().into()),
                environment_id: Some(harness.environment_id.into()),
                mode: golem_api_grpc::proto::golem::worker::AgentInvocationMode::Schedule as i32,
                ..Default::default()
            };
            target.apply(&mut start);
            let _ = harness
                .worker_service
                .invoke_agent_session(start, Box::pin(stream::empty()), true, AuthCtx::System)
                .await
                .unwrap();
            let (owner, admitted) = harness
                .worker_client
                .invocation_session_starts()
                .last()
                .unwrap()
                .clone();
            assert!(owners.insert(owner));
            assert_eq!(
                owner,
                AgentId::ephemeral_invocation(
                    harness.environment_id,
                    test_type_id(),
                    &prepared.owner_key(method)
                )
            );
            assert_eq!(
                InvocationRequestKey::try_from(admitted.request_key.clone().unwrap()).unwrap(),
                prepared
            );
            assert_eq!(admitted.idempotency_key, Some(raw.clone().into()));
            let _ = harness
                .worker_service
                .invoke_agent_session(admitted, Box::pin(stream::empty()), true, AuthCtx::System)
                .await
                .unwrap();
            assert_eq!(
                harness
                    .worker_client
                    .invocation_session_starts()
                    .last()
                    .unwrap()
                    .0,
                owner
            );
        }
        assert_eq!(owners.len(), 5);
    }

    #[test]
    async fn ephemeral_lookup_accepts_the_final_invocation_identity() {
        let harness = RestHarness::new(AgentMode::Ephemeral);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let idempotency_key = IdempotencyKey::fresh();
        let agent_id = AgentId::ephemeral_invocation(
            harness.environment_id,
            test_type_id(),
            &IdempotencyKey::fresh(),
        );

        harness
            .worker_service
            .lookup_agent_invocation(
                InvocationStart {
                    target: Some(AgentTarget::Id(agent_id).try_into().unwrap()),
                    environment_id: Some(harness.environment_id.into()),
                    idempotency_key: Some(idempotency_key.clone().into()),
                    request_key: Some(InvocationRequestKey::external(None, None).into()),
                    ..Default::default()
                },
                AuthCtx::system(),
            )
            .await
            .unwrap();

        let (routed_agent_id, start) = harness.worker_client.invocation_session_start();
        assert_eq!(routed_agent_id, agent_id);
        assert_eq!(start.target_agent_id(), Some(&agent_id.into()));
        assert_eq!(start.idempotency_key, Some(idempotency_key.into()));
        assert_eq!(
            start.mode(),
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup
        );
        assert_eq!(
            start.freshness_disposition(),
            golem_api_grpc::proto::golem::worker::InvocationFreshnessDisposition::MayExist
        );
    }

    /// Unrelated keyless requests receive distinct prepared seeds in both invocation modes.
    #[test]
    async fn keyless_invocations_are_each_given_their_own_key() {
        let harness = RestHarness::new(AgentMode::Durable);

        // Twice per mode, not once each: a mint that handed every request of one
        // mode the same constant would still look fine across two different
        // modes, and only collides with itself.
        for mode in [AgentInvocationMode::Await, AgentInvocationMode::Schedule] {
            for _ in 0..2 {
                let mut request = harness.invoke_request();
                request.mode = mode.clone();
                harness
                    .invoke_rest(request, None, AuthCtx::system())
                    .await
                    .unwrap();
            }
        }

        let keys: Vec<IdempotencyKey> = harness
            .worker_client
            .invocations()
            .into_iter()
            .map(|(_, key, _)| key)
            .collect();

        assert_eq!(keys.len(), 4);
        let distinct: BTreeSet<&str> = keys.iter().map(|key| key.value.as_str()).collect();
        assert_eq!(
            distinct.len(),
            4,
            "unrelated invocations must not be handed the same key, or one would \
             join another instead of running: {keys:?}"
        );
    }

    #[test]
    async fn durable_rest_paths_use_keyed_agent_ids() {
        let harness = RestHarness::new(AgentMode::Durable);

        let upsert_response = harness
            .worker_service
            .upsert_agent_rest(
                harness.environment_id,
                &harness.agent_type_name,
                AgentKeyRequest {
                    parameters: empty_json_tuple(),
                },
                AuthCtx::system(),
            )
            .await
            .unwrap();
        let invoke_response = harness
            .invoke_rest(harness.invoke_request(), None, AuthCtx::system())
            .await
            .unwrap();

        let expected = AgentId::keyed(harness.environment_id, test_type_id(), &harness.agent_key());
        assert_eq!(upsert_response.agent_id, expected);
        assert_eq!(harness.worker_client.created_agent_id(), expected);
        assert_eq!(invoke_response.agent_id, expected);
        assert_eq!(harness.worker_client.invoked_agent_id(), expected);
        assert_eq!(
            harness.worker_client.invocations()[0].2,
            InvocationFreshnessDisposition::MayExist
        );
        assert_eq!(
            *harness
                .worker_client
                .creating_ignore_already_existing
                .lock()
                .unwrap(),
            vec![true],
            "an upsert joins the keyed agent when it exists"
        );
    }

    #[test]
    async fn unknown_agent_ids_require_environment_access_before_tag_resolution() {
        use golem_common::model::card::owner::EnvironmentOwnerPattern;
        use golem_common::model::card::{EnvironmentResourcePattern, EnvironmentVerb};

        let harness = RestHarness::new(AgentMode::Durable);
        let environment_permission = PermissionTarget::Environment(ClassPermissionTarget {
            owner: EnvironmentOwnerPattern::AnyEnvironments,
            verb: Some(EnvironmentVerb::View),
            resource: EnvironmentResourcePattern::Any,
        });
        let agent_permission = PermissionTarget::Agent(ClassPermissionTarget {
            owner: AgentOwnerPattern::AnyAgents,
            verb: None,
            resource: AgentResourcePattern::Any,
        });
        let authorized =
            auth_ctx_with_permissions(vec![environment_permission.clone(), agent_permission]);
        let environment_only = auth_ctx_with_permissions(vec![environment_permission]);
        for tag in [42, 43, 98765, 0] {
            let agent = harness.owned(&AgentId::from_parts(tag, 123));
            for (auth, allowed) in [
                (authorized.clone(), true),
                (scoped_invocation_auth(), false),
                (environment_only.clone(), tag != 42),
                (auth_ctx_with_permissions(vec![]), false),
            ] {
                let read = harness
                    .worker_service
                    .get_metadata(&agent, None, auth.clone())
                    .await;
                let delete = harness.worker_service.delete(&agent, None, auth).await;
                for error in [read.unwrap_err(), delete.unwrap_err()] {
                    if allowed {
                        assert!(
                            matches!(error, WorkerServiceError::AgentNotFound(_)),
                            "tag={tag}: {error:?}"
                        );
                    } else {
                        assert!(
                            matches!(error, WorkerServiceError::AgentAccessDenied(_)),
                            "tag={tag}: {error:?}"
                        );
                    }
                }
            }
        }
        let missing_environment_agent = OwnedAgentId {
            environment_id: EnvironmentId::new(),
            agent_id: AgentId::from_parts(42, 123),
        };
        for auth in [authorized, auth_ctx_with_permissions(vec![])] {
            let read = harness
                .worker_service
                .get_metadata(&missing_environment_agent, None, auth.clone())
                .await;
            let delete = harness
                .worker_service
                .delete(&missing_environment_agent, None, auth)
                .await;
            for error in [read.unwrap_err(), delete.unwrap_err()] {
                assert!(
                    matches!(error, WorkerServiceError::AgentAccessDenied(id) if id == missing_environment_agent.agent_id),
                    "{error:?}"
                );
            }
        }
        assert!(
            harness
                .worker_client
                .deleted_agent_ids
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    /// #3133's answer survives the routing layer, which is the whole point of
    /// settling existence with a read.
    ///
    /// Issue #2404 was a user deleting an agent that did not exist and being
    /// told it worked. #3133 made that an error. A retried *delete* cannot keep
    /// that promise — `delete_worker_internal` opens with a metadata lookup, so
    /// "I already deleted it" and "it was never here" arrive identically — so
    /// `delete` asks first, with a read, and refuses before dispatching
    /// anything.
    ///
    /// The second assertion is the load-bearing one: it is not enough to return
    /// the right error, nothing may go out at all. A dispatched delete is a
    /// delete that can be retried, and a retry is what turns this answer into a
    /// success.
    #[test]
    async fn deleting_an_agent_that_never_existed_still_reports_it_missing() {
        let harness = RestHarness::new(AgentMode::Durable);

        let err = harness
            .worker_service
            .delete(&harness.some_agent_id(), None, AuthCtx::system())
            .await
            .expect_err("deleting an agent that is not there must fail");

        // Both spellings of the same condition: `RecordingWorkerClient` reports a
        // missing agent as `AgentNotFound`, while the real client maps the
        // executor's failure through `err.into()` and lands on `GolemError`.
        assert!(
            matches!(
                err,
                WorkerServiceError::AgentNotFound(_)
                    | WorkerServiceError::GolemError(WorkerExecutorError::AgentNotFound { .. })
            ),
            "expected AgentNotFound, got {err:?}"
        );
        assert!(
            harness
                .worker_client
                .deleted_agent_ids
                .lock()
                .unwrap()
                .is_empty(),
            "the read already answered, so no delete may be dispatched to be retried"
        );
    }

    /// The other half: an agent that is there is still deleted, and the delete
    /// really does reach the client rather than being swallowed by the check.
    #[test]
    async fn deleting_an_existing_agent_dispatches_the_delete() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(ComponentRevision::INITIAL);
        let agent_id = harness.some_agent_id();

        harness
            .worker_service
            .delete(&agent_id, None, AuthCtx::system())
            .await
            .unwrap();

        assert_eq!(
            *harness.worker_client.deleted_agent_ids.lock().unwrap(),
            vec![agent_id.agent_id]
        );
    }

    async fn tool_description_harness() -> (RestHarness, Arc<StaticComponentService>) {
        use golem_common::model::tool::{
            CompiledToolBinding, RegisteredTool, SecretKeyScope, ToolDeploymentState,
            ToolProvisionConfig, ToolSource,
        };
        use golem_common::model::tool_middleware::CompiledToolMiddlewareChain;
        use golem_common::schema::tool::{CommandNode, CommandTree, Doc, Globals, Tool};
        use std::collections::BTreeMap;

        let mut harness =
            RestHarness::new_with_pinned_and_latest_output(OutputSchema::Unit, OutputSchema::Unit);
        let components = harness
            .worker_service
            .component_service
            .get_all_revisions(harness.component_id)
            .await
            .unwrap();
        let component = components.first().unwrap();
        let tool_name = ToolName::try_from("weather").unwrap();
        let definition = Tool {
            version: "1.0.0".into(),
            requires_filesystem: false,
            http_mount: None,
            schema: SchemaGraph::empty(),
            commands: CommandTree {
                nodes: vec![CommandNode {
                    name: "weather".into(),
                    aliases: vec![],
                    doc: Doc::default(),
                    http: None,
                    globals: Globals::default(),
                    subcommands: vec![],
                    body: None,
                }],
            },
        };
        let registered = RegisteredTool {
            type_id: golem_common::model::type_id::TypeId::try_from(1_u32).unwrap(),
            registration: golem_common::model::tool::ToolRegistration {
                deployment_revision: DeploymentRevision::INITIAL,
                release_id: None,
                definition: definition.clone(),
                provision: ToolProvisionConfig::default(),
                component_bindings: BTreeMap::new(),
                source: ToolSource::Component {
                    component_id: component.id,
                    component_revision: component.revision,
                    component_name: component.component_name.clone(),
                },
                owner_account_id: component.account_id,
                owner_account_email: component.account_email.clone(),
                metadata_version: "0.1.0".into(),
                metadata_digest: Default::default(),
            },
        };
        let mut state = ToolDeploymentState {
            deployment_revision: DeploymentRevision::INITIAL,
            registered_tools: BTreeMap::from([(tool_name.clone(), registered.clone())]),
            tool_bindings: BTreeMap::new(),
            mcp_imports: Vec::new(),
            registered_tool_middlewares: BTreeMap::new(),
            tool_middleware_chains: BTreeMap::new(),
            tool_middleware_configuration: Default::default(),
        };
        for (owner, summary) in [
            (
                ToolBindingOwner::ComponentBaseline {
                    component_id: component.id,
                },
                "component middleware",
            ),
            (
                ToolBindingOwner::AgentType {
                    agent_type_name: harness.agent_type_name.clone(),
                },
                "agent middleware",
            ),
        ] {
            let binding = CompiledToolBinding {
                deployment_revision: state.deployment_revision,
                release_id: None,
                owner: owner.clone(),
                tool_name: tool_name.clone(),
                version: definition.version.clone(),
                metadata_version: registered.registration.metadata_version.clone(),
                metadata_digest: Default::default(),
                account_id: component.account_id,
                account_email: component.account_email.clone(),
                parameters: NormalizedJsonValue::new(serde_json::json!({})),
                config_keys_readable: Default::default(),
                secret_keys_readable: SecretKeyScope::All,
                secret_keys_revealable: SecretKeyScope::All,
                filesystem_access: Default::default(),
                source: registered.registration.source.clone(),
                shell: false,
            };
            let mut effective_definition = definition.clone();
            effective_definition.commands.nodes[0].doc.summary = summary.into();
            state.tool_bindings.insert(
                owner.clone(),
                BTreeMap::from([(tool_name.clone(), binding)]),
            );
            state.tool_middleware_chains.insert(
                owner.clone(),
                BTreeMap::from([(
                    tool_name.clone(),
                    CompiledToolMiddlewareChain {
                        deployment_revision: state.deployment_revision,
                        owner,
                        tool_name: tool_name.clone(),
                        effective_definition,
                        occurrences: vec![],
                    },
                )]),
            );
        }
        let service = Arc::new(StaticComponentService {
            components,
            tool_state: Some(state),
            tool_queries: Mutex::new(vec![]),
            revision_gate: Mutex::new(None),
        });
        harness.worker_service.component_service = service.clone();
        (harness, service)
    }

    #[test]
    async fn describe_tool_uses_effective_owner_definition_without_invoking() {
        let (harness, components) = tool_description_harness().await;
        let request = NativeToolDescribeRequest {
            agent_id: None,
            component_id: Some(harness.component_id),
            tool_name: "weather".into(),
        };
        let described = harness
            .worker_service
            .describe_tool_rest(harness.environment_id, request.clone(), AuthCtx::system())
            .await
            .unwrap();
        assert_eq!(
            described.definition.commands.nodes[0].doc.summary,
            "component middleware"
        );
        assert_eq!(described.component_revision, harness.component_revision);
        harness
            .worker_client
            .set_metadata_component_revision(ComponentRevision::INITIAL);
        let mut agent_request = request;
        agent_request.component_id = None;
        agent_request.agent_id = Some(harness.keyed_agent_id());
        let described = harness
            .worker_service
            .describe_tool_rest(harness.environment_id, agent_request, AuthCtx::system())
            .await
            .unwrap();
        assert_eq!(
            described.definition.commands.nodes[0].doc.summary,
            "agent middleware"
        );
        assert_eq!(described.component_revision, ComponentRevision::INITIAL);
        assert_eq!(
            components
                .tool_queries
                .lock()
                .unwrap()
                .iter()
                .map(|(_, _, r)| *r)
                .collect::<Vec<_>>(),
            [harness.component_revision, ComponentRevision::INITIAL]
        );
        assert!(
            harness
                .worker_client
                .invocation_session_starts
                .lock()
                .unwrap()
                .is_empty()
        );
        assert!(harness.worker_client.invocations.lock().unwrap().is_empty());
    }

    #[test]
    async fn describe_tool_rejects_unauthorized_reserved_and_unbound_targets() {
        let (mut harness, components) = tool_description_harness().await;
        let mut request = NativeToolDescribeRequest {
            agent_id: None,
            component_id: Some(harness.component_id),
            tool_name: "weather".into(),
        };
        let auth = AuthCtx::agent_with_effective_surface(
            AccountId(Uuid::new_v4()),
            AccountEmail::new("denied@example.com"),
            EffectiveSurface {
                source_card_ids: vec![],
                lower: vec![],
                upper: vec![],
            },
        );
        assert!(
            harness
                .worker_service
                .describe_tool_rest(harness.environment_id, request.clone(), auth)
                .await
                .is_err()
        );
        assert!(components.tool_queries.lock().unwrap().is_empty());
        assert!(harness.worker_client.effects.lock().unwrap().is_empty());
        request.agent_id = Some(AgentId::external_tool_owner(
            harness.environment_id,
            harness.component_id,
            &IdempotencyKey::fresh(),
        ));
        request.component_id = None;
        assert!(
            harness
                .worker_service
                .describe_tool_rest(harness.environment_id, request.clone(), AuthCtx::system())
                .await
                .is_err()
        );
        assert!(components.tool_queries.lock().unwrap().is_empty());
        let mut state = components.tool_state.clone().unwrap();
        state
            .tool_bindings
            .remove(&ToolBindingOwner::ComponentBaseline {
                component_id: harness.component_id,
            });
        harness.worker_service.component_service = Arc::new(StaticComponentService {
            components: components.components.clone(),
            tool_state: Some(state),
            tool_queries: Mutex::new(vec![]),
            revision_gate: Mutex::new(None),
        });
        request.agent_id = None;
        request.component_id = Some(harness.component_id);
        assert!(
            harness
                .worker_service
                .describe_tool_rest(harness.environment_id, request, AuthCtx::system())
                .await
                .is_err()
        );
        assert!(
            harness
                .worker_client
                .invocation_session_starts
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    async fn native_tool_fresh_owner_uses_reserved_identity_and_native_wire_shape() {
        let harness = RestHarness::new(AgentMode::Durable);
        let mut request = harness.tool_request();
        request.command_path = vec!["now".to_string()];
        let key = RestHarness::tool_key();
        harness
            .worker_client
            .set_invocation_output(AgentInvocationOutput {
                result: golem_common::model::AgentInvocationResult::ExternalTool {
                    result: Ok(
                        golem_common::model::tool::SerializableToolInvocationResult {
                            result: Some(Box::new(empty_constructor_parameters())),
                        },
                    ),
                },
                consumed_fuel: None,
                invocation_status: None,
                component_revision: Some(harness.component_revision),
                agent_id: None,
                idempotency_key: None,
                oplog_index: Some(OplogIndex::from_u64(7)),
                agent_fingerprint: Some(harness.worker_client.fingerprint),
                background_failure: None,
            });

        let response = harness
            .worker_service
            .invoke_tool_rest(
                harness.environment_id,
                request,
                Some(key.clone()),
                AuthCtx::system(),
            )
            .await
            .unwrap();
        let starts = harness
            .worker_client
            .invocation_session_starts
            .lock()
            .unwrap();
        let (target, start) = &starts[0];
        let prepared = InvocationRequestKey::try_from(start.request_key.clone().unwrap()).unwrap();
        assert!(matches!(prepared, InvocationRequestKey::Unkeyed(_)));
        assert_eq!(start.idempotency_key, Some(key.into()));
        assert_eq!(
            *target,
            AgentId::external_tool_owner(
                harness.environment_id,
                harness.component_id,
                &prepared.owner_key("tool:weather/current")
            )
        );
        assert_eq!(target.type_id(), None);
        assert_eq!(start.target_agent_id(), Some(&(*target).into()));
        assert_eq!(
            start
                .component_id
                .map(|id| ComponentId::try_from(id).unwrap()),
            Some(harness.component_id)
        );
        assert!(start.constructor.is_none());
        assert!(start.method_name.is_none());
        assert!(start.input.is_none());
        assert!(start.expected_callee_fingerprint.is_none());
        let tool = start.external_tool.as_ref().unwrap();
        assert!(tool.fresh_owner);
        assert!(!tool.stdin && !tool.stdout);
        assert_eq!(tool.tool_name, "weather");
        assert_eq!(tool.command_path, ["current"]);
        assert!(matches!(
            response.result,
            Some(NativeToolResult::Success(_))
        ));
        assert_eq!(response.oplog_index, Some(OplogIndex::from_u64(7)));
    }

    #[test]
    async fn native_tool_existing_owner_requires_metadata_and_pins_fingerprint_for_lookup() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let mut request = harness.tool_request();
        let existing = harness.keyed_agent_id();
        request.agent_id = Some(existing);
        request.component_id = None;
        request.mode = NativeToolInvocationMode::Lookup;
        request.input = None;

        harness
            .worker_service
            .invoke_tool_rest(
                harness.environment_id,
                request,
                Some(IdempotencyKey::fresh()),
                AuthCtx::system(),
            )
            .await
            .unwrap();
        let starts = harness
            .worker_client
            .invocation_session_starts
            .lock()
            .unwrap();
        let (target, start) = &starts[0];
        assert_eq!(target, &existing);
        assert_eq!(
            start.expected_callee_fingerprint.map(Uuid::from),
            Some(harness.worker_client.fingerprint.0)
        );
        assert_eq!(
            start.mode,
            golem_api_grpc::proto::golem::worker::AgentInvocationMode::Lookup as i32
        );
        assert!(!start.external_tool.as_ref().unwrap().fresh_owner);
    }

    #[test]
    async fn native_tool_lookup_preserves_final_virtual_owner_and_key() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let mut request = harness.tool_request();
        let final_owner = AgentId::external_tool_owner(
            harness.environment_id,
            harness.component_id,
            &IdempotencyKey::from("prepared-owner-seed"),
        );
        let final_key = IdempotencyKey::fresh();
        request.agent_id = Some(final_owner);
        request.component_id = None;
        request.mode = NativeToolInvocationMode::Lookup;
        request.input = None;
        harness
            .worker_service
            .invoke_tool_rest(
                harness.environment_id,
                request,
                Some(final_key.clone()),
                AuthCtx::System,
            )
            .await
            .unwrap();
        let (target, start) = harness.worker_client.invocation_session_start();
        assert_eq!(target, final_owner);
        assert_eq!(start.target_agent_id(), Some(&final_owner.into()));
        assert!(
            start.component_id.is_none(),
            "a lookup never creates its owner"
        );
        assert_eq!(start.idempotency_key, Some(final_key.into()));
        assert_eq!(
            start.expected_callee_fingerprint.map(Uuid::from),
            Some(harness.worker_client.fingerprint.0)
        );
        assert!(!start.external_tool.unwrap().fresh_owner);
        assert_eq!(
            *harness.worker_client.effects.lock().unwrap(),
            vec!["metadata"]
        );
    }

    #[test]
    async fn native_tool_rejects_a_virtual_existing_owner_before_dispatch() {
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let mut request = harness.tool_request();
        request.agent_id = Some(AgentId::external_tool_owner(
            harness.environment_id,
            harness.component_id,
            &IdempotencyKey::fresh(),
        ));
        request.component_id = None;

        let result = harness
            .worker_service
            .invoke_tool_rest(
                harness.environment_id,
                request,
                Some(RestHarness::tool_key()),
                AuthCtx::system(),
            )
            .await;
        assert!(
            matches!(result, Err(WorkerServiceError::TypeChecker(_))),
            "{result:?}"
        );
        assert_eq!(
            *harness.worker_client.effects.lock().unwrap(),
            vec!["metadata"]
        );
        assert!(
            harness
                .worker_client
                .invocation_session_starts
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    async fn native_tool_denial_happens_before_metadata_or_dispatch() {
        let harness = RestHarness::new(AgentMode::Durable);
        let auth = AuthCtx::agent_with_effective_surface(
            AccountId(Uuid::new_v4()),
            AccountEmail::new("unauthorized-tool@golem"),
            EffectiveSurface {
                source_card_ids: vec![],
                lower: vec![],
                upper: vec![],
            },
        );

        assert!(
            harness
                .worker_service
                .invoke_tool_rest(
                    harness.environment_id,
                    harness.tool_request(),
                    Some(RestHarness::tool_key()),
                    auth,
                )
                .await
                .is_err()
        );
        assert!(harness.worker_client.effects.lock().unwrap().is_empty());
        assert!(
            harness
                .worker_client
                .invocation_session_starts
                .lock()
                .unwrap()
                .is_empty()
        );
    }

    fn invocation_summary_proto(
        invocation_id: golem_common::model::InvocationId,
        state: golem_api_grpc::proto::golem::workerexecutor::v1::InvocationLifecycleState,
        kind: golem_api_grpc::proto::golem::workerexecutor::v1::InvocationKind,
        operation: Option<String>,
    ) -> golem_api_grpc::proto::golem::workerexecutor::v1::InvocationSummary {
        golem_api_grpc::proto::golem::workerexecutor::v1::InvocationSummary {
            invocation_id: Some(invocation_id.0.into()),
            state: state as i32,
            kind: kind as i32,
            operation,
            changed_at: Some(::prost_types::Timestamp {
                seconds: 1_700_000_000,
                nanos: 0,
            }),
            ..Default::default()
        }
    }

    /// A completed method result whose operation is not known cannot be decoded, so the read
    /// answers the summary with no result instead of failing.
    #[test]
    async fn a_completed_invocation_with_an_unknown_operation_keeps_its_summary() {
        use golem_api_grpc::proto::golem::workerexecutor::v1::{
            InvocationKind as ProtoKind, InvocationLifecycleState as ProtoState,
        };
        let harness = RestHarness::new(AgentMode::Durable);
        harness
            .worker_client
            .set_metadata_component_revision(harness.component_revision);
        let invocation_id = golem_common::model::InvocationId(Uuid::new_v4());
        let owned = golem_common::model::OwnedInvocationId::new(
            harness.some_agent_id().agent_id,
            invocation_id,
        );
        *harness.worker_client.invocation_summary.lock().unwrap() = Some(invocation_summary_proto(
            invocation_id,
            ProtoState::Completed,
            ProtoKind::Unspecified,
            None,
        ));
        harness
            .worker_client
            .set_invocation_output(AgentInvocationOutput {
                result: golem_common::model::AgentInvocationResult::AgentMethod {
                    output: SchemaValue::Tuple { elements: vec![] },
                },
                consumed_fuel: None,
                invocation_status: None,
                component_revision: Some(harness.component_revision),
                agent_id: None,
                idempotency_key: None,
                oplog_index: None,
                agent_fingerprint: None,
                background_failure: None,
            });

        let got = harness
            .worker_service
            .get_invocation(harness.environment_id, &owned, AuthCtx::system())
            .await;
        let got = got.unwrap_or_else(|error| {
            panic!("get of a completed invocation with unknown operation failed: {error}")
        });
        assert!(got.result.is_none());
    }
}
