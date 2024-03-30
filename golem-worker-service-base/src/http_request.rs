use std::collections::HashMap;

use crate::merge::Merge;
use crate::tokeniser::tokenizer::Token;
use derive_more::{Display, FromStr, Into};
use golem_service_base::type_inference::infer_analysed_type;
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::json::get_typed_value_from_json;
use golem_wasm_rpc::TypeAnnotatedValue;
use hyper::http::{HeaderMap, Method};
use serde_json::Value;

// An input request from external API gateways, that is then resolved to a worker request, using API definitions
pub struct InputHttpRequest<'a> {
    pub input_path: ApiInputPath<'a>,
    pub headers: &'a HeaderMap,
    pub req_method: &'a Method,
    pub req_body: serde_json::Value,
}

impl InputHttpRequest<'_> {
    // Converts all request details to type-annotated-value
    // and place them under the key `request`
    pub fn get_type_annotated_value(
        &self,
        spec_query_variables: Vec<String>,
        spec_path_variables: &HashMap<usize, String>,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let request_body = &self.req_body;
        let request_header = self.headers;
        let request_path_values: HashMap<usize, String> = self.input_path.path_components();

        let request_query_variables: HashMap<String, String> = self.input_path.query_components();

        let request_header_values = Self::get_headers(request_header)?;
        let body_value = Self::get_request_body(request_body)?;
        let path_value = Self::get_request_path_query_values(
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

    pub fn get_request_body(request_body: &Value) -> Result<TypeAnnotatedValue, Vec<String>> {
        let inferred_type = infer_analysed_type(request_body);
        let typed_value = get_typed_value_from_json(&request_body, &inferred_type)?;

        Ok(TypeAnnotatedValue::Record {
            value: vec![("body".to_string(), typed_value)],
            typ: vec![("body".to_string(), inferred_type)],
        })
    }

    pub fn get_headers(headers: &HeaderMap) -> Result<TypeAnnotatedValue, Vec<String>> {
        let mut headers_map: Vec<(String, TypeAnnotatedValue)> = vec![];

        for (header_name, header_value) in headers {
            let header_value_str = header_value.to_str().map_err(|err| vec![err.to_string()])?;

            headers_map.push((
                header_name.to_string(),
                TypeAnnotatedValue::Str(header_value_str.to_string()),
            ));
        }

        let type_annotated_value = TypeAnnotatedValue::Record {
            value: headers_map.clone(),
            typ: headers_map
                .clone()
                .iter()
                .map(|(key, _)| (key.clone(), AnalysedType::Str))
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

    pub fn get_request_path_query_values(
        request_query_variables: HashMap<String, String>,
        spec_query_variables: Vec<String>,
        request_path_values: &HashMap<usize, String>,
        spec_path_variables: &HashMap<usize, String>,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let request_query_values =
            Self::get_request_query_values(request_query_variables, spec_query_variables)?;

        let request_path_values =
            Self::get_request_path_values(&request_path_values, spec_path_variables)?;

        let path_values = request_query_values.merge(&request_path_values);

        Ok(TypeAnnotatedValue::Record {
            value: vec![("path".to_string(), path_values.clone())],
            typ: vec![("path".to_string(), AnalysedType::from(&path_values))],
        })
    }

    pub fn get_request_path_values(
        request_path_values: &HashMap<usize, String>,
        spec_path_variables: &HashMap<usize, String>,
    ) -> Result<TypeAnnotatedValue, Vec<String>> {
        let mut unavailable_path_variables: Vec<String> = vec![];
        let mut path_variables_map = vec![];

        for (index, spec_path_variable) in spec_path_variables.iter() {
            if let Some(path_value) = request_path_values.get(index) {
                path_variables_map.push((
                    spec_path_variable.clone(),
                    TypeAnnotatedValue::Str(path_value.trim().to_string()),
                ));
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
                    .map(|(key, _)| (key.clone(), AnalysedType::Str))
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
                query_variable_map.push((
                    spec_query_variable.clone(),
                    TypeAnnotatedValue::Str(query_value.trim().to_string()),
                ));
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
                    .map(|(key, _)| (key.clone(), AnalysedType::Str))
                    .collect::<Vec<(String, AnalysedType)>>(),
            };
            Ok(type_annotated_value)
        } else {
            Err(unavailable_query_variables)
        }
    }
}

#[derive(PartialEq, Debug, Display, FromStr, Into)]
pub struct WorkerRequestResolutionError(pub String);

pub struct ApiInputPath<'a> {
    pub base_path: &'a str,
    pub query_path: Option<&'a str>,
}

impl<'a> ApiInputPath<'a> {
    // Return the each component of the path which can either be a literal or the value of a path_var, along with it's index
    pub fn path_components(&self) -> HashMap<usize, String> {
        let mut path_components: HashMap<usize, String> = HashMap::new();

        // initial `/` is excluded to not break indexes
        let path = if self.base_path.starts_with('/') {
            &self.base_path[1..self.base_path.len()]
        } else {
            self.base_path
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
    pub fn query_components(&self) -> HashMap<String, String> {
        let mut query_components: HashMap<String, String> = HashMap::new();

        if let Some(query_path) = self.query_path {
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

#[cfg(test)]
mod tests {
    use crate::api_definition::ApiDefinition;
    use crate::worker_request::WorkerRequest;

    use crate::api_request_route_resolver::WorkerBindingResolver;
    use golem_common::model::TemplateId;
    use http::{HeaderMap, HeaderName, HeaderValue, Method};

    use crate::http_request::{ApiInputPath, InputHttpRequest};

    #[test]
    fn test_worker_request_resolution() {
        let empty_headers = HeaderMap::new();
        let api_request = get_api_request("foo/1", None, &empty_headers, serde_json::Value::Null);
        let function_params = "[\"a\", \"b\"]";

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition =
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

        let api_specification: ApiDefinition =
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

        let api_specification: ApiDefinition =
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

        let api_specification: ApiDefinition =
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

        let api_specification: ApiDefinition =
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

        let api_specification: ApiDefinition = get_api_spec(
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

            let api_specification: ApiDefinition = get_api_spec(
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

    fn get_api_request<'a>(
        base_path: &'a str,
        query_path: Option<&'a str>,
        headers: &'a HeaderMap,
        req_body: serde_json::Value,
    ) -> InputHttpRequest<'a> {
        InputHttpRequest {
            input_path: ApiInputPath {
                base_path,
                query_path,
            },
            headers,
            req_method: &Method::GET,
            req_body,
        }
    }

    fn get_api_spec(path_pattern: &str, worker_id: &str, function_params: &str) -> ApiDefinition {
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
}
