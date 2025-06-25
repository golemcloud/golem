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

use super::wait_for_startup;
use super::CloudService;
use super::{
    new_account_grpc_client, new_project_grpc_client, new_token_grpc_client,
    AccoutServiceGrpcClient, ProjectServiceGrpcClient, TokenServiceGrpcClient,
};
use crate::components::rdb::Rdb;
use crate::components::{new_reqwest_client, ChildProcessLogger};
use crate::config::GolemClientProtocol;
use async_trait::async_trait;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::OnceCell;
use tonic::transport::Channel;
use tracing::info;
use tracing::Level;

pub struct SpawnedCloudService {
    http_port: u16,
    grpc_port: u16,
    child: Arc<Mutex<Option<Child>>>,
    _logger: ChildProcessLogger,
    client_protocol: GolemClientProtocol,
    base_http_client: OnceCell<reqwest::Client>,
    account_grpc_client: OnceCell<AccoutServiceGrpcClient<Channel>>,
    token_grpc_client: OnceCell<TokenServiceGrpcClient<Channel>>,
    project_grpc_client: OnceCell<ProjectServiceGrpcClient<Channel>>,
}

impl SpawnedCloudService {
    pub async fn new(
        executable: &Path,
        working_directory: &Path,
        http_port: u16,
        grpc_port: u16,
        rdb: Arc<dyn Rdb>,
        client_protocol: GolemClientProtocol,
        verbosity: Level,
        out_level: Level,
        err_level: Level,
    ) -> Self {
        info!("Starting cloud-service process");

        if !executable.exists() {
            panic!("Expected to have precompiled cloud-service at {executable:?}");
        }

        let mut child = Command::new(executable)
            .current_dir(working_directory)
            .envs(super::env_vars(http_port, grpc_port, rdb, verbosity, false).await)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to start golem-component-service");

        let logger = ChildProcessLogger::log_child_process(
            "[componentsvc]",
            out_level,
            err_level,
            &mut child,
        );

        wait_for_startup(
            client_protocol,
            "localhost",
            grpc_port,
            http_port,
            Duration::from_secs(90),
        )
        .await;

        Self {
            http_port,
            grpc_port,
            child: Arc::new(Mutex::new(Some(child))),
            _logger: logger,
            client_protocol,
            base_http_client: OnceCell::new(),
            account_grpc_client: OnceCell::new(),
            token_grpc_client: OnceCell::new(),
            project_grpc_client: OnceCell::new(),
        }
    }
}

#[async_trait]
impl CloudService for SpawnedCloudService {
    fn client_protocol(&self) -> GolemClientProtocol {
        self.client_protocol
    }

    async fn base_http_client(&self) -> reqwest::Client {
        self.base_http_client
            .get_or_init(async || new_reqwest_client())
            .await
            .clone()
    }

    async fn account_grpc_client(&self) -> AccoutServiceGrpcClient<Channel> {
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

    fn private_host(&self) -> String {
        "localhost".to_string()
    }

    fn private_http_port(&self) -> u16 {
        self.http_port
    }

    fn private_grpc_port(&self) -> u16 {
        self.grpc_port
    }

    async fn kill(&self) {
        info!("Stopping cloud-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}

impl Drop for SpawnedCloudService {
    fn drop(&mut self) {
        info!("Stopping golem-component-service");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            let _ = child.kill();
        }
    }
}
