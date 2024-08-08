use crate::api_definition::http::{QueryInfo, VarInfo};
use crate::merge::Merge;

use crate::primitive::GetPrimitive;
use golem_service_base::type_inference::infer_analysed_type;
use golem_wasm_ast::analysis::{AnalysedType, TypeRecord};
use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::{NameTypePair, NameValuePair, TypedRecord};
use golem_wasm_rpc::protobuf::{Type, TypeAnnotatedValue as RootTypeAnnotatedValue};
use http::HeaderMap;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug)]
pub enum RequestDetails {
    Http(TypedHttRequestDetails),
}
impl RequestDetails {
    pub fn from(
        path_params: &HashMap<VarInfo, &str>,
        query_variable_values: &HashMap<String, String>,
        query_variable_names: &[QueryInfo],
        request_body: &Value,
        headers: &HeaderMap,
    ) -> Result<Self, Vec<String>> {
        Ok(Self::Http(TypedHttRequestDetails::from_input_http_request(
            path_params,
            query_variable_values,
            query_variable_names,
            request_body,
            headers,
        )?))
    }

    pub fn to_type_annotated_value(&self) -> TypeAnnotatedValue {
        match self {
            RequestDetails::Http(http) => http.clone().to_type_annotated_value(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TypedHttRequestDetails {
    pub typed_path_key_values: TypedPathKeyValues,
    pub typed_request_body: TypedRequestBody,
    pub typed_query_values: TypedQueryKeyValues,
    pub typed_header_values: TypedHeaderValues,
}

impl TypedHttRequestDetails {
    pub fn empty() -> TypedHttRequestDetails {
        TypedHttRequestDetails {
            typed_path_key_values: TypedPathKeyValues(TypedKeyValueCollection::default()),
            typed_request_body: TypedRequestBody(TypeAnnotatedValue::Record(TypedRecord {
                value: vec![],
                typ: vec![],
            })),
            typed_query_values: TypedQueryKeyValues(TypedKeyValueCollection::default()),
            typed_header_values: TypedHeaderValues(TypedKeyValueCollection::default()),
        }
    }

    pub fn get_accept_content_type_header(&self) -> Option<String> {
        self.typed_header_values
            .0
            .fields
            .iter()
            .find(|field| field.name == http::header::ACCEPT.to_string())
            .and_then(|field| field.value.get_primitive().map(|x| x.as_string()))
    }

    fn to_type_annotated_value(&self) -> TypeAnnotatedValue {
        let mut typed_path_values: TypeAnnotatedValue = self.typed_path_key_values.clone().0.into();
        let typed_query_values: TypeAnnotatedValue = self.typed_query_values.clone().0.into();
        let merged_type_annotated_value = typed_path_values.merge(&typed_query_values).clone();

        TypeAnnotatedValue::Record(TypedRecord {
            typ: vec![
                NameTypePair {
                    name: "path".to_string(),
                    typ: Type::try_from(&merged_type_annotated_value).ok(),
                },
                NameTypePair {
                    name: "body".to_string(),
                    typ: Type::try_from(&self.typed_request_body.0).ok(),
                },
                NameTypePair {
                    name: "headers".to_string(),
                    typ: {
                        let typ: AnalysedType = self.typed_header_values.0.clone().into();
                        Some((&typ).into())
                    },
                },
            ],
            value: vec![
                NameValuePair {
                    name: "path".to_string(),
                    value: Some(RootTypeAnnotatedValue {
                        type_annotated_value: Some(merged_type_annotated_value),
                    }),
                },
                NameValuePair {
                    name: "body".to_string(),
                    value: Some({
                        RootTypeAnnotatedValue {
                            type_annotated_value: Some(self.typed_request_body.0.clone()),
                        }
                    }),
                },
                NameValuePair {
                    name: "headers".to_string(),
                    value: Some({
                        RootTypeAnnotatedValue {
                            type_annotated_value: Some(self.typed_header_values.clone().0.into()),
                        }
                    }),
                },
            ],
        })
    }

    fn from_input_http_request(
        path_params: &HashMap<VarInfo, &str>,
        query_variable_values: &HashMap<String, String>,
        query_variable_names: &[QueryInfo],
        request_body: &Value,
        headers: &HeaderMap,
    ) -> Result<Self, Vec<String>> {
        let request_body = TypedRequestBody::from(request_body)?;
        let path_params = TypedPathKeyValues::from(path_params);
        let query_params = TypedQueryKeyValues::from(query_variable_values, query_variable_names)?;
        let header_params = TypedHeaderValues::from(headers)?;

        Ok(Self {
            typed_path_key_values: path_params,
            typed_request_body: request_body,
            typed_query_values: query_params,
            typed_header_values: header_params,
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct TypedPathKeyValues(pub TypedKeyValueCollection);

impl TypedPathKeyValues {
    fn from(path_variables: &HashMap<VarInfo, &str>) -> TypedPathKeyValues {
        let record_fields: Vec<TypedKeyValue> = path_variables
            .iter()
            .map(|(key, value)| TypedKeyValue {
                name: key.key_name.clone(),
                value: internal::get_typed_value_from_primitive(value),
            })
            .collect();

        TypedPathKeyValues(TypedKeyValueCollection {
            fields: record_fields,
        })
    }
}

#[derive(Debug, Clone)]
pub struct TypedQueryKeyValues(pub TypedKeyValueCollection);

impl TypedQueryKeyValues {
    fn from(
        query_key_values: &HashMap<String, String>,
        query_keys: &[QueryInfo],
    ) -> Result<TypedQueryKeyValues, Vec<String>> {
        let mut unavailable_query_variables: Vec<String> = vec![];
        let mut query_variable_map: TypedKeyValueCollection = TypedKeyValueCollection::default();

        for spec_query_variable in query_keys.iter() {
            let key = &spec_query_variable.key_name;
            if let Some(query_value) = query_key_values.get(key) {
                let typed_value = internal::get_typed_value_from_primitive(query_value);
                query_variable_map.push(key.clone(), typed_value);
            } else {
                unavailable_query_variables.push(spec_query_variable.to_string());
            }
        }

        if unavailable_query_variables.is_empty() {
            Ok(TypedQueryKeyValues(query_variable_map))
        } else {
            Err(unavailable_query_variables)
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypedHeaderValues(TypedKeyValueCollection);
impl TypedHeaderValues {
    fn from(headers: &HeaderMap) -> Result<TypedHeaderValues, Vec<String>> {
        let mut headers_map: TypedKeyValueCollection = TypedKeyValueCollection::default();

        for (header_name, header_value) in headers {
            let header_value_str = header_value.to_str().map_err(|err| vec![err.to_string()])?;

            let typed_header_value = internal::get_typed_value_from_primitive(header_value_str);

            headers_map.push(header_name.to_string(), typed_header_value);
        }

        Ok(TypedHeaderValues(headers_map))
    }
}

#[derive(Debug, Clone)]
pub struct TypedRequestBody(TypeAnnotatedValue);

impl TypedRequestBody {
    fn from(request_body: &Value) -> Result<TypedRequestBody, Vec<String>> {
        let inferred_type = infer_analysed_type(request_body);
        let typed_value = TypeAnnotatedValue::parse_with_type(request_body, &inferred_type)?;

        Ok(TypedRequestBody(typed_value))
    }
}

#[derive(Clone, Debug, Default)]
pub struct TypedKeyValueCollection {
    pub fields: Vec<TypedKeyValue>,
}

impl TypedKeyValueCollection {
    pub fn push(&mut self, key: String, value: TypeAnnotatedValue) {
        self.fields.push(TypedKeyValue { name: key, value });
    }
}

impl From<TypedKeyValueCollection> for AnalysedType {
    fn from(typed_key_value_collection: TypedKeyValueCollection) -> Self {
        let mut fields = Vec::new();

        for record in &typed_key_value_collection.fields {
            fields.push(golem_wasm_ast::analysis::NameTypePair {
                name: record.name.clone(),
                typ: AnalysedType::try_from(&record.value)
                    .expect("Internal error: Failed to retrieve type from Type Annotated Value"),
            });
        }

        AnalysedType::Record(TypeRecord { fields })
    }
}

impl From<TypedKeyValueCollection> for TypeAnnotatedValue {
    fn from(typed_key_value_collection: TypedKeyValueCollection) -> Self {
        let mut typ: Vec<NameTypePair> = vec![];
        let mut value: Vec<NameValuePair> = vec![];

        for record in typed_key_value_collection.fields {
            typ.push(NameTypePair {
                name: record.name.clone(),
                typ: Some(
                    Type::try_from(&record.value).expect(
                        "Internal error: Failed to retrieve type from Type Annotated Value",
                    ),
                ),
            });

            value.push(NameValuePair {
                name: record.name.clone(),
                value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(record.value),
                }),
            });
        }

        TypeAnnotatedValue::Record(TypedRecord { typ, value })
    }
}

#[derive(Clone, Debug)]
pub struct TypedKeyValue {
    pub name: String,
    pub value: TypeAnnotatedValue,
}

mod internal {

    use crate::primitive::{Number, Primitive};
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

    pub(crate) fn get_typed_value_from_primitive(value: impl AsRef<str>) -> TypeAnnotatedValue {
        let primitive = Primitive::from(value.as_ref().to_string());
        match primitive {
            Primitive::Num(number) => match number {
                Number::PosInt(value) => TypeAnnotatedValue::U64(value),
                Number::NegInt(value) => TypeAnnotatedValue::S64(value),
                Number::Float(value) => TypeAnnotatedValue::F64(value),
            },
            Primitive::String(value) => TypeAnnotatedValue::Str(value),
            Primitive::Bool(value) => TypeAnnotatedValue::Bool(value),
        }
    }
}
