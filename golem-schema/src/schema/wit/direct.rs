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

//! Concrete guest values converted directly to the canonical wire arena.
//!
//! These conversions check shape and affine ownership, not schema constraints.
//! The caller must validate the input against its declared schema at the host
//! boundary. No schema graph is needed to convert a statically known Rust type.

use super::wire::ValueNodeIndex;
use super::{GuestPermissionCardHandle, GuestQuotaTokenHandle, GuestSecretHandle, wire};
use crate::schema::SchemaValueStream;
use crate::schema::{Quantity, QuantityUnit, QuantityValue};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Builds the flat WIT schema arena used alongside directly encoded values.
/// Named definitions are reserved before their bodies are appended, allowing
/// derived recursive types to refer to themselves without constructing a
/// recursive schema model.
#[derive(Default)]
pub struct WireSchemaBuilder {
    type_nodes: Vec<wire::SchemaTypeNode>,
    defs: Vec<wire::SchemaTypeDef>,
    named: HashMap<String, wire::DefIndex>,
}

impl WireSchemaBuilder {
    pub fn push(&mut self, body: wire::SchemaTypeBody) -> wire::TypeNodeIndex {
        self.push_with_metadata(body, empty_metadata())
    }

    pub fn push_with_metadata(
        &mut self,
        body: wire::SchemaTypeBody,
        metadata: wire::MetadataEnvelope,
    ) -> wire::TypeNodeIndex {
        let index = self.type_nodes.len() as wire::TypeNodeIndex;
        self.type_nodes
            .push(wire::SchemaTypeNode { body, metadata });
        index
    }

    pub fn reserve(&mut self, id: String, name: Option<String>) -> (wire::DefIndex, bool) {
        if let Some(index) = self.named.get(&id) {
            return (*index, false);
        }
        let index = self.defs.len() as wire::DefIndex;
        self.named.insert(id.clone(), index);
        self.defs.push(wire::SchemaTypeDef { id, name, body: -1 });
        (index, true)
    }

    pub fn commit(&mut self, definition: wire::DefIndex, body: wire::TypeNodeIndex) {
        self.defs[definition as usize].body = body;
    }

    pub fn reference(&mut self, definition: wire::DefIndex) -> wire::TypeNodeIndex {
        self.push(wire::SchemaTypeBody::RefType(definition))
    }

    pub fn finish(self, root: wire::TypeNodeIndex) -> wire::SchemaGraph {
        wire::SchemaGraph {
            type_nodes: self.type_nodes,
            defs: self.defs,
            root,
        }
    }
}

pub fn empty_metadata() -> wire::MetadataEnvelope {
    wire::MetadataEnvelope {
        doc: None,
        aliases: Vec::new(),
        examples: Vec::new(),
        deprecated: None,
        role: None,
    }
}

/// Appends the concrete type's schema directly to a shared wire arena.
pub trait WireSchema {
    const IS_UNIT: bool = false;

    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex;

    fn wire_type_id() -> String {
        std::any::type_name::<Self>().replace("::", ".")
    }
}

pub fn schema<T: WireSchema + ?Sized>() -> wire::SchemaGraph {
    let mut builder = WireSchemaBuilder::default();
    let root = T::append_schema(&mut builder);
    builder.finish(root)
}

macro_rules! wire_schema_scalar {
    ($($ty:ty => $body:expr),* $(,)?) => {$(
        impl WireSchema for $ty {
            fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
                builder.push($body)
            }
        }
    )*};
}

wire_schema_scalar! {
    bool => wire::SchemaTypeBody::BoolType,
    i8 => wire::SchemaTypeBody::S8Type(None), i16 => wire::SchemaTypeBody::S16Type(None),
    i32 => wire::SchemaTypeBody::S32Type(None), i64 => wire::SchemaTypeBody::S64Type(None),
    u8 => wire::SchemaTypeBody::U8Type(None), u16 => wire::SchemaTypeBody::U16Type(None),
    u32 => wire::SchemaTypeBody::U32Type(None), u64 => wire::SchemaTypeBody::U64Type(None),
    f32 => wire::SchemaTypeBody::F32Type(None), f64 => wire::SchemaTypeBody::F64Type(None),
    char => wire::SchemaTypeBody::CharType, String => wire::SchemaTypeBody::StringType,
    str => wire::SchemaTypeBody::StringType,
}

impl<U: QuantityUnit> WireSchema for Quantity<U> {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        builder.push(wire::SchemaTypeBody::QuantityType(wire::QuantitySpec {
            base_unit: U::base_unit().to_string(),
            allowed_suffixes: U::allowed_suffixes()
                .iter()
                .map(|value| (*value).to_string())
                .collect(),
            min: None,
            max: None,
        }))
    }
}

impl<U: QuantityUnit> IntoWire for Quantity<U> {
    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        let value = self.as_quantity_value();
        Ok(writer.push(wire::SchemaValueNode::QuantityValueNode(
            wire::QuantityValue {
                mantissa: value.mantissa,
                scale: value.scale,
                unit: value.unit.clone(),
            },
        )))
    }
}

impl<U: QuantityUnit> FromWire for Quantity<U> {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        let wire::SchemaValueNode::QuantityValueNode(value) = reader.take(index)? else {
            return Err(WireError::Shape("quantity"));
        };
        Quantity::from_quantity_value(QuantityValue {
            mantissa: value.mantissa,
            scale: value.scale,
            unit: value.unit,
        })
        .map_err(|_| WireError::Shape("quantity unit"))
    }
}

impl WireSchema for std::path::PathBuf {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        builder.push(wire::SchemaTypeBody::PathType(wire::PathSpec {
            direction: wire::PathDirection::InOut,
            kind: wire::PathKind::Any,
            allowed_mime_types: None,
            allowed_extensions: None,
        }))
    }
}

impl FromWire for std::path::PathBuf {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        match reader.take(index)? {
            wire::SchemaValueNode::PathValue(path) => Ok(path.into()),
            _ => Err(WireError::Shape("path")),
        }
    }
}

impl IntoWire for std::path::PathBuf {
    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        Ok(writer.push(wire::SchemaValueNode::PathValue(
            self.to_string_lossy().into_owned(),
        )))
    }
}

impl<T: WireSchema + ?Sized> WireSchema for Box<T> {
    const IS_UNIT: bool = T::IS_UNIT;

    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        T::append_schema(builder)
    }
}
impl<T: WireSchema> WireSchema for Vec<T> {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        let element = T::append_schema(builder);
        builder.push(wire::SchemaTypeBody::ListType(element))
    }
}
impl<T: WireSchema> WireSchema for Option<T> {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        let inner = T::append_schema(builder);
        builder.push(wire::SchemaTypeBody::OptionType(inner))
    }
}
impl<T: WireSchema, E: WireSchema> WireSchema for Result<T, E> {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        let ok = if T::IS_UNIT {
            None
        } else {
            Some(T::append_schema(builder))
        };
        let err = if E::IS_UNIT {
            None
        } else {
            Some(E::append_schema(builder))
        };
        builder.push(wire::SchemaTypeBody::ResultType(wire::ResultSpec {
            ok,
            err,
        }))
    }
}
impl WireSchema for () {
    const IS_UNIT: bool = true;

    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        builder.push(wire::SchemaTypeBody::TupleType(Vec::new()))
    }
}
impl<K: WireSchema, V: WireSchema> WireSchema for HashMap<K, V> {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        let key = K::append_schema(builder);
        let value = V::append_schema(builder);
        builder.push(wire::SchemaTypeBody::MapType(wire::MapSpec { key, value }))
    }
}
impl<K: WireSchema, V: WireSchema> WireSchema for BTreeMap<K, V> {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        let key = K::append_schema(builder);
        let value = V::append_schema(builder);
        builder.push(wire::SchemaTypeBody::MapType(wire::MapSpec { key, value }))
    }
}

macro_rules! wire_map {
    ($map:ident) => {
        impl<K: IntoWire, V: IntoWire> IntoWire for $map<K, V> {
            fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
                for (key, value) in self {
                    key.preflight(resources)?;
                    value.preflight(resources)?;
                }
                Ok(())
            }
            async fn prepare_wire(&self) -> Result<(), WireError> {
                for (key, value) in self {
                    key.prepare_wire().await?;
                    value.prepare_wire().await?;
                }
                Ok(())
            }
            fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
                let entries = self
                    .iter()
                    .map(|(key, value)| {
                        Ok(wire::MapEntry {
                            key: key.write_wire(writer)?,
                            value: value.write_wire(writer)?,
                        })
                    })
                    .collect::<Result<_, WireError>>()?;
                Ok(writer.push(wire::SchemaValueNode::MapValue(entries)))
            }
        }
        impl<K: FromWire + Ord, V: FromWire> FromWire for $map<K, V> {
            fn read_wire(
                reader: &mut WireReader,
                index: ValueNodeIndex,
            ) -> Result<Self, WireError> {
                let wire::SchemaValueNode::MapValue(entries) = reader.take(index)? else {
                    return Err(WireError::Shape("map"));
                };
                entries
                    .into_iter()
                    .map(|entry| {
                        Ok((
                            K::read_wire(reader, entry.key)?,
                            V::read_wire(reader, entry.value)?,
                        ))
                    })
                    .collect()
            }
        }
    };
}
wire_map!(BTreeMap);

macro_rules! wire_schema_tuple {
    ($($ty:ident),+) => { impl<$($ty: WireSchema),+> WireSchema for ($($ty,)+) {
        fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
            let elements = vec![$($ty::append_schema(builder)),+];
            builder.push(wire::SchemaTypeBody::TupleType(elements))
        }
    }};
}
wire_schema_tuple!(A);
wire_schema_tuple!(A, B);
wire_schema_tuple!(A, B, C);
wire_schema_tuple!(A, B, C, D);
wire_schema_tuple!(A, B, C, D, E);
wire_schema_tuple!(A, B, C, D, E, F);
wire_schema_tuple!(A, B, C, D, E, F, G);
wire_schema_tuple!(A, B, C, D, E, F, G, H);

#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub enum WireError {
    #[error("expected wire {0}")]
    Shape(&'static str),
    #[error("wire value index {0} is out of bounds")]
    OutOfBounds(ValueNodeIndex),
    #[error("wire value index {0} is referenced more than once")]
    AliasedNode(ValueNodeIndex),
    #[error("wire resource at index {0} is not reachable from the root")]
    UnreachableResource(ValueNodeIndex),
    #[error("{0} resource appears more than once")]
    AliasedResource(&'static str),
    #[error("{0} resource has already been transferred")]
    ConsumedResource(&'static str),
    #[error("native streams require asynchronous wire encoding")]
    AsyncStream,
}

/// Decode a concrete value, consuming every visited wire node exactly once.
pub trait FromWire: Sized {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError>;

    fn read_result_payload(
        reader: &mut WireReader,
        index: Option<ValueNodeIndex>,
    ) -> Result<Self, WireError> {
        Self::read_wire(reader, index.ok_or(WireError::Shape("result payload"))?)
    }
}

/// Encode a concrete value without constructing an owned schema value.
///
/// Implementations must visit every resource in `preflight` before moving any
/// handles in `write_wire`. After a successful preflight, `write_wire` must not
/// fail except when a resource was concurrently transferred through an alias.
#[allow(async_fn_in_trait)]
pub trait IntoWire {
    fn preflight(&self, _resources: &mut WirePreflight) -> Result<(), WireError> {
        Ok(())
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError>;

    /// Wrap native stream endpoints without reading from them. This is called
    /// only after the complete value has passed resource preflight.
    async fn prepare_wire(&self) -> Result<(), WireError> {
        Ok(())
    }

    fn write_result_payload(
        &self,
        writer: &mut WireWriter,
    ) -> Result<Option<ValueNodeIndex>, WireError> {
        self.write_wire(writer).map(Some)
    }
}

/// Owns nodes until a concrete decoder takes them. Dropping a failed reader
/// drops all resources that have not already moved into decoded Rust values.
pub struct WireReader {
    nodes: Vec<Option<wire::SchemaValueNode>>,
}

impl WireReader {
    pub fn new(nodes: Vec<wire::SchemaValueNode>) -> Self {
        Self {
            nodes: nodes.into_iter().map(Some).collect(),
        }
    }

    pub fn take(&mut self, index: ValueNodeIndex) -> Result<wire::SchemaValueNode, WireError> {
        self.nodes
            .get_mut(index as usize)
            .ok_or(WireError::OutOfBounds(index))?
            .take()
            .ok_or(WireError::AliasedNode(index))
    }

    /// Consume an unbound field, releasing its resources and checking its edges
    /// without constructing a recursive value. The explicit stack also bounds
    /// stack usage for deeply nested fields not consumed by a typed decoder.
    pub fn discard(&mut self, index: ValueNodeIndex) -> Result<(), WireError> {
        let mut pending = vec![index];
        while let Some(index) = pending.pop() {
            match self.take(index)? {
                wire::SchemaValueNode::RecordValue(children)
                | wire::SchemaValueNode::TupleValue(children)
                | wire::SchemaValueNode::ListValue(children)
                | wire::SchemaValueNode::FixedListValue(children) => pending.extend(children),
                wire::SchemaValueNode::MapValue(entries) => {
                    for entry in entries {
                        pending.extend([entry.key, entry.value]);
                    }
                }
                wire::SchemaValueNode::VariantValue(value) => pending.extend(value.payload),
                wire::SchemaValueNode::OptionValue(value) => pending.extend(value),
                wire::SchemaValueNode::ResultValue(value) => match value {
                    wire::ResultValuePayload::OkValue(value)
                    | wire::ResultValuePayload::ErrValue(value) => pending.extend(value),
                },
                wire::SchemaValueNode::UnionValue(value) => pending.push(value.body),
                _ => {}
            }
        }
        Ok(())
    }

    pub fn finish(self) -> Result<(), WireError> {
        for (index, node) in self.nodes.iter().enumerate() {
            if matches!(
                node,
                Some(
                    wire::SchemaValueNode::SecretValue(_)
                        | wire::SchemaValueNode::QuotaTokenHandle(_)
                        | wire::SchemaValueNode::PermissionCardHandle(_)
                        | wire::SchemaValueNode::StreamValue(_)
                )
            ) {
                return Err(WireError::UnreachableResource(index as ValueNodeIndex));
            }
        }
        Ok(())
    }
}

pub fn decode<T: FromWire>(tree: wire::SchemaValueTree) -> Result<T, WireError> {
    let mut reader = WireReader::new(tree.value_nodes);
    let value = T::read_wire(&mut reader, tree.root)?;
    reader.finish()?;
    Ok(value)
}

#[derive(Default)]
pub struct WireWriter {
    nodes: Vec<wire::SchemaValueNode>,
}

impl WireWriter {
    pub fn push(&mut self, node: wire::SchemaValueNode) -> ValueNodeIndex {
        let index = self.nodes.len() as ValueNodeIndex;
        self.nodes.push(node);
        index
    }

    pub fn finish(self, root: ValueNodeIndex) -> wire::SchemaValueTree {
        wire::SchemaValueTree {
            value_nodes: self.nodes,
            root,
        }
    }
}

/// A resource-only preflight preserves live handles when aliasing or an
/// already-consumed handle would make an encode fail. It contains no value or
/// schema model and does not poll any stream.
#[derive(Default)]
pub struct WirePreflight {
    seen: HashSet<(&'static str, *const ())>,
    allow_native_streams: bool,
}

impl WirePreflight {
    fn resource(
        &mut self,
        kind: &'static str,
        id: *const (),
        present: bool,
    ) -> Result<(), WireError> {
        if !present {
            return Err(WireError::ConsumedResource(kind));
        }
        if !self.seen.insert((kind, id)) {
            return Err(WireError::AliasedResource(kind));
        }
        Ok(())
    }
}

pub fn encode<T: IntoWire + ?Sized>(value: &T) -> Result<wire::SchemaValueTree, WireError> {
    let mut preflight = WirePreflight::default();
    value.preflight(&mut preflight)?;
    write(value)
}

pub async fn encode_async<T: IntoWire + ?Sized>(
    value: &T,
) -> Result<wire::SchemaValueTree, WireError> {
    let mut preflight = WirePreflight {
        allow_native_streams: true,
        ..Default::default()
    };
    value.preflight(&mut preflight)?;
    value.prepare_wire().await?;
    write(value)
}

fn write<T: IntoWire + ?Sized>(value: &T) -> Result<wire::SchemaValueTree, WireError> {
    let mut writer = WireWriter::default();
    let root = value.write_wire(&mut writer)?;
    Ok(writer.finish(root))
}

macro_rules! scalar {
    ($($ty:ty => $variant:ident),* $(,)?) => {$(
        impl FromWire for $ty {
            fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
                match reader.take(index)? {
                    wire::SchemaValueNode::$variant(value) => Ok(value),
                    _ => Err(WireError::Shape(stringify!($variant))),
                }
            }
        }

        impl IntoWire for $ty {
            fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
                Ok(writer.push(wire::SchemaValueNode::$variant(self.clone())))
            }
        }
    )*};
}

scalar! {
    bool => BoolValue, i8 => S8Value, i16 => S16Value, i32 => S32Value,
    i64 => S64Value, u8 => U8Value, u16 => U16Value, u32 => U32Value,
    u64 => U64Value, f32 => F32Value, f64 => F64Value, char => CharValue,
}

impl FromWire for String {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        match reader.take(index)? {
            wire::SchemaValueNode::StringValue(value) => Ok(value),
            // Text-refined tool arguments retain String as their Rust type.
            wire::SchemaValueNode::TextValue(value) => Ok(value.text),
            _ => Err(WireError::Shape("string or text")),
        }
    }
}

impl IntoWire for String {
    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        self.as_str().write_wire(writer)
    }
}

impl WireSchema for usize {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        <u64 as WireSchema>::append_schema(builder)
    }
}
impl IntoWire for usize {
    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        (*self as u64).write_wire(writer)
    }
}
impl FromWire for usize {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        usize::try_from(u64::read_wire(reader, index)?).map_err(|_| WireError::Shape("usize range"))
    }
}

impl IntoWire for str {
    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        Ok(writer.push(wire::SchemaValueNode::StringValue(self.to_string())))
    }
}

impl<T: IntoWire + ?Sized> IntoWire for Box<T> {
    fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
        (**self).preflight(resources)
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        (**self).write_wire(writer)
    }

    async fn prepare_wire(&self) -> Result<(), WireError> {
        Box::pin((**self).prepare_wire()).await
    }

    fn write_result_payload(
        &self,
        writer: &mut WireWriter,
    ) -> Result<Option<ValueNodeIndex>, WireError> {
        (**self).write_result_payload(writer)
    }
}

impl<T: FromWire> FromWire for Box<T> {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        T::read_wire(reader, index).map(Box::new)
    }

    fn read_result_payload(
        reader: &mut WireReader,
        index: Option<ValueNodeIndex>,
    ) -> Result<Self, WireError> {
        T::read_result_payload(reader, index).map(Box::new)
    }
}

impl<T: IntoWire> IntoWire for Vec<T> {
    fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
        for value in self {
            value.preflight(resources)?;
        }
        Ok(())
    }

    async fn prepare_wire(&self) -> Result<(), WireError> {
        for value in self {
            Box::pin(value.prepare_wire()).await?;
        }
        Ok(())
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        let indices = self
            .iter()
            .map(|value| value.write_wire(writer))
            .collect::<Result<_, _>>()?;
        Ok(writer.push(wire::SchemaValueNode::ListValue(indices)))
    }
}

impl<T: FromWire> FromWire for Vec<T> {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        match reader.take(index)? {
            wire::SchemaValueNode::ListValue(indices) => indices
                .into_iter()
                .map(|index| T::read_wire(reader, index))
                .collect(),
            _ => Err(WireError::Shape("list")),
        }
    }
}

impl<T: IntoWire> IntoWire for Option<T> {
    fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
        if let Some(value) = self {
            value.preflight(resources)?;
        }
        Ok(())
    }

    async fn prepare_wire(&self) -> Result<(), WireError> {
        if let Some(value) = self {
            value.prepare_wire().await?;
        }
        Ok(())
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        let index = self
            .as_ref()
            .map(|value| value.write_wire(writer))
            .transpose()?;
        Ok(writer.push(wire::SchemaValueNode::OptionValue(index)))
    }
}

impl<T: FromWire> FromWire for Option<T> {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        match reader.take(index)? {
            wire::SchemaValueNode::OptionValue(index) => {
                index.map(|index| T::read_wire(reader, index)).transpose()
            }
            _ => Err(WireError::Shape("option")),
        }
    }
}

impl<T: IntoWire, E: IntoWire> IntoWire for Result<T, E> {
    fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
        match self {
            Ok(value) => value.preflight(resources),
            Err(value) => value.preflight(resources),
        }
    }

    async fn prepare_wire(&self) -> Result<(), WireError> {
        match self {
            Ok(value) => value.prepare_wire().await,
            Err(value) => value.prepare_wire().await,
        }
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        let payload = match self {
            Ok(value) => wire::ResultValuePayload::OkValue(value.write_result_payload(writer)?),
            Err(value) => wire::ResultValuePayload::ErrValue(value.write_result_payload(writer)?),
        };
        Ok(writer.push(wire::SchemaValueNode::ResultValue(payload)))
    }
}

impl<T: FromWire, E: FromWire> FromWire for Result<T, E> {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        match reader.take(index)? {
            wire::SchemaValueNode::ResultValue(wire::ResultValuePayload::OkValue(index)) => {
                T::read_result_payload(reader, index).map(Ok)
            }
            wire::SchemaValueNode::ResultValue(wire::ResultValuePayload::ErrValue(index)) => {
                E::read_result_payload(reader, index).map(Err)
            }
            _ => Err(WireError::Shape("result")),
        }
    }
}

impl IntoWire for () {
    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        Ok(writer.push(wire::SchemaValueNode::TupleValue(Vec::new())))
    }

    fn write_result_payload(
        &self,
        _writer: &mut WireWriter,
    ) -> Result<Option<ValueNodeIndex>, WireError> {
        Ok(None)
    }
}

impl FromWire for () {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        match reader.take(index)? {
            wire::SchemaValueNode::TupleValue(indices) if indices.is_empty() => Ok(()),
            _ => Err(WireError::Shape("unit tuple")),
        }
    }

    fn read_result_payload(
        _reader: &mut WireReader,
        index: Option<ValueNodeIndex>,
    ) -> Result<Self, WireError> {
        match index {
            Some(_) => Err(WireError::Shape("absent unit result payload")),
            None => Ok(()),
        }
    }
}

macro_rules! tuple {
    ($($index:tt : $ty:ident),+) => {
        impl<$($ty: IntoWire),+> IntoWire for ($($ty,)+) {
            fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
                $(self.$index.preflight(resources)?;)+
                Ok(())
            }

            async fn prepare_wire(&self) -> Result<(), WireError> {
                $(self.$index.prepare_wire().await?;)+
                Ok(())
            }

            fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
                let indices = vec![$(self.$index.write_wire(writer)?),+];
                Ok(writer.push(wire::SchemaValueNode::TupleValue(indices)))
            }
        }

        impl<$($ty: FromWire),+> FromWire for ($($ty,)+) {
            #[allow(non_snake_case)]
            fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
                match reader.take(index)? {
                    wire::SchemaValueNode::TupleValue(indices) => {
                        let [$($ty),+] = indices.as_slice() else {
                            return Err(WireError::Shape("tuple arity"));
                        };
                        Ok(($($ty::read_wire(reader, *$ty)?,)+))
                    }
                    _ => Err(WireError::Shape("tuple")),
                }
            }
        }
    };
}

tuple!(0: A);
tuple!(0: A, 1: B);
tuple!(0: A, 1: B, 2: C);
tuple!(0: A, 1: B, 2: C, 3: D);
tuple!(0: A, 1: B, 2: C, 3: D, 4: E);
tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F);
tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G);
tuple!(0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G, 7: H);

macro_rules! resource {
    ($ty:ty, $variant:ident, $kind:literal) => {
        impl FromWire for $ty {
            fn read_wire(
                reader: &mut WireReader,
                index: ValueNodeIndex,
            ) -> Result<Self, WireError> {
                match reader.take(index)? {
                    wire::SchemaValueNode::$variant(value) => Ok(Self::new(value)),
                    _ => Err(WireError::Shape($kind)),
                }
            }
        }

        impl IntoWire for $ty {
            fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
                resources.resource($kind, self.cell_id(), self.is_present())
            }

            fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
                let handle = self.take().ok_or(WireError::ConsumedResource($kind))?;
                Ok(writer.push(wire::SchemaValueNode::$variant(handle)))
            }
        }
    };
}

resource!(GuestSecretHandle, SecretValue, "secret");
resource!(GuestQuotaTokenHandle, QuotaTokenHandle, "quota-token");
resource!(
    GuestPermissionCardHandle,
    PermissionCardHandle,
    "permission-card"
);

impl WireSchema for GuestSecretHandle {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        let inner = <String as WireSchema>::append_schema(builder);
        builder.push(wire::SchemaTypeBody::SecretType(wire::SecretSpec {
            inner,
            category: None,
        }))
    }
}

impl WireSchema for GuestQuotaTokenHandle {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        builder.push(wire::SchemaTypeBody::QuotaTokenType(wire::QuotaTokenSpec {
            resource_name: None,
        }))
    }
}

impl WireSchema for GuestPermissionCardHandle {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        builder.push(wire::SchemaTypeBody::PermissionCardType(
            wire::PermissionCardSpec { polymorphic: false },
        ))
    }
}

impl FromWire for SchemaValueStream {
    fn read_wire(reader: &mut WireReader, index: ValueNodeIndex) -> Result<Self, WireError> {
        match reader.take(index)? {
            wire::SchemaValueNode::StreamValue(stream) => Ok(Self::from_wrapped(stream)),
            _ => Err(WireError::Shape("stream")),
        }
    }
}

impl IntoWire for SchemaValueStream {
    fn preflight(&self, resources: &mut WirePreflight) -> Result<(), WireError> {
        resources.resource("stream", self.cell_id(), self.is_present())?;
        if !resources.allow_native_streams && !self.is_wrapped() {
            return Err(WireError::AsyncStream);
        }
        Ok(())
    }

    async fn prepare_wire(&self) -> Result<(), WireError> {
        self.ensure_wrapped()
            .await
            .map_err(|_| WireError::ConsumedResource("stream"))
    }

    fn write_wire(&self, writer: &mut WireWriter) -> Result<ValueNodeIndex, WireError> {
        let stream = self.take_wrapped().ok_or(WireError::AsyncStream)?;
        Ok(writer.push(wire::SchemaValueNode::StreamValue(stream)))
    }
}

impl WireSchema for SchemaValueStream {
    fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
        builder.push(wire::SchemaTypeBody::StreamType(None))
    }
}
