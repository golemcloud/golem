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
use std::collections::HashSet;

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
    String => StringValue,
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
