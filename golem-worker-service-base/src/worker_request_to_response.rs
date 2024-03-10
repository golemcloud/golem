use crate::resolved_variables::ResolvedVariables;
use crate::worker_request::WorkerRequest;
use async_trait::async_trait;

// A generic interface that can convert a worker request to a any type of response
// give some variable values and a mapping spec mainly consisting of expressions. Example: In the case of http response,
// a response header can be ${worker.response.user} expression. We call this response_mapping
#[async_trait]
pub trait WorkerRequestToResponse<Mapper, Response> {
    async fn execute(
        &self,
        resolved_worker_request: WorkerRequest,
        response_mapping: &Option<Mapper>,
        resolved_variables: &ResolvedVariables, // resolved variables from the input request can also be useful to form the response
    ) -> Response;
}
