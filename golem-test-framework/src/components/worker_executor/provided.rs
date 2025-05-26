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

use crate::components::worker_executor::{new_client_lazy, WorkerExecutor};
use async_trait::async_trait;
use golem_api_grpc::proto::golem::workerexecutor::v1::worker_executor_client::WorkerExecutorClient;
use tonic::transport::Channel;
use tracing::info;

pub struct ProvidedWorkerExecutor {
    host: String,
    http_port: u16,
    grpc_port: u16,
    client: Option<WorkerExecutorClient<Channel>>,
}

impl ProvidedWorkerExecutor {
    pub fn new(host: String, http_port: u16, grpc_port: u16, shared_client: bool) -> Self {
        info!("Using already running golem-worker-executor on {host}, http port: {http_port}, grpc port: {grpc_port}");
        Self {
            host: host.clone(),
            http_port,
            grpc_port,
            client: if shared_client {
                Some(new_client_lazy(&host, grpc_port).expect("Failed to create client"))
            } else {
                None
            },
        }
    }
}

#[async_trait]
impl WorkerExecutor for ProvidedWorkerExecutor {
    async fn client(&self) -> crate::Result<WorkerExecutorClient<Channel>> {
        match &self.client {
            Some(client) => Ok(client.clone()),
            None => new_client_lazy(&self.host, self.grpc_port),
        }
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

    async fn kill(&self) {
        panic!("Cannot kill provided worker executor");
    }

    async fn restart(&self) {
        panic!("Cannot restart provided worker-executor");
    }
}
