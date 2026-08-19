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

use crate::model::app_raw::{ManifestSecretKeyScope, ToolBinding};
use crate::model::cascade::layer::Layer;
use crate::model::cascade::property::Property;
use crate::model::cascade::property::map::MapMergeMode;
use indexmap::IndexMap;
use serde::Serialize;

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolBindingState {
    pub version: Option<String>,
    pub parameters: IndexMap<String, serde_json::Value>,
    pub account: Option<String>,
    pub secret_keys_readable: Vec<ManifestSecretKeyScope>,
    pub secret_keys_revealable: Vec<ManifestSecretKeyScope>,
}

impl ToolBindingState {
    pub(crate) fn apply(&mut self, binding: ToolBinding) {
        if let Some(version) = binding.version {
            self.version = Some(version);
        }
        if let Some(account) = binding.account {
            self.account = Some(account);
        }

        let parameters = binding.parameters.unwrap_or_default();
        match binding.parameters_merge_mode.unwrap_or_default() {
            MapMergeMode::Upsert => {
                self.parameters.extend(parameters);
            }
            MapMergeMode::Replace => self.parameters = parameters,
            MapMergeMode::Remove => {
                for key in parameters.keys() {
                    self.parameters.shift_remove(key);
                }
            }
        }

        if let Some(scope) = binding.secret_keys_readable {
            self.secret_keys_readable.push(scope);
        }
        if let Some(scope) = binding.secret_keys_revealable {
            self.secret_keys_revealable.push(scope);
        }
    }

    pub(crate) fn from_binding(binding: ToolBinding) -> Self {
        let mut result = Self::default();
        result.apply(binding);
        result
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ToolBindingsPropertyTraceElem<L: Layer> {
    #[serde(rename_all = "camelCase")]
    Upsert {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        bindings: IndexMap<String, ToolBinding>,
    },
    #[serde(rename_all = "camelCase")]
    Replace {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        bindings: IndexMap<String, ToolBinding>,
    },
    #[serde(rename_all = "camelCase")]
    Remove {
        id: L::Id,
        #[serde(skip_serializing_if = "Option::is_none")]
        selection: Option<L::AppliedSelection>,
        removed_names: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolBindingsProperty<L: Layer> {
    value: IndexMap<String, ToolBindingState>,
    trace: Vec<ToolBindingsPropertyTraceElem<L>>,
}

impl<L: Layer> Default for ToolBindingsProperty<L> {
    fn default() -> Self {
        Self {
            value: IndexMap::new(),
            trace: Vec::new(),
        }
    }
}

impl<L: Layer> Property<L> for ToolBindingsProperty<L> {
    type Value = IndexMap<String, ToolBindingState>;
    type PropertyLayer = (MapMergeMode, IndexMap<String, ToolBinding>);
    type TraceElem = ToolBindingsPropertyTraceElem<L>;

    fn value(&self) -> &Self::Value {
        &self.value
    }

    fn trace(&self) -> &[Self::TraceElem] {
        &self.trace
    }

    fn apply_layer(
        &mut self,
        id: &L::Id,
        selection: Option<&L::AppliedSelection>,
        (mode, bindings): Self::PropertyLayer,
    ) {
        match mode {
            MapMergeMode::Upsert => {
                for (name, binding) in bindings.clone() {
                    self.value.entry(name).or_default().apply(binding);
                }
                self.trace.push(ToolBindingsPropertyTraceElem::Upsert {
                    id: id.clone(),
                    selection: selection.cloned(),
                    bindings,
                });
            }
            MapMergeMode::Replace => {
                self.value = bindings
                    .clone()
                    .into_iter()
                    .map(|(name, binding)| (name, ToolBindingState::from_binding(binding)))
                    .collect();
                self.trace.push(ToolBindingsPropertyTraceElem::Replace {
                    id: id.clone(),
                    selection: selection.cloned(),
                    bindings,
                });
            }
            MapMergeMode::Remove => {
                let mut removed_names = Vec::new();
                for name in bindings.keys() {
                    if self.value.shift_remove(name).is_some() {
                        removed_names.push(name.clone());
                    }
                }
                self.trace.push(ToolBindingsPropertyTraceElem::Remove {
                    id: id.clone(),
                    selection: selection.cloned(),
                    removed_names,
                });
            }
        }
    }

    fn compact_trace(&mut self) {
        self.trace.retain(|element| match element {
            ToolBindingsPropertyTraceElem::Upsert { bindings, .. }
            | ToolBindingsPropertyTraceElem::Replace { bindings, .. } => !bindings.is_empty(),
            ToolBindingsPropertyTraceElem::Remove { removed_names, .. } => {
                !removed_names.is_empty()
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::ToolBindingsProperty;
    use crate::model::app_raw::ToolBinding;
    use crate::model::cascade::property::Property;
    use crate::model::cascade::property::map::MapMergeMode;
    use crate::model::cascade::property::test_support::TestLayer;
    use indexmap::IndexMap;
    use serde_json::json;
    use test_r::test;

    #[test]
    fn upsert_merges_binding_fields_and_parameters() {
        let mut property = ToolBindingsProperty::<TestLayer>::default();
        let id = "agent".to_string();
        property.apply_layer(
            &id,
            None,
            (
                MapMergeMode::Upsert,
                IndexMap::from_iter([(
                    "grep".to_string(),
                    ToolBinding {
                        version: Some("1.0.0".to_string()),
                        parameters: Some(IndexMap::from_iter([("root".to_string(), json!("/"))])),
                        ..ToolBinding::default()
                    },
                )]),
            ),
        );
        property.apply_layer(
            &id,
            None,
            (
                MapMergeMode::Upsert,
                IndexMap::from_iter([(
                    "grep".to_string(),
                    ToolBinding {
                        parameters: Some(IndexMap::from_iter([("depth".to_string(), json!(2))])),
                        ..ToolBinding::default()
                    },
                )]),
            ),
        );

        let grep = &property.value()["grep"];
        assert_eq!(grep.version.as_deref(), Some("1.0.0"));
        assert_eq!(grep.parameters["root"], json!("/"));
        assert_eq!(grep.parameters["depth"], json!(2));
    }

    #[test]
    fn parameter_replace_and_remove_apply_within_agent_layers() {
        let mut property = ToolBindingsProperty::<TestLayer>::default();
        let id = "agent".to_string();
        property.apply_layer(
            &id,
            None,
            (
                MapMergeMode::Upsert,
                IndexMap::from_iter([(
                    "grep".to_string(),
                    ToolBinding {
                        parameters: Some(IndexMap::from_iter([
                            ("root".to_string(), json!("/workspace")),
                            ("depth".to_string(), json!(2)),
                        ])),
                        ..ToolBinding::default()
                    },
                )]),
            ),
        );
        property.apply_layer(
            &id,
            None,
            (
                MapMergeMode::Upsert,
                IndexMap::from_iter([(
                    "grep".to_string(),
                    ToolBinding {
                        parameters_merge_mode: Some(MapMergeMode::Replace),
                        parameters: Some(IndexMap::from_iter([(
                            "root".to_string(),
                            json!("/src"),
                        )])),
                        ..ToolBinding::default()
                    },
                )]),
            ),
        );
        property.apply_layer(
            &id,
            None,
            (
                MapMergeMode::Upsert,
                IndexMap::from_iter([(
                    "grep".to_string(),
                    ToolBinding {
                        parameters_merge_mode: Some(MapMergeMode::Remove),
                        parameters: Some(IndexMap::from_iter([(
                            "root".to_string(),
                            serde_json::Value::Null,
                        )])),
                        ..ToolBinding::default()
                    },
                )]),
            ),
        );

        assert!(property.value()["grep"].parameters.is_empty());
    }

    #[test]
    fn tool_map_replace_and_remove_only_affect_the_independent_map() {
        let mut property = ToolBindingsProperty::<TestLayer>::default();
        let id = "agent".to_string();
        property.apply_layer(
            &id,
            None,
            (
                MapMergeMode::Upsert,
                IndexMap::from_iter([
                    ("grep".to_string(), ToolBinding::default()),
                    ("git".to_string(), ToolBinding::default()),
                ]),
            ),
        );
        property.apply_layer(
            &id,
            None,
            (
                MapMergeMode::Replace,
                IndexMap::from_iter([("search".to_string(), ToolBinding::default())]),
            ),
        );
        property.apply_layer(
            &id,
            None,
            (
                MapMergeMode::Remove,
                IndexMap::from_iter([("search".to_string(), ToolBinding::default())]),
            ),
        );

        assert!(property.value().is_empty());
    }
}
