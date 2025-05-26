use crate::auth::AuthService;
use crate::debug_session::DebugSessions;
use std::sync::Arc;

#[derive(Clone)]
pub struct AdditionalDeps {
    auth_service: Arc<dyn AuthService>,
    debug_session: Arc<dyn DebugSessions>,
}

impl AdditionalDeps {
    pub fn new(auth_service: Arc<dyn AuthService>, debug_session: Arc<dyn DebugSessions>) -> Self {
        Self {
            auth_service,
            debug_session,
        }
    }

    pub fn auth_service(&self) -> Arc<dyn AuthService + Sync + Send> {
        self.auth_service.clone()
    }

    pub fn debug_session(&self) -> Arc<dyn DebugSessions + Sync + Send> {
        self.debug_session.clone()
    }
}
