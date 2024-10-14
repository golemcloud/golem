use async_trait::async_trait;
use std::sync::Arc;
use testcontainers::{ContainerAsync, Image};
use tokio::sync::Mutex;

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
