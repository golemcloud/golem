use async_trait::async_trait;
use crate::gateway_middleware::AuthCallBackDetails;
use crate::gateway_security::SchemeIdentifier;


#[async_trait]
pub trait SecuritySchemeService<Namespace, AuthCtx> {
    async fn get_security_scheme(
        &self, security_scheme_name: &SchemeIdentifier,
        namespace: Namespace,
        auth_ctx: AuthCtx,
    ) -> Option<AuthCallBackDetails>;
    async fn get_security_scheme_name(&self, namespace: Namespace, auth_ctx: AuthCtx, security_scheme_name: &SchemeIdentifier) -> Option<AuthCallBackDetails>;
    async fn get_security_schemes(&self, namespace: Namespace, auth_ctx: AuthCtx) -> Vec<AuthCallBackDetails>;
}