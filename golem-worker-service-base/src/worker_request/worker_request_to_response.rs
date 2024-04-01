use async_trait::async_trait;
use golem_wasm_rpc::TypeAnnotatedValue;

use crate::worker_request::WorkerRequest;

// A generic interface that can convert a worker request to any type of response
// given some variable values and a mapping spec mainly consisting of expressions. Example: If the response is Http, we can have a mapping
// that sets the status code as  ${match worker.response { some(value) => 200 else 401 }} expression.
// All variables used in the mapping can look up from this dictionary of input request variables which is also represented using TypeAnnotatedValue.
// This will ensure that any reference to input request variables is also typed, than just a text/Json or HashMap
// TODO; The Mapper can be concrete to ResponseMapping once ResponseMapping is a generic Expr instead of specific to http
#[async_trait]
pub trait WorkerRequestToResponse<Mapper, Response> {
    async fn execute(
        &self,
        resolved_worker_request: WorkerRequest,
        response_mapping: &Option<Mapper>,
        input_request: &TypeAnnotatedValue,
    ) -> Response;
}
