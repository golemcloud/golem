// Copyright 2024-2025 Golem Cloud
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

use bitflags::bitflags;
use golem_common::model::invocation_context::{InvocationContextStack, SpanId, TraceId};
use golem_wasm_ast::analysis::AnalysedType;
use golem_wasm_rpc::{Value, ValueAndType};
use http::{HeaderMap, HeaderValue};
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
                        .unwrap_or_else(|| "unable to infer header".to_string());

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

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct TraceFlags: u32 {
        const SAMPLED = 0b00000001;
        const RANDOM = 0b00000010;
    }
}

// TODO: move to golem-common
/// Simple Trace Context header implementation
///
/// See https://www.w3.org/TR/trace-context-2
#[derive(Debug, Clone, PartialEq)]
pub struct TraceContextHeaders {
    pub version: u8,
    pub trace_id: TraceId,
    pub parent_id: SpanId,
    pub trace_flags: TraceFlags,
    pub trace_states: Vec<String>,
}

impl TraceContextHeaders {
    pub fn parse(headers: &HeaderMap) -> Option<TraceContextHeaders> {
        let trace_parent = headers.get("traceparent")?;
        let parts = trace_parent
            .to_str()
            .ok()?
            .split('-')
            .collect::<Vec<&str>>();
        if parts.len() == 4 {
            let version = u8::from_str_radix(parts[0], 16).ok()?;
            let trace_id = TraceId::from_string(parts[1]).ok()?;
            let parent_id = SpanId::from_string(parts[2]).ok()?;
            let trace_flags = u8::from_str_radix(parts[3], 16).ok()?;

            let trace_state_headers: Vec<HeaderValue> =
                headers.get_all("tracestate").iter().cloned().collect();
            let mut trace_states = Vec::new();

            for hdr in trace_state_headers {
                if let Ok(hdr) = hdr.to_str() {
                    let states = hdr
                        .split_terminator(',')
                        .map(|s| s.to_string())
                        .collect::<Vec<String>>();
                    trace_states.extend(states);
                }
            }

            Some(TraceContextHeaders {
                version,
                trace_id,
                parent_id,
                trace_flags: TraceFlags::from_bits_truncate(trace_flags as u32),
                trace_states,
            })
        } else {
            None
        }
    }

    pub fn from_invocation_context(
        invocation_context: InvocationContextStack,
    ) -> TraceContextHeaders {
        Self {
            version: 0,
            trace_id: invocation_context.trace_id,
            parent_id: invocation_context.spans.first().span_id.clone(),
            trace_flags: TraceFlags::empty(),
            trace_states: invocation_context.trace_states,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::headers::{ResolvedResponseHeaders, TraceContextHeaders, TraceFlags};
    use golem_common::model::invocation_context::{SpanId, TraceId};
    use golem_wasm_rpc::protobuf::{
        type_annotated_value::TypeAnnotatedValue, NameTypePair, NameValuePair, Type, TypedRecord,
    };
    use golem_wasm_rpc::ValueAndType;
    use http::{HeaderMap, HeaderValue};
    use std::num::{NonZeroU128, NonZeroU64};
    use test_r::test;

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
        let header_map: ValueAndType = create_record(vec![
            (
                "header1".to_string(),
                TypeAnnotatedValue::Str("value1".to_string()),
            ),
            ("header2".to_string(), TypeAnnotatedValue::F32(1.0)),
        ])
        .try_into()
        .unwrap();

        let resolved_headers = ResolvedResponseHeaders::from_typed_value(header_map).unwrap();

        let mut header_map = HeaderMap::new();

        header_map.insert("header1", HeaderValue::from_str("value1").unwrap());
        header_map.insert("header2", HeaderValue::from_str("1").unwrap());

        let expected = ResolvedResponseHeaders {
            headers: header_map,
        };

        assert_eq!(resolved_headers, expected)
    }

    #[test]
    fn trace_context_headers_1() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            HeaderValue::from_str("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")
                .unwrap(),
        );

        let result = TraceContextHeaders::parse(&headers);

        assert_eq!(
            result,
            Some(TraceContextHeaders {
                version: 0,
                trace_id: TraceId(NonZeroU128::new(0x4bf92f3577b34da6a3ce929d0e0e4736).unwrap()),
                parent_id: SpanId(NonZeroU64::new(0x00f067aa0ba902b7).unwrap()),
                trace_flags: TraceFlags::SAMPLED,
                trace_states: Vec::new()
            })
        )
    }

    #[test]
    fn trace_context_headers_2() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "traceparent",
            HeaderValue::from_str("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00")
                .unwrap(),
        );

        let result = TraceContextHeaders::parse(&headers);

        assert_eq!(
            result,
            Some(TraceContextHeaders {
                version: 0,
                trace_id: TraceId(NonZeroU128::new(0x4bf92f3577b34da6a3ce929d0e0e4736).unwrap()),
                parent_id: SpanId(NonZeroU64::new(0x00f067aa0ba902b7).unwrap()),
                trace_flags: TraceFlags::empty(),
                trace_states: Vec::new()
            })
        )
    }
}
