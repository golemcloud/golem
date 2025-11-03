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

pub mod error;
pub mod layer;
pub mod property;
pub mod selector;
pub mod store;

use serde::Serialize;
use std::fmt::Debug;
use std::hash::Hash;

#[cfg(test)]
mod test {
    mod example_component_properties {
        use crate::model::cascade::property::map::{MapMergeMode, MapProperty};
        use crate::model::cascade::property::optional::OptionalProperty;
        use crate::model::cascade::property::Property;
        use crate::model::cascade::test::example_component_properties::ComponentLayerId::{
            BaseDefinition, BaseTemplate, DefinitionPresets, TemplatePresets,
        };

        use crate::model::cascade::layer::Layer;
        use crate::model::cascade::store::Store;
        use crate::model::deploy_diff::ToYamlValueWithoutNulls;
        use serde_derive::Serialize;
        use std::collections::HashMap;
        use test_r::test;

        #[derive(Debug, Eq, Hash, PartialEq, Clone, Serialize)]
        #[serde(rename_all = "kebab-case")]
        enum ComponentLayerId {
            BaseTemplate(String),
            TemplatePresets(String),
            BaseDefinition(String),
            DefinitionPresets(String),
        }

        impl ComponentLayerId {
            pub fn is_template(&self) -> bool {
                match self {
                    BaseTemplate(_) => true,
                    TemplatePresets(_) => true,
                    BaseDefinition(_) => false,
                    DefinitionPresets(_) => false,
                }
            }
        }

        #[derive(Debug, Clone, Hash, PartialEq, Eq)]
        struct ComponentSelector {
            pub selected_presets: Vec<String>,
            pub template_env: Vec<(String, String)>,
        }

        #[derive(Debug, Default, Clone, Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ComponentProperties {
            pub component_type: OptionalProperty<ComponentLayer, String>,
            pub build: OptionalProperty<ComponentLayer, String>,
            pub env: MapProperty<ComponentLayer, String, String>,
            pub env_merge: Option<MapMergeMode>,
        }

        #[derive(Debug, Clone, Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ComponentLayer {
            id: ComponentLayerId,
            parents: Vec<ComponentLayerId>,
            base_properties: Option<ComponentProperties>,
            preset_properties: HashMap<String, ComponentProperties>,
            default_preset: Option<String>,
        }

        #[derive(Debug, Clone, Serialize)]
        #[serde(rename_all = "camelCase")]
        struct ComponentAppliedSelection {
            pub preset: Option<String>,
            pub used_template_env: Option<Vec<(String, String)>>,
        }

        impl ComponentAppliedSelection {
            pub fn is_empty(&self) -> bool {
                self.preset.is_none() && self.used_template_env.is_none()
            }
        }

        impl Layer for ComponentLayer {
            type Id = ComponentLayerId;
            type Value = ComponentProperties;
            type Selector = ComponentSelector;
            type AppliedSelection = ComponentAppliedSelection;
            type ApplyError = String;

            fn id(&self) -> &Self::Id {
                &self.id
            }

            fn parent_layers(&self) -> &[Self::Id] {
                self.parents.as_slice()
            }

            fn apply_onto_parent(
                &self,
                selector: &Self::Selector,
                value: &mut Self::Value,
            ) -> Result<(), String> {
                let Some((properties, preset)) = (match &self.id {
                    BaseTemplate(_) | BaseDefinition(_) => self
                        .base_properties
                        .as_ref()
                        .map(|properties| (properties, None)),
                    TemplatePresets(_) | DefinitionPresets(_) => selector
                        .selected_presets
                        .iter()
                        .find_map(|preset| {
                            self.preset_properties
                                .get(preset)
                                .map(|properties| (properties, Some(preset)))
                        })
                        .or_else(|| {
                            self.default_preset.as_ref().and_then(|preset| {
                                self.preset_properties
                                    .get(preset)
                                    .map(|properties| (properties, Some(preset)))
                            })
                        }),
                }) else {
                    return Ok(());
                };

                let id = self.id();

                let used_template_env = {
                    if id.is_template() {
                        Some(&selector.template_env)
                    } else {
                        None
                    }
                };

                let templated_selection = ComponentAppliedSelection {
                    preset: preset.map(|preset| preset.clone()),
                    used_template_env: used_template_env.cloned(),
                };
                let templated_selection =
                    (!templated_selection.is_empty()).then_some(&templated_selection);

                let simple_selection = ComponentAppliedSelection {
                    preset: preset.map(|preset| preset.clone()),
                    used_template_env: None,
                };
                let simple_selection = (!simple_selection.is_empty()).then_some(&simple_selection);

                value.component_type.apply_layer(
                    id,
                    simple_selection,
                    properties.component_type.value().clone(),
                );
                value.build.apply_layer(
                    id,
                    templated_selection,
                    properties
                        .build
                        .value()
                        .clone()
                        .map(|build| match used_template_env {
                            Some(used_template_env) => {
                                format!("{}: {:?}", build, used_template_env)
                            }
                            None => build,
                        }),
                );
                value.env.apply_layer(
                    id,
                    simple_selection,
                    (
                        properties.env_merge.unwrap_or_default(),
                        properties.env.value().clone(),
                    ),
                );

                Ok(())
            }
        }

        #[test]
        fn example() {
            let store = {
                let mut store = Store::<ComponentLayer>::new();

                {
                    store
                        .add_layer(ComponentLayer {
                            id: BaseTemplate("rust".to_string()),
                            parents: vec![],
                            base_properties: Some(ComponentProperties {
                                component_type: OptionalProperty::none(),
                                build: OptionalProperty::none(),
                                env: Default::default(),
                                env_merge: None,
                            }),
                            preset_properties: Default::default(),
                            default_preset: None,
                        })
                        .unwrap();

                    store
                        .add_layer(ComponentLayer {
                            id: TemplatePresets("rust".to_string()),
                            parents: vec![BaseTemplate("rust".to_string())],
                            base_properties: None,
                            preset_properties: HashMap::from([
                                (
                                    "debug".to_string(),
                                    ComponentProperties {
                                        component_type: "durable".to_string().into(),
                                        build: "build-debug".to_string().into(),
                                        env: HashMap::from([("X".to_string(), "x".to_string())])
                                            .into(),
                                        env_merge: None,
                                    },
                                ),
                                (
                                    "release".to_string(),
                                    ComponentProperties {
                                        component_type: "ephemeral".to_string().into(),
                                        build: "build-release".to_string().into(),
                                        env: Default::default(),
                                        env_merge: None,
                                    },
                                ),
                            ]),
                            default_preset: Some("debug".to_string()),
                        })
                        .unwrap();
                }

                {
                    store
                        .add_layer(ComponentLayer {
                            id: BaseTemplate("common-env".to_string()),
                            parents: vec![],
                            base_properties: Some(ComponentProperties {
                                component_type: Default::default(),
                                build: OptionalProperty::none(),
                                env: HashMap::from([(
                                    "COMMON_ENV".to_string(),
                                    "common_env".to_string(),
                                )])
                                .into(),
                                env_merge: None,
                            }),
                            preset_properties: Default::default(),
                            default_preset: None,
                        })
                        .unwrap();
                }

                {
                    store
                        .add_layer(ComponentLayer {
                            id: BaseDefinition("app:comp-a".to_string()),
                            parents: vec![TemplatePresets("rust".to_string())],
                            base_properties: None,
                            preset_properties: Default::default(),
                            default_preset: None,
                        })
                        .unwrap();

                    store
                        .add_layer(ComponentLayer {
                            id: BaseDefinition("app:comp-b".to_string()),
                            parents: vec![
                                TemplatePresets("rust".to_string()),
                                BaseTemplate("common-env".to_string()),
                            ],
                            base_properties: None,
                            preset_properties: Default::default(),
                            default_preset: None,
                        })
                        .unwrap();
                }

                store
            };

            let comp = store
                .value(
                    &BaseDefinition("app:comp-b".to_string()),
                    &ComponentSelector {
                        selected_presets: vec!["release".to_string()],
                        template_env: vec![("componentName".to_string(), "appCompB".to_string())],
                    },
                )
                .unwrap();

            println!(
                "{}",
                serde_yaml::to_string(
                    &serde_yaml::to_value(&comp.clone())
                        .unwrap()
                        .to_yaml_value_without_nulls()
                        .unwrap()
                )
                .unwrap()
            )
        }
    }
}
