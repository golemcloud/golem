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

use bitflags::bitflags;
use golem_common::model::invocation_context::{InvocationContextStack, SpanId, TraceId};
use http::{HeaderMap, HeaderValue};

bitflags! {
    #[derive(Debug, Copy, Clone, PartialEq)]
    pub struct TraceFlags: u32 {
        const SAMPLED = 0b00000001;
        const RANDOM = 0b00000010;
    }
}

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
            parent_id: invocation_context.spans.first().span_id().clone(),
            trace_flags: TraceFlags::empty(),
            trace_states: invocation_context.trace_states,
        }
    }

    pub fn to_raw_headers_map(&self) -> Vec<(String, String)> {
        let mut headers = Vec::new();
        headers.push((
            "traceparent".to_string(),
            format!(
                "{:02x}-{}-{}-{:02x}",
                self.version,
                self.trace_id,
                self.parent_id,
                self.trace_flags.bits()
            ),
        ));

        if !self.trace_states.is_empty() {
            headers.push(("tracestate".to_string(), self.trace_states.join(",")));
        }

        headers
    }
}

#[cfg(test)]
mod test {
    use crate::headers::{TraceContextHeaders, TraceFlags};
    use golem_common::model::invocation_context::{SpanId, TraceId};
    use http::{HeaderMap, HeaderValue};
    use std::num::{NonZeroU128, NonZeroU64};
    use test_r::test;

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
        );
        assert_eq!(
            result.unwrap().to_raw_headers_map(),
            vec![(
                "traceparent".to_string(),
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01".to_string()
            )]
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
        headers.insert(
            "tracestate",
            HeaderValue::from_str("abc=123,defgh=010203").unwrap(),
        );
        headers.append("tracestate", HeaderValue::from_str("ijklmn=op").unwrap());

        let result = TraceContextHeaders::parse(&headers);

        assert_eq!(
            result,
            Some(TraceContextHeaders {
                version: 0,
                trace_id: TraceId(NonZeroU128::new(0x4bf92f3577b34da6a3ce929d0e0e4736).unwrap()),
                parent_id: SpanId(NonZeroU64::new(0x00f067aa0ba902b7).unwrap()),
                trace_flags: TraceFlags::empty(),
                trace_states: vec![
                    "abc=123".to_string(),
                    "defgh=010203".to_string(),
                    "ijklmn=op".to_string()
                ]
            })
        );
        assert_eq!(
            result.unwrap().to_raw_headers_map(),
            vec![
                (
                    "traceparent".to_string(),
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00".to_string()
                ),
                (
                    "tracestate".to_string(),
                    "abc=123,defgh=010203,ijklmn=op".to_string()
                )
            ]
        )
    }
}
