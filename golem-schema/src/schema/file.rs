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

use super::{
    FromSchema, FromSchemaError, IntoSchema, Role, SchemaBuilder, SchemaType, SchemaValue, TypeId,
};

/// A config declaration for file content, represented as a path at the manifest boundary.
/// This type does not provide filesystem access or mount a file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct File(pub String);

impl File {
    const ROLE: &'static str = "golem.config.file";

    pub fn is_declared_type(ty: &SchemaType) -> bool {
        matches!(ty, SchemaType::String { metadata }
            if matches!(&metadata.role, Some(Role::Other(role)) if role == Self::ROLE))
    }
}

impl IntoSchema for File {
    fn type_id() -> TypeId {
        TypeId::new("golem.config.File")
    }

    fn register_in(_: &mut SchemaBuilder) -> SchemaType {
        let mut ty = SchemaType::string();
        ty.metadata_mut().role = Some(Role::Other(Self::ROLE.into()));
        ty
    }

    fn to_value(&self) -> SchemaValue {
        self.0.to_value()
    }
}

impl FromSchema for File {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        String::from_value(value).map(Self)
    }
}

#[cfg(all(feature = "guest", not(feature = "host")))]
mod wire_impl {
    use super::File;
    use crate::schema::wit::direct::{
        self, FromWire, IntoWire, WireError, WireReader, WireSchema, WireSchemaBuilder, WireWriter,
    };
    use crate::schema::wit::wire;

    impl WireSchema for File {
        fn append_schema(builder: &mut WireSchemaBuilder) -> wire::TypeNodeIndex {
            let mut metadata = direct::empty_metadata();
            metadata.role = Some(wire::Role::Other(Self::ROLE.into()));
            builder.push_with_metadata(wire::SchemaTypeBody::StringType, metadata)
        }
    }

    impl IntoWire for File {
        fn write_wire(&self, writer: &mut WireWriter) -> Result<wire::ValueNodeIndex, WireError> {
            self.0.write_wire(writer)
        }
    }

    impl FromWire for File {
        fn read_wire(
            reader: &mut WireReader,
            index: wire::ValueNodeIndex,
        ) -> Result<Self, WireError> {
            String::read_wire(reader, index).map(Self)
        }
    }
}
