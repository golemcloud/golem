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

use anyhow::anyhow;
use std::collections::BTreeMap;
use toml_edit::{value, Array, DocumentMut, Item, Table};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencySpec {
    Version(String),
    Path(String),
    Unsupported(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyTable {
    Dependencies,
    DevDependencies,
    BuildDependencies,
}

impl DependencyTable {
    fn as_str(&self) -> &'static str {
        match self {
            DependencyTable::Dependencies => "dependencies",
            DependencyTable::DevDependencies => "dev-dependencies",
            DependencyTable::BuildDependencies => "build-dependencies",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DependencyLocation {
    Package(DependencyTable),
    WorkspaceDependencies,
}

pub fn merge_documents(base_source: &str, update_source: &str) -> anyhow::Result<String> {
    let mut base: DocumentMut = base_source.parse().map_err(|e| anyhow!("{e}"))?;
    let update: DocumentMut = update_source.parse().map_err(|e| anyhow!("{e}"))?;

    merge_tables(base.as_table_mut(), update.as_table());

    Ok(base.to_string())
}

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

pub fn collect_versions(
    source: &str,
    names: &[&str],
) -> anyhow::Result<BTreeMap<String, Option<String>>> {
    let specs = collect_dependency_specs(source, names)?;
    Ok(specs
        .into_iter()
        .map(|(name, spec)| {
            let version = spec.and_then(|spec| match spec {
                DependencySpec::Version(version) => Some(version),
                DependencySpec::Path(_) | DependencySpec::Unsupported(_) => None,
            });
            (name, version)
        })
        .collect())
}

pub fn collect_dependency_specs(
    source: &str,
    names: &[&str],
) -> anyhow::Result<BTreeMap<String, Option<DependencySpec>>> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    let mut result = BTreeMap::new();

    for name in names {
        result.insert((*name).to_string(), collect_dependency_spec(&doc, name));
    }

    Ok(result)
}

pub fn resolve_dependency_location(
    source: &str,
    name: &str,
) -> anyhow::Result<Option<DependencyLocation>> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;

    for table in [
        DependencyTable::Dependencies,
        DependencyTable::DevDependencies,
        DependencyTable::BuildDependencies,
    ] {
        if let Some(item) = doc
            .get(table.as_str())
            .and_then(|table_item| table_item.get(name))
        {
            let is_workspace_ref = item
                .as_table_like()
                .and_then(|tbl| tbl.get("workspace"))
                .and_then(|workspace| workspace.as_value())
                .and_then(|workspace| workspace.as_bool())
                .unwrap_or(false);

            if is_workspace_ref {
                return Ok(Some(DependencyLocation::WorkspaceDependencies));
            }

            return Ok(Some(DependencyLocation::Package(table)));
        }
    }

    if doc
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|deps| deps.get(name))
        .is_some()
    {
        return Ok(Some(DependencyLocation::WorkspaceDependencies));
    }

    Ok(None)
}

pub fn upsert_dependency_in_package(
    source: &str,
    table: DependencyTable,
    name: &str,
    spec: &DependencySpec,
) -> anyhow::Result<String> {
    let mut doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    let table_name = table.as_str();

    if !doc.as_table().contains_key(table_name) {
        doc[table_name] = Item::Table(Table::default());
    }

    doc[table_name][name] = dependency_spec_to_item(spec)?;
    Ok(doc.to_string())
}

pub fn upsert_dependency_in_workspace_dependencies(
    source: &str,
    name: &str,
    spec: &DependencySpec,
) -> anyhow::Result<String> {
    let mut doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;

    if !doc.as_table().contains_key("workspace") {
        doc["workspace"] = Item::Table(Table::default());
    }
    if doc["workspace"]
        .as_table_like()
        .and_then(|workspace| workspace.get("dependencies"))
        .is_none()
    {
        doc["workspace"]["dependencies"] = Item::Table(Table::default());
    }

    doc["workspace"]["dependencies"][name] = dependency_spec_to_item(spec)?;
    Ok(doc.to_string())
}

pub fn upsert_dependency_auto(
    source: &str,
    name: &str,
    spec: &DependencySpec,
    preferred_table: DependencyTable,
) -> anyhow::Result<String> {
    match resolve_dependency_location(source, name)? {
        Some(DependencyLocation::WorkspaceDependencies) => {
            upsert_dependency_in_workspace_dependencies(source, name, spec)
        }
        Some(DependencyLocation::Package(table)) => {
            upsert_dependency_in_package(source, table, name, spec)
        }
        None => upsert_dependency_in_package(source, preferred_table, name, spec),
    }
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
            table["features"] = value(features_to_array(merge_features(
                Vec::new(),
                &spec.features,
            )));
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

fn collect_dependency_spec(doc: &DocumentMut, name: &str) -> Option<DependencySpec> {
    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(item) = doc.get(table_name).and_then(|table| table.get(name)) {
            if is_workspace_ref(item) {
                return workspace_dependency_item(doc, name).and_then(dependency_spec);
            }
            return dependency_spec(item);
        }
    }

    workspace_dependency_item(doc, name).and_then(dependency_spec)
}

fn dependency_spec(item: &Item) -> Option<DependencySpec> {
    if item.is_str() {
        return item
            .as_str()
            .map(|s| DependencySpec::Version(s.to_string()));
    }

    let table = item.as_table_like()?;

    if let Some(path) = table
        .get("path")
        .and_then(|item| item.as_value())
        .and_then(|value| value.as_str())
    {
        return Some(DependencySpec::Path(path.to_string()));
    }

    table
        .get("version")
        .and_then(|item| item.as_value())
        .and_then(|value| value.as_str())
        .map(|value| DependencySpec::Version(value.to_string()))
        .or_else(|| Some(DependencySpec::Unsupported(item.to_string())))
}

fn workspace_dependency_item<'a>(doc: &'a DocumentMut, name: &str) -> Option<&'a Item> {
    doc.get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|deps| deps.get(name))
}

fn is_workspace_ref(item: &Item) -> bool {
    item.as_table_like()
        .and_then(|table| table.get("workspace"))
        .and_then(|item| item.as_value())
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn dependency_spec_to_item(spec: &DependencySpec) -> anyhow::Result<Item> {
    match spec {
        DependencySpec::Version(version) => Ok(value(version.as_str())),
        DependencySpec::Path(path) => {
            let mut table = Table::default();
            table["path"] = value(path.as_str());
            Ok(Item::Table(table))
        }
        DependencySpec::Unsupported(spec) => {
            Err(anyhow!("Unsupported dependency spec for update: {spec}"))
        }
    }
}

fn merge_tables(base: &mut Table, update: &Table) {
    for (key, update_item) in update {
        if let Some(base_item) = base.get_mut(key) {
            merge_items(base_item, update_item);
        } else {
            base.insert(key, update_item.clone());
        }
    }
}

fn merge_items(base: &mut Item, update: &Item) {
    if let (Some(base_table), Some(update_table)) =
        (base.as_table_like_mut(), update.as_table_like())
    {
        for (key, update_item) in update_table.iter() {
            if key == "features" {
                if let (Some(base_features), Some(update_features)) = (
                    base_table.get("features").and_then(|it| it.as_array()),
                    update_item.as_array(),
                ) {
                    let existing = base_features
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    let update = update_features
                        .iter()
                        .filter_map(|v| v.as_str())
                        .map(str::to_string)
                        .collect::<Vec<_>>();
                    base_table.insert(
                        "features",
                        value(features_to_array(merge_features(existing, &update))),
                    );
                    continue;
                }
            }

            if let Some(base_item) = base_table.get_mut(key) {
                merge_items(base_item, update_item);
            } else {
                base_table.insert(key, update_item.clone());
            }
        }
        return;
    }

    *base = update.clone();
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
    let mut merged = Vec::new();
    for feature in existing.into_iter().chain(new_features.iter().cloned()) {
        if !merged.iter().any(|item| item == &feature) {
            merged.push(feature);
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
