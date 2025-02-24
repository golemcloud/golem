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
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::num::{NonZeroU128, NonZeroU64};
use std::sync::Arc;
use tokio::sync::RwLock;
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

#[derive(Debug)]
pub struct InvocationContextSpan {
    pub span_id: SpanId,
    pub parent: Option<Arc<InvocationContextSpan>>,
    pub start: Timestamp,
    attributes: RwLock<HashMap<String, AttributeValue>>,
}

impl InvocationContextSpan {
    pub fn new(span_id: Option<SpanId>) -> Arc<Self> {
        let span_id = span_id.unwrap_or(SpanId::generate());
        Arc::new(Self {
            span_id,
            parent: None,
            start: Timestamp::now_utc(),
            attributes: RwLock::new(HashMap::new()),
        })
    }

    pub fn new_with_attributes(
        span_id: Option<SpanId>,
        attributes: HashMap<String, AttributeValue>,
    ) -> Arc<Self> {
        let span_id = span_id.unwrap_or(SpanId::generate());
        Arc::new(Self {
            span_id,
            parent: None,
            start: Timestamp::now_utc(),
            attributes: RwLock::new(attributes),
        })
    }

    pub fn start_span(self: &Arc<Self>, span_id: Option<SpanId>) -> Arc<Self> {
        Self::new(span_id)
    }

    pub async fn get_attribute(&self, key: &str, inherit: bool) -> Option<AttributeValue> {
        let mut current = self;
        loop {
            let attributes = current.attributes.read().await;
            match attributes.get(key) {
                Some(value) => break Some(value.clone()),
                None => {
                    if inherit {
                        match current.parent.as_ref() {
                            Some(parent) => {
                                current = parent;
                            }
                            None => break None,
                        }
                    } else {
                        break None;
                    }
                }
            }
        }
    }

    pub async fn get_attribute_chain(&self, key: &str) -> Option<Vec<AttributeValue>> {
        let mut current = self;
        let mut result = Vec::new();
        loop {
            let attributes = current.attributes.read().await;
            match attributes.get(key) {
                Some(value) => result.push(value.clone()),
                None => match current.parent.as_ref() {
                    Some(parent) => {
                        current = parent;
                    }
                    None => {
                        if result.is_empty() {
                            break None;
                        } else {
                            break Some(result);
                        }
                    }
                },
            }
        }
    }

    pub async fn get_attributes(&self, inherit: bool) -> HashMap<String, Vec<AttributeValue>> {
        let mut current = self;
        let mut result = HashMap::new();
        loop {
            let attributes = current.attributes.read().await;
            for (key, value) in attributes.iter() {
                result
                    .entry(key.clone())
                    .or_insert_with(Vec::new)
                    .push(value.clone());
            }
            if inherit {
                match current.parent.as_ref() {
                    Some(parent) => {
                        current = parent;
                    }
                    None => break result,
                }
            } else {
                break result;
            }
        }
    }

    pub async fn set_attribute(&self, key: String, value: AttributeValue) {
        self.attributes.write().await.insert(key, value);
    }
}

impl PartialEq for InvocationContextSpan {
    fn eq(&self, other: &Self) -> bool {
        self.span_id == other.span_id
            && self.start == other.start
            && self.parent == other.parent
            && *self.attributes.blocking_read() == *other.attributes.blocking_read()
    }
}

impl Encode for InvocationContextSpan {
    fn encode<E: Encoder>(&self, encoder: &mut E) -> Result<(), EncodeError> {
        self.span_id.encode(encoder)?;
        self.start.encode(encoder)?;
        self.parent.encode(encoder)?;
        self.attributes.blocking_read().encode(encoder)
    }
}

impl Decode for InvocationContextSpan {
    fn decode<D: Decoder>(decoder: &mut D) -> Result<Self, DecodeError> {
        let span_id = SpanId::decode(decoder)?;
        let start = Timestamp::decode(decoder)?;
        let parent = Option::<Arc<InvocationContextSpan>>::decode(decoder)?;
        let attributes = RwLock::new(HashMap::decode(decoder)?);
        Ok(Self {
            span_id,
            start,
            parent,
            attributes,
        })
    }
}

impl<'de> BorrowDecode<'de> for InvocationContextSpan {
    fn borrow_decode<D: BorrowDecoder<'de>>(decoder: &mut D) -> Result<Self, DecodeError> {
        let span_id = SpanId::borrow_decode(decoder)?;
        let start = Timestamp::borrow_decode(decoder)?;
        let parent = Option::<Arc<InvocationContextSpan>>::borrow_decode(decoder)?;
        let attributes = RwLock::new(HashMap::borrow_decode(decoder)?);
        Ok(Self {
            span_id,
            start,
            parent,
            attributes,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
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

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::invocation_context::{
        AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId, TraceId,
    };
    use nonempty_collections::NEVec;
    use std::collections::HashMap;
    use std::num::NonZeroU64;
    use std::sync::Arc;
    use tokio::sync::RwLock;

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
            let value_attributes = value.attributes.blocking_read();
            let mut attributes = HashMap::new();
            for (key, value) in &*value_attributes {
                attributes.insert(key.clone(), value.clone().into());
            }
            Self {
                span_id: value.span_id.0.get(),
                start: Some(value.start.into()),
                attributes,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::worker::InvocationSpan> for InvocationContextSpan {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::worker::InvocationSpan,
        ) -> Result<Self, Self::Error> {
            let mut attributes = HashMap::new();
            for (key, value) in value.attributes {
                attributes.insert(key, value.try_into()?);
            }
            Ok(Self {
                span_id: SpanId(
                    NonZeroU64::new(value.span_id)
                        .ok_or_else(|| "Span ID cannot be 0".to_string())?,
                ),
                start: value
                    .start
                    .ok_or_else(|| "Missing timestamp".to_string())?
                    .into(),
                attributes: RwLock::new(attributes),
                parent: None,
            })
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
