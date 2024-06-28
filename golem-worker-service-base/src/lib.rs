use ::http::Uri;
pub mod api;
pub mod api_definition;
pub mod app_config;
pub mod auth;
pub mod evaluator;
pub mod http;
mod merge;
pub mod metrics;
mod parser;
mod primitive;
pub mod repo;
pub mod service;
mod worker_binding;
pub mod worker_bridge_execution;

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
