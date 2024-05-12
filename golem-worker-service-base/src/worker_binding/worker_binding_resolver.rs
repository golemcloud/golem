use std::collections::HashMap;
use golem_wasm_rpc::TypeAnnotatedValue;
use crate::api_definition::http::{HttpApiDefinition, VarInfo};
use crate::http::http_request::router;
use crate::http::InputHttpRequest;
use crate::http::router::RouterPattern;

use crate::worker_binding::{GolemWorkerBinding, RequestDetails};

// For any input request type, there should be a way to resolve the
// worker binding component, which is then used to form the worker request
// resolved binding is always kept along with the request as binding may refer
// to request details
pub trait WorkerBindingResolver<ApiDefinition> {
    fn resolve(&self, api_specification: &ApiDefinition) -> Option<ResolvedWorkerBinding>;
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkerBinding {
    pub resolved_worker_binding_template: GolemWorkerBinding,
    pub request_details: RequestDetails,
}

impl WorkerBindingResolver<HttpApiDefinition> for InputHttpRequest {
    fn resolve(&self, api_definition: &HttpApiDefinition) -> Option<ResolvedWorkerBinding> {
        let api_request = self;
        let router = router::build(api_definition.routes.clone());
        let path: Vec<&str> = RouterPattern::split(&api_request.input_path.base_path).collect();
        let request_query_variables = self.input_path.query_components().unwrap_or_default();
        let request_body = &self.req_body;
        let headers = &self.headers;

        let router::RouteEntry {
            path_params,
            query_params,
            binding,
        } = router.check_path(&api_request.req_method, &path)?;

        let zipped_path_params: HashMap<VarInfo, &str> = {
            path_params
                .iter()
                .map(|(var, index)| (var.clone(), path[*index]))
                .collect()
        };

        let request_details =
            RequestDetails::from(&zipped_path_params, &request_query_variables, query_params, request_body, headers)?;

        let resolved_binding = ResolvedWorkerBinding {
            resolved_worker_binding_template: binding.clone(),
            request_details
        };

        Some(resolved_binding)
    }
}