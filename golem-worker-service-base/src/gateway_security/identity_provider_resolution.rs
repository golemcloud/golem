use crate::gateway_security::default_provider::DefaultIdentityProvider;
use crate::gateway_security::{IdentityProvider, Provider};
use std::sync::Arc;

pub trait IdentityProviderResolver {
    fn resolve(&self, provider_type: &Provider) -> Arc<dyn IdentityProvider + Sync + Send>;
}

pub struct DefaultIdentityProviderResolver;

impl IdentityProviderResolver for DefaultIdentityProviderResolver {
    fn resolve(&self, provider_type: &Provider) -> Arc<dyn IdentityProvider + Sync + Send> {
        match provider_type {
            Provider::Google => Arc::new(DefaultIdentityProvider),
            Provider::Facebook => Arc::new(DefaultIdentityProvider),
            Provider::Gitlab => Arc::new(DefaultIdentityProvider),
            Provider::Microsoft => Arc::new(DefaultIdentityProvider),
        }
    }
}
