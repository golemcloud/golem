use ::http::Uri;

pub mod api;
pub mod repo;
pub mod service;
pub mod auth;
pub mod worker_request;
pub mod metrics;
pub mod app_config;
mod definition;
mod expression;
mod http;
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
