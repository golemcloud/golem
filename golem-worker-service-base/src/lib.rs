use ::http::Uri;
use golem_common::golem_version;

pub mod api;
pub mod gateway_api_definition;
pub mod app_config;
pub mod getter;
pub mod metrics;
pub(crate) mod path;
pub mod repo;
pub mod service;
mod gateway_binding;
pub mod gateway_execution;
mod gateway_rib_compiler;
pub mod gateway_rib_interpreter;
pub mod gateway_request;
pub mod gateway_api_deployment;
mod gateway_plugins;

#[cfg(test)]
test_r::enable!();

const VERSION: &str = golem_version!();

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
