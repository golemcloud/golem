use golem_common::golem_version;

pub mod api;
pub mod config;
pub mod grpcapi;
pub mod metrics;
pub mod model;
pub mod service;

pub const VERSION: &str = golem_version!();

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
