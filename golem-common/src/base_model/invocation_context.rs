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

use golem_wasm::analysis::{analysed_type, AnalysedType};
use golem_wasm::{FromValue, IntoValue, Value};
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::num::{NonZeroU128, NonZeroU64};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
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

    pub fn generate() -> Self {
        Self(NonZeroU128::new(Uuid::new_v4().as_u128()).unwrap())
    }
}

impl Display for TraceId {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl IntoValue for TraceId {
    fn into_value(self) -> Value {
        Value::String(self.to_string())
    }

    fn get_type() -> AnalysedType {
        analysed_type::str()
    }
}

impl FromValue for TraceId {
    fn from_value(value: Value) -> Result<Self, String> {
        let str = String::from_value(value)?;
        Self::from_string(str)
    }
}

impl Serialize for TraceId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(&self.to_string(), serializer)
    }
}

impl<'de> Deserialize<'de> for TraceId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::from_string(<String as Deserialize>::deserialize(deserializer)?)
            .map_err(Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
#[cfg_attr(feature = "full", desert(transparent))]
pub struct SpanId(pub NonZeroU64);

impl SpanId {
    pub fn from_string(value: impl AsRef<str>) -> Result<Self, String> {
        let n = u64::from_str_radix(value.as_ref(), 16)
            .map_err(|err| format!("Span ID must be a 64bit value in hexadecimal format: {err}"))?;
        let n = NonZeroU64::new(n).ok_or_else(|| "Span ID must be a non-zero value".to_string())?;
        Ok(Self(n))
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

impl IntoValue for SpanId {
    fn into_value(self) -> Value {
        Value::String(self.to_string())
    }

    fn get_type() -> AnalysedType {
        analysed_type::str()
    }
}

impl FromValue for SpanId {
    fn from_value(value: Value) -> Result<Self, String> {
        let str = String::from_value(value)?;
        Self::from_string(str)
    }
}

impl Serialize for SpanId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        Serialize::serialize(&self.to_string(), serializer)
    }
}

impl<'de> Deserialize<'de> for SpanId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::from_string(<String as Deserialize>::deserialize(deserializer)?)
            .map_err(Error::custom)
    }
}
