use golem_worker_service_base::service::worker::WorkerRequestMetadata;

pub mod api;
pub mod config;
pub mod grpcapi;
pub mod service;

#[cfg(test)]
test_r::enable!();

fn empty_worker_metadata() -> WorkerRequestMetadata {
    WorkerRequestMetadata {
        account_id: Some(golem_common::model::AccountId {
            value: "-1".to_string(),
        }),
        limits: None,
    }
}
