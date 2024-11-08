pub(crate) use http::*;
mod http;

// A set of middleware can exist in a binding.
// These middlewares will be processed in a sequential order.
// The information contained in each middleware is made available is also made available to
// the Rib environment as a key-value pair. This implies, users can look up the data
// served by the middleware in the Rib script.
// Also, depending on the middleware type, gateway can make certain decisions
// automatically, such as making sure to add origin header into the response body
// instead of polluting the Rib script. However, if there are conflicts  (Example: user specified
// a CORS header already, then gateway resolves these conflicts by giving priority to user input)
// In most cases, it is best for users to do every pre-processing of input and forming the shape of response by themselves.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Middlewares(pub Vec<Middleware>);

impl Middlewares {
    pub fn get_cors(&self) -> Option<CorsPreflight> {
        self.0.iter().find_map(|m| m.get_cors())
    }
}

// A middleware will not add, remove or update the input to worker what-so-ever,
// as Rib is well typed and there wouldn't be any magical pre-processing such as adding a field to the worker input record.
// This is really hard to debug, and not to mention, Rib is well typed. So users need to satisfy the Rib compiler (satisfy the worker input then and there itself)
// That said, depending on the middleware type, gateway can make certain decisions automatically
// such as adding CORS headers to the http response body instead of asking users to do this everytime the Rib script,
// even after specifying a CORS middleware plugin. However, these automated decisions will still be rare.
// In most cases, it is best for users to do every pre-processing of input and forming the shape of response by themselves,
// as every data contained in the configured middlewares is made available in the context of Rib compiler.
#[derive(Debug, Clone, PartialEq)]
pub enum Middleware {
    Http(HttpMiddleware),
}

impl Middleware {
    pub fn get_cors(&self) -> Option<CorsPreflight> {
        match self {
            Middleware::Http(HttpMiddleware::Cors(cors)) => Some(cors.clone()),
        }
    }

    pub fn http(http_middleware: HttpMiddleware) -> Middleware {
        Middleware::Http(http_middleware)
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::Middleware> for Middlewares {
    type Error = String;

    fn try_from(value: golem_api_grpc::proto::golem::apidefinition::Middleware) -> Result<Self, Self::Error> {
        let mut middlewares = Vec::new();
        if let Some(cors) = value.cors {
            let cors = CorsPreflight::try_from(cors)?;
            middlewares.push(Middleware::http(HttpMiddleware::cors(cors)));
        }
        Ok(Middlewares(middlewares))
    }
}

impl From<Middlewares> for golem_api_grpc::proto::golem::apidefinition::Middleware {
    fn from(value: Middlewares) -> Self {
        let mut middleware = golem_api_grpc::proto::golem::apidefinition::Middleware::default();
        middleware.cors = value.0.iter().find_map(|m| m.get_cors().map(|c| c.into()));
        middleware
    }
}
