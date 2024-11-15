use crate::gateway_binding::GatewayRequestDetails;
use crate::gateway_execution::gateway_session::GatewaySessionStore;
pub use http::*;
pub use middleware_in::*;
pub use middleware_out::*;

mod http;
mod middleware_in;
mod middleware_out;

// A set of middleware can exist in a binding.
// These middlewares will be processed in a sequential order.
// The information contained in each middleware is made available to
// the Rib environment as a key-value pair. This implies, users can look up the data
// related to the middleware in their Rib script.
// Also, depending on the middleware type, gateway can make certain decisions
// automatically, such as making sure to add origin header into the response body
// instead of polluting the Rib script when CORS is enabled.
// However, if there are conflicts  (Example: user specified
// a CORS header already, then gateway resolves these conflicts by giving priority to user input)
// In most cases, it is best for users to do every pre-processing of input and forming the shape of response by themselves.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Middlewares(pub Vec<Middleware>);

impl Middlewares {
    pub fn http_middlewares(&self) -> Vec<HttpMiddleware> {
        self.0
            .iter()
            .flat_map(|m| match m {
                Middleware::Http(http_middleware) => Some(http_middleware.clone()),
            })
            .collect()
    }

    pub fn process_middleware_in<R>(
        &self,
        session_store: &GatewaySessionStore,
        input: &GatewayRequestDetails,
    ) where
        HttpAuthorizer: MiddlewareIn<R>,
    {
        for middleware in self.http_middlewares() {
            match middleware {
                HttpMiddleware::AddCorsHeaders(_) => {}
                HttpMiddleware::AuthenticateRequest(auth) => {
                    auth.process_input(input, session_store)
                }
            }
        }
    }

    pub fn process_middleware_out<Out>(
        &self,
        session_store: &GatewaySessionStore,
        response: &mut Out,
    ) where
        Cors: MiddlewareOut<Out>,
    {
        for middleware in self.http_middlewares() {
            match middleware {
                HttpMiddleware::AddCorsHeaders(cors) => cors.process(session_store, response),
                HttpMiddleware::AuthenticateRequest(_) => {}
            }
        }
    }

    pub fn add(&mut self, middleware: Middleware) {
        self.0.push(middleware);
    }

    pub fn get_cors(&self) -> Option<Cors> {
        self.0.iter().find_map(|m| m.get_cors())
    }
}

// A middleware will not add, remove or update the input to worker what-so-ever,
// as Rib is well typed and there wouldn't be any magical pre-processing such as adding a field to the worker input record.
// In other words, users need to satisfy the Rib compiler (to not complain about the input or output of worker) while registering API definition.
// That said, depending on the middleware type, gateway can make certain decisions automatically
// such as adding CORS headers to the http response body instead of asking users to do this everytime the Rib script,
// even after specifying a CORS middleware plugin. However, these automated decisions will still be rare.
// In most cases, it is best for users to do every pre-processing of input and forming the shape of response by themselves,
// as every data related to the configured middleware is made available to Rib compiler.
#[derive(Debug, Clone, PartialEq)]
pub enum Middleware {
    Http(HttpMiddleware),
}

impl Middleware {
    pub fn cors(cors: &Cors) -> Middleware {
        Middleware::Http(HttpMiddleware::cors(cors.clone()))
    }

    pub fn get_cors(&self) -> Option<Cors> {
        match self {
            Middleware::Http(HttpMiddleware::AddCorsHeaders(cors)) => Some(cors.clone()),
        }
    }

    pub fn http(http_middleware: HttpMiddleware) -> Middleware {
        Middleware::Http(http_middleware)
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::Middleware> for Middlewares {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::Middleware,
    ) -> Result<Self, Self::Error> {
        let mut middlewares = Vec::new();
        if let Some(cors) = value.cors {
            let cors = Cors::try_from(cors)?;
            middlewares.push(Middleware::http(HttpMiddleware::cors(cors)));
        }
        Ok(Middlewares(middlewares))
    }
}

impl From<Middlewares> for golem_api_grpc::proto::golem::apidefinition::Middleware {
    fn from(value: Middlewares) -> Self {
        golem_api_grpc::proto::golem::apidefinition::Middleware {
            cors: value.0.iter().find_map(|m| m.get_cors().map(|c| c.into())),
        }
    }
}
