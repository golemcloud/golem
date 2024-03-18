use crate::resolved_variables::ResolvedVariables;
use crate::worker_request::WorkerRequest;
use async_trait::async_trait;

// A generic interface that can convert a worker request to any type of response
// given some variable values and a mapping spec mainly consisting of expressions. Example: If the response is Http, we can have a mapping
// that sets the status code as  ${match worker.response { some(value) => 200 else 401 }} expression.
// all variables used in the mapping can look up from this dictionary resolved_variables. `Resolved Variables` are
// formed usually from input http request
#[async_trait]
pub trait WorkerRequestToResponse<Mapper, Response> {
    async fn execute(
        &self,
        resolved_worker_request: WorkerRequest,
        response_mapping: &Option<Mapper>,
        resolved_variables: &ResolvedVariables,
    ) -> Response;
}
