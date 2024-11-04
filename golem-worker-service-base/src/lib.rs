use ::http::Uri;
use golem_common::golem_version;
use service::worker::WorkerRequestMetadata;

pub mod api;
pub mod api_definition;
pub mod app_config;
pub mod getter;
mod headers;
pub mod http;
pub mod metrics;
mod parser;
pub(crate) mod path;
pub mod repo;
pub mod service;
pub mod worker_binding;
pub mod worker_bridge_execution;
mod worker_service_rib_compiler;
pub mod worker_service_rib_interpreter;

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
        account_id: Some(golem_common::model::AccountId {
            value: "-1".to_string(),
        }),
        limits: None,
    }
}
