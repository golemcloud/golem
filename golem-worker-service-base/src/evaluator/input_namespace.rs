use std::collections::HashMap;
use golem_wasm_rpc::TypeAnnotatedValue;
use crate::worker_bridge_execution::{WorkerBridgeResponse, WorkerResponse};


pub struct EvaluatorInputNamespace {
    worker: WorkerDetails,
    request: RequestDetails
}




impl EvaluatorInputNamespace {
    fn get_request_details(&self) -> &RequestDetails {
        &self.request
    }
}
