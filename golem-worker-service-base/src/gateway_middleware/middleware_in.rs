use crate::gateway_binding::GatewayRequestDetails;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::HttpAuthorizer;
use async_trait::async_trait;

// Implementation note: We have multiple `Middleware` (see `Middlewares`).
// Some middlewares are specific to `MiddlewareIn` or `MiddlewareOut`.
// This separation ensures input middlewares aren't used for outgoing responses, and vice versa.
// When adding new middlewares (see `Middlewares`), we follow the compiler to create the correct instance.
// These middlewares are protocol-independent. `GatewayRequestDetails` serves as input,
// with its enum defining the protocol type. The middleware decides which protocol to process.
// This approach centralizes middleware management, making it easy to add new ones without protocol-specific logic.
// Simply use `middlewares.process_input(gateway_request_details)` for input, and
// `middlewares.process_output(protocol_independent_response)` for output.
#[async_trait]
pub trait MiddlewareIn<Out> {
    async fn process_input(
        &self,
        input: &GatewayRequestDetails,
        session_store: &GatewaySessionStore,
    ) -> Result<MiddlewareResult<Out>, String>;
}

pub enum MiddlewareResult<Out> {
    PassThrough,
    Redirect(Out),
}

pub struct PassThroughMetadata {
    value: serde_json::Value,
}

#[async_trait]
impl MiddlewareIn<poem::Response> for HttpAuthorizer {
    async fn process_input(
        &self,
        input: &GatewayRequestDetails,
        session_store: &GatewaySessionStore,
    ) -> Result<MiddlewareResult<poem::Response>, String> {
        match input {
            GatewayRequestDetails::Http(http_request) => {
                self.apply_http_auth(http_request, session_store).await
            }
            _ => Ok(MiddlewareResult::PassThrough),
        }
    }
}
