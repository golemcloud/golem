// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_wasm_rpc::json::TypeAnnotatedValueJsonExtensions;
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use golem_wasm_rpc::protobuf::TypedRecord;
use http::HeaderMap;
use poem::web::headers::ContentType;
use rib::GetLiteralValue;
use std::collections::HashMap;
use std::str::FromStr;

#[derive(Default, Debug, PartialEq)]
pub struct ResolvedResponseHeaders {
    pub headers: HeaderMap,
}

impl ResolvedResponseHeaders {
    pub fn from_typed_value(
        header_map: &TypeAnnotatedValue,
    ) -> Result<ResolvedResponseHeaders, String> {
        match header_map {
            TypeAnnotatedValue::Record(TypedRecord { value, .. }) => {
                let mut resolved_headers: HashMap<String, String> = HashMap::new();

                for name_value_pair in value {
                    let value_str = name_value_pair
                        .value
                        .as_ref()
                        .and_then(|v| v.type_annotated_value.clone())
                        .ok_or("Unable to resolve header value".to_string())?
                        .get_literal()
                        .map(|primitive| primitive.to_string())
                        .unwrap_or_else(|| "Unable to resolve header".to_string());

                    resolved_headers.insert(name_value_pair.name.clone(), value_str);
                }

                let headers = (&resolved_headers)
                    .try_into()
                    .map_err(|e: http::Error| e.to_string())
                    .map_err(|e| format!("Unable to resolve valid headers. Error: {e}"))?;

                Ok(ResolvedResponseHeaders { headers })
            }

            _ => Err(format!(
                "Header expression is not a record. It is resolved to {}",
                header_map.to_json_value()
            )),
        }
    }

    pub fn get_content_type(&self) -> Option<ContentType> {
        self.headers
            .get(http::header::CONTENT_TYPE.to_string())
            .and_then(|header_value| {
                header_value
                    .to_str()
                    .ok()
                    .and_then(|header_str| ContentType::from_str(header_str).ok())
            })
    }
}

#[cfg(test)]
mod test {
    use golem_wasm_rpc::protobuf::{
        type_annotated_value::TypeAnnotatedValue, NameTypePair, NameValuePair, Type, TypedRecord,
    };

    #[allow(dead_code)]
    fn create_record(values: Vec<(String, TypeAnnotatedValue)>) -> TypeAnnotatedValue {
        let mut name_type_pairs = vec![];
        let mut name_value_pairs = vec![];

        for (key, value) in values.iter() {
            let typ = Type::try_from(value).unwrap();
            name_type_pairs.push(NameTypePair {
                name: key.to_string(),
                typ: Some(typ),
            });

            name_value_pairs.push(NameValuePair {
                name: key.to_string(),
                value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(value.clone()),
                }),
            });
        }

        TypeAnnotatedValue::Record(TypedRecord {
            typ: name_type_pairs,
            value: name_value_pairs,
        })
    }

    #[test]
    fn test_get_response_headers_from_typed_value() {
        let header_map = create_record(vec![
            (
                "header1".to_string(),
                TypeAnnotatedValue::Str("value1".to_string()),
            ),
            ("header2".to_string(), TypeAnnotatedValue::F32(1.0)),
        ]);

        let resolved_headers = ResolvedResponseHeaders::from_typed_value(&header_map).unwrap();

        let mut map = HashMap::new();

        map.insert("header1".to_string(), "value1".to_string());
        map.insert("header2".to_string(), "1".to_string());

        let header_map: HeaderMap = map.try_into().unwrap();

        let expected = ResolvedResponseHeaders {
            headers: header_map,
        };

        assert_eq!(resolved_headers, expected)
    }
}
