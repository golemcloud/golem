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

use crate::service::agent_resolution_cache::AgentResolutionCache;
use crate::service::auth::{AuthService, AuthServiceError};
use crate::service::component::{ComponentService, ComponentServiceError};
use crate::service::limit::{LimitService, LimitServiceError};
use crate::service::worker::{WorkerClient, WorkerResult, WorkerService, WorkerStream};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::Utc;
use futures::Stream;
use golem_api_grpc::proto::golem::worker::{InvocationContext, LogEvent};
use golem_common::model::AgentInvocationOutput;
use golem_common::model::account::AccountId;
use golem_common::model::agent::{AgentMode, AgentType, AgentTypeName};
use golem_common::model::agent::{NamedElementSchemas, Snapshotting};
use golem_common::model::application::ApplicationId;
use golem_common::model::auth::EnvironmentRole;
use golem_common::model::component::{
    CanonicalFilePath, ComponentId, ComponentName, ComponentRevision, PluginPriority,
};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::diff::Hash;
use golem_common::model::environment::EnvironmentId;
use golem_common::model::oplog::{OplogCursor, OplogIndex};
use golem_common::model::worker::{AgentConfigEntryDto, AgentMetadataDto, RevertWorkerTarget};
use golem_common::model::{AgentFilter, AgentId, IdempotencyKey, ScanCursor};
use golem_service_base::clients::registry::{RegistryService, RegistryServiceError};
use golem_service_base::model::auth::{AuthCtx, AuthDetailsForEnvironment, EnvironmentAction};
use golem_service_base::model::component::Component;
use golem_service_base::model::{ComponentFileSystemNode, GetOplogResponse};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Default)]
struct NoopRegistryService;

#[async_trait]
impl RegistryService for NoopRegistryService {
    async fn authenticate_token(
        &self,
        _: &golem_common::model::auth::TokenSecret,
    ) -> Result<AuthCtx, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_auth_details_for_environment(
        &self,
        _: EnvironmentId,
        _: bool,
        _: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, RegistryServiceError> {
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
    ) -> Result<golem_service_base::model::AccountResourceLimits, RegistryServiceError> {
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
    ) -> Result<Vec<golem_common::model::agent::RegisteredAgentType>, RegistryServiceError> {
        unimplemented!()
    }

    async fn get_agent_type(
        &self,
        _: EnvironmentId,
        _: ComponentId,
        _: ComponentRevision,
        _: &AgentTypeName,
    ) -> Result<golem_common::model::agent::RegisteredAgentType, RegistryServiceError> {
        unimplemented!()
    }

    async fn resolve_agent_type_by_names(
        &self,
        _: &golem_common::model::application::ApplicationName,
        _: &golem_common::model::environment::EnvironmentName,
        _: &AgentTypeName,
        _: Option<golem_common::model::deployment::DeploymentRevision>,
        _: Option<&str>,
        _: &AuthCtx,
    ) -> Result<golem_common::model::agent::ResolvedAgentType, RegistryServiceError> {
        unimplemented!()
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
    component: Component,
}

#[async_trait]
impl ComponentService for StaticComponentService {
    async fn get_latest_by_id_in_cache(&self, component_id: ComponentId) -> Option<Component> {
        (self.component.id == component_id).then(|| self.component.clone())
    }

    async fn get_latest_by_id_uncached(
        &self,
        component_id: ComponentId,
    ) -> Result<Component, ComponentServiceError> {
        if self.component.id == component_id {
            Ok(self.component.clone())
        } else {
            Err(ComponentServiceError::ComponentNotFound)
        }
    }

    async fn get_revision(
        &self,
        component_id: ComponentId,
        component_revision: ComponentRevision,
    ) -> Result<Component, ComponentServiceError> {
        if self.component.id == component_id && self.component.revision == component_revision {
            Ok(self.component.clone())
        } else {
            Err(ComponentServiceError::ComponentNotFound)
        }
    }

    async fn get_all_revisions(
        &self,
        component_id: ComponentId,
    ) -> Result<Vec<Component>, ComponentServiceError> {
        if self.component.id == component_id {
            Ok(vec![self.component.clone()])
        } else {
            Err(ComponentServiceError::ComponentNotFound)
        }
    }
}

struct AllowAllAuthService {
    account_id: AccountId,
}

#[async_trait]
impl AuthService for AllowAllAuthService {
    async fn authenticate_token(
        &self,
        _: golem_common::model::auth::TokenSecret,
    ) -> Result<AuthCtx, AuthServiceError> {
        unimplemented!()
    }

    async fn authorize_environment_actions(
        &self,
        _: EnvironmentId,
        _: EnvironmentAction,
        _: &AuthCtx,
    ) -> Result<AuthDetailsForEnvironment, AuthServiceError> {
        Ok(AuthDetailsForEnvironment {
            account_id_owning_environment: self.account_id,
            environment_roles_from_shares: BTreeSet::<EnvironmentRole>::new(),
        })
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
    agent_ids: Arc<Mutex<Vec<AgentId>>>,
    invocation_output: AgentInvocationOutput,
}

#[async_trait]
impl WorkerClient for RecordingWorkerClient {
    async fn create(
        &self,
        _: &AgentId,
        _: HashMap<String, String>,
        _: BTreeMap<String, String>,
        _: Vec<AgentConfigEntryDto>,
        _: bool,
        _: AccountId,
        _: EnvironmentId,
        _: AuthCtx,
        _: Option<InvocationContext>,
        _: Option<golem_api_grpc::proto::golem::component::Principal>,
    ) -> WorkerResult<AgentId> {
        unimplemented!()
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
        _: &AgentId,
        _: EnvironmentId,
        _: AuthCtx,
    ) -> WorkerResult<AgentMetadataDto> {
        unimplemented!()
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

    async fn resume(&self, _: &AgentId, _: bool, _: EnvironmentId, _: AuthCtx) -> WorkerResult<()> {
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
    ) -> Result<GetOplogResponse, crate::service::worker::WorkerServiceError> {
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
    ) -> Result<GetOplogResponse, crate::service::worker::WorkerServiceError> {
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

    async fn get_file_contents(
        &self,
        _: &AgentId,
        _: CanonicalFilePath,
        _: EnvironmentId,
        _: AccountId,
        _: AuthCtx,
    ) -> WorkerResult<Pin<Box<dyn Stream<Item = WorkerResult<Bytes>> + Send + 'static>>> {
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
        _: Option<golem_api_grpc::proto::golem::component::UntypedDataValue>,
        _: i32,
        _: Option<::prost_types::Timestamp>,
        _: Option<IdempotencyKey>,
        _: Option<InvocationContext>,
        _: EnvironmentId,
        _: AccountId,
        _: AuthCtx,
        _: golem_api_grpc::proto::golem::component::Principal,
    ) -> WorkerResult<AgentInvocationOutput> {
        self.agent_ids.lock().unwrap().push(agent_id.clone());
        Ok(self.invocation_output.clone())
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

pub(crate) struct InvocationHarness {
    pub(crate) worker_service: Arc<WorkerService>,
    pub(crate) component_id: ComponentId,
    pub(crate) environment_id: EnvironmentId,
    pub(crate) account_id: AccountId,
    agent_ids: Arc<Mutex<Vec<AgentId>>>,
}

impl InvocationHarness {
    pub(crate) fn new(invocation_output: AgentInvocationOutput) -> Self {
        let component_id = ComponentId(Uuid::new_v4());
        let environment_id = EnvironmentId(Uuid::new_v4());
        let account_id = AccountId(Uuid::new_v4());
        let component = Component {
            id: component_id,
            revision: ComponentRevision::INITIAL,
            environment_id,
            component_name: ComponentName("mcp-component".to_string()),
            hash: Hash::empty(),
            application_id: ApplicationId(Uuid::new_v4()),
            account_id,
            component_size: 0,
            metadata: ComponentMetadata::from_parts(
                vec![],
                vec![],
                None,
                None,
                vec![AgentType {
                    type_name: AgentTypeName("mcp-agent".to_string()),
                    description: String::new(),
                    source_language: String::new(),
                    constructor: golem_common::model::agent::AgentConstructor {
                        name: None,
                        description: String::new(),
                        prompt_hint: None,
                        input_schema: golem_common::model::agent::DataSchema::Tuple(
                            NamedElementSchemas::empty(),
                        ),
                    },
                    methods: vec![],
                    dependencies: vec![],
                    mode: AgentMode::Durable,
                    http_mount: None,
                    snapshotting: Snapshotting::Disabled(golem_common::model::Empty {}),
                    config: vec![],
                }],
                BTreeMap::new(),
            ),
            created_at: Utc::now(),
            wasm_hash: Hash::empty(),
            object_store_key: String::new(),
        };
        let agent_ids = Arc::new(Mutex::new(Vec::new()));
        let worker_client = Arc::new(RecordingWorkerClient {
            agent_ids: agent_ids.clone(),
            invocation_output,
        });

        Self {
            worker_service: Arc::new(WorkerService::new(
                Arc::new(StaticComponentService { component }),
                Arc::new(AllowAllAuthService { account_id }),
                Arc::new(NoopLimitService),
                worker_client,
                Arc::new(AgentResolutionCache::new(
                    Arc::new(NoopRegistryService),
                    1,
                    Duration::from_secs(60),
                    Duration::from_secs(60),
                )),
            )),
            component_id,
            environment_id,
            account_id,
            agent_ids,
        }
    }

    pub(crate) fn recorded_agent_id(&self) -> AgentId {
        self.agent_ids.lock().unwrap()[0].clone()
    }
}

pub(crate) fn phantom_id(agent_id: &AgentId) -> Option<Uuid> {
    agent_id
        .agent_id
        .rsplit_once('[')
        .and_then(|(_, phantom_id)| phantom_id.strip_suffix(']'))
        .and_then(|phantom_id| Uuid::parse_str(phantom_id).ok())
}
