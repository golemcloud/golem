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

use crate::model::Timestamp;
use bincode::de::{BorrowDecoder, Decoder};
use bincode::enc::Encoder;
use bincode::error::{DecodeError, EncodeError};
use bincode::{BorrowDecode, Decode, Encode};
use nonempty_collections::NEVec;
use serde::de::Error;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::num::{NonZeroU128, NonZeroU64};
use std::sync::{Arc, RwLock};
use tracing::warn;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct TraceId(pub NonZeroU128);

impl TraceId {
    pub fn from_string(value: impl AsRef<str>) -> Result<Self, String> {
        let n = u128::from_str_radix(value.as_ref(), 16).map_err(|err| {
            format!("Trace ID must be a 128bit value in hexadecimal format: {err}")
        })?;
        let n =
            NonZeroU128::new(n).ok_or_else(|| "Trace ID must be a non-zero value".to_string())?;
        Ok(Self(n))
    }

    pub fn from_attribute_value(value: AttributeValue) -> Result<Self, String> {
        match value {
            AttributeValue::String(value) => Self::from_string(value),
        }
    }

    pub fn generate() -> Self {
        Self(NonZeroU128::new(Uuid::new_v4().as_u128()).unwrap())
    }
}

impl Display for TraceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct SpanId(pub NonZeroU64);

impl SpanId {
    pub fn from_string(value: impl AsRef<str>) -> Result<Self, String> {
        let n = u64::from_str_radix(value.as_ref(), 16)
            .map_err(|err| format!("Span ID must be a 64bit value in hexadecimal format: {err}"))?;
        let n = NonZeroU64::new(n).ok_or_else(|| "Span ID must be a non-zero value".to_string())?;
        Ok(Self(n))
    }

    pub fn from_attribute_value(value: AttributeValue) -> Result<Self, String> {
        match value {
            AttributeValue::String(value) => Self::from_string(value),
        }
    }

    pub fn generate() -> Self {
        loop {
            let (lo, hi) = Uuid::new_v4().as_u64_pair();
            let n = lo ^ hi;
            if n != 0 {
                break Self(unsafe { NonZeroU64::new_unchecked(n) });
            }
        }
    }
}

impl Display for SpanId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:016x}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub enum AttributeValue {
    String(String),
}

impl Display for AttributeValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(value) => write!(f, "{}", value),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct LocalInvocationContextSpanState {
    parent: Option<Arc<InvocationContextSpan>>,
    attributes: HashMap<String, AttributeValue>,
}

#[derive(Debug)]
pub enum InvocationContextSpan {
    Local {
        span_id: SpanId,
        start: Timestamp,
        state: RwLock<LocalInvocationContextSpanState>,
    },
    ExternalParent {
        span_id: SpanId,
    },
}

impl InvocationContextSpan {
    pub fn new(span_id: Option<SpanId>) -> Arc<Self> {
        let span_id = span_id.unwrap_or(SpanId::generate());
        Arc::new(Self::Local {
            span_id,
            start: Timestamp::now_utc(),
            state: RwLock::new(LocalInvocationContextSpanState {
                parent: None,
                attributes: HashMap::new(),
            }),
        })
    }

    fn new_with_parent(span_id: Option<SpanId>, parent: Arc<InvocationContextSpan>) -> Arc<Self> {
        let span_id = span_id.unwrap_or(SpanId::generate());
        Arc::new(Self::Local {
            span_id,
            start: Timestamp::now_utc(),
            state: RwLock::new(LocalInvocationContextSpanState {
                parent: Some(parent),
                attributes: HashMap::new(),
            }),
        })
    }

    pub fn new_at(span_id: Option<SpanId>, start: Timestamp) -> Arc<Self> {
        let span_id = span_id.unwrap_or(SpanId::generate());
        Arc::new(Self::Local {
            span_id,
            start,
            state: RwLock::new(LocalInvocationContextSpanState {
                parent: None,
                attributes: HashMap::new(),
            }),
        })
    }

    pub fn external_parent(span_id: SpanId) -> Arc<Self> {
        Arc::new(Self::ExternalParent { span_id })
    }

    pub fn new_with_attributes(
        span_id: Option<SpanId>,
        attributes: HashMap<String, AttributeValue>,
        parent: Option<Arc<Self>>,
    ) -> Arc<Self> {
        let span_id = span_id.unwrap_or(SpanId::generate());
        Arc::new(Self::Local {
            span_id,
            start: Timestamp::now_utc(),
            state: RwLock::new(LocalInvocationContextSpanState { parent, attributes }),
        })
    }

    pub fn span_id(&self) -> &SpanId {
        match self {
            Self::Local { span_id, .. } => span_id,
            Self::ExternalParent { span_id } => span_id,
        }
    }

    pub fn parent(&self) -> Option<Arc<Self>> {
        match self {
            Self::Local { state, .. } => {
                let state = state.read().unwrap();
                state.parent.clone()
            }
            Self::ExternalParent { .. } => None,
        }
    }

    pub fn start_span(self: &Arc<Self>, span_id: Option<SpanId>) -> Arc<Self> {
        Self::new_with_parent(span_id, self.clone())
    }

    pub fn get_attribute(self: &Arc<Self>, key: &str, inherit: bool) -> Option<AttributeValue> {
        let mut current = self.clone();
        loop {
            current = match &*current {
                Self::Local { state, .. } => {
                    let state = state.read().unwrap();
                    match state.attributes.get(key) {
                        Some(value) => break Some(value.clone()),
                        None => {
                            if inherit {
                                match &state.parent {
                                    Some(parent) => parent.clone(),
                                    None => break None,
                                }
                            } else {
                                break None;
                            }
                        }
                    }
                }
                _ => break None,
            }
        }
    }

    pub fn get_attribute_chain(self: &Arc<Self>, key: &str) -> Option<Vec<AttributeValue>> {
        let mut current = self.clone();
        let mut result = Vec::new();
        loop {
            current = match &*current {
                Self::Local { state, .. } => {
                    let state = state.read().unwrap();
                    if let Some(value) = state.attributes.get(key) {
                        result.push(value.clone());
                    }
                    match state.parent.as_ref() {
                        Some(parent) => parent.clone(),
                        None => {
                            if result.is_empty() {
                                break None;
                            } else {
                                break Some(result);
                            }
                        }
                    }
                }
                _ => {
                    if result.is_empty() {
                        break None;
                    } else {
                        break Some(result);
                    }
                }
            }
        }
    }

    pub fn get_attributes(self: &Arc<Self>, inherit: bool) -> HashMap<String, Vec<AttributeValue>> {
        let mut current = self.clone();
        let mut result = HashMap::new();
        loop {
            current = match &*current {
                Self::Local { state, .. } => {
                    let state = state.read().unwrap();
                    for (key, value) in state.attributes.iter() {
                        result
                            .entry(key.clone())
                            .or_insert_with(Vec::new)
                            .push(value.clone());
                    }
                    if inherit {
                        match state.parent.as_ref() {
                            Some(parent) => parent.clone(),
                            None => break result,
                        }
                    } else {
                        break result;
                    }
                }
                _ => break result,
            }
        }
    }

    pub fn set_attribute(&self, key: String, value: AttributeValue) {
        warn!("set attribute {key}={value} in {}", self.span_id());
        match self {
            Self::Local { state, .. } => {
                state.write().unwrap().attributes.insert(key, value);
            }
            _ => {
                panic!("Cannot set attribute on external parent span")
            }
        }
    }

    pub fn replace_parent(&self, parent: Option<Arc<Self>>) {
        match self {
            Self::Local { state, .. } => {
                state.write().unwrap().parent = parent;
            }
            _ => {
                panic!("Cannot replace parent on external parent span")
            }
        }
    }
}

impl PartialEq for InvocationContextSpan {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Local {
                    span_id: span_id1,
                    start: start1,
                    state: state1,
                },
                Self::Local {
                    span_id: span_id2,
                    start: start2,
                    state: state2,
                },
            ) => {
                span_id1 == span_id2
                    && start1 == start2
                    && *state1.read().unwrap() == *state2.read().unwrap()
            }
            (
                Self::ExternalParent { span_id: span_id1 },
                Self::ExternalParent { span_id: span_id2 },
            ) => span_id1 == span_id2,
            _ => false,
        }
    }
}

impl Encode for InvocationContextSpan {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        match self {
            Self::Local {
                span_id,
                start,
                state,
            } => {
                let state = state.read().unwrap();
                0u8.encode(encoder)?;
                span_id.encode(encoder)?;
                start.encode(encoder)?;
                state.parent.encode(encoder)?;
                state.attributes.encode(encoder)
            }
            Self::ExternalParent { span_id } => {
                1u8.encode(encoder)?;
                span_id.encode(encoder)
            }
        }
    }
}

impl Decode for InvocationContextSpan {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let tag = u8::decode(decoder)?;
        match tag {
            0 => {
                let span_id = SpanId::decode(decoder)?;
                let start = Timestamp::decode(decoder)?;
                let parent = Option::<Arc<InvocationContextSpan>>::decode(decoder)?;
                let attributes = HashMap::decode(decoder)?;
                let state = RwLock::new(LocalInvocationContextSpanState { parent, attributes });
                Ok(Self::Local {
                    span_id,
                    start,
                    state,
                })
            }
            1 => {
                let span_id = SpanId::decode(decoder)?;
                Ok(Self::ExternalParent { span_id })
            }
            _ => Err(DecodeError::custom(format!(
                "Invalid tag for InvocationContextSpan: {tag}"
            ))),
        }
    }
}

impl<'de> BorrowDecode<'de> for InvocationContextSpan {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let tag = u8::borrow_decode(decoder)?;
        match tag {
            0 => {
                let span_id = SpanId::borrow_decode(decoder)?;
                let start = Timestamp::borrow_decode(decoder)?;
                let parent = Option::<Arc<InvocationContextSpan>>::borrow_decode(decoder)?;
                let attributes = HashMap::borrow_decode(decoder)?;
                let state = RwLock::new(LocalInvocationContextSpanState { parent, attributes });
                Ok(Self::Local {
                    span_id,
                    start,
                    state,
                })
            }
            1 => {
                let span_id = SpanId::borrow_decode(decoder)?;
                Ok(Self::ExternalParent { span_id })
            }
            _ => Err(DecodeError::custom(format!(
                "Invalid tag for InvocationContextSpan: {tag}"
            ))),
        }
    }
}

#[derive(Clone, PartialEq)]
pub struct InvocationContextStack {
    pub trace_id: TraceId,
    pub spans: NEVec<Arc<InvocationContextSpan>>,
    pub trace_states: Vec<String>,
}

impl InvocationContextStack {
    pub fn fresh() -> Self {
        let trace_id = TraceId::generate();
        let root = InvocationContextSpan::new(None);
        Self {
            trace_id,
            spans: NEVec::new(root),
            trace_states: Vec::new(),
        }
    }

    pub fn new(
        trace_id: TraceId,
        root_span: Arc<InvocationContextSpan>,
        trace_states: Vec<String>,
    ) -> Self {
        Self {
            trace_id,
            spans: NEVec::new(root_span),
            trace_states,
        }
    }

    pub fn push(&mut self, span: Arc<InvocationContextSpan>) {
        self.spans.insert(0, span);
    }

    pub fn span_ids(&self) -> HashSet<SpanId> {
        self.spans
            .iter()
            .map(|span| span.span_id().clone())
            .collect()
    }
}

impl Encode for InvocationContextStack {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.trace_id.encode(encoder)?;
        Encode::encode(&(self.spans.len().get() as u64), encoder)?;
        for item in self.spans.iter() {
            item.encode(encoder)?;
        }
        self.trace_states.encode(encoder)
    }
}

impl Decode for InvocationContextStack {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let trace_id = TraceId::decode(decoder)?;
        let spans = Vec::<Arc<InvocationContextSpan>>::decode(decoder)?;
        let trace_state = Vec::<String>::decode(decoder)?;
        Ok(Self {
            trace_id,
            spans: NEVec::try_from_vec(spans).ok_or(DecodeError::custom("No spans"))?,
            trace_states: trace_state,
        })
    }
}

impl<'de> BorrowDecode<'de> for InvocationContextStack {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let trace_id = TraceId::borrow_decode(decoder)?;
        let spans = Vec::borrow_decode(decoder)?;
        let trace_state = Vec::borrow_decode(decoder)?;
        Ok(Self {
            trace_id,
            spans: NEVec::try_from_vec(spans).ok_or(DecodeError::custom("No spans"))?,
            trace_states: trace_state,
        })
    }
}

impl Debug for InvocationContextStack {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "InvocationContextStack trace_id={}", self.trace_id)?;
        for span in &self.spans {
            writeln!(
                f,
                "  span {} parent={}: {}",
                span.span_id(),
                span.parent()
                    .map(|parent| parent.span_id().to_string())
                    .unwrap_or("none".to_string()),
                span.get_attributes(true)
                    .iter()
                    .map(|(key, values)| format!(
                        "{key}=[{}]",
                        values
                            .iter()
                            .map(|v| v.to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            )?;
        }
        Ok(())
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::invocation_context::{
        AttributeValue, InvocationContextSpan, InvocationContextStack,
        LocalInvocationContextSpanState, SpanId, TraceId,
    };
    use nonempty_collections::NEVec;
    use std::collections::HashMap;
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use std::sync::RwLock;

    impl From<AttributeValue> for golem_api_grpc::proto::golem::worker::AttributeValue {
        fn from(value: AttributeValue) -> Self {
            match value {
                AttributeValue::String(value) => Self {
                    value: Some(
                        golem_api_grpc::proto::golem::worker::attribute_value::Value::StringValue(
                            value,
                        ),
                    ),
                },
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::AttributeValue> for AttributeValue {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::AttributeValue,
        ) -> Result<Self, Self::Error> {
            match value.value {
                Some(
                    golem_api_grpc::proto::golem::worker::attribute_value::Value::StringValue(
                        value,
                    ),
                ) => Ok(Self::String(value)),
                _ => Err("Invalid attribute value".to_string()),
            }
        }
    }

    impl From<&InvocationContextSpan> for golem_api_grpc::proto::golem::worker::InvocationSpan {
        fn from(value: &InvocationContextSpan) -> Self {
            match value {
                InvocationContextSpan::Local {
                    state,
                    span_id,
                    start,
                    ..
                } => {
                    let value_state = state.read().unwrap();
                    let mut attributes = HashMap::new();
                    for (key, value) in &value_state.attributes {
                        attributes.insert(key.clone(), value.clone().into());
                    }
                    Self {
                        span: Some(
                            golem_api_grpc::proto::golem::worker::invocation_span::Span::Local(
                                golem_api_grpc::proto::golem::worker::LocalInvocationSpan {
                                    span_id: span_id.0.get(),
                                    start: Some((*start).into()),
                                    attributes,
                                },
                            ),
                        ),
                    }
                }
                InvocationContextSpan::ExternalParent { span_id } => Self {
                    span: Some(
                        golem_api_grpc::proto::golem::worker::invocation_span::Span::ExternalParent(
                            golem_api_grpc::proto::golem::worker::ExternalParentSpan {
                                span_id: span_id.0.get(),
                            },
                        ),
                    ),
                },
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::InvocationSpan> for InvocationContextSpan {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::InvocationSpan,
        ) -> Result<Self, Self::Error> {
            match value.span {
                Some(golem_api_grpc::proto::golem::worker::invocation_span::Span::Local(value)) => {
                    let span_id = SpanId(
                        NonZeroU64::new(value.span_id)
                            .ok_or_else(|| "Span ID cannot be 0".to_string())?,
                    );
                    let start = value
                        .start
                        .ok_or_else(|| "Missing timestamp".to_string())?
                        .into();
                    let mut attributes = HashMap::new();
                    for (key, value) in value.attributes {
                        attributes.insert(key, value.try_into()?);
                    }
                    Ok(Self::Local {
                        span_id,
                        start,
                        state: RwLock::new(LocalInvocationContextSpanState {
                            parent: None,
                            attributes,
                        }),
                    })
                }
                Some(
                    golem_api_grpc::proto::golem::worker::invocation_span::Span::ExternalParent(
                        value,
                    ),
                ) => Ok(Self::ExternalParent {
                    span_id: SpanId(
                        NonZeroU64::new(value.span_id)
                            .ok_or_else(|| "Span ID cannot be 0".to_string())?,
                    ),
                }),
                None => Err("Missing span".to_string()),
            }
        }
    }

    impl From<InvocationContextStack>
        for golem_api_grpc::proto::golem::worker::TracingInvocationContext
    {
        fn from(value: InvocationContextStack) -> Self {
            let spans = value
                .spans
                .into_iter()
                .map(|span| (&*span).into())
                .collect();
            Self {
                trace_id: value.trace_id.to_string(),
                spans,
                trace_state: value.trace_states,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::TracingInvocationContext>
        for InvocationContextStack
    {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::TracingInvocationContext,
        ) -> Result<Self, Self::Error> {
            let trace_id = TraceId::from_string(value.trace_id)?;
            let trace_state = value.trace_state;
            let spans = NEVec::try_from_vec(
                value
                    .spans
                    .into_iter()
                    .map(|span| span.try_into())
                    .map(|span: Result<InvocationContextSpan, String>| span.map(Arc::new))
                    .collect::<Result<Vec<Arc<InvocationContextSpan>>, String>>()?,
            )
            .ok_or_else(|| "No spans".to_string())?;
            Ok(Self {
                trace_id,
                spans,
                trace_states: trace_state,
            })
        }
    }
}
