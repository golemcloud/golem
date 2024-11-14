use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use openidconnect::{CsrfToken, Nonce};
use tokio::sync::Mutex;
use crate::gateway_identity_provider::OpenIdClient;

pub trait GatewaySession<Id, Data> {
    async fn insert(&self, key: Id, value: Data) -> Result<(), String>;
    async fn get(&self, key: Id) -> Result<Data, String>;
}

// Should be used only for testing
pub struct InMemoryGatewaySession<Id, Data> {
    data: Arc<Mutex<HashMap<Id, Data>>>,
}

impl<Id: Hash + Eq, Data> InMemoryGatewaySession<Id, Data> {
    pub fn new() -> Self {
        InMemoryGatewaySession {
            data: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub type OpenIdAuthSession = InMemoryGatewaySession<String, SessionParameters>;

// No debug or SafeString or String
#[derive(Clone)]
pub struct SessionParameters {
    client: OpenIdClient,
    csrf_state: CsrfToken,
    nonce: Nonce,
    original_uri: String,
}
