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

use super::*;

fn public_attribute_to_wit(value: PublicAttribute) -> oplog::Attribute {
    oplog::Attribute {
        key: value.key,
        value: value.value.into(),
    }
}

pub(super) fn log_trace_context_to_wit(
    value: golem_common::model::oplog::LogTraceContext,
) -> oplog::LogTraceContext {
    oplog::LogTraceContext {
        trace_id: value.trace_id.to_string(),
        span_id: value.span_id.to_string(),
    }
}

pub(super) fn log_trace_context_from_wit(
    value: oplog::LogTraceContext,
) -> Result<golem_common::model::oplog::LogTraceContext, String> {
    Ok(golem_common::model::oplog::LogTraceContext {
        trace_id: golem_common::model::invocation_context::TraceId::from_string(&value.trace_id)?,
        span_id: golem_common::model::invocation_context::SpanId::from_string(&value.span_id)?,
    })
}

pub(super) fn public_span_started_to_wit(value: PublicSpanStarted) -> oplog::SpanStarted {
    oplog::SpanStarted {
        span_id: value.span_id.to_string(),
        trace_id: value.trace_id.to_string(),
        trace_states: value.trace_states,
        parent_span_id: value.parent_span_id.map(|id| id.to_string()),
        links: value
            .links
            .into_iter()
            .map(|link| oplog::SpanLink {
                trace_id: link.trace_id.to_string(),
                span_id: link.span_id.to_string(),
                trace_states: link.trace_states,
            })
            .collect(),
        started_at: value.started_at.into(),
        attributes: value
            .attributes
            .into_iter()
            .map(public_attribute_to_wit)
            .collect(),
        kind: match value.kind {
            PublicSpanKind::Internal => oplog::SpanKind::Internal,
            PublicSpanKind::Client => oplog::SpanKind::Client,
            PublicSpanKind::Server => oplog::SpanKind::Server,
        },
    }
}

pub(super) fn public_span_finished_to_wit(value: PublicSpanFinished) -> oplog::SpanFinished {
    oplog::SpanFinished {
        span_id: value.span_id.to_string(),
        finished_at: value.finished_at.into(),
        outcome: match value.outcome {
            PublicSpanOutcome::Completed => oplog::SpanOutcome::Completed,
            PublicSpanOutcome::Failed => oplog::SpanOutcome::Failed,
            PublicSpanOutcome::Cancelled => oplog::SpanOutcome::Cancelled,
            PublicSpanOutcome::Abandoned => oplog::SpanOutcome::Abandoned,
            PublicSpanOutcome::Denied => oplog::SpanOutcome::Denied,
        },
    }
}

pub(super) fn public_span_attributes_to_wit(value: PublicSpanAttributes) -> oplog::SpanAttributes {
    oplog::SpanAttributes {
        span_id: value.span_id.to_string(),
        attributes: value
            .attributes
            .into_iter()
            .map(public_attribute_to_wit)
            .collect(),
    }
}

pub(super) fn raw_span_started_to_wit(
    value: golem_common::model::oplog::SpanStarted,
) -> oplog::SpanStarted {
    public_span_started_to_wit(super::super::public_span_started(value))
}

pub(super) fn raw_span_finished_to_wit(
    value: golem_common::model::oplog::SpanFinished,
) -> oplog::SpanFinished {
    public_span_finished_to_wit(super::super::public_span_finished(value))
}

pub(super) fn raw_span_attributes_to_wit(
    value: golem_common::model::oplog::SpanAttributes,
) -> oplog::SpanAttributes {
    public_span_attributes_to_wit(super::super::public_span_attributes(value))
}

fn raw_attributes_from_wit(
    attributes: Vec<oplog::Attribute>,
) -> golem_common::model::oplog::AttributeMap {
    golem_common::model::oplog::AttributeMap(
        attributes
            .into_iter()
            .map(|attribute| (attribute.key, attribute.value.into()))
            .collect(),
    )
}

pub(super) fn raw_span_started_from_wit(
    value: oplog::SpanStarted,
) -> Result<golem_common::model::oplog::SpanStarted, String> {
    use golem_common::model::invocation_context::{SpanId, TraceId};
    Ok(golem_common::model::oplog::SpanStarted {
        span_id: SpanId::from_string(&value.span_id)?,
        trace_id: TraceId::from_string(&value.trace_id)?,
        trace_states: value.trace_states,
        parent_span_id: value
            .parent_span_id
            .map(|id| SpanId::from_string(&id))
            .transpose()?,
        links: value
            .links
            .into_iter()
            .map(|link| {
                Ok(golem_common::model::oplog::SpanLink {
                    trace_id: TraceId::from_string(&link.trace_id)?,
                    span_id: SpanId::from_string(&link.span_id)?,
                    trace_states: link.trace_states,
                })
            })
            .collect::<Result<_, String>>()?,
        started_at: timestamp_from_datetime(value.started_at),
        attributes: raw_attributes_from_wit(value.attributes),
        kind: match value.kind {
            oplog::SpanKind::Internal => golem_common::model::oplog::SpanKind::Internal,
            oplog::SpanKind::Client => golem_common::model::oplog::SpanKind::Client,
            oplog::SpanKind::Server => golem_common::model::oplog::SpanKind::Server,
        },
    })
}

pub(super) fn raw_span_finished_from_wit(
    value: oplog::SpanFinished,
) -> Result<golem_common::model::oplog::SpanFinished, String> {
    Ok(golem_common::model::oplog::SpanFinished {
        span_id: golem_common::model::invocation_context::SpanId::from_string(&value.span_id)?,
        finished_at: timestamp_from_datetime(value.finished_at),
        outcome: match value.outcome {
            oplog::SpanOutcome::Completed => golem_common::model::oplog::SpanOutcome::Completed,
            oplog::SpanOutcome::Failed => golem_common::model::oplog::SpanOutcome::Failed,
            oplog::SpanOutcome::Cancelled => golem_common::model::oplog::SpanOutcome::Cancelled,
            oplog::SpanOutcome::Abandoned => golem_common::model::oplog::SpanOutcome::Abandoned,
            oplog::SpanOutcome::Denied => golem_common::model::oplog::SpanOutcome::Denied,
        },
    })
}

pub(super) fn raw_span_attributes_from_wit(
    value: oplog::SpanAttributes,
) -> Result<golem_common::model::oplog::SpanAttributes, String> {
    Ok(golem_common::model::oplog::SpanAttributes {
        span_id: golem_common::model::invocation_context::SpanId::from_string(&value.span_id)?,
        attributes: raw_attributes_from_wit(value.attributes),
    })
}

impl TryFrom<oplog::SpanData> for golem_common::model::oplog::SpanData {
    type Error = String;

    fn try_from(span: oplog::SpanData) -> Result<Self, String> {
        match span {
            oplog::SpanData::LocalSpan(local) => {
                let span_id =
                    golem_common::model::invocation_context::SpanId::from_string(&local.span_id)?;
                let start = timestamp_from_datetime(local.start);
                let parent_id = local
                    .parent
                    .map(|p| golem_common::model::invocation_context::SpanId::from_string(&p))
                    .transpose()?;
                let attributes = local
                    .attributes
                    .into_iter()
                    .map(|attr| (attr.key, attr.value.into()))
                    .collect();
                Ok(Self::LocalSpan {
                    span_id,
                    start,
                    parent_id,
                    linked_context: None,
                    attributes,
                    inherited: local.inherited,
                })
            }
            oplog::SpanData::ExternalSpan(ext) => {
                let span_id =
                    golem_common::model::invocation_context::SpanId::from_string(&ext.span_id)?;
                Ok(Self::ExternalSpan { span_id })
            }
        }
    }
}

// Note: From<oplog::AttributeValue> for AttributeValue is provided in invocation_context_api.rs

impl From<golem_common::model::oplog::SpanData> for oplog::SpanData {
    fn from(span: golem_common::model::oplog::SpanData) -> Self {
        match span {
            golem_common::model::oplog::SpanData::LocalSpan {
                span_id,
                start,
                parent_id,
                // The raw model carries `Option<Vec<SpanData>>`, but the WIT
                // `local-span-data.linked-context` is `option<u64>` (an index into the
                // agent-invocation's invocation-context list). There is no value to map
                // into at this conversion boundary, so it is dropped here exactly as the
                // reverse `TryFrom<oplog::SpanData> for SpanData` does (it sets `None`).
                linked_context: _,
                attributes,
                inherited,
            } => Self::LocalSpan(oplog::LocalSpanData {
                span_id: span_id.to_string(),
                start: start.into(),
                parent: parent_id.map(|id| id.to_string()),
                linked_context: None,
                attributes: attributes
                    .into_iter()
                    .map(|(key, value)| oplog::Attribute {
                        key,
                        value: value.into(),
                    })
                    .collect(),
                inherited,
            }),
            golem_common::model::oplog::SpanData::ExternalSpan { span_id } => {
                Self::ExternalSpan(oplog::ExternalSpanData {
                    span_id: span_id.to_string(),
                })
            }
        }
    }
}

impl From<PublicSpanData> for oplog::SpanData {
    fn from(value: PublicSpanData) -> Self {
        match value {
            PublicSpanData::LocalSpan(local_span) => Self::LocalSpan(oplog::LocalSpanData {
                span_id: local_span.span_id.to_string(),
                start: local_span.start.into(),
                parent: local_span.parent_id.map(|id| id.to_string()),
                linked_context: local_span.linked_context,
                attributes: local_span
                    .attributes
                    .into_iter()
                    .map(|attr| oplog::Attribute {
                        key: attr.key,
                        value: attr.value.into(),
                    })
                    .collect(),
                inherited: local_span.inherited,
            }),
            PublicSpanData::ExternalSpan(external_span) => {
                Self::ExternalSpan(oplog::ExternalSpanData {
                    span_id: external_span.span_id.to_string(),
                })
            }
        }
    }
}

impl From<PublicAttributeValue> for oplog::AttributeValue {
    fn from(value: PublicAttributeValue) -> Self {
        match value {
            PublicAttributeValue::String(StringAttributeValue { value }) => Self::String(value),
        }
    }
}
