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

use anyhow::anyhow;
use toml_edit::{value, Array, DocumentMut, Item, Table};

pub fn merge_dependencies(
    source: &str,
    dependencies: &[(String, String)],
    dev_dependencies: &[(String, String)],
) -> anyhow::Result<String> {
    let mut doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;

    merge_table(&mut doc, "dependencies", dependencies)?;
    merge_table(&mut doc, "dev-dependencies", dev_dependencies)?;

    Ok(doc.to_string())
}

pub fn check_required_deps(source: &str, required: &[&str]) -> anyhow::Result<Vec<String>> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    let mut missing = Vec::new();
    for dep in required {
        if !has_dependency(&doc, dep) {
            missing.push((*dep).to_string());
        }
    }
    Ok(missing)
}

fn merge_table(
    doc: &mut DocumentMut,
    table_name: &str,
    deps: &[(String, String)],
) -> anyhow::Result<()> {
    if deps.is_empty() {
        return Ok(());
    }
    if !doc.as_table().contains_key(table_name) {
        doc[table_name] = Item::Table(Table::default());
    }
    for (name, version) in deps {
        let spec = VersionSpec::parse(version)?;
        if spec.is_workspace() {
            continue;
        }
        let entry = &mut doc[table_name][name];
        if entry.is_none() {
            *entry = spec.to_item();
            continue;
        }
        if entry
            .as_table_like()
            .and_then(|table| table.get("workspace"))
            .and_then(|item| item.as_value())
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
        {
            continue;
        }
        if spec.features.is_empty() {
            *entry = spec.to_item();
            continue;
        }
        if entry.is_str() {
            let mut table = Table::default();
            table["version"] = value(spec.version.as_str());
            table["features"] = value(features_to_array(merge_features(Vec::new(), &spec.features)));
            *entry = Item::Table(table);
            continue;
        }
        if let Some(table) = entry.as_table_like_mut() {
            if let Some(version_item) = table.get_mut("version") {
                *version_item = value(spec.version.as_str());
            } else {
                table.insert("version", value(spec.version.as_str()));
            }
            let existing = table
                .get("features")
                .and_then(|item| item.as_array())
                .map(|array| {
                    array
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(Vec::new);
            table.insert(
                "features",
                value(features_to_array(merge_features(existing, &spec.features))),
            );
            continue;
        }
        *entry = spec.to_item();
    }
    Ok(())
}

fn has_dependency(doc: &DocumentMut, name: &str) -> bool {
    doc.get("dependencies")
        .and_then(|table| table.get(name))
        .is_some()
        || doc
            .get("dev-dependencies")
            .and_then(|table| table.get(name))
            .is_some()
        || doc
            .get("build-dependencies")
            .and_then(|table| table.get(name))
            .is_some()
        || doc
            .get("workspace")
            .and_then(|table| table.get("dependencies"))
            .and_then(|table| table.get(name))
            .is_some()
}

#[derive(Debug)]
struct VersionSpec {
    version: String,
    features: Vec<String>,
}

impl VersionSpec {
    fn parse(raw: &str) -> anyhow::Result<Self> {
        let trimmed = raw.trim();
        if trimmed.eq_ignore_ascii_case("workspace") {
            return Ok(Self {
                version: "workspace".to_string(),
                features: Vec::new(),
            });
        }
        let mut parts = trimmed.splitn(2, '+');
        let version = parts
            .next()
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .ok_or_else(|| anyhow!("Invalid version spec"))?;
        let features = parts
            .next()
            .map(|rest| {
                rest.split(',')
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok(Self {
            version: version.to_string(),
            features,
        })
    }

    fn to_item(&self) -> Item {
        if self.features.is_empty() {
            value(self.version.as_str())
        } else {
            let mut table = Table::default();
            table["version"] = value(self.version.as_str());
            table["features"] = value(features_to_array(self.features.clone()));
            Item::Table(table)
        }
    }

    fn is_workspace(&self) -> bool {
        self.version == "workspace"
    }
}

fn merge_features(existing: Vec<String>, new_features: &[String]) -> Vec<String> {
    let mut merged = existing;
    for feature in new_features {
        if !merged.iter().any(|item| item == feature) {
            merged.push(feature.clone());
        }
    }
    merged
}

fn features_to_array(features: Vec<String>) -> Array {
    let mut array = Array::default();
    for feature in features {
        array.push(feature);
    }
    array
}
