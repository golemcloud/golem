use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::{Cors, HttpMiddleware};
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
pub trait MiddlewareOut<R> {
    async fn process_output(
        &self,
        session_store: &GatewaySessionStore,
        input: &mut R,
    ) -> Result<(), MiddlewareOutError>;
}

pub enum MiddlewareOutError {
    InternalError(String),
}

impl SafeDisplay for MiddlewareOutError {
    fn to_safe_string(&self) -> String {
        match self {
            MiddlewareOutError::InternalError(error) => error.to_string(),
        }
    }
}

#[async_trait]
impl MiddlewareOut<poem::Response> for Cors {
    async fn process_output(
        &self,
        _session_store: &GatewaySessionStore,
        input: &mut poem::Response,
    ) -> Result<(), MiddlewareOutError> {
        HttpMiddleware::apply_cors(input, self);
        Ok(())
    }
}
