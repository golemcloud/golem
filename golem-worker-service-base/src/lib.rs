use ::http::Uri;

pub mod api;
pub mod http;
pub mod app_config;
pub mod auth;
pub mod expression;
pub mod merge;
pub mod metrics;
pub mod definition;
pub mod parser;
pub mod service;
pub mod tokeniser;
pub mod worker_request;
pub mod worker_binding;

pub mod repo;

pub mod evaluator;

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
