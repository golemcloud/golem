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
use indexmap::IndexMap;
use serde::Serialize;
use serde_derive::Deserialize;
use std::hash::Hash;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
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
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        #[serde(skip_serializing_if = "IndexMap::is_empty")]
        inserted_entries: IndexMap<K, V>,
        #[serde(skip_serializing_if = "IndexMap::is_empty")]
        updated_entries: IndexMap<K, V>,
    },
    #[serde(rename_all = "camelCase")]
    Replace {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        new_entries: IndexMap<K, V>,
    },
    #[serde(rename_all = "camelCase")]
    Remove {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        removed_entries: IndexMap<K, V>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MapProperty<L: Layer, K: Serialize, V: Serialize> {
    value: IndexMap<K, V>,
    trace: Vec<MapPropertyTraceElem<L, K, V>>,
}

impl<L: Layer, K: Serialize, V: Serialize> Default for MapProperty<L, K, V> {
    fn default() -> Self {
        Self {
            value: IndexMap::new(),
            trace: vec![],
        }
    }
}

impl<L: Layer, K: Serialize, V: Serialize> MapProperty<L, K, V> {
    pub fn new(map: IndexMap<K, V>) -> Self {
        Self {
            value: map,
            trace: vec![],
        }
    }
}

impl<L: Layer, K: Serialize, V: Serialize> From<IndexMap<K, V>> for MapProperty<L, K, V> {
    fn from(value: IndexMap<K, V>) -> Self {
        Self::new(value)
    }
}

impl<L: Layer, K: Eq + Hash + Clone + Serialize, V: Clone + Serialize> Property<L>
    for MapProperty<L, K, V>
{
    type Value = IndexMap<K, V>;
    type PropertyLayer = (MapMergeMode, IndexMap<K, V>);
    type TraceElem = MapPropertyTraceElem<L, K, V>;

    fn value(&self) -> &Self::Value {
        &self.value
    }

    fn trace(&self) -> &[Self::TraceElem] {
        self.trace.as_slice()
    }

    fn apply_layer(
        &mut self,
        id: &L::Id,
        selection: Option<&L::AppliedSelection>,
        layer: Self::PropertyLayer,
    ) {
        let (mode, map) = layer;
        match mode {
            MapMergeMode::Upsert => {
                let mut inserted_entries = IndexMap::new();
                let mut updated_entries = IndexMap::new();
                for (key, value) in map {
                    if self.value.insert(key.clone(), value.clone()).is_some() {
                        updated_entries.insert(key, value);
                    } else {
                        inserted_entries.insert(key, value);
                    }
                }
                self.trace.push(MapPropertyTraceElem::Upsert {
                    id: id.clone(),
                    selection: selection.cloned(),
                    inserted_entries,
                    updated_entries,
                });
            }
            MapMergeMode::Replace => {
                self.value = map.clone();
                self.trace.push(MapPropertyTraceElem::Replace {
                    id: id.clone(),
                    selection: selection.cloned(),
                    new_entries: map,
                })
            }
            MapMergeMode::Remove => {
                let mut removed_entries = IndexMap::new();
                for (key, _) in map {
                    if let Some(value) = self.value.shift_remove(&key) {
                        removed_entries.insert(key, value);
                    }
                }
                self.trace.push(MapPropertyTraceElem::Remove {
                    id: id.clone(),
                    selection: selection.cloned(),
                    removed_entries,
                })
            }
        }
    }

    fn compact_trace(&mut self) {
        self.trace.retain(|elem| match elem {
            MapPropertyTraceElem::Upsert {
                inserted_entries,
                updated_entries,
                ..
            } => !inserted_entries.is_empty() || !updated_entries.is_empty(),
            MapPropertyTraceElem::Replace { .. } => true,
            MapPropertyTraceElem::Remove {
                removed_entries, ..
            } => !removed_entries.is_empty(),
        })
    }
}
