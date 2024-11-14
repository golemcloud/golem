use std::collections::HashMap;
use std::sync::Arc;
use async_trait::async_trait;
use tokio::sync::Mutex;
use crate::gateway_middleware::AuthCallBackDetails;
use crate::gateway_security::security_scheme::SchemeIdentifier;

#[async_trait]
pub trait SecuritySchemeStore<Namespace> {
    async fn get_security_scheme(&self, security_scheme_name: &SchemeIdentifier, namesapce: ) -> Option<AuthCallBackDetails>;
    async fn get_security_scheme_name(&self, security_scheme_name: &SchemeIdentifier) -> Option<AuthCallBackDetails>;
    async fn get_security_schemes(&self) -> Vec<AuthCallBackDetails>;
}

pub struct InMemorySecuritySchemeStore {
    security_schemes: Arc<Mutex<HashMap<SchemeIdentifier, AuthCallBackDetails>>>
}

impl InMemorySecuritySchemeStore {
    pub fn new() -> Self {
        InMemorySecuritySchemeStore {
            security_schemes: Arc::new(Mutex::new(HashMap::new()))
        }
    }
}

#[async_trait]
impl SecuritySchemeStore for InMemorySecuritySchemeStore {
    async fn get_security_scheme(&self, security_scheme_name: &SchemeIdentifier) -> Option<AuthCallBackDetails> {
        let security_schemes = self.security_schemes.lock().await;
        security_schemes.get(security_scheme_name).cloned()
    }

    async fn get_security_scheme_name(&self, security_scheme_name: &SchemeIdentifier) -> Option<AuthCallBackDetails> {
        let security_schemes = self.security_schemes.lock().await;
        security_schemes.get(security_scheme_name).cloned()
    }

    async fn get_security_schemes(&self) -> Vec<AuthCallBackDetails> {
        let security_schemes = self.security_schemes.lock().await;
        security_schemes.values().cloned().collect()
    }
}