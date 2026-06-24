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

//! Typed, unit-aware wrapper around the rich [`SchemaType::Quantity`] type.
//!
//! The schema of a Rust type is static — [`IntoSchema::register_in`] has no
//! access to a value — so the accepted unit set is carried at the type level by
//! a [`QuantityUnit`] marker type. This mirrors how validation enforces units
//! from the schema's [`QuantitySpec`] rather than from each runtime value.

use std::marker::PhantomData;

use super::{FromSchema, FromSchemaError, IntoSchema, SchemaBuilder, type_id_with_args, value_kind};
use crate::schema::metadata::TypeId;
use crate::schema::schema_type::{QuantitySpec, QuantityValue, SchemaType};
use crate::schema::schema_value::SchemaValue;

/// Marker trait describing the unit constraints of a [`Quantity`].
pub trait QuantityUnit: 'static {
    /// Stable identifier of this unit, embedded into the [`Quantity`] type id.
    fn type_id() -> TypeId;

    /// Canonical base unit (e.g. `"kg"`, `"m"`, `"s"`, `"B"`).
    fn base_unit() -> &'static str;

    /// Suffixes accepted on input and rendered on output.
    ///
    /// If empty, only [`QuantityUnit::base_unit`] is accepted. If non-empty,
    /// only the listed suffixes are accepted, so include the base unit here if
    /// it should remain valid.
    fn allowed_suffixes() -> &'static [&'static str] {
        &[]
    }
}

/// A fixed-point decimal value with a unit, constrained by the [`QuantityUnit`]
/// marker `U`. Encodes to the rich [`SchemaValue::Quantity`].
///
/// `Clone`/`Debug`/`PartialEq`/`Eq` are implemented by hand so the marker type
/// `U` itself does not need to implement them.
pub struct Quantity<U: QuantityUnit> {
    value: QuantityValue,
    _unit: PhantomData<U>,
}

impl<U: QuantityUnit> Clone for Quantity<U> {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            _unit: PhantomData,
        }
    }
}

impl<U: QuantityUnit> std::fmt::Debug for Quantity<U> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Quantity")
            .field("value", &self.value)
            .finish()
    }
}

impl<U: QuantityUnit> PartialEq for Quantity<U> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<U: QuantityUnit> Eq for Quantity<U> {}

impl<U: QuantityUnit> Quantity<U> {
    /// Constructs a quantity, validating the unit against `U`.
    pub fn new(
        mantissa: i64,
        scale: i32,
        unit: impl Into<String>,
    ) -> Result<Self, FromSchemaError> {
        Self::from_quantity_value(QuantityValue {
            mantissa,
            scale,
            unit: unit.into(),
        })
    }

    /// Wraps a [`QuantityValue`], validating its unit against `U`.
    pub fn from_quantity_value(value: QuantityValue) -> Result<Self, FromSchemaError> {
        if !Self::unit_allowed(&value.unit) {
            return Err(FromSchemaError::custom(format!(
                "quantity unit `{}` is not allowed for `{}`",
                value.unit,
                U::type_id().as_str()
            )));
        }
        Ok(Self {
            value,
            _unit: PhantomData,
        })
    }

    pub fn as_quantity_value(&self) -> &QuantityValue {
        &self.value
    }

    pub fn into_quantity_value(self) -> QuantityValue {
        self.value
    }

    fn unit_allowed(unit: &str) -> bool {
        let allowed = U::allowed_suffixes();
        if allowed.is_empty() {
            unit == U::base_unit()
        } else {
            allowed.iter().any(|candidate| *candidate == unit)
        }
    }

    fn spec() -> QuantitySpec {
        QuantitySpec {
            base_unit: U::base_unit().to_string(),
            allowed_suffixes: U::allowed_suffixes()
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
            min: None,
            max: None,
        }
    }
}

impl<U: QuantityUnit> IntoSchema for Quantity<U> {
    fn type_id() -> TypeId {
        type_id_with_args("golem.schema.Quantity", &[U::type_id()])
    }
    fn register_in(_b: &mut SchemaBuilder) -> SchemaType {
        SchemaType::quantity(Self::spec())
    }
    fn to_value(&self) -> SchemaValue {
        SchemaValue::Quantity(self.value.clone())
    }
}

impl<U: QuantityUnit> FromSchema for Quantity<U> {
    fn from_value(v: &SchemaValue) -> Result<Self, FromSchemaError> {
        match v {
            SchemaValue::Quantity(q) => Self::from_quantity_value(q.clone()),
            other => Err(FromSchemaError::shape_mismatch(
                "quantity",
                value_kind(other),
                "Quantity",
            )),
        }
    }
}
