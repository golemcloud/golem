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

#[cfg(test)]
mod conformance;
pub mod resolver;

use crate::model::cli_output::StructuredOutput;
use crate::model::component::mask_json_leaf_values;
use crate::model::masking::{Masked, MaskingConfig};
use crate::model::text_format::{TextOutput, logln};
use anyhow::Context;
use golem_common::model::agent::AgentTypeName;
use golem_common::model::component::{ComponentName, ConfigBindingDto};
use golem_common::model::component_metadata::{ConfigBindingOrigin, ConfigOriginLocation};
use golem_common::model::json::NormalizedJsonValue;
use golem_common::schema::SchemaGraph;
use golem_common::schema::config::ConfigOrigin;
use serde::Serialize;
use serde::Serializer;
use std::fmt::Display;
use std::path::{Path, PathBuf};

pub(crate) const CONFIG_FIELD: &str = "config";
pub(crate) const PRESETS_FIELD: &str = "presets";

pub(crate) fn manifest_property_origin(
    file: &Path,
    owner_path: &[String],
    preset: Option<&str>,
    property: &str,
    layer: &impl Display,
) -> ConfigOrigin {
    let mut path = owner_path.to_vec();
    if let Some(preset) = preset {
        path.extend([PRESETS_FIELD.into(), preset.into()]);
    }
    path.push(property.into());
    ConfigOrigin {
        file: file.to_path_buf(),
        path,
        layer: layer.to_string(),
        package: None,
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticConfigSource {
    pub path: Vec<String>,
    pub origin: ConfigOrigin,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StaticConfigFieldView {
    pub component: ComponentName,
    pub agent_type: AgentTypeName,
    pub path: Vec<String>,
    pub schema: SchemaGraph,
    pub fill: NormalizedJsonValue,
    pub sources: Vec<StaticConfigSource>,
    pub catalog_origin: Option<ConfigOrigin>,
}

impl StaticConfigFieldView {
    pub fn config_binding(
        &self,
        app_root: &Path,
        component_wasm: &Path,
    ) -> anyhow::Result<ConfigBindingDto> {
        let origins = self
            .sources
            .iter()
            .map(|source| {
                let location = if let Some(package) = &source.origin.package {
                    ConfigOriginLocation::Package(package.clone())
                } else if crate::fs::path_eq_normalized(&source.origin.file, component_wasm) {
                    ConfigOriginLocation::ComponentMetadata
                } else {
                    let file = crate::fs::absolute_lexical_path_from_base_dir(
                        &source.origin.file,
                        app_root,
                    );
                    let common = app_root
                        .ancestors()
                        .find(|root| file.starts_with(root))
                        .context(
                            "Config origin and application root must share a filesystem root",
                        )?;
                    let relative = app_root
                        .strip_prefix(common)?
                        .components()
                        .map(|_| "..")
                        .collect::<PathBuf>()
                        .join(file.strip_prefix(common)?);
                    ConfigOriginLocation::Manifest {
                        file: crate::fs::path_to_unix_str(&relative)?,
                    }
                };
                Ok(ConfigBindingOrigin {
                    value_path: source
                        .path
                        .strip_prefix(self.path.as_slice())
                        .expect("resolved config sources belong to their binding")
                        .to_vec(),
                    location,
                    source_path: source.origin.path.clone(),
                    layer: source.origin.layer.clone(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(ConfigBindingDto {
            path: self.path.clone(),
            value: self.fill.clone(),
            origins: Some(origins),
        })
    }
}

impl TextOutput for StaticConfigFieldView {
    fn log(&self) {
        logln(format!(
            "{}.{} = {}",
            self.agent_type,
            self.path.join("."),
            self.fill.0
        ));
        logln(format!(
            "  Type: {}",
            golem_common::schema::render::cli_text::type_to_cli_text(
                &self.schema,
                &self.schema.root
            )
        ));
        for source in &self.sources {
            logln(format!(
                "  {}: {} ({}:{})",
                source.path.join("."),
                source.origin.layer,
                source.origin.file.display(),
                source.origin.path.join(".")
            ));
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigListView {
    pub fields: Vec<StaticConfigFieldView>,
}

impl TextOutput for ConfigListView {
    fn log(&self) {
        for field in &self.fields {
            field.log();
        }
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        self.masked(config)?.log();
        Ok(())
    }
}

impl StructuredOutput for ConfigListView {
    const KIND: &'static str = "config.list";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl Masked for ConfigListView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        if !config.show_secrets {
            for field in &mut self.fields {
                mask_json_leaf_values(&mut field.fill.0);
            }
        }
        Ok(self)
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfigGetView {
    pub field: StaticConfigFieldView,
}

impl TextOutput for ConfigGetView {
    fn log(&self) {
        self.field.log();
    }

    fn log_masked(self, config: MaskingConfig) -> anyhow::Result<()> {
        self.masked(config)?.log();
        Ok(())
    }
}

impl StructuredOutput for ConfigGetView {
    const KIND: &'static str = "config.get";

    fn serialize_masked<S>(self, serializer: S, config: MaskingConfig) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.masked(config)
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}

impl Masked for ConfigGetView {
    fn masked(mut self, config: MaskingConfig) -> anyhow::Result<Self> {
        if !config.show_secrets {
            mask_json_leaf_values(&mut self.field.fill.0);
        }
        Ok(self)
    }
}

pub fn value_at_path<'a>(
    root: &'a serde_json::Value,
    path: &[String],
) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in path {
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            _ => return None,
        };
    }
    Some(current)
}

pub fn collect_leaf_paths(value: &serde_json::Value) -> Vec<Vec<String>> {
    fn collect(value: &serde_json::Value, prefix: &mut Vec<String>, result: &mut Vec<Vec<String>>) {
        match value {
            serde_json::Value::Object(map) => {
                if map.is_empty() {
                    result.push(prefix.clone());
                }
                for (key, nested) in map {
                    prefix.push(key.clone());
                    collect(nested, prefix, result);
                    prefix.pop();
                }
            }
            _ => result.push(prefix.clone()),
        }
    }

    let mut result = Vec::new();
    collect(value, &mut vec![], &mut result);
    result
}

pub fn collect_unused_leaf_paths<F>(
    value: &serde_json::Value,
    mut is_consumed: F,
) -> Vec<Vec<String>>
where
    F: FnMut(&[String]) -> bool,
{
    let mut unused = collect_leaf_paths(value)
        .into_iter()
        .filter(|path| !path.is_empty())
        .filter(|path| !is_consumed(path))
        .collect::<Vec<_>>();

    unused.sort();
    unused.dedup();
    unused
}

#[cfg(test)]
mod test {
    use super::{collect_leaf_paths, collect_unused_leaf_paths, value_at_path};
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use test_r::test;

    #[test]
    fn value_at_path_returns_nested_values() {
        let input = json!({
            "a": {
                "b": {
                    "c": 42
                }
            }
        });

        assert_eq!(
            value_at_path(&input, &["a".to_string(), "b".to_string(), "c".to_string()]),
            Some(&json!(42))
        );
    }

    #[test]
    fn value_at_path_returns_none_for_non_objects() {
        let input = json!({ "a": 1 });

        assert_eq!(
            value_at_path(&input, &["a".to_string(), "b".to_string()]),
            None
        );
    }

    #[test]
    fn collect_leaf_paths_collects_all_terminal_paths() {
        let input = json!({
            "a": { "x": 1, "y": true },
            "b": [1,2],
            "c": "v"
        });

        let mut result = collect_leaf_paths(&input)
            .into_iter()
            .map(|path| path.join("."))
            .collect::<Vec<_>>();
        result.sort();

        assert_eq!(result, vec!["a.x", "a.y", "b", "c"]);
    }

    #[test]
    fn collect_leaf_paths_treats_empty_object_as_terminal() {
        let input = json!({
            "db": {},
            "nested": {
                "conn": {}
            }
        });

        let mut result = collect_leaf_paths(&input)
            .into_iter()
            .map(|path| path.join("."))
            .collect::<Vec<_>>();
        result.sort();

        assert_eq!(result, vec!["db", "nested.conn"]);
    }

    #[test]
    fn collect_unused_leaf_paths_supports_exact_match() {
        let input = json!({
            "a": { "x": 1, "y": true },
            "b": [1,2],
            "c": "v"
        });

        let declared = [
            vec!["a".to_string(), "x".to_string()],
            vec!["c".to_string()],
        ];

        let mut result =
            collect_unused_leaf_paths(&input, |path| declared.contains(&path.to_vec()))
                .into_iter()
                .map(|path| path.join("."))
                .collect::<Vec<_>>();
        result.sort();

        assert_eq!(result, vec!["a.y", "b"]);
    }

    #[test]
    fn collect_unused_leaf_paths_supports_prefix_match() {
        let input = json!({
            "db": {
                "user": "u",
                "password": "p"
            },
            "x": 1
        });

        let consumed = [vec!["db".to_string()]];

        let result = collect_unused_leaf_paths(&input, |path| {
            consumed.iter().any(|prefix| path.starts_with(prefix))
        })
        .into_iter()
        .map(|path| path.join("."))
        .collect::<Vec<_>>();

        assert_eq!(result, vec!["x"]);
    }

    #[test]
    fn collect_unused_leaf_paths_filters_root_empty_path() {
        let input = json!({});

        let result = collect_unused_leaf_paths(&input, |_| false);

        assert!(result.is_empty());
    }
}
