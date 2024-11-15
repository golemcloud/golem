use crate::gateway_binding::{GatewayRequestDetails, HttpRequestDetails};
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::HttpAuth;

trait MiddlewareIn<In, Out> {
    async fn process_input(&self, input: GatewayRequestDetails, session_store: GatewaySessionStore) -> Result<Out, String>;

}

impl MiddlewareIn<HttpRequestDetails, String> for HttpAuth {
    async fn process_input(&self, input: GatewayRequestDetails, session_store: GatewaySessionStore) -> Result<String, String> {
        let identity_provider =
            &self.scheme_internal.identity_provider;
        let client =
            identity_provider.get_client(&self.scheme_internal.security_scheme).map_err(|err| err.to_string())?;
        let identity =
            identity_provider.get_authorization_url(&client, self.get_scopes());



    }
}