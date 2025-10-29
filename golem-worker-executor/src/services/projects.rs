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

use crate::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError};
use crate::services::golem_config::ProjectServiceConfig;
use async_trait::async_trait;
use golem_api_grpc::proto::golem::project::v1::cloud_project_service_client::CloudProjectServiceClient;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{AccountId, ProjectId, RetryConfig};
use golem_common::retries::with_retries;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use http::Uri;
use std::hash::Hash;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use uuid::Uuid;

#[async_trait]
pub trait ProjectService: Send + Sync {
    async fn get_project_owner(
        &self,
        project_id: &ProjectId,
    ) -> Result<AccountId, WorkerExecutorError>;
    async fn resolve_project(
        &self,
        account_id: &AccountId,
        project_name: &str,
    ) -> Result<Option<ProjectId>, WorkerExecutorError>;
}

pub fn configured(config: &ProjectServiceConfig) -> Arc<dyn ProjectService> {
    match config {
        ProjectServiceConfig::Grpc(config) => Arc::new(ProjectServiceGrpc::new(
            config.uri(),
            config
                .access_token
                .parse::<Uuid>()
                .expect("Access token must be an UUID"),
            config.retries.clone(),
            config.connect_timeout,
            config.max_resolved_project_cache_capacity,
            config.cache_time_to_idle,
        )),
        ProjectServiceConfig::Disabled(config) => Arc::new(ProjectServiceDisabled {
            account_id: config.account_id.clone(),
        }),
    }
}

struct ProjectServiceGrpc {
    project_client: GrpcClient<CloudProjectServiceClient<Channel>>,
    retry_config: RetryConfig,
    access_token: Uuid,
    resolved_project_cache: Cache<(AccountId, String), (), Option<ProjectId>, WorkerExecutorError>,
    project_owner_cache: Cache<ProjectId, (), AccountId, WorkerExecutorError>,
}

impl ProjectServiceGrpc {
    pub fn new(
        project_endpoint: Uri,
        access_token: Uuid,
        retry_config: RetryConfig,
        connect_timeout: Duration,
        max_resolved_project_capacity: usize,
        time_to_idle: Duration,
    ) -> Self {
        Self {
            project_client: GrpcClient::new(
                "project_service",
                move |channel| {
                    CloudProjectServiceClient::new(channel)
                        .send_compressed(CompressionEncoding::Gzip)
                        .accept_compressed(CompressionEncoding::Gzip)
                },
                project_endpoint,
                GrpcClientConfig {
                    retries_on_unavailable: retry_config.clone(),
                    connect_timeout,
                },
            ),
            resolved_project_cache: create_cache(max_resolved_project_capacity, time_to_idle),
            project_owner_cache: create_cache(max_resolved_project_capacity, time_to_idle),
            access_token,
            retry_config,
        }
    }

    async fn resolve_project_remotely(
        &self,
        account_id: &AccountId,
        project_name: &str,
    ) -> Result<Option<ProjectId>, WorkerExecutorError> {
        use golem_api_grpc::proto::golem::project::v1::{
            get_projects_response, GetProjectsRequest, ProjectError,
        };
        use golem_api_grpc::proto::golem::project::Project as GrpcProject;

        let client = self.project_client.clone();
        let retry_config = self.retry_config.clone();
        let access_token = self.access_token;

        fn get_account(project: &GrpcProject) -> AccountId {
            project
                .data
                .clone()
                .expect("did not receive account data")
                .owner_account_id
                .expect("failed to receive project owner_account_id")
                .into()
        }

        let account_id = account_id.clone();
        let project_name = project_name.to_string();

        with_retries(
            "projects",
            "resolve_project_remotely",
            Some(format!("{account_id}/{project_name}").to_string()),
            &retry_config,
            &(
                client,
                account_id.clone(),
                project_name.to_string(),
                access_token,
            ),
            |(client, account_id, project_name, access_token)| {
                Box::pin(async move {
                    let response = client
                        .call("lookup_project_by_name", move |client| {
                            let request = authorised_grpc_request(
                                GetProjectsRequest {
                                    project_name: Some(project_name.to_string()),
                                },
                                access_token,
                            );
                            Box::pin(client.get_projects(request))
                        })
                        .await?
                        .into_inner();

                    match response
                        .result
                        .expect("Didn't receive expected field result")
                    {
                        get_projects_response::Result::Success(payload) => {
                            let project_id = payload
                                .data
                                .into_iter()
                                // TODO: Push account filter to the server
                                .find(|p| get_account(p) == *account_id)
                                .map(|c| c.id.expect("didn't receive expected project_id"));
                            Ok(project_id
                                .map(|c| c.try_into().expect("failed to convert project_id")))
                        }
                        get_projects_response::Result::Error(err) => Err(GrpcError::Domain(err))?,
                    }
                })
            },
            is_grpc_retriable::<ProjectError>,
        )
        .await
        .map_err(|err| WorkerExecutorError::unknown(format!("Failed to get project: {err}")))
    }

    async fn get_project_owner_remotely(
        &self,
        project_id: &ProjectId,
    ) -> Result<AccountId, WorkerExecutorError> {
        use golem_api_grpc::proto::golem::project::v1::{
            get_project_response, GetProjectRequest, ProjectError,
        };

        let client = self.project_client.clone();
        let retry_config = self.retry_config.clone();
        let access_token = self.access_token;

        with_retries(
            "projects",
            "get_project_owner_remotely",
            Some(project_id.to_string()),
            &retry_config,
            &(client, project_id.clone(), access_token),
            |(client, project_id, access_token)| {
                Box::pin(async move {
                    let response = client
                        .call("get_project_owner", move |client| {
                            let request = authorised_grpc_request(
                                GetProjectRequest {
                                    project_id: Some(project_id.clone().into()),
                                },
                                access_token,
                            );
                            Box::pin(client.get_project(request))
                        })
                        .await?
                        .into_inner();

                    match response
                        .result
                        .expect("Didn't receive expected field result")
                    {
                        get_project_response::Result::Success(payload) => Ok(payload
                            .data
                            .expect("didn't receive expected owner_account_id")
                            .owner_account_id
                            .expect("failed to receive project owner_account_id")
                            .into()),
                        get_project_response::Result::Error(err) => Err(GrpcError::Domain(err))?,
                    }
                })
            },
            is_grpc_retriable::<ProjectError>,
        )
        .await
        .map_err(|err| WorkerExecutorError::unknown(format!("Failed to get project owner: {err}")))
    }
}

#[async_trait]
impl ProjectService for ProjectServiceGrpc {
    async fn get_project_owner(
        &self,
        project_id: &ProjectId,
    ) -> Result<AccountId, WorkerExecutorError> {
        self.project_owner_cache
            .get_or_insert_simple(project_id, || {
                Box::pin(self.get_project_owner_remotely(project_id))
            })
            .await
            .map_err(|e| WorkerExecutorError::unknown(format!("Failed to get project owner: {e}")))
    }

    async fn resolve_project(
        &self,
        account_id: &AccountId,
        project_name: &str,
    ) -> Result<Option<ProjectId>, WorkerExecutorError> {
        self.resolved_project_cache
            .get_or_insert_simple(&(account_id.clone(), project_name.to_string()), || {
                Box::pin(self.resolve_project_remotely(account_id, project_name))
            })
            .await
    }
}

fn create_cache<K, V>(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<K, (), V, WorkerExecutorError>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    Cache::new(
        Some(max_capacity),
        FullCacheEvictionMode::LeastRecentlyUsed(1),
        BackgroundEvictionMode::OlderThan {
            ttl: time_to_idle,
            period: Duration::from_secs(60),
        },
        "resolved_project",
    )
}

/// Project service implementation that accepts arbitrary Project IDs and
/// treats them as existing projects belonging to the same account, with a name corresponding to the ID itself.
struct ProjectServiceDisabled {
    account_id: AccountId,
}

#[async_trait]
impl ProjectService for ProjectServiceDisabled {
    async fn get_project_owner(
        &self,
        _project_id: &ProjectId,
    ) -> Result<AccountId, WorkerExecutorError> {
        Ok(self.account_id.clone())
    }

    async fn resolve_project(
        &self,
        account_id: &AccountId,
        project_name: &str,
    ) -> Result<Option<ProjectId>, WorkerExecutorError> {
        if account_id != &self.account_id {
            Err(WorkerExecutorError::unknown(
                format!("Account ID passed to ProjectServiceDisabled::resolve_project ({account_id}) does not match the default account")
            ))
        } else if let Ok(project_id) = ProjectId::from_str(project_name) {
            Ok(Some(project_id))
        } else {
            Err(WorkerExecutorError::unknown(
                format!("Project name passed to ProjectServiceDisabled::resolve_project ({project_name}) must be the project id itself")
            ))
        }
    }
}
