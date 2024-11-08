use crate::gateway_middleware::{CorsPreflight, HttpMiddleware, Middleware};

// Static bindings must NOT contain Rib, in either pre-compiled or raw form,
// as it may introduce unnecessary latency
// in serving the requests when not needed.
// While a middleware can exist within other bindings,
// a middleware by itself can sometimes behave as a static binding.
// This separation is to ensure that no worker is involved in certain requests.
// Example: browser requests for preflights need only what's contained in a middleware and
// don't need to pass through to the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum StaticBinding {
    Middleware(Middleware),
}

impl StaticBinding {
    pub fn from_http_middleware(http_middleware: HttpMiddleware) -> Self {
        StaticBinding::Middleware(Middleware::http(http_middleware))
    }

    pub fn get_cors_preflight(&self) -> Option<CorsPreflight> {
        match self {
            StaticBinding::Middleware(Middleware::Http(HttpMiddleware::Cors(preflight))) => {
                Some(preflight.clone())
            }
        }
    }
}
