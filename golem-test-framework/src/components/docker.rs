use async_trait::async_trait;
use std::sync::Arc;
use testcontainers::{ContainerAsync, Image};
use tokio::sync::Mutex;

pub(super) const NETWORK: &str = "golem_test_network";

#[async_trait]
pub trait ContainerLifecycle {
    async fn start(&self);
    async fn stop(&self);
    async fn restart(&self);
}

#[async_trait]
impl<I: Image> ContainerLifecycle for Arc<Mutex<ContainerAsync<I>>> {
    async fn stop(&self) {
        let container = self.lock().await;
        container.stop().await.expect("failed to stop container");
    }

    async fn start(&self) {
        let container = self.lock().await;
        container.start().await.expect("failed to start container");
    }

    async fn restart(&self) {
        let container = self.lock().await;
        container.stop().await.expect("failed to stop container");
        container
            .start()
            .await
            .expect("failed to start the container again");
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
