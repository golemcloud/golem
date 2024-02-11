use std::collections::HashMap;

use hyper::http::Method;

use crate::api_definition::{ApiDefinition, MethodPattern, Route};
use crate::http_request::InputHttpRequest;
use crate::resolved_variables::ResolvedVariables;

pub trait RouteResolver {
    fn resolve(&self, api_specification: &ApiDefinition) -> Option<ResolvedRoute>;
}

pub struct ResolvedRoute {
    pub route_definition: Route,
    pub resolved_variables: ResolvedVariables,
}

impl<'a> RouteResolver for InputHttpRequest<'a> {
    fn resolve(&self, api_definition: &ApiDefinition) -> Option<ResolvedRoute> {
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
            let request_query_values: HashMap<String, String> =
                api_request.input_path.query_components();

            let request_body = &api_request.req_body;
            let request_header = api_request.headers;

            if match_method(request_method, spec_method)
                && match_literals(&request_path_components, &spec_path_literals)
            {
                let request_details: ResolvedVariables = ResolvedVariables::from_http_request(
                    request_body,
                    request_header,
                    request_query_values,
                    spec_query_variables,
                    &request_path_components,
                    &spec_path_variables,
                )
                .ok()?;

                let resolved_binding = ResolvedRoute {
                    route_definition: route.clone(),
                    resolved_variables: { request_details },
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

        assert_eq!(
            match_literals(&request_path_values, &spec_path_literals),
            true
        );
    }

    #[test]
    fn test_match_literals_empty_request_path() {
        let request_path_values = HashMap::new();

        let mut spec_path_literals = HashMap::new();
        spec_path_literals.insert(0, "get-cart-contents".to_string());

        assert_eq!(
            match_literals(&request_path_values, &spec_path_literals),
            false
        );
    }

    #[test]
    fn test_match_literals_empty_spec_path() {
        let mut request_path_values = HashMap::new();
        request_path_values.insert(0, "get-cart-contents".to_string());

        let spec_path_literals = HashMap::new();

        assert_eq!(
            match_literals(&request_path_values, &spec_path_literals),
            false
        );
    }
}
