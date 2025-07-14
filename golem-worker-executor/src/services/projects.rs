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

use crate::error::GolemError;
use crate::grpc::{authorised_grpc_request, is_grpc_retriable, GrpcError};
use crate::services::golem_config::{ComponentCacheConfig, ProjectServiceConfig};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::project::v1::cloud_project_service_client::CloudProjectServiceClient;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode, SimpleCache};
use golem_common::client::{GrpcClient, GrpcClientConfig};
use golem_common::model::{AccountId, OwnedWorkerId, ProjectId, RetryConfig, WorkerId};
use golem_common::retries::with_retries;
use http::Uri;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use uuid::Uuid;

#[async_trait]
pub trait ProjectService: Send + Sync {
    async fn get_project_owner(&self, project_id: &ProjectId) -> Result<AccountId, GolemError>;
    async fn resolve_project(
        &self,
        account_id: &AccountId,
        project_name: &str,
    ) -> Result<Option<ProjectId>, GolemError>;
    async fn to_owned_worker_id(&self, worker_id: &WorkerId) -> Result<OwnedWorkerId, GolemError>;
}

pub fn configured(
    config: &ProjectServiceConfig,
    cache_config: &ComponentCacheConfig,
) -> Arc<dyn ProjectService> {
    match config {
        ProjectServiceConfig::Grpc(config) => Arc::new(ProjectServiceGrpc::new(
            config.uri(),
            config
                .access_token
                .parse::<Uuid>()
                .expect("Access token must be an UUID"),
            config.retries.clone(),
            config.connect_timeout,
            cache_config.max_resolved_project_capacity,
            cache_config.time_to_idle,
        )),
        ProjectServiceConfig::Disabled(_) => {
            todo!()
        }
    }
}

struct ProjectServiceGrpc {
    project_client: GrpcClient<CloudProjectServiceClient<Channel>>,
    retry_config: RetryConfig,
    access_token: Uuid,
    resolved_project_cache: Cache<(AccountId, String), (), Option<ProjectId>, GolemError>,
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
            resolved_project_cache: create_resolved_project_cache(
                max_resolved_project_capacity,
                time_to_idle,
            ),
            access_token,
            retry_config,
        }
    }

    async fn resolve_project_remotely(
        &self,
        account_id: &AccountId,
        project_name: &str,
    ) -> Result<Option<ProjectId>, GolemError> {
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
            "component",
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
        .map_err(|err| GolemError::unknown(format!("Failed to get project: {err}")))
    }
}

#[async_trait]
impl ProjectService for ProjectServiceGrpc {
    async fn get_project_owner(&self, project_id: &ProjectId) -> Result<AccountId, GolemError> {
        todo!()
    }

    async fn resolve_project(
        &self,
        account_id: &AccountId,
        project_name: &str,
    ) -> Result<Option<ProjectId>, GolemError> {
        self.resolved_project_cache
            .get_or_insert_simple(&(account_id.clone(), project_name.to_string()), || {
                Box::pin(self.resolve_project_remotely(account_id, project_name))
            })
            .await
    }

    async fn to_owned_worker_id(&self, worker_id: &WorkerId) -> Result<OwnedWorkerId, GolemError> {
        todo!()
    }
}

fn create_resolved_project_cache(
    max_capacity: usize,
    time_to_idle: Duration,
) -> Cache<(AccountId, String), (), Option<ProjectId>, GolemError> {
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
