 use crate::gateway_middleware::http::cors::{CorsPreflight};

#[derive(Debug, Clone, PartialEq)]

pub enum HttpMiddleware {
    Cors(CorsPreflight)
}

impl HttpMiddleware {
    pub fn cors(cors: &CorsPreflight) -> Self {
        HttpMiddleware::Cors(cors.clone())
    }
}
