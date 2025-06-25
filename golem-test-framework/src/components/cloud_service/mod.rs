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

pub mod docker;
pub mod k8s;
pub mod provided;
pub mod spawned;

use crate::components::rdb::Rdb;
use crate::components::{wait_for_startup_grpc, wait_for_startup_http, EnvVarBuilder};
use crate::config::GolemClientProtocol;
use anyhow::anyhow;
use async_trait::async_trait;
use chrono::{Months, Utc};
pub use golem_api_grpc::proto::golem::account::v1::cloud_account_service_client::CloudAccountServiceClient as AccoutServiceGrpcClient;
use golem_api_grpc::proto::golem::account::v1::AccountCreateRequest;
pub use golem_api_grpc::proto::golem::project::v1::cloud_project_service_client::CloudProjectServiceClient as ProjectServiceGrpcClient;
use golem_api_grpc::proto::golem::project::v1::{
    get_default_project_response, GetDefaultProjectRequest,
};
pub use golem_api_grpc::proto::golem::token::v1::cloud_token_service_client::CloudTokenServiceClient as TokenServiceGrpcClient;
use golem_api_grpc::proto::golem::token::v1::CreateTokenRequest;
use golem_api_grpc::proto::golem::token::CreateTokenDto;
use golem_client::api::TokenClient;
use golem_client::api::{AccountClient, ProjectClient};
use golem_client::{Context, Security};
use golem_common::model::{AccountId, ProjectId};
use golem_service_base::clients::authorised_request;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tonic::codec::CompressionEncoding;
use tonic::transport::Channel;
use tracing::Level;
use url::Url;
use uuid::{uuid, Uuid};

const ADMIN_TOKEN: uuid::Uuid = uuid!("5c832d93-ff85-4a8f-9803-513950fdfdb1");
const ADMIN_ACCOUNT_ID: uuid::Uuid = uuid!("24a9f0e2-f491-4e96-974e-b9fbf78c924e");

#[async_trait]
pub trait CloudService: Send + Sync {
    fn client_protocol(&self) -> GolemClientProtocol;

    async fn base_http_client(&self) -> reqwest::Client;

    async fn account_http_client(&self, token: Uuid) -> golem_client::api::AccountClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        golem_client::api::AccountClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }
    async fn account_grpc_client(&self) -> AccoutServiceGrpcClient<Channel>;

    async fn token_http_client(&self, token: Uuid) -> golem_client::api::TokenClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        golem_client::api::TokenClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }
    async fn token_grpc_client(&self) -> TokenServiceGrpcClient<Channel>;

    async fn project_http_client(&self, token: Uuid) -> golem_client::api::ProjectClientLive {
        let url = format!("http://{}:{}", self.public_host(), self.public_http_port());
        golem_client::api::ProjectClientLive {
            context: Context {
                client: self.base_http_client().await,
                base_url: Url::parse(&url).expect("Failed to parse url"),
                security_token: Security::Bearer(token.to_string()),
            },
        }
    }
    async fn project_grpc_client(&self) -> ProjectServiceGrpcClient<Channel>;

    async fn get_default_project(&self, token: &Uuid) -> crate::Result<ProjectId> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut client = self.project_grpc_client().await;
                let request = authorised_request(GetDefaultProjectRequest {}, token);
                let response = client.get_default_project(request).await?;
                match response.into_inner().result.unwrap() {
                    get_default_project_response::Result::Success(result) => {
                        Ok(result.id.unwrap().try_into().unwrap())
                    }
                    get_default_project_response::Result::Error(error) => Err(anyhow!("{error:?}")),
                }
            }
            GolemClientProtocol::Http => {
                let client = self.project_http_client(*token).await;
                Ok(ProjectId(client.get_default_project().await?.project_id))
            }
        }
    }

    async fn create_account(
        &self,
        token: &Uuid,
        account_data: &golem_client::model::AccountData,
    ) -> crate::Result<AccountWithToken> {
        match self.client_protocol() {
            GolemClientProtocol::Grpc => {
                let mut account_client = self.account_grpc_client().await;
                let mut token_client = self.token_grpc_client().await;

                let account = {
                    let request = authorised_request(
                        AccountCreateRequest {
                            account_data: Some(
                                golem_api_grpc::proto::golem::account::AccountData {
                                    name: account_data.name.clone(),
                                    email: account_data.email.clone(),
                                },
                            ),
                        },
                        token,
                    );

                    let response = account_client.create_account(request).await?.into_inner();

                    match response.result.unwrap() {
                        golem_api_grpc::proto::golem::account::v1::account_create_response::Result::Account(inner) => inner,
                        golem_api_grpc::proto::golem::account::v1::account_create_response::Result::Error(error) => Err(anyhow!("{error:?}"))?
                    }
                };

                let account_id = account.id.unwrap();

                let account_token = {
                    let expires_at = Utc::now()
                        .checked_add_months(Months::new(24))
                        .expect("Failed to construct expiry date");
                    let request = authorised_request(
                        CreateTokenRequest {
                            account_id: Some(account_id.clone()),
                            create_token_dto: Some(CreateTokenDto {
                                expires_at: expires_at.to_rfc3339(),
                            }),
                        },
                        token,
                    );
                    let response = token_client.create_token(request).await?.into_inner();

                    match response.result.unwrap() {
                        golem_api_grpc::proto::golem::token::v1::create_token_response::Result::Success(inner) => inner.secret.unwrap(),
                        golem_api_grpc::proto::golem::token::v1::create_token_response::Result::Error(error) => Err(anyhow!("{error:?}"))?
                    }
                };

                Ok(AccountWithToken {
                    account_id: account_id.into(),
                    token: account_token.value.unwrap().into(),
                })
            }
            GolemClientProtocol::Http => {
                let account_client = self.account_http_client(*token).await;
                let account = account_client.create_account(account_data).await?;

                let token_client = self.token_http_client(*token).await;
                let expires_at = Utc::now()
                    .checked_add_months(Months::new(24))
                    .expect("Failed to construct expiry date");
                let account_token = token_client
                    .create_token(
                        &account.id,
                        &golem_client::model::CreateTokenDto { expires_at },
                    )
                    .await?;

                Ok(AccountWithToken {
                    account_id: AccountId { value: account.id },
                    token: account_token.secret.value,
                })
            }
        }
    }

    fn admin_token(&self) -> Uuid {
        ADMIN_TOKEN
    }

    fn admin_account_id(&self) -> AccountId {
        AccountId {
            value: ADMIN_ACCOUNT_ID.to_string(),
        }
    }

    fn private_host(&self) -> String;
    fn private_http_port(&self) -> u16;
    fn private_grpc_port(&self) -> u16;

    fn public_host(&self) -> String {
        self.private_host()
    }

    fn public_http_port(&self) -> u16 {
        self.private_http_port()
    }

    fn public_grpc_port(&self) -> u16 {
        self.private_grpc_port()
    }

    async fn kill(&self);
}

async fn new_account_grpc_client(host: &str, grpc_port: u16) -> AccoutServiceGrpcClient<Channel> {
    AccoutServiceGrpcClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to cloud-service")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

async fn new_token_grpc_client(host: &str, grpc_port: u16) -> TokenServiceGrpcClient<Channel> {
    TokenServiceGrpcClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to cloud-service")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

async fn new_project_grpc_client(host: &str, grpc_port: u16) -> ProjectServiceGrpcClient<Channel> {
    ProjectServiceGrpcClient::connect(format!("http://{host}:{grpc_port}"))
        .await
        .expect("Failed to connect to cloud-service")
        .send_compressed(CompressionEncoding::Gzip)
        .accept_compressed(CompressionEncoding::Gzip)
}

async fn wait_for_startup(
    protocol: GolemClientProtocol,
    host: &str,
    grpc_port: u16,
    http_port: u16,
    timeout: Duration,
) {
    match protocol {
        GolemClientProtocol::Grpc => {
            wait_for_startup_grpc(host, grpc_port, "cloud-service", timeout).await
        }
        GolemClientProtocol::Http => {
            wait_for_startup_http(host, http_port, "cloud-service", timeout).await
        }
    }
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    verbosity: Level,
    private_rdb_connection: bool,
) -> HashMap<String, String> {
    EnvVarBuilder::golem_service(verbosity)
        .with("GOLEM__ACCOUNTS__ROOT__ID", ADMIN_ACCOUNT_ID.to_string())
        .with("GOLEM__ACCOUNTS__ROOT__TOKEN", ADMIN_TOKEN.to_string())
        .with("GOLEM__GRPC_PORT", grpc_port.to_string())
        .with("GOLEM__HTTP_PORT", http_port.to_string())
        .with("GOLEM__LOGIN__TYPE", "Disabled".to_string())
        .with_all(rdb.info().env("cloud_service", private_rdb_connection))
        .build()
}

pub struct AccountWithToken {
    pub account_id: AccountId,
    pub token: Uuid,
}

pub struct AdminOnlyStubCloudService {
    admin_account_id: AccountId,
    admin_token: Uuid,
    admin_default_project: ProjectId,
}

impl AdminOnlyStubCloudService {
    pub fn new(
        admin_account_id: AccountId,
        admin_token: Uuid,
        admin_default_project: ProjectId,
    ) -> Self {
        Self {
            admin_account_id,
            admin_token,
            admin_default_project,
        }
    }
}

#[async_trait]
impl CloudService for AdminOnlyStubCloudService {
    fn admin_token(&self) -> Uuid {
        self.admin_token
    }

    fn admin_account_id(&self) -> AccountId {
        self.admin_account_id.clone()
    }

    async fn get_default_project(&self, token: &Uuid) -> crate::Result<ProjectId> {
        if *token != self.admin_token {
            Err(anyhow!("StubCloudService received unexpected token"))?
        }
        Ok(self.admin_default_project.clone())
    }

    fn client_protocol(&self) -> GolemClientProtocol {
        panic!("no cloud service running");
    }

    async fn base_http_client(&self) -> reqwest::Client {
        panic!("no cloud service running");
    }

    async fn account_grpc_client(&self) -> AccoutServiceGrpcClient<Channel> {
        panic!("no cloud service running");
    }

    async fn token_grpc_client(&self) -> TokenServiceGrpcClient<Channel> {
        panic!("no cloud service running");
    }

    async fn project_grpc_client(&self) -> ProjectServiceGrpcClient<Channel> {
        panic!("no cloud service running");
    }

    fn private_host(&self) -> String {
        panic!("no cloud service running");
    }

    fn private_http_port(&self) -> u16 {
        panic!("no cloud service running");
    }

    fn private_grpc_port(&self) -> u16 {
        panic!("no cloud service running");
    }

    async fn kill(&self) {}
}
