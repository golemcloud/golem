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

use crate::model::cascade::layer::Layer;
use crate::model::cascade::property::Property;
use serde::Serialize;
use std::collections::HashMap;
use std::hash::Hash;

#[derive(Debug, Clone, Copy, Default, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MapMergeMode {
    #[default]
    Upsert,
    Replace,
    Remove,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum MapPropertyTraceElem<L: Layer, K, V> {
    #[serde(rename_all = "camelCase")]
    Upsert {
        id: L::Id,
        inserted_entries: HashMap<K, V>,
        updated_entries: HashMap<K, V>,
    },
    #[serde(rename_all = "camelCase")]
    Replace {
        id: L::Id,
        new_entries: HashMap<K, V>,
    },
    #[serde(rename_all = "camelCase")]
    Remove {
        id: L::Id,
        removed_entries: HashMap<K, V>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapProperty<L: Layer, K: Serialize, V: Serialize> {
    map: HashMap<K, V>,
    trace: Vec<MapPropertyTraceElem<L, K, V>>,
}

impl<L: Layer, K: Serialize, V: Serialize> Default for MapProperty<L, K, V> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            trace: vec![],
        }
    }
}

impl<L: Layer, K: Serialize, V: Serialize> MapProperty<L, K, V> {
    pub fn new(map: HashMap<K, V>) -> Self {
        Self { map, trace: vec![] }
    }
}

impl<L: Layer, K: Serialize, V: Serialize> From<HashMap<K, V>> for MapProperty<L, K, V> {
    fn from(value: HashMap<K, V>) -> Self {
        Self::new(value)
    }
}

impl<L: Layer, K: Eq + Hash + Clone + Serialize, V: Clone + Serialize> Property<L>
    for MapProperty<L, K, V>
{
    type Value = HashMap<K, V>;
    type PropertyLayer = (MapMergeMode, HashMap<K, V>);
    type TraceElem = MapPropertyTraceElem<L, K, V>;

    fn value(&self) -> &Self::Value {
        &self.map
    }

    fn trace(&self) -> &[Self::TraceElem] {
        self.trace.as_slice()
    }

    fn apply_layer(
        &mut self,
        id: &L::Id,
        _selection: Option<&L::AppliedSelection>,
        layer: Self::PropertyLayer,
    ) {
        let (mode, map) = layer;
        match mode {
            MapMergeMode::Upsert => {
                let mut inserted_entries = HashMap::new();
                let mut updated_entries = HashMap::new();
                for (key, value) in map {
                    if self.map.insert(key.clone(), value.clone()).is_some() {
                        updated_entries.insert(key, value);
                    } else {
                        inserted_entries.insert(key, value);
                    }
                }
                self.trace.push(MapPropertyTraceElem::Upsert {
                    id: id.clone(),
                    inserted_entries,
                    updated_entries,
                });
            }
            MapMergeMode::Replace => {
                self.map = map.clone();
                self.trace.push(MapPropertyTraceElem::Replace {
                    id: id.clone(),
                    new_entries: map,
                })
            }
            MapMergeMode::Remove => {
                let mut removed_entries = HashMap::new();
                for (key, _) in map {
                    if let Some(value) = self.map.remove(&key) {
                        removed_entries.insert(key, value);
                    }
                }
                self.trace.push(MapPropertyTraceElem::Remove {
                    id: id.clone(),
                    removed_entries,
                })
            }
        }
    }
}
