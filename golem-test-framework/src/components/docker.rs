// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use bollard::network::CreateNetworkOptions;
use std::sync::Arc;
use testcontainers::{ContainerAsync, Image};
use tokio::sync::Mutex;

pub(super) struct ContainerHandle<I: Image> {
    container: Mutex<Option<ContainerAsync<I>>>,
    _network: Arc<DockerNetwork>,
}

impl<I: Image> ContainerHandle<I> {
    pub(super) fn new(container_async: ContainerAsync<I>, network: Arc<DockerNetwork>) -> Self {
        Self {
            container: Mutex::new(Some(container_async)),
            _network: network,
        }
    }

    pub(super) async fn stop(&self) {
        let guard = self.container.lock().await;
        if let Some(ref container) = *guard {
            container.stop().await.expect("failed to stop container");
        } else {
            panic!("container was already removed");
        }
    }

    pub(super) async fn start(&self) {
        let guard = self.container.lock().await;
        if let Some(ref container) = *guard {
            container.start().await.expect("failed to start container");
        } else {
            panic!("container was already removed");
        }
    }

    pub(super) async fn restart(&self) {
        let guard = self.container.lock().await;
        if let Some(ref container) = *guard {
            container.stop().await.expect("failed to stop container");
            container
                .start()
                .await
                .expect("failed to start the container again");
        } else {
            panic!("container was already removed");
        }
    }

    pub(super) async fn kill(&self) {
        if let Some(container) = self.container.lock().await.take() {
            let id = container.id().to_string();
            container
                .rm()
                .await
                .unwrap_or_else(|_| panic!("Failed to remove container {id}"));
        }
    }
}

pub struct DockerNetwork(String);

impl DockerNetwork {
    pub fn name(&self) -> String {
        self.0.clone()
    }
}

impl Drop for DockerNetwork {
    fn drop(&mut self) {
        // https://github.com/tokio-rs/tokio/issues/5843
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async {
                let client = testcontainers::core::client::docker_client_instance()
                    .await
                    .expect("Failed to get docker client instance");
                let network_id = self.name();
                let res = client.remove_network(&network_id).await;

                if let Err(error) = res {
                    tracing::warn!("failed to drop network {network_id}: {error}");
                }
            })
        });
    }
}

pub async fn create_docker_test_network() -> DockerNetwork {
    let client = testcontainers::core::client::docker_client_instance()
        .await
        .expect("Failed to get docker client instance");

    let network_name = format!("golem-test-network-{}", uuid::Uuid::new_v4());

    client
        .create_network(CreateNetworkOptions {
            name: network_name.clone(),
            check_duplicate: false,
            driver: "bridge".to_string(),
            ..Default::default()
        })
        .await
        .expect("Failed to create network");

    DockerNetwork(network_name)
}

pub(super) async fn get_docker_container_name(
    network: &DockerNetwork,
    container_id: &str,
) -> String {
    let client = testcontainers::core::client::docker_client_instance()
        .await
        .expect("Failed to get docker client instance");
    let network = client
        .inspect_network::<String>(&network.name(), None)
        .await
        .expect("Failed to get network");
    let containers = network.containers.expect("Containers not found in network");
    let container = containers
        .get(container_id)
        .expect("Container not found in network");
    container.name.clone().expect("Container name not found")
}
