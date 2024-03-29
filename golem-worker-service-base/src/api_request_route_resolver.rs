use golem_wasm_rpc::TypeAnnotatedValue;
use std::collections::HashMap;

use hyper::http::Method;

use crate::api_definition::{ApiDefinition, GolemWorkerBinding, MethodPattern};
use crate::http_request::InputHttpRequest;

// For any input request type, there should be a way to resolve the
// worker binding template, which is then used to form the worker request
pub trait WorkerBindingResolver {
    fn resolve(&self, api_specification: &ApiDefinition) -> Option<ResolvedWorkerBinding>;
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkerBinding {
    pub resolved_worker_binding_template: GolemWorkerBinding,
    pub typed_value_from_input: TypeAnnotatedValue,
}

impl<'a> WorkerBindingResolver for InputHttpRequest<'a> {
    fn resolve(&self, api_definition: &ApiDefinition) -> Option<ResolvedWorkerBinding> {
        let api_request = self;
        let routes = &api_definition.routes;

        for route in routes {
            let spec_method = &route.method;
            let spec_path_variables = route.path.get_path_variables();
            let spec_path_literals = route.path.get_path_literals();
            let spec_query_variables = route.path.get_query_variables();

            let request_method: &Method = api_request.req_method;
            let request_path_components: HashMap<usize, String> =
                api_request.input_path.path_components();

            if match_method(request_method, spec_method)
                && match_literals(&request_path_components, &spec_path_literals)
            {
                let request_details: TypeAnnotatedValue = api_request
                    .get_type_annotated_value(spec_query_variables, &spec_path_variables)
                    .ok()?;

                let resolved_binding = ResolvedWorkerBinding {
                    resolved_worker_binding_template: route.binding.clone(),
                    typed_value_from_input: { request_details },
                };
                return Some(resolved_binding);
            } else {
                continue;
            }
        }

        None
    }
}

fn match_method(input_request_method: &Method, spec_method_pattern: &MethodPattern) -> bool {
    match input_request_method.clone() {
        Method::CONNECT => spec_method_pattern.is_connect(),
        Method::GET => spec_method_pattern.is_get(),
        Method::POST => spec_method_pattern.is_post(),
        Method::HEAD => spec_method_pattern.is_head(),
        Method::DELETE => spec_method_pattern.is_delete(),
        Method::PUT => spec_method_pattern.is_put(),
        Method::PATCH => spec_method_pattern.is_patch(),
        Method::OPTIONS => spec_method_pattern.is_options(),
        Method::TRACE => spec_method_pattern.is_trace(),
        _ => false,
    }
}

fn match_literals(
    request_path_values: &HashMap<usize, String>,
    spec_path_literals: &HashMap<usize, String>,
) -> bool {
    if spec_path_literals.is_empty() && !request_path_values.is_empty() {
        false
    } else {
        let mut literals_match = true;

        for (index, spec_literal) in spec_path_literals.iter() {
            if let Some(request_literal) = request_path_values.get(index) {
                if request_literal.trim() != spec_literal.trim() {
                    literals_match = false;
                    break;
                }
            } else {
                literals_match = false;
                break;
            }
        }

        literals_match
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_match_literals() {
        let mut request_path_values = HashMap::new();
        request_path_values.insert(0, "users".to_string());
        request_path_values.insert(1, "1".to_string());

        let mut spec_path_literals = HashMap::new();
        spec_path_literals.insert(0, "users".to_string());
        spec_path_literals.insert(1, "1".to_string());

        assert!(match_literals(&request_path_values, &spec_path_literals));
    }

    #[test]
    fn test_match_literals_empty_request_path() {
        let request_path_values = HashMap::new();

        let mut spec_path_literals = HashMap::new();
        spec_path_literals.insert(0, "get-cart-contents".to_string());

        assert!(!match_literals(&request_path_values, &spec_path_literals));
    }

    #[test]
    fn test_match_literals_empty_spec_path() {
        let mut request_path_values = HashMap::new();
        request_path_values.insert(0, "get-cart-contents".to_string());

        let spec_path_literals = HashMap::new();

        assert!(!match_literals(&request_path_values, &spec_path_literals));
    }
}
