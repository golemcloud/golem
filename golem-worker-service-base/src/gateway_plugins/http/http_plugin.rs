use crate::gateway_plugins::http::cors::{CorsPreflight};

#[derive(Debug, Clone, PartialEq)]
pub enum HttpPlugin {
    Cors(CorsPreflight)
}
