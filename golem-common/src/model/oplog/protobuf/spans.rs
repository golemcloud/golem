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

use std::num::NonZeroU64;

use golem_api_grpc::proto::golem::worker::{
    AttributeValue, ExternalParentSpan, InvocationSpan, LocalInvocationSpan, invocation_span,
};

use crate::model::invocation_context::{SpanId, TraceId};
use crate::model::oplog::raw_types::{
    AttributeMap, SpanAttributes, SpanFinished, SpanKind, SpanLink, SpanOutcome, SpanStarted,
};
use crate::model::oplog::{
    LogTraceContext, PublicAttribute, PublicAttributeValue, PublicExternalSpanData,
    PublicLocalSpanData, PublicSpanAttributes, PublicSpanData, PublicSpanFinished, PublicSpanKind,
    PublicSpanLink, PublicSpanOutcome, PublicSpanStarted, StringAttributeValue,
};

impl From<PublicAttributeValue> for AttributeValue {
    fn from(value: PublicAttributeValue) -> Self {
        match value {
            PublicAttributeValue::String(StringAttributeValue { value }) => {
                golem_api_grpc::proto::golem::worker::AttributeValue {
                    value: Some(
                        golem_api_grpc::proto::golem::worker::attribute_value::Value::StringValue(
                            value,
                        ),
                    ),
                }
            }
        }
    }
}

impl TryFrom<AttributeValue> for PublicAttributeValue {
    type Error = String;

    fn try_from(value: AttributeValue) -> Result<Self, Self::Error> {
        match value.value {
            Some(golem_api_grpc::proto::golem::worker::attribute_value::Value::StringValue(
                value,
            )) => Ok(PublicAttributeValue::String(StringAttributeValue { value })),
            _ => Err("Invalid attribute value".to_string()),
        }
    }
}

pub(super) fn public_span_started_from_proto(
    value: golem_api_grpc::proto::golem::worker::SpanStarted,
) -> Result<PublicSpanStarted, String> {
    Ok(PublicSpanStarted {
        span_id: SpanId(NonZeroU64::new(value.span_id).ok_or("Span ID must be non-zero")?),
        trace_id: TraceId::from_string(value.trace_id)?,
        trace_states: value.trace_states,
        parent_span_id: value
            .parent_span_id
            .map(|id| {
                NonZeroU64::new(id)
                    .map(SpanId)
                    .ok_or("Parent span ID must be non-zero")
            })
            .transpose()?,
        links: value
            .links
            .into_iter()
            .map(|link| {
                Ok(PublicSpanLink {
                    trace_id: TraceId::from_string(link.trace_id)?,
                    span_id: SpanId(
                        NonZeroU64::new(link.span_id).ok_or("Link span ID must be non-zero")?,
                    ),
                    trace_states: link.trace_states,
                })
            })
            .collect::<Result<_, String>>()?,
        started_at: value.started_at.ok_or("Missing span started_at")?.into(),
        attributes: value
            .attributes
            .into_iter()
            .map(|(key, value)| value.try_into().map(|value| PublicAttribute { key, value }))
            .collect::<Result<_, _>>()?,
        kind: match golem_api_grpc::proto::golem::worker::SpanKind::try_from(value.kind)
            .map_err(|_| "Invalid span kind")?
        {
            golem_api_grpc::proto::golem::worker::SpanKind::Internal => PublicSpanKind::Internal,
            golem_api_grpc::proto::golem::worker::SpanKind::Client => PublicSpanKind::Client,
            golem_api_grpc::proto::golem::worker::SpanKind::Server => PublicSpanKind::Server,
        },
    })
}

impl From<LogTraceContext> for golem_api_grpc::proto::golem::worker::LogTraceContext {
    fn from(value: LogTraceContext) -> Self {
        Self {
            trace_id: value.trace_id.to_string(),
            span_id: value.span_id.to_string(),
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::worker::LogTraceContext> for LogTraceContext {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::worker::LogTraceContext,
    ) -> Result<Self, String> {
        Ok(Self {
            trace_id: TraceId::from_string(value.trace_id)?,
            span_id: SpanId::from_string(&value.span_id)?,
        })
    }
}

pub(super) fn public_span_started_to_proto(
    value: PublicSpanStarted,
) -> golem_api_grpc::proto::golem::worker::SpanStarted {
    golem_api_grpc::proto::golem::worker::SpanStarted {
        span_id: value.span_id.0.get(),
        trace_id: value.trace_id.to_string(),
        trace_states: value.trace_states,
        parent_span_id: value.parent_span_id.map(|id| id.0.get()),
        links: value
            .links
            .into_iter()
            .map(|link| golem_api_grpc::proto::golem::worker::SpanLink {
                trace_id: link.trace_id.to_string(),
                span_id: link.span_id.0.get(),
                trace_states: link.trace_states,
            })
            .collect(),
        started_at: Some(value.started_at.into()),
        attributes: value
            .attributes
            .into_iter()
            .map(|a| (a.key, a.value.into()))
            .collect(),
        kind: match value.kind {
            PublicSpanKind::Internal => golem_api_grpc::proto::golem::worker::SpanKind::Internal,
            PublicSpanKind::Client => golem_api_grpc::proto::golem::worker::SpanKind::Client,
            PublicSpanKind::Server => golem_api_grpc::proto::golem::worker::SpanKind::Server,
        } as i32,
    }
}

pub(super) fn public_span_finished_from_proto(
    value: golem_api_grpc::proto::golem::worker::SpanFinished,
) -> Result<PublicSpanFinished, String> {
    Ok(PublicSpanFinished {
        span_id: SpanId(NonZeroU64::new(value.span_id).ok_or("Span ID must be non-zero")?),
        finished_at: value.finished_at.ok_or("Missing span finished_at")?.into(),
        outcome: match golem_api_grpc::proto::golem::worker::SpanOutcome::try_from(value.outcome)
            .map_err(|_| "Invalid span outcome")?
        {
            golem_api_grpc::proto::golem::worker::SpanOutcome::Completed => {
                PublicSpanOutcome::Completed
            }
            golem_api_grpc::proto::golem::worker::SpanOutcome::Failed => PublicSpanOutcome::Failed,
            golem_api_grpc::proto::golem::worker::SpanOutcome::Cancelled => {
                PublicSpanOutcome::Cancelled
            }
            golem_api_grpc::proto::golem::worker::SpanOutcome::Abandoned => {
                PublicSpanOutcome::Abandoned
            }
            golem_api_grpc::proto::golem::worker::SpanOutcome::Denied => PublicSpanOutcome::Denied,
        },
    })
}

pub(super) fn public_span_finished_to_proto(
    value: PublicSpanFinished,
) -> golem_api_grpc::proto::golem::worker::SpanFinished {
    golem_api_grpc::proto::golem::worker::SpanFinished {
        span_id: value.span_id.0.get(),
        finished_at: Some(value.finished_at.into()),
        outcome: match value.outcome {
            PublicSpanOutcome::Completed => {
                golem_api_grpc::proto::golem::worker::SpanOutcome::Completed
            }
            PublicSpanOutcome::Failed => golem_api_grpc::proto::golem::worker::SpanOutcome::Failed,
            PublicSpanOutcome::Cancelled => {
                golem_api_grpc::proto::golem::worker::SpanOutcome::Cancelled
            }
            PublicSpanOutcome::Abandoned => {
                golem_api_grpc::proto::golem::worker::SpanOutcome::Abandoned
            }
            PublicSpanOutcome::Denied => golem_api_grpc::proto::golem::worker::SpanOutcome::Denied,
        } as i32,
    }
}

pub(super) fn public_span_attributes_from_proto(
    value: golem_api_grpc::proto::golem::worker::SpanAttributes,
) -> Result<PublicSpanAttributes, String> {
    Ok(PublicSpanAttributes {
        span_id: SpanId(NonZeroU64::new(value.span_id).ok_or("Span ID must be non-zero")?),
        attributes: value
            .attributes
            .into_iter()
            .map(|(key, value)| value.try_into().map(|value| PublicAttribute { key, value }))
            .collect::<Result<_, _>>()?,
    })
}

pub(super) fn public_span_attributes_to_proto(
    value: PublicSpanAttributes,
) -> golem_api_grpc::proto::golem::worker::SpanAttributes {
    golem_api_grpc::proto::golem::worker::SpanAttributes {
        span_id: value.span_id.0.get(),
        attributes: value
            .attributes
            .into_iter()
            .map(|a| (a.key, a.value.into()))
            .collect(),
    }
}

pub(super) fn raw_span_started_to_proto(
    value: SpanStarted,
) -> golem_api_grpc::proto::golem::worker::RawSpanStarted {
    golem_api_grpc::proto::golem::worker::RawSpanStarted {
        span_id: value.span_id.to_string(),
        trace_id: value.trace_id.to_string(),
        trace_states: value.trace_states,
        parent_span_id: value.parent_span_id.map(|id| id.to_string()),
        links: value
            .links
            .into_iter()
            .map(|link| golem_api_grpc::proto::golem::worker::RawSpanLink {
                trace_id: link.trace_id.to_string(),
                span_id: link.span_id.to_string(),
                trace_states: link.trace_states,
            })
            .collect(),
        started_at: Some(value.started_at.into()),
        attributes: value
            .attributes
            .0
            .into_iter()
            .map(|(key, value)| match value {
                crate::model::invocation_context::AttributeValue::String(value) => (key, value),
            })
            .collect(),
        kind: match value.kind {
            SpanKind::Internal => golem_api_grpc::proto::golem::worker::RawSpanKind::Internal,
            SpanKind::Client => golem_api_grpc::proto::golem::worker::RawSpanKind::Client,
            SpanKind::Server => golem_api_grpc::proto::golem::worker::RawSpanKind::Server,
        } as i32,
    }
}

pub(super) fn raw_span_started_from_proto(
    value: golem_api_grpc::proto::golem::worker::RawSpanStarted,
) -> Result<SpanStarted, String> {
    Ok(SpanStarted {
        span_id: SpanId::from_string(value.span_id)?,
        trace_id: TraceId::from_string(value.trace_id)?,
        trace_states: value.trace_states,
        parent_span_id: value.parent_span_id.map(SpanId::from_string).transpose()?,
        links: value
            .links
            .into_iter()
            .map(|link| {
                Ok(SpanLink {
                    trace_id: TraceId::from_string(link.trace_id)?,
                    span_id: SpanId::from_string(link.span_id)?,
                    trace_states: link.trace_states,
                })
            })
            .collect::<Result<_, String>>()?,
        started_at: value.started_at.ok_or("Missing span started_at")?.into(),
        attributes: AttributeMap(
            value
                .attributes
                .into_iter()
                .map(|(key, value)| {
                    (
                        key,
                        crate::model::invocation_context::AttributeValue::String(value),
                    )
                })
                .collect(),
        ),
        kind: match golem_api_grpc::proto::golem::worker::RawSpanKind::try_from(value.kind)
            .map_err(|_| "Invalid span kind")?
        {
            golem_api_grpc::proto::golem::worker::RawSpanKind::Internal => SpanKind::Internal,
            golem_api_grpc::proto::golem::worker::RawSpanKind::Client => SpanKind::Client,
            golem_api_grpc::proto::golem::worker::RawSpanKind::Server => SpanKind::Server,
        },
    })
}

pub(super) fn raw_span_finished_to_proto(
    value: SpanFinished,
) -> golem_api_grpc::proto::golem::worker::RawSpanFinished {
    golem_api_grpc::proto::golem::worker::RawSpanFinished {
        span_id: value.span_id.to_string(),
        finished_at: Some(value.finished_at.into()),
        outcome: match value.outcome {
            SpanOutcome::Completed => {
                golem_api_grpc::proto::golem::worker::RawSpanOutcome::Completed
            }
            SpanOutcome::Failed => golem_api_grpc::proto::golem::worker::RawSpanOutcome::Failed,
            SpanOutcome::Cancelled => {
                golem_api_grpc::proto::golem::worker::RawSpanOutcome::Cancelled
            }
            SpanOutcome::Abandoned => {
                golem_api_grpc::proto::golem::worker::RawSpanOutcome::Abandoned
            }
            SpanOutcome::Denied => golem_api_grpc::proto::golem::worker::RawSpanOutcome::Denied,
        } as i32,
    }
}

pub(super) fn raw_span_finished_from_proto(
    value: golem_api_grpc::proto::golem::worker::RawSpanFinished,
) -> Result<SpanFinished, String> {
    Ok(SpanFinished {
        span_id: SpanId::from_string(value.span_id)?,
        finished_at: value.finished_at.ok_or("Missing span finished_at")?.into(),
        outcome: match golem_api_grpc::proto::golem::worker::RawSpanOutcome::try_from(value.outcome)
            .map_err(|_| "Invalid span outcome")?
        {
            golem_api_grpc::proto::golem::worker::RawSpanOutcome::Completed => {
                SpanOutcome::Completed
            }
            golem_api_grpc::proto::golem::worker::RawSpanOutcome::Failed => SpanOutcome::Failed,
            golem_api_grpc::proto::golem::worker::RawSpanOutcome::Cancelled => {
                SpanOutcome::Cancelled
            }
            golem_api_grpc::proto::golem::worker::RawSpanOutcome::Abandoned => {
                SpanOutcome::Abandoned
            }
            golem_api_grpc::proto::golem::worker::RawSpanOutcome::Denied => SpanOutcome::Denied,
        },
    })
}

pub(super) fn raw_span_attributes_to_proto(
    value: SpanAttributes,
) -> golem_api_grpc::proto::golem::worker::RawSpanAttributes {
    golem_api_grpc::proto::golem::worker::RawSpanAttributes {
        span_id: value.span_id.to_string(),
        attributes: value
            .attributes
            .0
            .into_iter()
            .map(|(key, value)| match value {
                crate::model::invocation_context::AttributeValue::String(value) => (key, value),
            })
            .collect(),
    }
}

pub(super) fn raw_span_attributes_from_proto(
    value: golem_api_grpc::proto::golem::worker::RawSpanAttributes,
) -> Result<SpanAttributes, String> {
    Ok(SpanAttributes {
        span_id: SpanId::from_string(value.span_id)?,
        attributes: AttributeMap(
            value
                .attributes
                .into_iter()
                .map(|(key, value)| {
                    (
                        key,
                        crate::model::invocation_context::AttributeValue::String(value),
                    )
                })
                .collect(),
        ),
    })
}

pub(super) fn public_span_started_to_raw(value: PublicSpanStarted) -> SpanStarted {
    SpanStarted {
        span_id: value.span_id,
        trace_id: value.trace_id,
        trace_states: value.trace_states,
        parent_span_id: value.parent_span_id,
        links: value
            .links
            .into_iter()
            .map(|link| SpanLink {
                trace_id: link.trace_id,
                span_id: link.span_id,
                trace_states: link.trace_states,
            })
            .collect(),
        started_at: value.started_at,
        attributes: AttributeMap(
            value
                .attributes
                .into_iter()
                .map(|a| {
                    (
                        a.key,
                        match a.value {
                            PublicAttributeValue::String(v) => {
                                crate::model::invocation_context::AttributeValue::String(v.value)
                            }
                        },
                    )
                })
                .collect(),
        ),
        kind: match value.kind {
            PublicSpanKind::Internal => SpanKind::Internal,
            PublicSpanKind::Client => SpanKind::Client,
            PublicSpanKind::Server => SpanKind::Server,
        },
    }
}

pub(super) fn public_span_finished_to_raw(value: PublicSpanFinished) -> SpanFinished {
    SpanFinished {
        span_id: value.span_id,
        finished_at: value.finished_at,
        outcome: match value.outcome {
            PublicSpanOutcome::Completed => SpanOutcome::Completed,
            PublicSpanOutcome::Failed => SpanOutcome::Failed,
            PublicSpanOutcome::Cancelled => SpanOutcome::Cancelled,
            PublicSpanOutcome::Abandoned => SpanOutcome::Abandoned,
            PublicSpanOutcome::Denied => SpanOutcome::Denied,
        },
    }
}

pub(super) fn public_span_attributes_to_raw(value: PublicSpanAttributes) -> SpanAttributes {
    SpanAttributes {
        span_id: value.span_id,
        attributes: AttributeMap(
            value
                .attributes
                .into_iter()
                .map(|a| {
                    (
                        a.key,
                        match a.value {
                            PublicAttributeValue::String(v) => {
                                crate::model::invocation_context::AttributeValue::String(v.value)
                            }
                        },
                    )
                })
                .collect(),
        ),
    }
}

pub(super) fn encode_public_span_data(
    spans: Vec<InvocationSpan>,
) -> Result<Vec<Vec<PublicSpanData>>, String> {
    let mut result = Vec::new();
    let mut current = Vec::new();

    for span in spans.into_iter().rev() {
        match span.span {
            Some(invocation_span::Span::Local(span)) => {
                let linked_context = if !span.linked_context.is_empty() {
                    let id = result.len() as u64;
                    let encoded_linked_context = encode_public_span_data(span.linked_context)?;
                    result.extend(encoded_linked_context);
                    Some(id)
                } else {
                    None
                };
                let span_data = PublicSpanData::LocalSpan(PublicLocalSpanData {
                    span_id: SpanId(NonZeroU64::new(span.span_id).unwrap()),
                    start: span.start.ok_or("Missing start field")?.into(),
                    parent_id: current
                        .first()
                        .map(|span: &PublicSpanData| span.span_id().clone()),
                    linked_context,
                    attributes: span
                        .attributes
                        .into_iter()
                        .map(|(k, v)| v.try_into().map(|v| PublicAttribute { key: k, value: v }))
                        .collect::<Result<Vec<_>, _>>()?,
                    inherited: span.inherited,
                });
                current.insert(0, span_data);
            }
            Some(invocation_span::Span::ExternalParent(span)) => {
                let span_data = PublicSpanData::ExternalSpan(PublicExternalSpanData {
                    span_id: SpanId(NonZeroU64::new(span.span_id).unwrap()),
                });
                current.insert(0, span_data);
            }
            None => return Err("Missing span field".to_string()),
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

    Ok(result)
}

pub(super) fn decode_public_span_data(
    invocation_context: &Vec<Vec<PublicSpanData>>,
    idx: usize,
) -> Vec<InvocationSpan> {
    if idx >= invocation_context.len() {
        Vec::new()
    } else {
        let stack = &invocation_context[idx];
        let mut result = Vec::new();
        for span_data in stack {
            let span = InvocationSpan {
                span: Some(match span_data {
                    PublicSpanData::LocalSpan(local_span_data) => {
                        invocation_span::Span::Local(LocalInvocationSpan {
                            span_id: local_span_data.span_id.0.get(),
                            start: Some(local_span_data.start.into()),
                            linked_context: local_span_data
                                .linked_context
                                .map(|id| decode_public_span_data(invocation_context, id as usize))
                                .unwrap_or_default(),
                            attributes: local_span_data
                                .attributes
                                .iter()
                                .map(|attr| (attr.key.clone(), attr.value.clone().into()))
                                .collect(),
                            inherited: local_span_data.inherited,
                        })
                    }
                    PublicSpanData::ExternalSpan(external_span_data) => {
                        invocation_span::Span::ExternalParent(ExternalParentSpan {
                            span_id: external_span_data.span_id.0.get(),
                        })
                    }
                }),
            };
            result.push(span);
        }

        result
    }
}

pub(super) fn span_data_to_proto(
    span: crate::model::oplog::raw_types::SpanData,
) -> golem_api_grpc::proto::golem::worker::RawSpanData {
    use golem_api_grpc::proto::golem::worker::{
        RawExternalSpan, RawLocalSpan, RawSpanData as ProtoSpanData, raw_span_data,
    };

    use crate::model::oplog::raw_types::SpanData;
    match span {
        SpanData::LocalSpan {
            span_id,
            start,
            parent_id,
            linked_context,
            attributes,
            inherited,
        } => ProtoSpanData {
            span: Some(raw_span_data::Span::LocalSpan(RawLocalSpan {
                span_id: span_id.to_string(),
                start: Some(start.into()),
                parent_id: parent_id.map(|id| id.to_string()),
                linked_context: linked_context
                    .unwrap_or_default()
                    .into_iter()
                    .map(span_data_to_proto)
                    .collect(),
                attributes: attributes
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            match v {
                                crate::model::invocation_context::AttributeValue::String(s) => s,
                            },
                        )
                    })
                    .collect(),
                inherited,
            })),
        },
        SpanData::ExternalSpan { span_id } => ProtoSpanData {
            span: Some(raw_span_data::Span::ExternalSpan(RawExternalSpan {
                span_id: span_id.to_string(),
            })),
        },
    }
}

pub(super) fn span_data_from_proto(
    proto: golem_api_grpc::proto::golem::worker::RawSpanData,
) -> Result<crate::model::oplog::raw_types::SpanData, String> {
    use golem_api_grpc::proto::golem::worker::raw_span_data::Span;

    use crate::model::oplog::raw_types::SpanData;
    match proto.span.ok_or("Missing span data")? {
        Span::LocalSpan(local) => {
            let span_id = SpanId::from_string(local.span_id)?;
            let start: crate::model::Timestamp = local
                .start
                .ok_or("Missing start timestamp in LocalSpan")?
                .into();
            let parent_id = local.parent_id.map(SpanId::from_string).transpose()?;
            let linked_context = if local.linked_context.is_empty() {
                None
            } else {
                Some(
                    local
                        .linked_context
                        .into_iter()
                        .map(span_data_from_proto)
                        .collect::<Result<Vec<_>, _>>()?,
                )
            };
            let attributes = local
                .attributes
                .into_iter()
                .map(|(k, v)| {
                    (
                        k,
                        crate::model::invocation_context::AttributeValue::String(v),
                    )
                })
                .collect();
            Ok(SpanData::LocalSpan {
                span_id,
                start,
                parent_id,
                linked_context,
                attributes,
                inherited: local.inherited,
            })
        }
        Span::ExternalSpan(ext) => Ok(SpanData::ExternalSpan {
            span_id: SpanId::from_string(ext.span_id)?,
        }),
    }
}
