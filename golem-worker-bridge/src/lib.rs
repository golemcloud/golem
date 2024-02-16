pub mod api;
pub mod api_definition;
pub mod api_request_route_resolver;
pub mod app_config;
pub mod evaluator;
pub mod expr;
mod http_request;
pub mod oas_worker_bridge;
pub mod parser;
pub mod register;
pub mod resolved_variables;
pub mod tokeniser;
pub mod value_typed;
pub mod worker;
pub mod worker_bridge_reponse;
pub mod worker_request;
pub mod worker_request_executor;

pub mod service;


pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
