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

use std::sync::Arc;
use testcontainers::{ContainerAsync, Image};
use tokio::sync::Mutex;

pub(super) struct ContainerHandle<I: Image>(Arc<Mutex<Option<ContainerAsync<I>>>>);

impl<I: Image> ContainerHandle<I> {
    pub(super) fn new(container_async: ContainerAsync<I>) -> Self {
        Self(Arc::new(Mutex::new(Some(container_async))))
    }

    /// Stops the container and starts it again; its file system, and so its data, is kept.
    pub(super) async fn restart(&self) {
        let guard = self.0.lock().await;
        let container = guard.as_ref().expect("The container was already removed");
        container
            .stop_with_timeout(Some(1))
            .await
            .expect("Failed to stop the container");
        container
            .start()
            .await
            .expect("Failed to start the container again");
    }

    /// Stops the container without removing it; [`Self::start`] starts it again with its data.
    pub(super) async fn stop(&self) {
        let guard = self.0.lock().await;
        let container = guard.as_ref().expect("The container was already removed");
        container
            .stop_with_timeout(Some(1))
            .await
            .expect("Failed to stop the container");
    }

    pub(super) async fn start(&self) {
        let guard = self.0.lock().await;
        let container = guard.as_ref().expect("The container was already removed");
        container
            .start()
            .await
            .expect("Failed to start the container again");
    }

    pub(super) async fn kill(&self) {
        if let Some(container) = self.0.lock().await.take() {
            let id = container.id().to_string();
            container
                .rm()
                .await
                .unwrap_or_else(|_| panic!("Failed to remove container {id}"));
        }
    }
}

pub(super) async fn get_docker_container_name(prefix: &str, container_id: &str) -> String {
    let client = testcontainers::core::client::docker_client_instance()
        .await
        .expect("Failed to get docker client instance");
    let network = client
        .inspect_network(&network(prefix), None)
        .await
        .expect("Failed to get network");
    let containers = network.containers.expect("Containers not found in network");
    let container = containers
        .get(container_id)
        .expect("Container not found in network");
    container.name.clone().expect("Container name not found")
}

pub fn network(prefix: &str) -> String {
    format!("golem_test_network_{}", prefix.replace('-', "_"))
}
