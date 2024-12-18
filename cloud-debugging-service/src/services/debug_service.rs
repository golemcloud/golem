use async_trait::async_trait;
use golem_worker_executor_base::error::GolemError;
use std::sync::Arc;

#[async_trait]
pub trait DebugService {
    async fn noop(&self) -> Result<(), GolemError>;
}

pub struct DefaultDebugService;

#[async_trait]
impl DebugService for DefaultDebugService {
    async fn noop(&self) -> Result<(), GolemError> {
        Ok(())
    }
}

pub fn configured() -> Arc<dyn DebugService + Send + Sync> {
    Arc::new(DefaultDebugService)
}
