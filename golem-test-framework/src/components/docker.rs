use async_trait::async_trait;
use std::sync::Arc;
use testcontainers::{ContainerAsync, Image};
use tokio::sync::Mutex;

pub(super) const NETWORK: &str = "golem_test_network";

#[async_trait]
pub trait KillContainer {
    async fn kill(&self, keep: bool);
}

#[async_trait]
impl<I: Image> KillContainer for Arc<Mutex<Option<ContainerAsync<I>>>> {
    async fn kill(&self, keep: bool) {
        if let Some(container) = self.lock().await.take() {
            let id = container.id().to_string();
            if keep {
                container
                    .stop()
                    .await
                    .unwrap_or_else(|_| panic!("Failed to stop container {id}"));
            } else {
                container
                    .rm()
                    .await
                    .unwrap_or_else(|_| panic!("Failed to remove container {id}"));
            }
        }
    }
}

pub(super) async fn get_docker_container_name(container_id: &str) -> String {
    let client = testcontainers::core::client::docker_client_instance().await.expect("Failed to get docker client instance");
    let network = client.inspect_network::<String>(NETWORK, None).await.expect("Failed to get network");
    let containers = network.containers.expect("Containers not found in network");
    let container = containers.get(container_id).expect("Container not found in network");
    container.name.clone().expect("Container name not found")
}
