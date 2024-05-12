use std::collections::HashMap;
use golem_wasm_rpc::TypeAnnotatedValue;
use crate::worker_binding::RequestDetails;
use crate::worker_bridge_execution::{WorkerBridgeResponse, WorkerResponse};


// Evaluator of an expression doesn't necessarily need a context all the time, and can be empty.
// or contain worker details, request details, or both.
enum EvaluatorInputContext {
    WorkerDetailsOnly {
        worker_details: WorkerDetails,
    },
    RequestOnly {
        request: RequestDetails
    },
    All {
        worker_details: WorkerDetails,
        request_details: RequestDetails
    },
    Empty,
}


struct WorkerDetails {
    worker_name: String,
    worker_response: WorkerBridgeResponse
}
