pub mod api;
pub mod app;
pub mod aws_config;
pub mod aws_load_balancer;
pub mod config;
pub mod grpcapi;
pub mod model;
pub mod repo;
pub mod service;

pub mod http_request_definition_lookup;
pub mod worker_request_to_http_response;

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
