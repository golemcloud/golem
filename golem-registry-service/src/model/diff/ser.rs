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

use serde::{Serialize, Serializer};
use std::cell::Cell;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy)]
pub enum SerializeMode {
    HashOnly,
    ValueIfAvailable,
}

thread_local! {
    static SERIALIZE_MODE: Cell<SerializeMode> = const { Cell::new(SerializeMode::HashOnly) };
}

// TODO: maybe switch to serde_json::Value instead of the Box?
pub trait ToSerializableWithMode {
    fn to_serializable(&self, mode: SerializeMode) -> serde_json::Value;
}

impl<V: ToSerializableWithMode> ToSerializableWithMode for BTreeMap<String, V> {
    fn to_serializable(&self, mode: SerializeMode) -> serde_json::Value {
        serde_json::Value::Object(serde_json::Map::from_iter(
            self.iter()
                .map(|(k, v)| (k.clone(), v.to_serializable(mode))),
        ))
    }
}

pub fn serialize_with_mode<S: Serializer, T: ToSerializableWithMode>(
    value: &T,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    value
        .to_serializable(SERIALIZE_MODE.get())
        .serialize(serializer)
}

pub fn to_json_with_mode<T: Serialize>(
    value: &T,
    mode: SerializeMode,
) -> serde_json::Result<String> {
    SERIALIZE_MODE.set(mode);
    serde_json::to_string(value)
}

pub fn to_pretty_json_with_mode<T: Serialize>(
    value: &T,
    mode: SerializeMode,
) -> serde_json::Result<String> {
    SERIALIZE_MODE.set(mode);
    serde_json::to_string_pretty(value)
}

pub fn to_yaml_with_mode<T: Serialize>(
    value: &T,
    mode: SerializeMode,
) -> serde_yaml::Result<String> {
    SERIALIZE_MODE.set(mode);
    serde_yaml::to_string(value)
}
