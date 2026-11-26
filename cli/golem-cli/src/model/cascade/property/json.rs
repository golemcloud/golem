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

use crate::model::cascade::layer::Layer;
use crate::model::cascade::property::Property;
use indexmap::IndexMap;
use serde::Serialize;
use serde_derive::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum JsonMergeMode {
    #[default]
    MergeObjectReplaceOther,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum JsonPropertyTraceElem<L: Layer> {
    #[serde(rename_all = "camelCase")]
    ObjectMerge {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        #[serde(skip_serializing_if = "IndexMap::is_empty")]
        inserted_entries: IndexMap<String, Value>,
        #[serde(skip_serializing_if = "IndexMap::is_empty")]
        updated_entries: IndexMap<String, Value>,
    },
    #[serde(rename_all = "camelCase")]
    Replace {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        mode: JsonMergeMode,
        new_value: Value,
    },
    #[serde(rename_all = "camelCase")]
    Skip {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonProperty<L: Layer> {
    value: Option<Value>,
    trace: Vec<JsonPropertyTraceElem<L>>,
    // Source lookup for resolution; the trace already identifies its layers.
    #[serde(skip)]
    sources: BTreeMap<Vec<String>, PathBuf>,
}

impl<L: Layer> JsonProperty<L> {
    pub fn for_each_value_mut(&mut self, mut visit: impl FnMut(&mut Value)) {
        if let Some(value) = &mut self.value {
            visit(value);
        }
        for element in &mut self.trace {
            match element {
                JsonPropertyTraceElem::ObjectMerge {
                    inserted_entries,
                    updated_entries,
                    ..
                } => {
                    for value in inserted_entries
                        .values_mut()
                        .chain(updated_entries.values_mut())
                    {
                        visit(value);
                    }
                }
                JsonPropertyTraceElem::Replace { new_value, .. } => visit(new_value),
                JsonPropertyTraceElem::Skip { .. } => {}
            }
        }
    }

    pub fn source_at(&self, path: &[String]) -> Option<&Path> {
        (0..=path.len())
            .rev()
            .find_map(|len| self.sources.get(&path[..len]).map(PathBuf::as_path))
    }

    pub fn apply_sourced(
        &mut self,
        id: &L::Id,
        selection: Option<&L::AppliedSelection>,
        value: Option<Value>,
        source: &Path,
    ) {
        if let Some(value) = &value {
            self.overlay_sources(value, &[], source);
        }
        self.apply_layer(id, selection, value);
    }

    pub fn apply_inherited<M: Layer>(
        &mut self,
        id: &L::Id,
        selection: Option<&L::AppliedSelection>,
        parent: &JsonProperty<M>,
    ) {
        if let Some(value) = &parent.value {
            // A non-object value replaces its entire subtree; an object merges its keys.
            fn overlay(
                sources: &mut BTreeMap<Vec<String>, PathBuf>,
                path: &mut Vec<String>,
                value: &Value,
            ) {
                if let Value::Object(fields) = value {
                    sources.remove(path.as_slice());
                    for (key, child) in fields {
                        path.push(key.clone());
                        overlay(sources, path, child);
                        path.pop();
                    }
                } else {
                    sources.retain(|key, _| !key.starts_with(path));
                }
            }
            overlay(&mut self.sources, &mut Vec::new(), value);
            self.sources.extend(parent.sources.clone());
        }
        self.apply_layer(id, selection, parent.value.clone());
    }

    fn overlay_sources(&mut self, value: &Value, path: &[String], source: &Path) {
        if let Value::Object(fields) = value {
            self.sources.remove(path);
            let mut path = path.to_vec();
            for (key, child) in fields {
                path.push(key.clone());
                self.overlay_sources(child, &path, source);
                path.pop();
            }
        } else {
            self.sources.retain(|key, _| !key.starts_with(path));
            self.sources.insert(path.to_vec(), source.to_path_buf());
        }
    }
}

impl<L: Layer> Default for JsonProperty<L> {
    fn default() -> Self {
        Self {
            value: None,
            trace: vec![],
            sources: BTreeMap::new(),
        }
    }
}

impl<L: Layer> From<Option<Value>> for JsonProperty<L> {
    fn from(value: Option<Value>) -> Self {
        Self {
            value,
            trace: vec![],
            sources: BTreeMap::new(),
        }
    }
}

impl<L: Layer> Property<L> for JsonProperty<L> {
    type Value = Option<Value>;
    type PropertyLayer = Option<Value>;
    type TraceElem = JsonPropertyTraceElem<L>;

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
        match layer {
            None => self.trace.push(JsonPropertyTraceElem::Skip {
                id: id.clone(),
                selection: selection.cloned(),
            }),
            Some(layer_value) => {
                let current = self.value.take();
                match merge_json_object_values_with_trace(current, layer_value) {
                    JsonMergeResult::ObjectMerge {
                        merged,
                        inserted_entries,
                        updated_entries,
                    } => {
                        self.value = Some(merged);
                        self.trace.push(JsonPropertyTraceElem::ObjectMerge {
                            id: id.clone(),
                            selection: selection.cloned(),
                            inserted_entries,
                            updated_entries,
                        });
                    }
                    JsonMergeResult::Replace { new_value } => {
                        self.value = Some(new_value.clone());
                        self.trace.push(JsonPropertyTraceElem::Replace {
                            id: id.clone(),
                            selection: selection.cloned(),
                            mode: JsonMergeMode::MergeObjectReplaceOther,
                            new_value,
                        });
                    }
                }
            }
        }
    }

    fn compact_trace(&mut self) {
        self.trace.retain(|elem| match elem {
            JsonPropertyTraceElem::ObjectMerge {
                inserted_entries,
                updated_entries,
                ..
            } => !inserted_entries.is_empty() || !updated_entries.is_empty(),
            JsonPropertyTraceElem::Replace { .. } => true,
            JsonPropertyTraceElem::Skip { .. } => false,
        });
    }
}

enum JsonMergeResult {
    ObjectMerge {
        merged: Value,
        inserted_entries: IndexMap<String, Value>,
        updated_entries: IndexMap<String, Value>,
    },
    Replace {
        new_value: Value,
    },
}

fn merge_json_object_values_with_trace(current: Option<Value>, layer: Value) -> JsonMergeResult {
    match (current, layer) {
        (Some(Value::Object(mut current_obj)), Value::Object(layer_obj)) => {
            let mut inserted_entries = IndexMap::new();
            let mut updated_entries = IndexMap::new();

            for (k, layer_v) in layer_obj {
                if let Some(current_v) = current_obj.remove(&k) {
                    let merged = merge_json_values(current_v, layer_v);
                    updated_entries.insert(k.clone(), merged.clone());
                    current_obj.insert(k, merged);
                } else {
                    inserted_entries.insert(k.clone(), layer_v.clone());
                    current_obj.insert(k, layer_v);
                }
            }

            JsonMergeResult::ObjectMerge {
                merged: Value::Object(current_obj),
                inserted_entries,
                updated_entries,
            }
        }
        (None, Value::Object(layer_obj)) => JsonMergeResult::ObjectMerge {
            merged: Value::Object(layer_obj.clone()),
            inserted_entries: layer_obj.into_iter().collect(),
            updated_entries: IndexMap::new(),
        },
        (_, layer_value) => JsonMergeResult::Replace {
            new_value: layer_value,
        },
    }
}

fn merge_json_values(current: Value, layer: Value) -> Value {
    match (current, layer) {
        (Value::Object(mut current_obj), Value::Object(layer_obj)) => {
            for (k, layer_v) in layer_obj {
                if let Some(current_v) = current_obj.remove(&k) {
                    current_obj.insert(k.clone(), merge_json_values(current_v, layer_v));
                } else {
                    current_obj.insert(k, layer_v);
                }
            }
            Value::Object(current_obj)
        }
        (_, layer_value) => layer_value,
    }
}

#[cfg(test)]
mod test {
    use super::{JsonProperty, JsonPropertyTraceElem};
    use crate::model::cascade::property::Property;
    use crate::model::cascade::property::test_support::TestLayer;
    use serde_json::json;
    use test_r::test;

    #[test]
    fn object_merge_tracks_inserted_and_updated_entries() {
        let mut prop: JsonProperty<TestLayer> = None.into();
        let id = "layer-a".to_string();

        prop.apply_layer(&id, None, Some(json!({"a": 1, "nested": {"x": 1}})));
        prop.apply_layer(
            &id,
            Some(&"sel".to_string()),
            Some(json!({"a": 2, "b": 3, "nested": {"y": 2}})),
        );

        assert_eq!(
            prop.value(),
            &Some(json!({"a": 2, "b": 3, "nested": {"x": 1, "y": 2}}))
        );

        match &prop.trace()[1] {
            JsonPropertyTraceElem::ObjectMerge {
                inserted_entries,
                updated_entries,
                ..
            } => {
                assert_eq!(inserted_entries.get("b"), Some(&json!(3)));
                assert_eq!(updated_entries.get("a"), Some(&json!(2)));
                assert_eq!(
                    updated_entries.get("nested"),
                    Some(&json!({"x": 1, "y": 2}))
                );
            }
            _ => panic!("expected object-merge trace"),
        }
    }

    #[test]
    fn non_object_replaces_current_value() {
        let mut prop: JsonProperty<TestLayer> = None.into();
        let id = "layer-a".to_string();

        prop.apply_layer(&id, None, Some(json!({"a": 1})));
        prop.apply_layer(&id, None, Some(json!("scalar")));

        assert_eq!(prop.value(), &Some(json!("scalar")));

        match &prop.trace()[1] {
            JsonPropertyTraceElem::Replace { new_value, .. } => {
                assert_eq!(new_value, &json!("scalar"));
            }
            _ => panic!("expected replace trace"),
        }
    }

    #[test]
    fn compact_trace_removes_empty_object_merge_and_skips() {
        let mut prop: JsonProperty<TestLayer> = None.into();
        let id = "layer-a".to_string();

        prop.apply_layer(&id, None, Some(json!({})));
        prop.apply_layer(&id, None, None);
        prop.apply_layer(&id, None, Some(json!(1)));

        assert_eq!(prop.trace().len(), 3);
        prop.compact_trace();
        assert_eq!(prop.trace().len(), 1);
        assert!(matches!(
            prop.trace()[0],
            JsonPropertyTraceElem::Replace { .. }
        ));
    }

    #[test]
    fn nested_non_object_replaces_subtree() {
        let mut prop: JsonProperty<TestLayer> = None.into();
        let id = "layer-a".to_string();

        prop.apply_layer(&id, None, Some(json!({"nested": {"a": 1, "b": 2}})));
        prop.apply_layer(&id, None, Some(json!({"nested": 5})));

        assert_eq!(prop.value(), &Some(json!({"nested": 5})));
    }
}
