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

use crate::model::oplog::SpanData;
use crate::model::Timestamp;
use desert_rust::adt::{AdtDeserializer, AdtMetadata, AdtSerializer};
use desert_rust::{
    BinaryCodec, BinaryDeserializer, BinaryOutput, BinarySerializer, DeserializationContext,
    Evolution, SerializationContext,
};
use lazy_static::lazy_static;
use nonempty_collections::NEVec;
use std::collections::{HashMap, HashSet};
use std::fmt::{Debug, Display, Formatter};
use std::sync::{Arc, RwLock};

pub use crate::base_model::invocation_context::*;
use crate::base_model::oplog::PublicAttributeValue;

impl TraceId {
    pub fn from_attribute_value(value: AttributeValue) -> Result<Self, String> {
        match value {
            AttributeValue::String(value) => Self::from_string(value),
        }
    }
}

impl SpanId {
    pub fn from_attribute_value(value: AttributeValue) -> Result<Self, String> {
        match value {
            AttributeValue::String(value) => Self::from_string(value),
        }
    }
}

impl poem_openapi::types::Type for TraceId {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::from(format!("string({})", stringify!(SpanId)))
    }

    fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
        poem_openapi::registry::MetaSchemaRef::Inline(Box::new(
            poem_openapi::registry::MetaSchema::new("string"),
        ))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl poem_openapi::types::ParseFromParameter for TraceId {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        Ok(Self::from_string(value)?)
    }
}

impl poem_openapi::types::ParseFromJSON for TraceId {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        match value {
            Some(serde_json::Value::String(s)) => Ok(Self::from_string(&s)?),
            _ => Err(poem_openapi::types::ParseError::<TraceId>::custom(format!(
                "Unexpected representation of {}",
                stringify!(SpanId)
            ))),
        }
    }
}

impl poem_openapi::types::ToJSON for TraceId {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.to_string()))
    }
}

impl poem_openapi::types::Type for SpanId {
    const IS_REQUIRED: bool = true;
    type RawValueType = Self;
    type RawElementValueType = Self;

    fn name() -> std::borrow::Cow<'static, str> {
        std::borrow::Cow::from(format!("string({})", stringify!(SpanId)))
    }

    fn schema_ref() -> poem_openapi::registry::MetaSchemaRef {
        poem_openapi::registry::MetaSchemaRef::Inline(Box::new(
            poem_openapi::registry::MetaSchema::new("string"),
        ))
    }

    fn as_raw_value(&self) -> Option<&Self::RawValueType> {
        Some(self)
    }

    fn raw_element_iter<'a>(
        &'a self,
    ) -> Box<dyn Iterator<Item = &'a Self::RawElementValueType> + 'a> {
        Box::new(self.as_raw_value().into_iter())
    }
}

impl poem_openapi::types::ParseFromParameter for SpanId {
    fn parse_from_parameter(value: &str) -> poem_openapi::types::ParseResult<Self> {
        Ok(Self::from_string(value)?)
    }
}

impl poem_openapi::types::ParseFromJSON for SpanId {
    fn parse_from_json(value: Option<serde_json::Value>) -> poem_openapi::types::ParseResult<Self> {
        match value {
            Some(serde_json::Value::String(s)) => Ok(Self::from_string(&s)?),
            _ => Err(poem_openapi::types::ParseError::<SpanId>::custom(format!(
                "Unexpected representation of {}",
                stringify!(SpanId)
            ))),
        }
    }
}

impl poem_openapi::types::ToJSON for SpanId {
    fn to_json(&self) -> Option<serde_json::Value> {
        Some(serde_json::Value::String(self.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub enum AttributeValue {
    String(String),
}

impl golem_wasm::IntoValue for AttributeValue {
    fn into_value(self) -> golem_wasm::Value {
        match self {
            AttributeValue::String(s) => golem_wasm::Value::Variant {
                case_idx: 0,
                case_value: Some(Box::new(golem_wasm::Value::String(s))),
            },
        }
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        use golem_wasm::analysis::analysed_type::*;
        variant(vec![case("string", str())])
            .named("attribute-value")
            .owned("golem:api@1.5.0/context")
    }
}

impl golem_wasm::FromValue for AttributeValue {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Variant {
                case_idx: 0,
                case_value,
            } => {
                let s = String::from_value(*case_value.ok_or("Expected case_value for string")?)?;
                Ok(AttributeValue::String(s))
            }
            golem_wasm::Value::Variant { case_idx, .. } => {
                Err(format!("Invalid case_idx for AttributeValue: {case_idx}"))
            }
            other => Err(format!(
                "Expected Variant for AttributeValue, got {other:?}"
            )),
        }
    }
}

impl Display for AttributeValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(value) => write!(f, "{value}"),
        }
    }
}

impl From<PublicAttributeValue> for AttributeValue {
    fn from(value: PublicAttributeValue) -> Self {
        match value {
            PublicAttributeValue::String(value) => Self::String(value.value),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct LocalInvocationContextSpanState {
    pub parent: Option<Arc<InvocationContextSpan>>,
    pub attributes: HashMap<String, AttributeValue>,
    pub linked_context: Option<Arc<InvocationContextSpan>>,
}

pub struct LocalInvocationContextSpanBuilder {
    span_id: Option<SpanId>,
    start: Timestamp,
    parent: Option<Arc<InvocationContextSpan>>,
    attributes: HashMap<String, AttributeValue>,
    linked_context: Option<Arc<InvocationContextSpan>>,
    inherited: bool,
}

impl LocalInvocationContextSpanBuilder {
    fn new() -> Self {
        Self {
            span_id: None,
            start: Timestamp::now_utc(),
            parent: None,
            attributes: HashMap::new(),
            linked_context: None,
            inherited: false,
        }
    }

    pub fn rounded(mut self) -> Self {
        self.start = self.start.rounded();
        self
    }

    pub fn span_id(mut self, span_id: Option<SpanId>) -> Self {
        self.span_id = span_id;
        self
    }

    pub fn with_span_id(mut self, span_id: SpanId) -> Self {
        self.span_id = Some(span_id);
        self
    }

    pub fn with_start(mut self, start: Timestamp) -> Self {
        self.start = start;
        self
    }

    pub fn parent(mut self, parent: Option<Arc<InvocationContextSpan>>) -> Self {
        self.parent = parent;
        self
    }

    pub fn with_parent(mut self, parent: Arc<InvocationContextSpan>) -> Self {
        self.parent = Some(parent);
        self
    }

    pub fn with_attributes(mut self, attributes: HashMap<String, AttributeValue>) -> Self {
        self.attributes = attributes;
        self
    }

    pub fn with_inherited(mut self, inherited: bool) -> Self {
        self.inherited = inherited;
        self
    }

    pub fn linked_context(mut self, linked_context: Option<Arc<InvocationContextSpan>>) -> Self {
        self.linked_context = linked_context;
        self
    }

    pub fn with_linked_context(mut self, linked_context: Arc<InvocationContextSpan>) -> Self {
        self.linked_context = Some(linked_context);
        self
    }

    pub fn build(self) -> Arc<InvocationContextSpan> {
        Arc::new(InvocationContextSpan::Local {
            span_id: self.span_id.unwrap_or(SpanId::generate()),
            start: self.start,
            state: RwLock::new(LocalInvocationContextSpanState {
                parent: self.parent,
                attributes: self.attributes,
                linked_context: self.linked_context,
            }),
            inherited: self.inherited,
        })
    }
}

#[derive(Debug)]
pub enum InvocationContextSpan {
    Local {
        span_id: SpanId,
        start: Timestamp,
        state: RwLock<LocalInvocationContextSpanState>,
        inherited: bool,
    },
    ExternalParent {
        span_id: SpanId,
    },
}

impl InvocationContextSpan {
    pub fn local() -> LocalInvocationContextSpanBuilder {
        LocalInvocationContextSpanBuilder::new()
    }

    pub fn external_parent(span_id: SpanId) -> Arc<Self> {
        Arc::new(Self::ExternalParent { span_id })
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

    pub fn inherited(&self) -> bool {
        match self {
            Self::Local { inherited, .. } => *inherited,
            Self::ExternalParent { .. } => true,
        }
    }

    pub fn linked_context(&self) -> Option<Arc<Self>> {
        match self {
            Self::Local { state, .. } => {
                let state = state.read().unwrap();
                state.linked_context.clone()
            }
            Self::ExternalParent { .. } => None,
        }
    }

    pub fn start(&self) -> Option<Timestamp> {
        match self {
            Self::Local { start, .. } => Some(*start),
            Self::ExternalParent { .. } => None,
        }
    }

    pub fn start_span(self: &Arc<Self>, span_id: Option<SpanId>) -> Arc<Self> {
        Self::local()
            .with_parent(self.clone())
            .span_id(span_id)
            .build()
    }

    pub fn add_link(&self, linked_span: Arc<InvocationContextSpan>) {
        match self {
            Self::Local { state, .. } => {
                state.write().unwrap().linked_context = Some(linked_span);
            }
            _ => {
                panic!("Cannot add link to external parent span")
            }
        }
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
                                // First look in the linked context
                                if let Some(linked_context) = &state.linked_context {
                                    if let Some(value) = linked_context.get_attribute(key, false) {
                                        break Some(value);
                                    }
                                }

                                // Otherwise recurse to the parent
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
                    // Add value from the linked context
                    if let Some(linked_context) = &state.linked_context {
                        if let Some(value) = linked_context.get_attribute(key, false) {
                            result.push(value.clone());
                        }
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
                        if let Some(linked_context) = &state.linked_context {
                            for (key, value) in linked_context.get_attributes(false) {
                                result
                                    .entry(key.clone())
                                    .or_insert_with(Vec::new)
                                    .extend(value);
                            }
                        }

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

    pub fn as_inherited(&self) -> Arc<InvocationContextSpan> {
        match self {
            Self::Local {
                span_id,
                start,
                state,
                ..
            } => {
                let state = state.read().unwrap();
                Arc::new(Self::Local {
                    span_id: span_id.clone(),
                    start: *start,
                    state: RwLock::new(LocalInvocationContextSpanState {
                        parent: state.parent.clone(),
                        attributes: state.attributes.clone(),
                        linked_context: state
                            .linked_context
                            .as_ref()
                            .map(|link| link.as_inherited()),
                    }),
                    inherited: true,
                })
            }
            Self::ExternalParent { span_id } => Arc::new(Self::ExternalParent {
                span_id: span_id.clone(),
            }),
        }
    }

    pub fn to_chain(self: &Arc<Self>) -> NEVec<Arc<InvocationContextSpan>> {
        let mut current = self.clone();
        let mut result = NEVec::new(current.clone());
        loop {
            current = match &*current {
                Self::Local { state, .. } => {
                    let state = state.read().unwrap();
                    match state.parent.as_ref() {
                        Some(parent) => {
                            result.push(parent.clone());
                            parent.clone()
                        }
                        None => break result,
                    }
                }
                _ => break result,
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
                    inherited: inherited1,
                },
                Self::Local {
                    span_id: span_id2,
                    start: start2,
                    state: state2,
                    inherited: inherited2,
                },
            ) => {
                span_id1 == span_id2
                    && start1 == start2
                    && *state1.read().unwrap() == *state2.read().unwrap()
                    && inherited1 == inherited2
            }
            (
                Self::ExternalParent { span_id: span_id1 },
                Self::ExternalParent { span_id: span_id2 },
            ) => span_id1 == span_id2,
            _ => false,
        }
    }
}

lazy_static! {
    static ref INVOCATION_CONTEXT_SPAN_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref INVOCATION_CONTEXT_SPAN_LOCAL_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
    static ref INVOCATION_CONTEXT_SPAN_EXTERNAL_PARENT_METADATA: AdtMetadata =
        AdtMetadata::new(vec![Evolution::InitialVersion]);
}

impl BinarySerializer for InvocationContextSpan {
    fn serialize<Output: BinaryOutput>(
        &self,
        context: &mut SerializationContext<Output>,
    ) -> desert_rust::Result<()> {
        let mut serializer =
            AdtSerializer::<_, 1>::new_v0(&INVOCATION_CONTEXT_SPAN_METADATA, context);
        match self {
            InvocationContextSpan::Local {
                span_id,
                start,
                state,
                inherited,
            } => {
                serializer.write_constructor(0, |context| {
                    let state = state.read().unwrap();
                    let mut inner = AdtSerializer::<_, 1>::new_v0(
                        &INVOCATION_CONTEXT_SPAN_LOCAL_METADATA,
                        context,
                    );
                    inner.write_field("span_id", span_id)?;
                    inner.write_field("start", start)?;
                    inner.write_field("parent", &state.parent)?;
                    inner.write_field("attributes", &state.attributes)?;
                    inner.write_field("linked_context", &state.linked_context)?;
                    inner.write_field("inherited", inherited)?;
                    inner.finish()
                })?;
                serializer.finish()
            }
            InvocationContextSpan::ExternalParent { span_id } => {
                serializer.write_constructor(1, |context| {
                    let mut inner = AdtSerializer::<_, 1>::new_v0(
                        &INVOCATION_CONTEXT_SPAN_EXTERNAL_PARENT_METADATA,
                        context,
                    );
                    inner.write_field("span_id", span_id)?;
                    inner.finish()
                })?;
                serializer.finish()
            }
        }
    }
}

impl BinaryDeserializer for InvocationContextSpan {
    fn deserialize(context: &mut DeserializationContext<'_>) -> desert_rust::Result<Self> {
        use desert_rust::BinaryInput;
        let stored_version = context.read_u8()?;
        let mut deserializer = if stored_version == 0 {
            AdtDeserializer::<1>::new_v0(&INVOCATION_CONTEXT_SPAN_METADATA, context)?
        } else {
            AdtDeserializer::<1>::new(&INVOCATION_CONTEXT_SPAN_METADATA, context, stored_version)?
        };

        let constructor_id = deserializer.read_constructor_idx()?;

        match constructor_id {
            0u32 => {
                let stored_version = context.read_u8()?;
                let mut deserializer = if stored_version == 0 {
                    AdtDeserializer::<1>::new_v0(&INVOCATION_CONTEXT_SPAN_LOCAL_METADATA, context)?
                } else {
                    AdtDeserializer::<1>::new(
                        &INVOCATION_CONTEXT_SPAN_LOCAL_METADATA,
                        context,
                        stored_version,
                    )?
                };

                let span_id = deserializer.read_field("span_id", None)?;
                let start = deserializer.read_field("start", None)?;
                let parent = deserializer.read_optional_field("parent", Some(None))?;
                let attributes = deserializer.read_field("attributes", None)?;
                let linked_context =
                    deserializer.read_optional_field("linked_context", Some(None))?;
                let inherited = deserializer.read_field("inherited", None)?;

                Ok(Self::Local {
                    span_id,
                    start,
                    state: RwLock::new(LocalInvocationContextSpanState {
                        parent,
                        attributes,
                        linked_context,
                    }),
                    inherited,
                })
            }
            1u32 => {
                let stored_version = context.read_u8()?;
                let mut deserializer = if stored_version == 0 {
                    AdtDeserializer::<1>::new_v0(
                        &INVOCATION_CONTEXT_SPAN_EXTERNAL_PARENT_METADATA,
                        context,
                    )?
                } else {
                    AdtDeserializer::<1>::new(
                        &INVOCATION_CONTEXT_SPAN_EXTERNAL_PARENT_METADATA,
                        context,
                        stored_version,
                    )?
                };

                let span_id = deserializer.read_field("span_id", None)?;

                Ok(Self::ExternalParent { span_id })
            }
            _ => Err(desert_rust::Error::InvalidConstructorId {
                type_name: "InvocationContextSpan".to_string(),
                constructor_id,
            }),
        }
    }
}

#[derive(Clone, PartialEq, BinaryCodec)]
#[desert(evolution())]
pub struct InvocationContextStack {
    pub trace_id: TraceId,
    pub spans: NEVec<Arc<InvocationContextSpan>>,
    pub trace_states: Vec<String>,
}

impl InvocationContextStack {
    pub fn fresh() -> Self {
        let trace_id = TraceId::generate();
        let root = InvocationContextSpan::local().build();
        Self {
            trace_id,
            spans: NEVec::new(root),
            trace_states: Vec::new(),
        }
    }

    pub fn fresh_rounded() -> Self {
        let trace_id = TraceId::generate();
        let root = InvocationContextSpan::local().rounded().build();
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

    pub fn from_oplog_data(
        trace_id: TraceId,
        trace_states: Vec<String>,
        spans: Vec<SpanData>,
    ) -> Self {
        if spans.is_empty() {
            let root = InvocationContextSpan::local().build();
            Self {
                trace_id,
                spans: NEVec::new(root),
                trace_states,
            }
        } else {
            let mut result_spans = Vec::new();
            for span_data in spans.into_iter().rev() {
                let result_span = match span_data {
                    SpanData::ExternalSpan { span_id } => {
                        InvocationContextSpan::external_parent(span_id)
                    }
                    SpanData::LocalSpan {
                        span_id,
                        start,
                        parent_id,
                        linked_context,
                        attributes,
                        inherited,
                    } => InvocationContextSpan::local()
                        .with_span_id(span_id)
                        .with_start(start)
                        .parent(parent_id.and_then(|_| result_spans.first().cloned()))
                        .with_attributes(attributes)
                        .with_inherited(inherited)
                        .linked_context(linked_context.map(|linked_spans| {
                            let linked_stack = InvocationContextStack::from_oplog_data(
                                trace_id.clone(),
                                trace_states.clone(),
                                linked_spans,
                            );
                            linked_stack.spans.first().clone()
                        }))
                        .build(),
                };
                result_spans.insert(0, result_span);
            }

            InvocationContextStack {
                trace_id: trace_id.clone(),
                trace_states: trace_states.to_vec(),
                spans: NEVec::try_from_vec(result_spans).unwrap(),
            }
        }
    }

    pub fn to_oplog_data(&self) -> Vec<SpanData> {
        SpanData::from_chain(&self.spans)
    }

    pub fn push(&mut self, span: Arc<InvocationContextSpan>) {
        self.spans.insert(0, span);
    }

    /// Returns the span IDs in this stack, partitioned by local and inherited ones
    /// Return value is (local, inherited)
    ///
    /// Linked spans are not included in the result
    pub fn span_ids(&self) -> (HashSet<SpanId>, HashSet<SpanId>) {
        (
            self.spans
                .iter()
                .filter_map(|span| {
                    if !span.inherited() {
                        Some(span.span_id().clone())
                    } else {
                        None
                    }
                })
                .collect(),
            self.spans
                .iter()
                .filter_map(|span| {
                    if span.inherited() {
                        Some(span.span_id().clone())
                    } else {
                        None
                    }
                })
                .collect(),
        )
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
                    inherited,
                    ..
                } => {
                    let value_state = state.read().unwrap();
                    let mut attributes = HashMap::new();
                    for (key, value) in &value_state.attributes {
                        attributes.insert(key.clone(), value.clone().into());
                    }

                    let linked_context_stack = match &value_state.linked_context {
                        Some(linked_context) => {
                            let chain = linked_context.to_chain();
                            chain.iter().map(|span| (&**span).into()).collect()
                        }
                        None => Vec::new(),
                    };

                    Self {
                        span: Some(
                            golem_api_grpc::proto::golem::worker::invocation_span::Span::Local(
                                golem_api_grpc::proto::golem::worker::LocalInvocationSpan {
                                    span_id: span_id.0.get(),
                                    start: Some((*start).into()),
                                    attributes,
                                    inherited: *inherited,
                                    linked_context: linked_context_stack,
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

                    let linked_context_chain = value
                        .linked_context
                        .into_iter()
                        .map(|span| span.try_into())
                        .collect::<Result<Vec<InvocationContextSpan>, String>>()?
                        .into_iter()
                        .map(Arc::new)
                        .collect();
                    let linked_context = match NEVec::try_from_vec(linked_context_chain) {
                        Some(linked_context_chain) => {
                            for idx in 0..(linked_context_chain.len().get() - 1) {
                                linked_context_chain[idx]
                                    .replace_parent(Some(linked_context_chain[idx + 1].clone()));
                            }
                            Some(linked_context_chain.first().clone())
                        }
                        None => None,
                    };

                    Ok(Self::Local {
                        span_id,
                        start,
                        state: RwLock::new(LocalInvocationContextSpanState {
                            parent: None,
                            attributes,
                            linked_context,
                        }),
                        inherited: value.inherited,
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

            for idx in 0..(spans.len().get() - 1) {
                spans[idx].replace_parent(Some(spans[idx + 1].clone()));
            }

            Ok(Self {
                trace_id,
                spans,
                trace_states: trace_state,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::model::invocation_context::{
        AttributeValue, InvocationContextSpan, InvocationContextStack, SpanId, TraceId,
    };
    use crate::model::Timestamp;
    use crate::serialization::{deserialize, serialize};
    use std::collections::HashSet;
    use test_r::test;

    fn example_trace_id_1() -> TraceId {
        TraceId::from_string("4bf92f3577b34da6a3ce929d0e0e4736").unwrap()
    }

    fn example_span_id_1() -> SpanId {
        SpanId::from_string("cddd89c618fb7bf3").unwrap()
    }

    fn example_span_id_2() -> SpanId {
        SpanId::from_string("00f067aa0ba902b7").unwrap()
    }

    fn example_span_id_3() -> SpanId {
        SpanId::from_string("d0fa4a9110f2dcab").unwrap()
    }

    fn example_span_id_4() -> SpanId {
        SpanId::from_string("4a840260c6879c88").unwrap()
    }

    fn example_span_id_5() -> SpanId {
        SpanId::from_string("04d81050b3163556").unwrap()
    }

    fn example_span_id_6() -> SpanId {
        SpanId::from_string("b7027ded25941641").unwrap()
    }

    // span1 -> span2 -> span5 -> span6
    // span3 -> span4 /
    fn example_stack_1() -> InvocationContextStack {
        let timestamp = Timestamp::from(1724701930000);

        let root_span = InvocationContextSpan::external_parent(example_span_id_1());
        let trace_states = vec!["state1=x".to_string(), "state2=y".to_string()];

        let span2 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_2())
            .with_parent(root_span.clone())
            .with_inherited(true)
            .build();
        span2.set_attribute("x".to_string(), AttributeValue::String("1".to_string()));
        span2.set_attribute("y".to_string(), AttributeValue::String("2".to_string()));

        let span3 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_3())
            .build();
        span3.set_attribute("w".to_string(), AttributeValue::String("4".to_string()));

        let span4 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_4())
            .with_parent(span3)
            .build();
        span4.set_attribute("y".to_string(), AttributeValue::String("22".to_string()));

        let span5 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_5())
            .with_parent(span2.clone())
            .with_linked_context(span4)
            .build();
        span5.set_attribute("x".to_string(), AttributeValue::String("11".to_string()));
        span5.set_attribute("z".to_string(), AttributeValue::String("3".to_string()));

        let span6 = InvocationContextSpan::local()
            .with_start(timestamp)
            .with_span_id(example_span_id_6())
            .with_parent(span5.clone())
            .build();
        span6.set_attribute("z".to_string(), AttributeValue::String("33".to_string()));
        span6.set_attribute("a".to_string(), AttributeValue::String("0".to_string()));

        let mut stack = InvocationContextStack::new(example_trace_id_1(), root_span, trace_states);
        stack.push(span2);
        stack.push(span5);
        stack.push(span6);

        stack
    }

    #[test]
    fn get_span_ids() {
        let stack = example_stack_1();
        let (local, inherited) = stack.span_ids();
        assert_eq!(
            local,
            HashSet::from_iter(vec![example_span_id_5(), example_span_id_6()])
        );
        assert_eq!(
            inherited,
            HashSet::from_iter(vec![example_span_id_1(), example_span_id_2()])
        );
    }

    #[test]
    fn binary_serialization() {
        let stack = example_stack_1();
        let encoded = serialize(&stack).unwrap();
        let decoded: InvocationContextStack = deserialize(&encoded).unwrap();
        assert_eq!(stack, decoded);
    }

    #[test]
    fn protobuf_serialization() {
        let stack = example_stack_1();
        let encoded: golem_api_grpc::proto::golem::worker::TracingInvocationContext =
            stack.clone().into();
        let decoded: InvocationContextStack = encoded.try_into().unwrap();
        assert_eq!(stack, decoded);
    }
}
