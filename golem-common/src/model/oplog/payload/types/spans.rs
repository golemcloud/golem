// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::model::invocation_context::AttributeValue;
use crate::model::oplog::{
    PublicAttribute, PublicExternalSpanData, PublicLocalSpanData, PublicSpanData, SpanData,
};

pub fn encode_span_data(spans: &[SpanData]) -> Vec<Vec<PublicSpanData>> {
    let mut result = Vec::new();
    let mut current = Vec::new();

    for span in spans.iter().rev() {
        match span {
            SpanData::LocalSpan {
                span_id,
                start,
                parent_id,
                linked_context,
                attributes,
                inherited,
            } => {
                let linked_context = if let Some(linked_context) = linked_context {
                    let mut encoded_linked_context = encode_span_data(linked_context);

                    // Before merging encoded_linked_context into result, we need to adjust the indices in it
                    for spans in encoded_linked_context.iter_mut() {
                        for span in spans.iter_mut() {
                            match span {
                                PublicSpanData::LocalSpan(local_span) => {
                                    if let Some(idx) = local_span.linked_context.as_mut() {
                                        *idx += (result.len() as u64) + 1;
                                    }
                                }
                                PublicSpanData::ExternalSpan(_) => {}
                            }
                        }
                    }

                    result.extend(encoded_linked_context);

                    let id = result.len() as u64 + 1;
                    Some(id)
                } else {
                    None
                };
                let span_data = PublicSpanData::LocalSpan(PublicLocalSpanData {
                    span_id: span_id.clone(),
                    start: *start,
                    parent_id: parent_id.clone(),
                    linked_context,
                    attributes: attributes
                        .iter()
                        .map(|(k, v)| PublicAttribute {
                            key: k.clone(),
                            value: v.clone().into(),
                        })
                        .collect(),
                    inherited: *inherited,
                });
                current.insert(0, span_data);
            }
            SpanData::ExternalSpan { span_id } => {
                let span_data = PublicSpanData::ExternalSpan(PublicExternalSpanData {
                    span_id: span_id.clone(),
                });
                current.insert(0, span_data);
            }
        }
    }

    for stack in &mut result {
        for span in stack {
            if let PublicSpanData::LocalSpan(local_span) = span
                && let Some(linked_id) = &mut local_span.linked_context
            {
                *linked_id += 1;
            }
        }
    }
    result.insert(0, current);
    result
}

pub fn decode_span_data(spans: Vec<Vec<PublicSpanData>>) -> Vec<SpanData> {
    let mut result = Vec::new();
    let mut linked_contexts = Vec::new();

    for stack in spans {
        linked_contexts.push(stack);
    }

    if !linked_contexts.is_empty() {
        let current = linked_contexts.remove(0);
        for span in current {
            match span {
                PublicSpanData::LocalSpan(local_span) => {
                    let linked_context = if let Some(idx) = local_span.linked_context {
                        let linked_idx = (idx - 1) as usize;
                        if linked_idx < linked_contexts.len() {
                            Some(decode_span_data(vec![linked_contexts[linked_idx].clone()]))
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    result.push(SpanData::LocalSpan {
                        span_id: local_span.span_id,
                        start: local_span.start,
                        parent_id: local_span.parent_id,
                        linked_context,
                        attributes: local_span
                            .attributes
                            .into_iter()
                            .map(|attr| (attr.key, AttributeValue::from(attr.value)))
                            .collect(),
                        inherited: local_span.inherited,
                    });
                }
                PublicSpanData::ExternalSpan(external_span) => {
                    result.push(SpanData::ExternalSpan {
                        span_id: external_span.span_id,
                    });
                }
            }
        }
    }

    result
}
