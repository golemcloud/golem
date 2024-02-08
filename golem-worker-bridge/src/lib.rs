pub mod tokeniser;
pub mod expr;
pub mod parser;
pub mod api_definition;
mod http_request;
pub mod api_request_route_resolver;
pub mod resolved_variables;
pub mod worker_request_executor;
pub mod worker;
pub mod app_config;
pub mod worker_request;
pub mod evaluator;
pub mod value_typed;
pub mod worker_bridge_reponse;
pub mod api;
pub mod register;

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
