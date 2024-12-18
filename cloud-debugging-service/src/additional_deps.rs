use crate::services::debug_service::DebugService;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdditionalDeps {
    _debug_service: Arc<dyn DebugService + Sync + Send>,
}

impl AdditionalDeps {
    pub fn new(debug_service: Arc<dyn DebugService + Sync + Send>) -> Self {
        Self {
            _debug_service: debug_service,
        }
    }
}
