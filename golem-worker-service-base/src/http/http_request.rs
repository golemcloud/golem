use std::collections::HashMap;

use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::TypeAnnotatedValue;
use hyper::http::{HeaderMap, Method};
use serde_json::Value;

use crate::api_definition::http::HttpApiDefinition;
use crate::merge::Merge;
use crate::tokeniser::tokenizer::Token;
use crate::worker_binding::{ResolvedWorkerBinding, WorkerBindingResolver};

// An input request from external API gateways, that is then resolved to a worker request, using API definitions
#[derive(Clone)]
pub struct InputHttpRequest {
    pub input_path: ApiInputPath,
    pub headers: HeaderMap,
    pub req_method: Method,
    pub req_body: Value,
}

impl InputHttpRequest {
    // Converts all request details to type-annotated-value
    // and place them under the key `request`
    pub fn get_type_annotated_value(
        &self,
        spec_query_variables: Vec<String>,
        spec_path_variables: &HashMap<usize, String>,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let request_body = &self.req_body;
        let request_header = self.headers.clone();
        let request_path_values: HashMap<usize, String> = self.input_path.path_components();

        let request_query_variables: HashMap<String, String> = self.input_path.query_components();

        let request_header_values = internal::get_headers(&request_header)?;
        let body_value = internal::get_request_body(request_body)?;
        let path_value = internal::get_request_path_query_values(
            request_query_variables,
            spec_query_variables,
            &request_path_values,
            spec_path_variables,
        )?;

        let merged = body_value.merge(&request_header_values).merge(&path_value);

        let request_type_annotated_value = TypeAnnotatedValue::Record {
            value: vec![(Token::Request.to_string(), merged.clone())],
            typ: vec![(Token::Request.to_string(), AnalysedType::from(&merged))],
        };

        Ok(request_type_annotated_value)
    }
}

impl WorkerBindingResolver<HttpApiDefinition> for InputHttpRequest {
    fn resolve(&self, api_definition: &HttpApiDefinition) -> Option<ResolvedWorkerBinding> {
        let api_request = self;
        let routes = &api_definition.routes;

        for route in routes {
            let spec_method = &route.method;
            let spec_path_variables = route.path.get_path_variables();
            let spec_path_literals = route.path.get_path_literals();
            let spec_query_variables = route.path.get_query_variables();

            let request_method: &Method = &api_request.req_method;
            let request_path_components: HashMap<usize, String> =
                api_request.input_path.path_components();

            if internal::match_method(request_method, spec_method)
                && internal::match_literals(&request_path_components, &spec_path_literals)
            {
                let request_details = api_request
                    .get_type_annotated_value(spec_query_variables, &spec_path_variables);

                let request_details = request_details.clone().ok()?;

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

#[derive(Clone)]
pub struct ApiInputPath {
    pub base_path: String,
    pub query_path: Option<String>,
}

impl ApiInputPath {
    // Return the each component of the path which can either be a literal or the value of a path_var, along with it's index
    fn path_components(&self) -> HashMap<usize, String> {
        let mut path_components: HashMap<usize, String> = HashMap::new();

        // initial `/` is excluded to not break indexes
        let path = if self.base_path.starts_with('/') {
            &self.base_path[1..self.base_path.len()]
        } else {
            self.base_path.as_str()
        };

        let base_path_parts = path.split('/').map(|x| x.trim());

        for (index, part) in base_path_parts.enumerate() {
            if !part.is_empty() {
                path_components.insert(index, part.to_string());
            }
        }

        path_components
    }

    // Return the value of each query variable in a HashMap
    fn query_components(&self) -> HashMap<String, String> {
        let mut query_components: HashMap<String, String> = HashMap::new();

        if let Some(query_path) = self.query_path.clone() {
            let query_parts = query_path.split('&').map(|x| x.trim());

            for part in query_parts {
                let key_value: Vec<&str> = part.split('=').map(|x| x.trim()).collect();

                if let (Some(key), Some(value)) = (key_value.first(), key_value.get(1)) {
                    query_components.insert(key.to_string(), value.to_string());
                }
            }
        }

        query_components
    }
}

mod internal {
    use crate::api_definition::http::MethodPattern;
    use crate::http::http_request::internal;
    use crate::merge::Merge;
    use crate::primitive::{Number, Primitive};
    use golem_service_base::type_inference::infer_analysed_type;
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_rpc::json::get_typed_value_from_json;
    use golem_wasm_rpc::TypeAnnotatedValue;
    use http::{HeaderMap, Method};
    use serde_json::Value;
    use std::collections::HashMap;

    pub(crate) fn match_method(
        input_request_method: &Method,
        spec_method_pattern: &MethodPattern,
    ) -> bool {
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

    pub(crate) fn match_literals(
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

    pub(crate) fn get_typed_value_from_primitive(value: &str) -> TypeAnnotatedValue {
        let query_value = Primitive::from(value.to_string());
        match query_value {
            Primitive::Num(number) => match number {
                Number::PosInt(value) => TypeAnnotatedValue::U64(value),
                Number::NegInt(value) => TypeAnnotatedValue::S64(value),
                Number::Float(value) => TypeAnnotatedValue::F64(value),
            },
            Primitive::String(value) => TypeAnnotatedValue::Str(value),
            Primitive::Bool(value) => TypeAnnotatedValue::Bool(value),
        }
    }

    pub(crate) fn get_request_body(
        request_body: &Value,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let inferred_type = infer_analysed_type(request_body);
        let typed_value = get_typed_value_from_json(request_body, &inferred_type)?;

        Ok(TypeAnnotatedValue::Record {
            value: vec![("body".to_string(), typed_value)],
            typ: vec![("body".to_string(), inferred_type)],
        })
    }

    pub(crate) fn get_headers(headers: &HeaderMap) -> Result<TypeAnnotatedValue, Vec<String>> {
        let mut headers_map: Vec<(String, TypeAnnotatedValue)> = vec![];

        for (header_name, header_value) in headers {
            let header_value_str = header_value.to_str().map_err(|err| vec![err.to_string()])?;

            let typed_header_value = internal::get_typed_value_from_primitive(header_value_str);

            headers_map.push((header_name.to_string(), typed_header_value));
        }

        let type_annotated_value = TypeAnnotatedValue::Record {
            value: headers_map.clone(),
            typ: headers_map
                .clone()
                .iter()
                .map(|(key, v)| (key.clone(), AnalysedType::from(v)))
                .collect(),
        };

        Ok(TypeAnnotatedValue::Record {
            value: vec![("header".to_string(), type_annotated_value.clone())],
            typ: vec![(
                "header".to_string(),
                AnalysedType::from(&type_annotated_value),
            )],
        })
    }

    pub(crate) fn get_request_path_query_values(
        request_query_variables: HashMap<String, String>,
        spec_query_variables: Vec<String>,
        request_path_values: &HashMap<usize, String>,
        spec_path_variables: &HashMap<usize, String>,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let request_query_values =
            get_request_query_values(request_query_variables, spec_query_variables)?;

        let request_path_values =
            get_request_path_values(request_path_values, spec_path_variables)?;

        let path_values = request_query_values.merge(&request_path_values);

        Ok(TypeAnnotatedValue::Record {
            value: vec![("path".to_string(), path_values.clone())],
            typ: vec![("path".to_string(), AnalysedType::from(&path_values))],
        })
    }

    fn get_request_path_values(
        request_path_values: &HashMap<usize, String>,
        spec_path_variables: &HashMap<usize, String>,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let mut unavailable_path_variables: Vec<String> = vec![];
        let mut path_variables_map = vec![];

        for (index, spec_path_variable) in spec_path_variables.iter() {
            if let Some(path_value) = request_path_values.get(index) {
                let typed_value = internal::get_typed_value_from_primitive(path_value);

                path_variables_map.push((spec_path_variable.clone(), typed_value));
            } else {
                unavailable_path_variables.push(spec_path_variable.to_string());
            }
        }

        if unavailable_path_variables.is_empty() {
            let type_annotated_value = TypeAnnotatedValue::Record {
                value: path_variables_map.clone(),
                typ: path_variables_map
                    .clone()
                    .iter()
                    .map(|(key, v)| (key.clone(), AnalysedType::from(v)))
                    .collect(),
            };
            Ok(type_annotated_value)
        } else {
            Err(unavailable_path_variables)
        }
    }

    fn get_request_query_values(
        request_query_variables: HashMap<String, String>,
        spec_query_variables: Vec<String>,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let mut unavailable_query_variables: Vec<String> = vec![];
        let mut query_variable_map: Vec<(String, TypeAnnotatedValue)> = vec![];

        for spec_query_variable in spec_query_variables.iter() {
            if let Some(query_value) = request_query_variables.get(spec_query_variable) {
                let typed_value = internal::get_typed_value_from_primitive(query_value);
                query_variable_map.push((spec_query_variable.clone(), typed_value));
            } else {
                unavailable_query_variables.push(spec_query_variable.to_string());
            }
        }

        if unavailable_query_variables.is_empty() {
            let type_annotated_value = TypeAnnotatedValue::Record {
                value: query_variable_map.clone(),
                typ: query_variable_map
                    .clone()
                    .iter()
                    .map(|(key, v)| (key.clone(), AnalysedType::from(v)))
                    .collect::<Vec<(String, AnalysedType)>>(),
            };
            Ok(type_annotated_value)
        } else {
            Err(unavailable_query_variables)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use http::{HeaderMap, HeaderName, HeaderValue, Method};

    use golem_common::model::TemplateId;

    use crate::api_definition::http::HttpApiDefinition;
    use crate::http::http_request::{ApiInputPath, InputHttpRequest};
    use crate::worker_bridge_execution::WorkerRequest;

    use super::*;

    #[test]
    fn test_worker_request_resolution() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);
        let function_params = "[\"a\", \"b\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("a".to_string()),
                serde_json::Value::String("b".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_resolution_with_concrete_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

        let function_params = "[{\"x\" : \"y\"}]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let mut expected_map = serde_json::Map::new();

        expected_map.insert("x".to_string(), serde_json::Value::String("y".to_string()));

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Object(
                expected_map,
            )]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_resolution_with_path_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);

        let function_params = "[{\"x\" : \"${request.path.user-id}\"}]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let mut expected_map = serde_json::Map::new();

        expected_map.insert(
            "x".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1)),
        );

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Object(
                expected_map,
            )]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_resolution_with_path_and_query_params() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request(
            "foo/1",
            Some("token-id=2"),
            &empty_headers,
            serde_json::Value::Null,
        );

        let function_params = "[\"${request.path.user-id}\", \"${request.path.token-id}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let mut expected_map = serde_json::Map::new();

        expected_map.insert(
            "x".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1)),
        );

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::Number(serde_json::Number::from(1)),
                serde_json::Value::Number(serde_json::Number::from(2)),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_resolution_with_path_and_query_body_params() {
        let mut request_body_amp = serde_json::Map::new();

        request_body_amp.insert(
            "age".to_string(),
            serde_json::Value::Number(serde_json::Number::from(10)),
        );

        let empty_headers = HeaderMap::new();
        let api_request = get_api_request(
            "foo/1",
            Some("token-id=2"),
            &empty_headers,
            serde_json::Value::Object(request_body_amp),
        );

        let function_params =
            "[\"${request.path.user-id}\", \"${request.path.token-id}\",  \"age-${request.body.age}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::Number(serde_json::Number::from(1)),
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::String("age-10".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_resolution_with_record_params() {
        let mut request_body_amp = serde_json::Map::new();

        request_body_amp.insert(
            "age".to_string(),
            serde_json::Value::Number(serde_json::Number::from(10)),
        );

        let mut headers = HeaderMap::new();

        headers.insert(
            HeaderName::from_static("username"),
            HeaderValue::from_static("foo"),
        );

        let api_request = get_api_request(
            "foo/1",
            Some("token-id=2"),
            &headers,
            serde_json::Value::Object(request_body_amp),
        );

        let function_params =
            "[{ \"user-id\" : \"${request.path.user-id}\" }, \"${request.path.token-id}\",  \"age-${request.body.age}\", \"user-name\" : \"${request.header.username}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}?{token-id}",
            "shopping-cart-${request.path.user-id}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let mut user_id_map = serde_json::Map::new();

        user_id_map.insert(
            "user-id".to_string(),
            serde_json::Value::Number(serde_json::Number::from(1)),
        );

        let mut user_name_map = serde_json::Map::new();

        user_name_map.insert(
            "user-name".to_string(),
            serde_json::Value::String("foo".to_string()),
        );

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::Object(user_id_map),
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::String("age-10".to_string()),
                serde_json::Value::Object(user_name_map),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_cond_expr_resolution() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/2", None, &empty_headers, serde_json::Value::Null);
        let function_params = "[\"a\", \"b\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if (request.path.user-id>100) then 0 else 1}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("a".to_string()),
                serde_json::Value::String("b".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_request_body_resolution() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::String("address".to_string()),
        );

        let function_params = "[\"${request.body}\"]";

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if (request.path.user-id>100) then 0 else 1}",
            function_params,
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::String(
                "address".to_string(),
            )]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_resolution_for_predicate_gives_bool() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::String("address".to_string()),
        );

        let function_params = "[\"${1 == 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Bool(true)]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_resolution_for_predicate_gives_bool_greater() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::String("address".to_string()),
        );

        let function_params = "[\"${2 > 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Bool(true)]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_resolution_for_cond_expr_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::String("address".to_string()),
        );

        let function_params = "[\"${if (2 < 1) then 0 else 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(1),
            )]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_resolution_for_cond_expr_req_body_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(serde_json::Map::from_iter(vec![(
                "number".to_string(),
                serde_json::Value::Number(serde_json::Number::from(10)),
            )])),
        );

        let function_params = "[\"${if (request.body.number < 11) then 0 else 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Number(
                serde_json::Number::from(0),
            )]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_resolution_for_cond_expr_req_body_direct_fn_params() {
        let empty_headers = HeaderMap::new();

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Number(serde_json::Number::from(10)),
        );

        let function_params = "[\"${if (request.body < 11) then request.path.user-id else 1}\", \"${if (request.body < 5) then ${request.path.user-id} else 1}\"]";

        let api_specification: HttpApiDefinition =
            get_api_spec("foo/{user-id}", "shopping-cart", function_params);

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::Number(serde_json::Number::from(2)),
                serde_json::Value::Number(serde_json::Number::from(1)),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_map_request_body_resolution() {
        let empty_headers = HeaderMap::new();

        let mut request_body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            serde_json::Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            serde_json::Value::String("bar_value".to_string()),
        );

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(request_body),
        );

        let foo_key = "${request.body.foo_key}";
        let bar_key = "${request.body.bar_key}";

        let function_params = format!("[\"{}\", \"{}\"]", foo_key, bar_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if (request.path.user-id>100) then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_map_list_request_body_resolution() {
        let empty_headers = HeaderMap::new();

        let mut request_body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            serde_json::Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("bar_value".to_string())]),
        );

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(request_body),
        );

        let foo_key = "${request.body.foo_key}";
        let bar_key = "${request.body.bar_key[0]}";

        let function_params = format!("[\"{}\", \"{}\"]", foo_key, bar_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if (request.path.user-id>100) then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_request_body_direct() {
        let empty_headers = HeaderMap::new();

        let mut request_body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            serde_json::Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("bar_value".to_string())]),
        );

        let api_request = get_api_request(
            "foo/2",
            None,
            &empty_headers,
            serde_json::Value::Object(request_body.clone()),
        );

        let foo_key: &str = "${request.body}";

        let function_params = format!("[\"{}\"]", foo_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "foo/{user-id}",
            "shopping-cart-${if (request.path.user-id>100) then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![serde_json::Value::Object(
                request_body.clone(),
            )]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_with_request_header_resolution() {
        let mut headers = HeaderMap::new();

        headers.append(
            HeaderName::from_static("token"),
            HeaderValue::from_static("token_value"),
        );

        let mut request_body: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();

        request_body.insert(
            "foo_key".to_string(),
            serde_json::Value::String("foo_value".to_string()),
        );

        request_body.insert(
            "bar_key".to_string(),
            serde_json::Value::Array(vec![serde_json::Value::String("bar_value".to_string())]),
        );

        let api_request = get_api_request(
            "/foo/2",
            None,
            &headers,
            serde_json::Value::Object(request_body),
        );

        let foo_key = "${request.body.foo_key}";
        let bar_key = "${request.body.bar_key[0]}";
        let token_key = "${request.header.token}";

        let function_params = format!("[\"{}\", \"{}\", \"{}\"]", foo_key, bar_key, token_key);

        let api_specification: HttpApiDefinition = get_api_spec(
            "/foo/{user-id}",
            "shopping-cart-${if (request.path.user-id>100) then 0 else 1}",
            function_params.as_str(),
        );

        let resolved_route = api_request.resolve(&api_specification).unwrap();

        let result = WorkerRequest::from_resolved_route(resolved_route.clone());

        let expected = WorkerRequest {
            template: "0b6d9cd8-f373-4e29-8a5a-548e61b868a5"
                .parse::<TemplateId>()
                .unwrap(),
            worker_id: "shopping-cart-1".to_string(),
            function: "golem:it/api/get-cart-contents".to_string(),
            function_params: serde_json::Value::Array(vec![
                serde_json::Value::String("foo_value".to_string()),
                serde_json::Value::String("bar_value".to_string()),
                serde_json::Value::String("token_value".to_string()),
            ]),
        };

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_worker_request_resolution_paths() {
        fn test_paths(definition_path: &str, request_path: &str, ok: bool) {
            let empty_headers = HeaderMap::new();
            let api_request =
                get_api_request(request_path, None, &empty_headers, serde_json::Value::Null);

            let function_params = "[]";

            let api_specification: HttpApiDefinition = get_api_spec(
                definition_path,
                "shopping-cart-${request.path.cart-id}",
                function_params,
            );

            let resolved_route = api_request.resolve(&api_specification);

            let result = match resolved_route {
                Some(resolved_route) => WorkerRequest::from_resolved_route(resolved_route)
                    .map_err(|err| err.to_string()),
                None => Err("not found".to_string()),
            };

            assert_eq!(result.is_ok(), ok);
        }

        test_paths("getcartcontent/{cart-id}", "/noexist", false);
        test_paths("/getcartcontent/{cart-id}", "noexist", false);
        test_paths("getcartcontent/{cart-id}", "noexist", false);
        test_paths("/getcartcontent/{cart-id}", "/noexist", false);
        test_paths("getcartcontent/{cart-id}", "/getcartcontent/1", true);
        test_paths("/getcartcontent/{cart-id}", "getcartcontent/1", true);
        test_paths("getcartcontent/{cart-id}", "getcartcontent/1", true);
        test_paths("/getcartcontent/{cart-id}", "/getcartcontent/1", true);
    }

    fn get_api_request(
        base_path: &str,
        query_path: Option<&str>,
        headers: &HeaderMap,
        req_body: serde_json::Value,
    ) -> InputHttpRequest {
        InputHttpRequest {
            input_path: ApiInputPath {
                base_path: base_path.to_string(),
                query_path: query_path.map(|x| x.to_string()),
            },
            headers: headers.clone(),
            req_method: Method::GET,
            req_body,
        }
    }

    fn get_api_spec(
        path_pattern: &str,
        worker_id: &str,
        function_params: &str,
    ) -> HttpApiDefinition {
        let yaml_string = format!(
            r#"
          id: users-api
          version: 0.0.1
          routes:
          - method: Get
            path: {}
            binding:
              type: wit-worker
              template: 0b6d9cd8-f373-4e29-8a5a-548e61b868a5
              workerId: '{}'
              functionName: golem:it/api/get-cart-contents
              functionParams: {}
        "#,
            path_pattern, worker_id, function_params
        );

        serde_yaml::from_str(yaml_string.as_str()).unwrap()
    }

    #[test]
    fn test_match_literals() {
        let mut request_path_values = HashMap::new();
        request_path_values.insert(0, "users".to_string());
        request_path_values.insert(1, "1".to_string());

        let mut spec_path_literals = HashMap::new();
        spec_path_literals.insert(0, "users".to_string());
        spec_path_literals.insert(1, "1".to_string());

        assert!(internal::match_literals(
            &request_path_values,
            &spec_path_literals
        ));
    }

    #[test]
    fn test_match_literals_empty_request_path() {
        let request_path_values = HashMap::new();

        let mut spec_path_literals = HashMap::new();
        spec_path_literals.insert(0, "get-cart-contents".to_string());

        assert!(!internal::match_literals(
            &request_path_values,
            &spec_path_literals
        ));
    }

    #[test]
    fn test_match_literals_empty_spec_path() {
        let mut request_path_values = HashMap::new();
        request_path_values.insert(0, "get-cart-contents".to_string());

        let spec_path_literals = HashMap::new();

        assert!(!internal::match_literals(
            &request_path_values,
            &spec_path_literals
        ));
    }
}
