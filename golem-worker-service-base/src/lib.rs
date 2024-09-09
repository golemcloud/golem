use ::http::Uri;
pub mod api;
pub mod api_definition;
pub mod app_config;
pub mod getter;
pub mod http;
pub mod metrics;
mod parser;
pub(crate) mod path;
pub mod repo;
pub mod service;
mod worker_binding;
pub mod worker_bridge_execution;
pub mod worker_service_rib_interpreter;

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
