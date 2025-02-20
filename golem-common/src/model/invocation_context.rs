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
use std::collections::HashMap;
use std::fmt::{Display, Formatter};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct TraceId(String);

impl TraceId {
    pub fn generate() -> Self {
        Self(format!("{:x}", Uuid::new_v4().as_u128()))
    }
}

impl Display for TraceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Encode, Decode)]
pub struct SpanId(String);

impl SpanId {
    pub fn generate() -> Self {
        let (lo, hi) = Uuid::new_v4().as_u64_pair();
        Self(format!("{:x}", lo ^ hi))
    }
}

impl Display for SpanId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
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

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct InvocationContextStack {
    pub trace_id: TraceId,
    pub spans: Vec<Arc<InvocationContextSpan>>,
}

impl InvocationContextStack {
    pub fn fresh() -> Self {
        let trace_id = TraceId::generate();
        let root = InvocationContextSpan::new(None);
        Self {
            trace_id,
            spans: vec![root],
        }
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::model::invocation_context::{
        AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId, TraceId,
    };
    use std::collections::HashMap;
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
                span_id: value.span_id.0.clone(),
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
                span_id: SpanId(value.span_id),
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
                trace_id: value.trace_id.0,
                spans,
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
            let trace_id = TraceId(value.trace_id);
            let spans = value
                .spans
                .into_iter()
                .map(|span| span.try_into())
                .map(|span: Result<InvocationContextSpan, String>| span.map(Arc::new))
                .collect::<Result<Vec<Arc<InvocationContextSpan>>, String>>()?;
            Ok(Self { trace_id, spans })
        }
    }
}
