use crate::api_definition::http::{QueryInfo, VarInfo};

use http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum RequestDetails {
    Http(HttpRequestDetails),
}
impl RequestDetails {
    pub fn from(
        path_params: &HashMap<VarInfo, &str>,
        query_variable_values: &HashMap<String, String>,
        query_variable_names: &[QueryInfo],
        request_body: &Value,
        headers: &HeaderMap,
    ) -> Result<Self, Vec<String>> {
        Ok(Self::Http(HttpRequestDetails::from_input_http_request(
            path_params,
            query_variable_values,
            query_variable_names,
            request_body,
            headers,
        )?))
    }

    pub fn as_json(&self) -> Value {
        match self {
            RequestDetails::Http(http_request_details) => {
                let typed_path_values = http_request_details.request_path_values.clone().0;
                let typed_query_values = http_request_details.request_query_values.clone().0;

                let mut path_values = serde_json::Map::new();

                for field in typed_path_values.fields.iter() {
                    path_values.insert(field.name.clone(), field.value.clone());
                }

                for field in typed_query_values.fields.iter() {
                    path_values.insert(field.name.clone(), field.value.clone());
                }

                let merged_request_path_and_query = Value::Object(path_values);

                let mut header_records = serde_json::Map::new();

                for field in http_request_details.request_header_values.0.fields.iter() {
                    header_records.insert(field.name.clone(), field.value.clone());
                }

                let header_value = Value::Object(header_records);

                Value::Object(serde_json::Map::from_iter(vec![
                    ("path".to_string(), merged_request_path_and_query),
                    (
                        "body".to_string(),
                        http_request_details.request_body.0.clone(),
                    ),
                    ("headers".to_string(), header_value),
                ]))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct HttpRequestDetails {
    pub request_path_values: RequestPathValues,
    pub request_body: RequestBody,
    pub request_query_values: RequestQueryValues,
    pub request_header_values: RequestHeaderValues,
}

impl HttpRequestDetails {
    pub fn empty() -> HttpRequestDetails {
        HttpRequestDetails {
            request_path_values: RequestPathValues(JsonKeyValues::default()),
            request_body: RequestBody(Value::Null),
            request_query_values: RequestQueryValues(JsonKeyValues::default()),
            request_header_values: RequestHeaderValues(JsonKeyValues::default()),
        }
    }

    pub fn get_accept_content_type_header(&self) -> Option<String> {
        self.request_header_values
            .0
            .fields
            .iter()
            .find(|field| field.name == http::header::ACCEPT.to_string())
            .and_then(|field| field.value.as_str().map(|x| x.to_string()))
    }

    fn from_input_http_request(
        path_params: &HashMap<VarInfo, &str>,
        query_variable_values: &HashMap<String, String>,
        query_variable_names: &[QueryInfo],
        request_body: &Value,
        headers: &HeaderMap,
    ) -> Result<Self, Vec<String>> {
        let request_body = RequestBody::from(request_body)?;
        let path_params = RequestPathValues::from(path_params);
        let query_params = RequestQueryValues::from(query_variable_values, query_variable_names)?;
        let header_params = RequestHeaderValues::from(headers)?;

        Ok(Self {
            request_path_values: path_params,
            request_body,
            request_query_values: query_params,
            request_header_values: header_params,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct RequestPathValues(pub JsonKeyValues);

impl RequestPathValues {
    fn from(path_variables: &HashMap<VarInfo, &str>) -> RequestPathValues {
        let record_fields: Vec<JsonKeyValue> = path_variables
            .iter()
            .map(|(key, value)| JsonKeyValue {
                name: key.key_name.clone(),
                value: internal::refine_json_str_value(value),
            })
            .collect();

        RequestPathValues(JsonKeyValues {
            fields: record_fields,
        })
    }
}

#[derive(Debug, Clone)]
pub struct RequestQueryValues(pub JsonKeyValues);

impl RequestQueryValues {
    fn from(
        query_key_values: &HashMap<String, String>,
        query_keys: &[QueryInfo],
    ) -> Result<RequestQueryValues, Vec<String>> {
        let mut unavailable_query_variables: Vec<String> = vec![];
        let mut query_variable_map: JsonKeyValues = JsonKeyValues::default();

        for spec_query_variable in query_keys.iter() {
            let key = &spec_query_variable.key_name;
            if let Some(query_value) = query_key_values.get(key) {
                let typed_value = internal::refine_json_str_value(query_value);
                query_variable_map.push(key.clone(), typed_value);
            } else {
                unavailable_query_variables.push(spec_query_variable.to_string());
            }
        }

        if unavailable_query_variables.is_empty() {
            Ok(RequestQueryValues(query_variable_map))
        } else {
            Err(unavailable_query_variables)
        }
    }
}

#[derive(Debug, Clone)]
pub struct RequestHeaderValues(JsonKeyValues);
impl RequestHeaderValues {
    fn from(headers: &HeaderMap) -> Result<RequestHeaderValues, Vec<String>> {
        let mut headers_map: JsonKeyValues = JsonKeyValues::default();

        for (header_name, header_value) in headers {
            let header_value_str = header_value.to_str().map_err(|err| vec![err.to_string()])?;

            let typed_header_value = internal::refine_json_str_value(header_value_str);

            headers_map.push(header_name.to_string(), typed_header_value);
        }

        Ok(RequestHeaderValues(headers_map))
    }
}

#[derive(Debug, Clone)]
pub struct RequestBody(Value);

impl RequestBody {
    fn from(request_body: &Value) -> Result<RequestBody, Vec<String>> {
        Ok(RequestBody(request_body.clone()))
    }
}

#[derive(Clone, Debug, Default)]
pub struct JsonKeyValues {
    pub fields: Vec<JsonKeyValue>,
}

impl JsonKeyValues {
    pub fn push(&mut self, key: String, value: Value) {
        self.fields.push(JsonKeyValue { name: key, value });
    }
}

#[derive(Clone, Debug)]
pub struct JsonKeyValue {
    pub name: String,
    pub value: Value,
}

mod internal {
    use rib::{CoercedNumericValue, LiteralValue};
    use serde_json::Value;

    pub(crate) fn refine_json_str_value(value: impl AsRef<str>) -> Value {
        let primitive = LiteralValue::from(value.as_ref().to_string());
        match primitive {
            LiteralValue::Num(number) => match number {
                CoercedNumericValue::PosInt(value) => {
                    Value::Number(serde_json::Number::from(value))
                }
                CoercedNumericValue::NegInt(value) => {
                    Value::Number(serde_json::Number::from(value))
                }
                CoercedNumericValue::Float(value) => {
                    Value::Number(serde_json::Number::from_f64(value).unwrap())
                }
            },
            LiteralValue::String(value) => Value::String(value),
            LiteralValue::Bool(value) => Value::Bool(value),
        }
    }
}
