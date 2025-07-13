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

use super::AuthServiceGrpcClient;
use super::{
    new_account_grpc_client, new_project_grpc_client, new_token_grpc_client,
    AccountServiceGrpcClient, ProjectServiceGrpcClient, TokenServiceGrpcClient,
};
use super::{new_auth_grpc_client, CloudService};
use crate::components::new_reqwest_client;
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::info;

pub struct ProvidedCloudService {
    host: String,
    http_port: u16,
    grpc_port: u16,
    client_protocol: GolemClientProtocol,
    base_http_client: OnceCell<reqwest::Client>,
    account_grpc_client: OnceCell<AccountServiceGrpcClient<Channel>>,
    token_grpc_client: OnceCell<TokenServiceGrpcClient<Channel>>,
    project_grpc_client: OnceCell<ProjectServiceGrpcClient<Channel>>,
    auth_grpc_client: OnceCell<AuthServiceGrpcClient<Channel>>,
}

impl ProvidedCloudService {
    pub async fn new(
        host: String,
        http_port: u16,
        grpc_port: u16,
        client_protocol: GolemClientProtocol,
    ) -> Self {
        info!("Using already running cloud-service on {host}, http port: {http_port}, grpc port: {grpc_port}");

        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            client_protocol,
            base_http_client: OnceCell::new(),
            account_grpc_client: OnceCell::new(),
            token_grpc_client: OnceCell::new(),
            project_grpc_client: OnceCell::new(),
            auth_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl CloudService for ProvidedCloudService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn account_grpc_client(&self) -> AccountServiceGrpcClient<Channel> {
        self.account_grpc_client
            .get_or_init(async || {
                new_account_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    async fn token_grpc_client(&self) -> TokenServiceGrpcClient<Channel> {
        self.token_grpc_client
            .get_or_init(async || {
                new_token_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    async fn project_grpc_client(&self) -> ProjectServiceGrpcClient<Channel> {
        self.project_grpc_client
            .get_or_init(async || {
                new_project_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    async fn auth_grpc_client(&self) -> AuthServiceGrpcClient<Channel> {
        self.auth_grpc_client
            .get_or_init(async || {
                new_auth_grpc_client(&self.public_host(), self.public_grpc_port()).await
            })
            .await
            .clone()
    }

    fn private_host(&self) -> String {
        self.host.clone()
    }

    fn private_http_port(&self) -> u16 {
        self.http_port
    }

    fn private_grpc_port(&self) -> u16 {
        self.grpc_port
    }

    async fn kill(&self) {}
}
