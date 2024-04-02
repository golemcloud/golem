use ::http::Uri;

pub mod api;
pub mod repo;
pub mod service;
pub mod auth;
pub mod worker_bridge;
pub mod metrics;
pub mod app_config;
pub mod http;
pub mod api_definition;
mod expression;
mod merge;
mod parser;
mod tokeniser;
mod worker_binding;
mod evaluator;
mod primitive;
pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
