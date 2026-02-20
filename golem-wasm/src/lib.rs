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

#[allow(unused)]
#[rustfmt::skip]
#[cfg(not(feature = "host"))]
#[cfg(feature = "stub")]
mod bindings;

#[cfg(test)]
test_r::enable!();

pub mod agentic;

pub mod analysis;

/// Implements desert encoders and decoders for WitValue instances
#[cfg(feature = "host")]
pub mod desert;

/// A builder interface for WitValue instances
#[cfg(any(feature = "host", feature = "stub"))]
mod builder;

/// Extension methods for extracting values from WitValue instances
#[cfg(any(feature = "host", feature = "stub"))]
mod extractor;

/// Conversion to and from JSON, in the presence of golem-wasm-ast generated type information
#[cfg(any(feature = "host", feature = "client"))]
pub mod json;

/// The metadata module defines data structures for representing various metadata extracted from WASM binaries.
///
/// This module is optional and can be enabled with the `metadata` feature flag. It is enabled by default.
#[cfg(feature = "host")]
pub mod metadata;

/// Poem OpenAPI integration for some types
#[cfg(feature = "host")]
pub mod poem;

/// Protobuf-defined value types and conversion to them
#[cfg(feature = "host")]
pub mod protobuf;

/// Serde instances for WitValue
#[cfg(feature = "host")]
pub mod serde;

/// Conversion to/from the WAVE format
#[cfg(feature = "host")]
mod text;

mod value_and_type;

/// Conversion to/from wasmtime's value representation
#[cfg(feature = "host")]
pub mod wasmtime;

#[cfg(any(feature = "host", feature = "stub"))]
use crate::builder::WitValueBuilder;

#[cfg(any(feature = "host", feature = "stub"))]
pub use builder::{NodeBuilder, WitValueBuilderExtensions};

#[cfg(any(feature = "host", feature = "stub"))]
pub use extractor::{WitNodePointer, WitValueExtractor};

#[cfg(feature = "derive")]
pub mod derive {
    pub use golem_wasm_derive::{FromValue, IntoValue};
}

#[cfg(not(feature = "host"))]
#[cfg(feature = "stub")]
pub use wstd::wasi;

#[cfg(not(feature = "host"))]
#[cfg(feature = "stub")]
pub use bindings::golem::core as golem_core_1_5_x;

#[cfg(not(feature = "host"))]
#[cfg(feature = "stub")]
pub use golem_core_1_5_x::types::{
    AccountId, AgentId, ComponentId, NodeIndex, PromiseId, ResourceMode, Uri, Uuid, WitNode,
    WitType, WitTypeNode, WitValue,
};

#[cfg(not(feature = "host"))]
#[cfg(feature = "stub")]
pub use wstd::wasi::io::poll::Pollable;

#[cfg(feature = "host")]
pub use wasmtime_wasi::p2::DynPollable;

#[cfg(feature = "host")]
mod generated {
    use ::wasmtime::component::bindgen;
    bindgen!({
        path: "wit",
        world: "wasm-rpc",
        tracing: false,
        async: true,
        trappable_imports: true,
        with: {
            "wasi:io/poll/pollable": super::DynPollable,
            "wasi:clocks/wall-clock": wasmtime_wasi::p2::bindings::clocks::wall_clock,
        },
        wasmtime_crate: ::wasmtime,
    });
}

#[cfg(feature = "host")]
pub use generated::wasi;

#[cfg(feature = "host")]
pub use generated::golem::core as golem_core_1_5_x;

#[cfg(feature = "host")]
pub use golem_core_1_5_x::types::{
    AccountId, AgentId, ComponentId, Host, NodeIndex, PromiseId, ResourceMode, Uri, Uuid, WitNode,
    WitType, WitTypeNode, WitValue,
};

#[cfg(any(feature = "host", feature = "stub"))]
impl From<Uuid> for uuid::Uuid {
    fn from(value: Uuid) -> Self {
        uuid::Uuid::from_u64_pair(value.high_bits, value.low_bits)
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl From<uuid::Uuid> for Uuid {
    fn from(uuid: uuid::Uuid) -> Self {
        let (high_bits, low_bits) = uuid.as_u64_pair();
        Uuid {
            high_bits,
            low_bits,
        }
    }
}

#[cfg(feature = "host")]
pub struct WasmRpcEntry {
    pub payload: Box<dyn std::any::Any + Send + Sync>,
}

#[cfg(feature = "host")]
#[async_trait::async_trait]
pub trait SubscribeAny: std::any::Any {
    async fn ready(&mut self);
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

#[cfg(feature = "host")]
pub struct FutureInvokeResultEntry {
    pub payload: Box<dyn SubscribeAny + Send + Sync>,
}

#[cfg(feature = "host")]
#[async_trait::async_trait]
impl wasmtime_wasi::p2::Pollable for FutureInvokeResultEntry {
    async fn ready(&mut self) {
        self.payload.ready().await
    }
}

#[cfg(feature = "host")]
pub struct CancellationTokenEntry {
    pub schedule_id: Vec<u8>, // ScheduleId is defined locally in the worker-executor, so store a serialized version here
}

#[cfg(feature = "host")]
pub use text::{parse_value_and_type, print_value_and_type};

pub use value_and_type::*;

#[cfg(feature = "host")]
impl arbitrary::Arbitrary<'_> for Uri {
    fn arbitrary(u: &mut arbitrary::Unstructured) -> arbitrary::Result<Self> {
        let uri = u.arbitrary::<String>()?;
        Ok(Uri { value: uri })
    }
}

#[cfg(feature = "host")]
impl PartialEq for Uri {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

/// A tree representation of Value - isomorphic to the protobuf Val type but easier to work with in Rust
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "host", derive(arbitrary::Arbitrary))]
#[cfg_attr(feature = "host", derive(desert_rust::BinaryCodec))]
pub enum Value {
    #[cfg_attr(feature = "host", desert(transparent))]
    Bool(bool),
    #[cfg_attr(feature = "host", desert(transparent))]
    U8(u8),
    #[cfg_attr(feature = "host", desert(transparent))]
    U16(u16),
    #[cfg_attr(feature = "host", desert(transparent))]
    U32(u32),
    #[cfg_attr(feature = "host", desert(transparent))]
    U64(u64),
    #[cfg_attr(feature = "host", desert(transparent))]
    S8(i8),
    #[cfg_attr(feature = "host", desert(transparent))]
    S16(i16),
    #[cfg_attr(feature = "host", desert(transparent))]
    S32(i32),
    #[cfg_attr(feature = "host", desert(transparent))]
    S64(i64),
    #[cfg_attr(feature = "host", desert(transparent))]
    F32(f32),
    #[cfg_attr(feature = "host", desert(transparent))]
    F64(f64),
    #[cfg_attr(feature = "host", desert(transparent))]
    Char(char),
    #[cfg_attr(feature = "host", desert(transparent))]
    String(String),
    #[cfg_attr(feature = "host", desert(custom = crate::desert::VecValueWrapper))]
    List(Vec<Value>),
    #[cfg_attr(feature = "host", desert(transparent))]
    Tuple(Vec<Value>),
    #[cfg_attr(feature = "host", desert(transparent))]
    Record(Vec<Value>),
    Variant {
        case_idx: u32,
        case_value: Option<Box<Value>>,
    },
    #[cfg_attr(feature = "host", desert(transparent))]
    Enum(u32),
    #[cfg_attr(feature = "host", desert(transparent))]
    Flags(Vec<bool>),
    #[cfg_attr(feature = "host", desert(transparent))]
    Option(Option<Box<Value>>),
    #[cfg_attr(feature = "host", desert(transparent))]
    Result(Result<Option<Box<Value>>, Option<Box<Value>>>),
    Handle {
        uri: String,
        resource_id: u64,
    },
}

#[cfg(any(feature = "host", feature = "stub"))]
impl From<Value> for WitValue {
    fn from(value: Value) -> Self {
        let mut builder = WitValueBuilder::new();
        build_wit_value(value, &mut builder);
        builder.build()
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl PartialEq for WitValue {
    fn eq(&self, other: &Self) -> bool {
        let a: Value = self.clone().into();
        let b: Value = other.clone().into();
        a == b
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
fn build_wit_value(value: Value, builder: &mut WitValueBuilder) -> NodeIndex {
    match value {
        Value::Bool(value) => builder.add_bool(value),
        Value::U8(value) => builder.add_u8(value),
        Value::U16(value) => builder.add_u16(value),
        Value::U32(value) => builder.add_u32(value),
        Value::U64(value) => builder.add_u64(value),
        Value::S8(value) => builder.add_s8(value),
        Value::S16(value) => builder.add_s16(value),
        Value::S32(value) => builder.add_s32(value),
        Value::S64(value) => builder.add_s64(value),
        Value::F32(value) => builder.add_f32(value),
        Value::F64(value) => builder.add_f64(value),
        Value::Char(value) => builder.add_char(value),
        Value::String(value) => builder.add_string(&value),
        Value::List(values) => {
            let list_idx = builder.add_list();
            let mut items = Vec::new();
            for value in values {
                let item_idx = build_wit_value(value, builder);
                items.push(item_idx);
            }
            builder.finish_seq(items, list_idx);
            list_idx
        }
        Value::Tuple(values) => {
            let tuple_idx = builder.add_tuple();
            let mut items = Vec::new();
            for value in values {
                let item_idx = build_wit_value(value, builder);
                items.push(item_idx);
            }
            builder.finish_seq(items, tuple_idx);
            tuple_idx
        }
        Value::Record(fields) => {
            let record_idx = builder.add_record();
            let mut items = Vec::new();
            for value in fields {
                let item_idx = build_wit_value(value, builder);
                items.push(item_idx);
            }
            builder.finish_seq(items, record_idx);
            record_idx
        }
        Value::Variant {
            case_idx,
            case_value: Some(case_value),
        } => {
            let variant_idx = builder.add_variant(case_idx, -1);
            let inner_idx = build_wit_value(*case_value, builder);
            builder.finish_child(inner_idx, variant_idx);
            variant_idx
        }
        Value::Variant {
            case_idx,
            case_value: None,
        } => builder.add_variant_unit(case_idx),
        Value::Enum(value) => builder.add_enum_value(value),
        Value::Flags(values) => builder.add_flags(values),
        Value::Option(value) => {
            if let Some(value) = value {
                let option_idx = builder.add_option_some();
                let inner_idx = build_wit_value(*value, builder);
                builder.finish_child(inner_idx, option_idx);
                option_idx
            } else {
                builder.add_option_none()
            }
        }
        Value::Result(result) => match result {
            Ok(Some(ok)) => {
                let result_idx = builder.add_result_ok();
                let inner_idx = build_wit_value(*ok, builder);
                builder.finish_child(inner_idx, result_idx);
                result_idx
            }
            Ok(None) => builder.add_result_ok_unit(),
            Err(Some(err)) => {
                let result_idx = builder.add_result_err();
                let inner_idx = build_wit_value(*err, builder);
                builder.finish_child(inner_idx, result_idx);
                result_idx
            }
            Err(None) => builder.add_result_err_unit(),
        },
        Value::Handle { uri, resource_id } => builder.add_handle(Uri { value: uri }, resource_id),
    }
}

impl Value {
    pub fn type_case_name(&self) -> &'static str {
        match self {
            Value::Bool(_) => "bool",
            Value::U8(_) => "u8",
            Value::U16(_) => "u16",
            Value::U32(_) => "u32",
            Value::U64(_) => "u64",
            Value::S8(_) => "s8",
            Value::S16(_) => "s16",
            Value::S32(_) => "s32",
            Value::S64(_) => "s64",
            Value::F32(_) => "f32",
            Value::F64(_) => "f64",
            Value::Char(_) => "char",
            Value::String(_) => "string",
            Value::List(_) => "list",
            Value::Tuple(_) => "tuple",
            Value::Record(_) => "record",
            Value::Variant { .. } => "variant",
            Value::Enum(_) => "enum",
            Value::Flags(_) => "flags",
            Value::Option(_) => "option",
            Value::Result(_) => "result",
            Value::Handle { .. } => "handle",
        }
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl From<WitValue> for Value {
    fn from(value: WitValue) -> Self {
        assert!(!value.nodes.is_empty());
        build_tree(&value.nodes[0], &value.nodes)
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
fn build_tree(node: &WitNode, nodes: &[WitNode]) -> Value {
    match node {
        WitNode::RecordValue(field_indices) => {
            let mut fields = Vec::new();
            for index in field_indices {
                let value = build_tree(&nodes[*index as usize], nodes);
                fields.push(value);
            }
            Value::Record(fields)
        }
        WitNode::VariantValue((case_idx, Some(inner_idx))) => {
            let value = build_tree(&nodes[*inner_idx as usize], nodes);
            Value::Variant {
                case_idx: *case_idx,
                case_value: Some(Box::new(value)),
            }
        }
        WitNode::VariantValue((case_idx, None)) => Value::Variant {
            case_idx: *case_idx,
            case_value: None,
        },
        WitNode::EnumValue(value) => Value::Enum(*value),
        WitNode::FlagsValue(values) => Value::Flags(values.clone()),
        WitNode::TupleValue(indices) => {
            let mut values = Vec::new();
            for index in indices {
                let value = build_tree(&nodes[*index as usize], nodes);
                values.push(value);
            }
            Value::Tuple(values)
        }
        WitNode::ListValue(indices) => {
            let mut values = Vec::new();
            for index in indices {
                let value = build_tree(&nodes[*index as usize], nodes);
                values.push(value);
            }
            Value::List(values)
        }
        WitNode::OptionValue(Some(index)) => {
            let value = build_tree(&nodes[*index as usize], nodes);
            Value::Option(Some(Box::new(value)))
        }
        WitNode::OptionValue(None) => Value::Option(None),
        WitNode::ResultValue(Ok(Some(index))) => {
            let value = build_tree(&nodes[*index as usize], nodes);
            Value::Result(Ok(Some(Box::new(value))))
        }
        WitNode::ResultValue(Ok(None)) => Value::Result(Ok(None)),
        WitNode::ResultValue(Err(Some(index))) => {
            let value = build_tree(&nodes[*index as usize], nodes);
            Value::Result(Err(Some(Box::new(value))))
        }
        WitNode::ResultValue(Err(None)) => Value::Result(Err(None)),
        WitNode::PrimU8(value) => Value::U8(*value),
        WitNode::PrimU16(value) => Value::U16(*value),
        WitNode::PrimU32(value) => Value::U32(*value),
        WitNode::PrimU64(value) => Value::U64(*value),
        WitNode::PrimS8(value) => Value::S8(*value),
        WitNode::PrimS16(value) => Value::S16(*value),
        WitNode::PrimS32(value) => Value::S32(*value),
        WitNode::PrimS64(value) => Value::S64(*value),
        WitNode::PrimFloat32(value) => Value::F32(*value),
        WitNode::PrimFloat64(value) => Value::F64(*value),
        WitNode::PrimChar(value) => Value::Char(*value),
        WitNode::PrimBool(value) => Value::Bool(*value),
        WitNode::PrimString(value) => Value::String(value.clone()),
        WitNode::Handle((uri, value)) => Value::Handle {
            uri: uri.value.clone(),
            resource_id: *value,
        },
    }
}

#[cfg(feature = "host")]
impl<'a> arbitrary::Arbitrary<'a> for WitValue {
    fn arbitrary(u: &mut arbitrary::Unstructured<'a>) -> arbitrary::Result<Self> {
        let arbitrary_value = u.arbitrary::<Value>()?;
        Ok(arbitrary_value.into())
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl From<uuid::Uuid> for AccountId {
    fn from(value: uuid::Uuid) -> Self {
        Self { uuid: value.into() }
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl From<AccountId> for uuid::Uuid {
    fn from(value: AccountId) -> Self {
        value.uuid.into()
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl std::str::FromStr for AccountId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(uuid::Uuid::parse_str(s)?.into())
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl std::fmt::Display for AccountId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let uuid: uuid::Uuid = self.uuid.into();
        write!(f, "{uuid}")
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl From<uuid::Uuid> for ComponentId {
    fn from(value: uuid::Uuid) -> Self {
        Self { uuid: value.into() }
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl From<ComponentId> for uuid::Uuid {
    fn from(value: ComponentId) -> Self {
        value.uuid.into()
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl std::str::FromStr for ComponentId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(uuid::Uuid::parse_str(s)?.into())
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl std::fmt::Display for ComponentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let uuid: uuid::Uuid = self.uuid.into();
        write!(f, "{uuid}")
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.component_id, self.agent_id)
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl std::str::FromStr for AgentId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let component_id = ComponentId::from_str(parts[0])
                .map_err(|_| format!("invalid component id: {s} - expected uuid"))?;
            let agent_id = parts[1].to_string();
            Ok(Self {
                component_id,
                agent_id,
            })
        } else {
            Err(format!(
                "invalid agent id: {s} - expected format: <component_id>/<agent_id>"
            ))
        }
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl TryFrom<Uri> for AgentId {
    type Error = String;

    fn try_from(uri: Uri) -> Result<Self, Self::Error> {
        let urn = uri.value;
        if !urn.starts_with("urn:worker:") {
            Err("Invalid URN: must start with 'urn:worker:', got '{urn}'".to_string())
        } else {
            let remaining = &urn[11..];
            let parts: Vec<&str> = remaining.split('/').collect();
            match parts.len() {
                2 => {
                    use std::str::FromStr;
                    let component_id = ComponentId::from_str(parts[0]).map_err(|err|
                        format!("Invalid URN: expected UUID for component_id: {err}")
                    )?;
                    let agent_id = parts[1];
                    Ok(AgentId {
                        component_id,
                        agent_id: agent_id.to_string(),
                    })
                }
                _ => Err(format!(
                    "Invalid URN: expected format 'urn:worker:<component_id>/<worker_name>', got '{urn}'",
                )),
            }
        }
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl std::fmt::Display for PromiseId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.agent_id, self.oplog_idx)
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
impl std::str::FromStr for PromiseId {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() == 2 {
            let agent_id = AgentId::from_str(parts[0]).map_err(|_| {
                format!("invalid agent id: {s} - expected format: <component_id>/<agent_id>")
            })?;
            let oplog_idx = parts[1]
                .parse()
                .map_err(|_| format!("invalid oplog index: {s} - expected integer"))?;
            Ok(Self {
                agent_id,
                oplog_idx,
            })
        } else {
            Err(format!(
                "invalid promise id: {s} - expected format: <agent_id>/<oplog_idx>"
            ))
        }
    }
}

#[cfg(any(feature = "host", feature = "stub"))]
#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::{Value, WitValue};
    use proptest::prelude::*;
    use proptest_arbitrary_interop::arb_sized;

    const CASES: u32 = 10000;
    const SIZE: usize = 4096;

    proptest! {

        #![proptest_config(ProptestConfig {
            cases: CASES, .. ProptestConfig::default()
        })]
        #[test]
        fn round_trip(value in arb_sized::<Value>(SIZE).prop_filter("Value must be equal to itself", |v| v.eq(v))) {
            let wit_value: WitValue = value.clone().into();
            let round_trip_value: Value = wit_value.into();
            prop_assert_eq!(value, round_trip_value);
        }
    }
}
