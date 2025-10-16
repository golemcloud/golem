// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use golem_wasm::analysis::AnalysedType;
use golem_wasm_rpc::{Value, ValueAndType};
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
    pub fn from_typed_value(header_map: ValueAndType) -> Result<ResolvedResponseHeaders, String> {
        match header_map {
            ValueAndType {
                value: Value::Record(field_values),
                typ: AnalysedType::Record(record),
            } => {
                let mut resolved_headers: HashMap<String, String> = HashMap::new();

                for (value, field_def) in field_values.into_iter().zip(record.fields) {
                    let value = ValueAndType::new(value, field_def.typ);
                    let value_str = value
                        .get_literal()
                        .map(|primitive| primitive.to_string())
                        .unwrap_or_else(|| {
                            "header values in the http response should be a literal".to_string()
                        });

                    resolved_headers.insert(field_def.name, value_str);
                }

                let headers = (&resolved_headers)
                    .try_into()
                    .map_err(|e: http::Error| e.to_string())
                    .map_err(|e| format!("unable to infer valid headers. Error: {e}"))?;

                Ok(ResolvedResponseHeaders { headers })
            }

            _ => Err(format!(
                "Header expression is not a record. It is resolved to {header_map}",
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
    use crate::headers::ResolvedResponseHeaders;
    use golem_wasm::analysis::analysed_type::{field, record};
    use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
    use http::{HeaderMap, HeaderValue};
    use test_r::test;

    fn create_record(values: Vec<(&str, ValueAndType)>) -> ValueAndType {
        ValueAndType::new(
            Value::Record(values.iter().map(|(_, vnt)| vnt.value.clone()).collect()),
            record(
                values
                    .iter()
                    .map(|(name, vnt)| field(name, vnt.typ.clone()))
                    .collect(),
            ),
        )
    }

    #[test]
    fn test_get_response_headers_from_typed_value() {
        let header_map: ValueAndType = create_record(vec![
            ("header1", "value1".into_value_and_type()),
            ("header2", 1.0f32.into_value_and_type()),
        ]);

        let resolved_headers = ResolvedResponseHeaders::from_typed_value(header_map).unwrap();

        let mut header_map = HeaderMap::new();

        header_map.insert("header1", HeaderValue::from_str("value1").unwrap());
        header_map.insert("header2", HeaderValue::from_str("1").unwrap());

        let expected = ResolvedResponseHeaders {
            headers: header_map,
        };

        assert_eq!(resolved_headers, expected)
    }
}
