use crate::gateway_binding::GatewayRequestDetails;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::HttpAuthorizer;
use async_trait::async_trait;
use golem_common::SafeDisplay;

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
    ) -> Result<MiddlewareSuccess<Out>, MiddlewareFailure>;
}

pub enum MiddlewareFailure {
    Unauthorized(String),
    InternalServerError(String),
}

impl SafeDisplay for MiddlewareFailure {
    fn to_safe_string(&self) -> String {
        match self {
            MiddlewareFailure::Unauthorized(msg) => format!("Unauthorized: {}", msg),
            MiddlewareFailure::InternalServerError(msg) => {
                format!("Internal Server Error: {}", msg)
            }
        }
    }
}

pub enum MiddlewareSuccess<Out> {
    PassThrough,
    Redirect(Out),
}

#[async_trait]
impl MiddlewareIn<poem::Response> for HttpAuthorizer {
    async fn process_input(
        &self,
        input: &GatewayRequestDetails,
        session_store: &GatewaySessionStore,
    ) -> Result<MiddlewareSuccess<poem::Response>, MiddlewareFailure> {
        match input {
            GatewayRequestDetails::Http(http_request) => {
                self.apply_http_auth(http_request, session_store).await
            }
        }
    }
}
