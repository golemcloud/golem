use crate::gateway_plugins::http::cors::{CorsPreflight};

pub enum HttpPlugin {
    Cors(CorsPreflight)
}
