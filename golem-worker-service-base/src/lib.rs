pub mod api;
pub mod http;
pub mod api_definition_repo;
pub mod worker_binding_resolver;
pub mod app_config;
pub mod auth;
pub mod evaluator;
pub mod expression;
pub mod getter;
pub mod merge;
pub mod metrics;
pub mod definition;
pub mod parser;
pub mod path;
pub mod primitive;
pub mod service;
pub mod tokeniser;
pub mod worker_request;
pub mod worker_request_to_response;
pub mod worker_response;
pub mod worker_binding;
pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
