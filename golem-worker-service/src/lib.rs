use golem_worker_service_base::service::worker::WorkerRequestMetadata;
pub mod api;
pub mod config;
pub mod grpcapi;
pub mod service;
pub mod worker_bridge_request_executor;
pub mod component_metadata_fetcher;
fn empty_worker_metadata() -> WorkerRequestMetadata {
    WorkerRequestMetadata {
        account_id: Some(golem_common::model::AccountId {
            value: "-1".to_string(),
        }),
        limits: None,
    }
}
