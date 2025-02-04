use crate::services::debug_service::DebugService;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdditionalDeps {
    debug_service: Arc<dyn DebugService + Sync + Send>,
}

impl AdditionalDeps {
    pub fn new(debug_service: Arc<dyn DebugService + Sync + Send>) -> Self {
        Self { debug_service }
    }

    pub fn get_debug_service(&self) -> Arc<dyn DebugService + Sync + Send> {
        self.debug_service.clone()
    }
}
