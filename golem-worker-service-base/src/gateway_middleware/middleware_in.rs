use crate::gateway_binding::GatewayRequestDetails;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::HttpAuthorizer;
use async_trait::async_trait;
use golem_common::SafeDisplay;

// Implementation note: We have multiple `Middleware` (see `Middlewares`).
// While some middlewares are  specific to `MiddlewareIn` other are specific to `MiddlewareOut`.
// This ensures orthogonality in middleware usages, such that, at type level, we distinguish whether it is
// used only to process input (of api gateway) or used only to manipulate the output (of probably rib evaluation result)
// These middlewares are protocol-independent. It's input `GatewayRequestDetails`,
// with is an enum of different types (protocols) of input the protocol type.
// An `enum` over type parameter is used merely for simplicity.
// The middleware decides which protocol out of `GatewayRequestDetails` to process.
// This approach centralizes middleware management, and easy to add new middlewares without worrying about protocols.
#[async_trait]
pub trait MiddlewareIn<Out> {
    async fn process_input(
        &self,
        input: &GatewayRequestDetails,
        session_store: &GatewaySessionStore,
    ) -> Result<MiddlewareSuccess<Out>, MiddlewareInError>;
}

pub enum MiddlewareInError {
    Unauthorized(String),
    InternalServerError(String),
}

impl SafeDisplay for MiddlewareInError {
    fn to_safe_string(&self) -> String {
        match self {
            MiddlewareInError::Unauthorized(msg) => format!("Unauthorized: {}", msg),
            MiddlewareInError::InternalServerError(msg) => {
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
    ) -> Result<MiddlewareSuccess<poem::Response>, MiddlewareInError> {
        match input {
            GatewayRequestDetails::Http(http_request) => {
                self.apply_http_auth(http_request, session_store).await
            }
        }
    }
}
