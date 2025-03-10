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

use std::sync::Arc;
use testcontainers::{ContainerAsync, Image};
use tokio::sync::Mutex;

pub(super) const NETWORK: &str = "golem_test_network";

pub(super) struct ContainerHandle<I: Image>(Arc<Mutex<Option<ContainerAsync<I>>>>);

impl<I: Image> ContainerHandle<I> {
    pub(super) fn new(container_async: ContainerAsync<I>) -> Self {
        Self(Arc::new(Mutex::new(Some(container_async))))
    }

    pub(super) async fn stop(&self) {
        let guard = self.0.lock().await;
        if let Some(ref container) = *guard {
            container.stop().await.expect("failed to stop container");
        } else {
            panic!("container was already removed");
        }
    }

    pub(super) async fn start(&self) {
        let guard = self.0.lock().await;
        if let Some(ref container) = *guard {
            container.start().await.expect("failed to start container");
        } else {
            panic!("container was already removed");
        }
    }

    pub(super) async fn restart(&self) {
        let guard = self.0.lock().await;
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
        if let Some(container) = self.0.lock().await.take() {
            let id = container.id().to_string();
            container
                .rm()
                .await
                .unwrap_or_else(|_| panic!("Failed to remove container {id}"));
        }
    }
}

pub(super) async fn get_docker_container_name(container_id: &str) -> String {
    let client = testcontainers::core::client::docker_client_instance()
        .await
        .expect("Failed to get docker client instance");
    let network = client
        .inspect_network::<String>(NETWORK, None)
        .await
        .expect("Failed to get network");
    let containers = network.containers.expect("Containers not found in network");
    let container = containers
        .get(container_id)
        .expect("Container not found in network");
    container.name.clone().expect("Container name not found")
}
