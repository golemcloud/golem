use crate::gateway_middleware::{CorsPreflight, HttpMiddleware, Middleware};

// Static bindings must NOT contain Rib, in either pre-compiled or raw form,
// as it may introduce unnecessary latency
// in serving the requests when not needed.
// a middleware by itself can sometimes behave as a static binding.
// Note that if middleware act as one of the binding time, it is a singleton `Middleware` and not `Middlewares`.
// This separation is to ensure that 2 things:
// An integration for a request endpoint deals with only 1 backend.
// When no worker is involved, the binding type needs to be explicit that there is no Rib, nor worker involved.
// Example: browser requests for preflights need only what's contained in a pre-flight CORS middleware and
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

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::StaticBinding> for StaticBinding {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::StaticBinding,
    ) -> Result<Self, String> {
        match value.static_binding {
            Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::Middleware(middleware)) => {
                let middleware = middleware.cors;
                if let Some(cors) = middleware {
                    // StaticBinding is about a single middleware
                    Ok(StaticBinding::Middleware(Middleware::Http(HttpMiddleware::Cors(cors.try_into()?))))
                } else {
                    Err("Middleware is not a CORS middleware".to_string())
                }
            }
            _ => Err("Unknown static binding type".to_string()),
        }
    }
}

impl From<StaticBinding> for golem_api_grpc::proto::golem::apidefinition::StaticBinding {
    fn from(value: StaticBinding) -> Self {
        match value {
            StaticBinding::Middleware(Middleware::Http(HttpMiddleware::Cors(cors))) => {
                golem_api_grpc::proto::golem::apidefinition::StaticBinding {
                    static_binding: Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::Middleware(
                        golem_api_grpc::proto::golem::apidefinition::Middleware {
                            cors: Some(golem_api_grpc::proto::golem::apidefinition::CorsPreflight::from(cors)),
                        }
                    )),
                }
            }
        }
    }
}
