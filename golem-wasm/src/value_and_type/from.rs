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

use crate::{Value, ValueAndType};

pub trait FromValue: Sized {
    fn from_value(value: Value) -> Result<Self, String>;
}

pub trait FromValueAndType: Sized {
    fn from_value_and_type(value_and_type: ValueAndType) -> Result<Self, String>;
}

impl<T: FromValue> FromValueAndType for T {
    fn from_value_and_type(value_and_type: ValueAndType) -> Result<Self, String> {
        Self::from_value(value_and_type.value)
    }
}

impl FromValue for u8 {
    fn from_value(value: Value) -> Result<Self, String> {
        match value {
            Value::U8(value) => Ok(value),
            _ => Err(format!("Expected u8 value, got {value:?}")),
        }
    }
}
