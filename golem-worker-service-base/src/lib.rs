use ::http::Uri;
use golem_common::golem_version;
use service::worker::WorkerRequestMetadata;

pub mod api;
pub mod app_config;

pub mod gateway_api_definition;
pub mod gateway_api_definition_transformer;
pub mod gateway_api_deployment;
pub mod gateway_binding;
pub mod gateway_execution;
pub mod gateway_middleware;
pub mod gateway_request;
mod gateway_rib_compiler;
pub mod gateway_rib_interpreter;
pub mod gateway_security;
pub mod getter;
mod headers;
pub mod metrics;
pub mod path;
pub mod repo;
pub mod service;

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

pub fn empty_worker_metadata() -> WorkerRequestMetadata {
    WorkerRequestMetadata {
        account_id: Some(golem_common::model::AccountId::placeholder()),
        limits: None,
    }
}
