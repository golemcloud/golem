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

#[cfg(any(feature = "host", feature = "client"))]
mod from;
#[cfg(any(feature = "host", feature = "client"))]
mod into;
#[cfg(all(any(feature = "host", feature = "client"), test))]
mod tests;

use crate::analysis::AnalysedType;
use crate::Value;
use uuid::Uuid;

#[cfg(any(feature = "host", feature = "client"))]
pub use into::ConvertToValueAndType;
#[cfg(any(feature = "host", feature = "client"))]
pub use into::IntoValue;
#[cfg(any(feature = "host", feature = "client"))]
pub use into::IntoValueAndType;

#[cfg(any(feature = "host", feature = "client"))]
pub use from::FromValue;
#[cfg(any(feature = "host", feature = "client"))]
pub use from::FromValueAndType;

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "host", derive(desert_rust::BinaryCodec))]
pub struct ValueAndType {
    pub value: Value,
    pub typ: AnalysedType,
}

#[cfg(feature = "host")]
impl std::fmt::Display for ValueAndType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            crate::text::print_value_and_type(self).unwrap_or("<unprintable>".to_string())
        )
    }
}

impl ValueAndType {
    pub fn new(value: Value, typ: AnalysedType) -> Self {
        Self { value, typ }
    }

    pub fn convert_to_value_and_type(self) -> ValueAndType {
        self
    }

    pub fn into_list_items(self) -> Option<Vec<ValueAndType>> {
        match (self.value, self.typ) {
            (Value::List(items), AnalysedType::List(item_type)) => Some(
                items
                    .into_iter()
                    .map(|item| ValueAndType::new(item, (*item_type.inner).clone()))
                    .collect(),
            ),
            _ => None,
        }
    }
}

impl From<ValueAndType> for Value {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.value
    }
}

impl From<ValueAndType> for AnalysedType {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.typ
    }
}

#[cfg(feature = "host")]
impl From<ValueAndType> for crate::WitValue {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.value.into()
    }
}

#[cfg(feature = "host")]
impl From<ValueAndType> for crate::WitType {
    fn from(value_and_type: ValueAndType) -> Self {
        value_and_type.typ.into()
    }
}

/// Helper for dynamically creating record ValueAndType values with String keys
pub struct Record<K: AsRef<str>>(pub Vec<(K, ValueAndType)>);

/// Wrapped Uuid, matching the schema provided by the Golem Rust SDK
#[derive(Clone, Debug)]
pub struct UuidRecord {
    pub value: Uuid,
}

impl From<Uuid> for UuidRecord {
    fn from(uuid: Uuid) -> Self {
        Self { value: uuid }
    }
}

impl From<UuidRecord> for Uuid {
    fn from(uuid_record: UuidRecord) -> Self {
        uuid_record.value
    }
}
