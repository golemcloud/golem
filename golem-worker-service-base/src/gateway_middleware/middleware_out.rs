use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::{Cors, HttpMiddleware};
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
pub trait MiddlewareOut<R> {
    async fn process_output(
        &self,
        session_store: &GatewaySessionStore,
        input: &mut R,
    ) -> Result<(), String>;
}

#[async_trait]
impl MiddlewareOut<poem::Response> for Cors {
    async fn process_output(
        &self,
        _session_store: &GatewaySessionStore,
        input: &mut poem::Response,
    ) -> Result<(), String> {
        HttpMiddleware::apply_cors(input, self);
        Ok(())
    }
}
