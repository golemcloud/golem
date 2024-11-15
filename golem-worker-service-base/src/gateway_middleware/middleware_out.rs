use async_trait::async_trait;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
use crate::gateway_middleware::{Cors, HttpMiddleware};

#[async_trait]
pub trait MiddlewareOut<R> {
    async fn process(&self, session_store: GatewaySessionStore, input: &mut R);
}


#[async_trait]
impl MiddlewareOut<poem::Response> for Cors {
    async fn process(&self, input: &mut poem::Response) {
        HttpMiddleware::apply_cors(input, self)
    }
}