pub mod api;
pub mod api_definition;
pub mod api_request_route_resolver;
pub mod app_config;
pub mod auth;
pub mod evaluator;
pub mod expr;
pub mod grpcapi;
mod http_request;
pub mod metrics;
pub mod oas_worker_bridge;
pub mod parser;
pub mod register;
pub mod resolved_variables;
pub mod service;
pub mod tokeniser;
pub mod value_typed;
pub mod worker_request;
pub mod worker_request_to_http_response;
pub mod worker_request_to_response;

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
