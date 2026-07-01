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
use std::collections::{BTreeMap, BTreeSet};
use toml_edit::{Array, DocumentMut, InlineTable, Item, Table, TableLike, Value, value};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DependencySpec {
    Version {
        version: String,
        features: Vec<String>,
    },
    Path {
        path: String,
        features: Vec<String>,
    },
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
                DependencySpec::Version { version, .. } => Some(version),
                DependencySpec::Path { .. } | DependencySpec::Unsupported(_) => None,
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

pub fn collect_package_dependency_specs(source: &str) -> anyhow::Result<Vec<DependencySpec>> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    let mut result = Vec::new();

    for_each_package_dependency(&doc, |_, item| {
        if !is_workspace_ref(item)
            && let Some(spec) = dependency_spec(item)
        {
            result.push(spec);
        }
    });

    Ok(result)
}

pub fn collect_workspace_dependency_refs(source: &str) -> anyhow::Result<BTreeSet<String>> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    let mut result = BTreeSet::new();

    for_each_package_dependency(&doc, |name, item| {
        if is_workspace_ref(item) {
            result.insert(name.to_string());
        }
    });

    Ok(result)
}

pub fn collect_workspace_dependency_specs(
    source: &str,
) -> anyhow::Result<BTreeMap<String, DependencySpec>> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    let mut result = BTreeMap::new();

    if let Some(table) = doc
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|deps| deps.as_table_like())
    {
        for (name, item) in table.iter() {
            if let Some(spec) = dependency_spec(item) {
                result.insert(name.to_string(), spec);
            }
        }
    }

    Ok(result)
}

fn for_each_package_dependency(doc: &DocumentMut, mut visit: impl FnMut(&str, &Item)) {
    for_each_package_dependency_in_container(doc.as_table(), &mut visit);

    if let Some(targets) = doc.get("target").and_then(|item| item.as_table_like()) {
        for (_, target_item) in targets.iter() {
            if let Some(target_table) = target_item.as_table_like() {
                for_each_package_dependency_in_container(target_table, &mut visit);
            }
        }
    }
}

fn for_each_package_dependency_in_container(
    container: &dyn TableLike,
    visit: &mut impl FnMut(&str, &Item),
) {
    [
        DependencyTable::Dependencies,
        DependencyTable::BuildDependencies,
    ]
    .iter()
    .for_each(|table| {
        if let Some(dependencies) = container
            .get(table.as_str())
            .and_then(|item| item.as_table_like())
        {
            for (name, item) in dependencies.iter() {
                visit(name, item);
            }
        }
    });
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

pub fn is_workspace_manifest(source: &str) -> anyhow::Result<bool> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    Ok(doc.get("workspace").is_some())
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

/// Where a dependency located by crate identity is declared, including target-specific tables.
///
/// This is distinct from [`DependencyLocation`] because it can also address
/// `[target.<triple>.dependencies]` tables, which the crate-identity matchers scan but the
/// name-based SDK dependency flow does not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MatchedDependencyLocation {
    /// A top-level `[dependencies]` or `[build-dependencies]` table.
    Package(DependencyTable),
    /// A `[target.<triple>.dependencies]` or `[target.<triple>.build-dependencies]` table.
    TargetSpecific {
        target: String,
        table: DependencyTable,
    },
    /// The `[workspace.dependencies]` table.
    Workspace,
}

/// A dependency located by its crate identity: the explicit `package` value if present,
/// otherwise the dependency's TOML map key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchedDependency {
    /// The TOML map key of the dependency (differs from `crate_name` when a `package` alias is used).
    pub key: String,
    /// The crate identity: the explicit `package` value if present, otherwise the map key.
    pub crate_name: String,
    /// Where the dependency is declared.
    pub location: MatchedDependencyLocation,
    /// The current dependency spec.
    pub spec: DependencySpec,
}

/// Finds dependencies in `[dependencies]`, `[build-dependencies]`, and their
/// `[target.<triple>.*]` variants whose crate identity is in `crate_names`. Dev-dependencies and
/// `{ workspace = true }` references are ignored (the latter are defined in
/// `[workspace.dependencies]`).
pub fn find_package_dependencies_by_crate_name(
    source: &str,
    crate_names: &BTreeSet<String>,
) -> anyhow::Result<Vec<MatchedDependency>> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    let mut result = Vec::new();

    collect_matched_package_dependencies(doc.as_table(), crate_names, &mut result, |table| {
        MatchedDependencyLocation::Package(table)
    });

    if let Some(targets) = doc.get("target").and_then(|item| item.as_table_like()) {
        for (target, target_item) in targets.iter() {
            if let Some(target_table) = target_item.as_table_like() {
                collect_matched_package_dependencies(
                    target_table,
                    crate_names,
                    &mut result,
                    |table| MatchedDependencyLocation::TargetSpecific {
                        target: target.to_string(),
                        table,
                    },
                );
            }
        }
    }

    Ok(result)
}

fn collect_matched_package_dependencies(
    container: &dyn TableLike,
    crate_names: &BTreeSet<String>,
    result: &mut Vec<MatchedDependency>,
    make_location: impl Fn(DependencyTable) -> MatchedDependencyLocation,
) {
    for table in [
        DependencyTable::Dependencies,
        DependencyTable::BuildDependencies,
    ] {
        if let Some(dependencies) = container
            .get(table.as_str())
            .and_then(|item| item.as_table_like())
        {
            for (key, item) in dependencies.iter() {
                if is_workspace_ref(item) {
                    continue;
                }
                let crate_name = dependency_package(item).unwrap_or_else(|| key.to_string());
                if crate_names.contains(&crate_name)
                    && let Some(spec) = dependency_spec(item)
                {
                    result.push(MatchedDependency {
                        key: key.to_string(),
                        crate_name,
                        location: make_location(table),
                        spec,
                    });
                }
            }
        }
    }
}

/// Finds dependencies in `[workspace.dependencies]` whose crate identity is in `crate_names`.
pub fn find_workspace_dependencies_by_crate_name(
    source: &str,
    crate_names: &BTreeSet<String>,
) -> anyhow::Result<Vec<MatchedDependency>> {
    let doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;
    let mut result = Vec::new();

    if let Some(table) = doc
        .get("workspace")
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(|deps| deps.as_table_like())
    {
        for (key, item) in table.iter() {
            let crate_name = dependency_package(item).unwrap_or_else(|| key.to_string());
            if crate_names.contains(&crate_name)
                && let Some(spec) = dependency_spec(item)
            {
                result.push(MatchedDependency {
                    key: key.to_string(),
                    crate_name,
                    location: MatchedDependencyLocation::Workspace,
                    spec,
                });
            }
        }
    }

    Ok(result)
}

/// Sets the `path` field of an existing dependency entry in place, preserving other keys such as
/// `package` and `features`. A bare version-string entry is replaced with an inline table that
/// only contains `path`.
pub fn set_dependency_path(
    source: &str,
    location: MatchedDependencyLocation,
    key: &str,
    new_path: &str,
) -> anyhow::Result<String> {
    let mut doc: DocumentMut = source.parse().map_err(|e| anyhow!("{e}"))?;

    let item = match location {
        MatchedDependencyLocation::Package(table) => doc
            .get_mut(table.as_str())
            .and_then(|item| item.as_table_like_mut())
            .and_then(|table| table.get_mut(key)),
        MatchedDependencyLocation::TargetSpecific { target, table } => doc
            .get_mut("target")
            .and_then(|item| item.as_table_like_mut())
            .and_then(|targets| targets.get_mut(&target))
            .and_then(|target_item| target_item.as_table_like_mut())
            .and_then(|target_table| target_table.get_mut(table.as_str()))
            .and_then(|item| item.as_table_like_mut())
            .and_then(|deps| deps.get_mut(key)),
        MatchedDependencyLocation::Workspace => doc
            .get_mut("workspace")
            .and_then(|workspace| workspace.as_table_like_mut())
            .and_then(|workspace| workspace.get_mut("dependencies"))
            .and_then(|deps| deps.as_table_like_mut())
            .and_then(|deps| deps.get_mut(key)),
    };

    let Some(item) = item else {
        return Ok(source.to_string());
    };

    if let Some(table) = item.as_table_like_mut() {
        // Converting to a local path dependency: drop any source or version keys that would
        // conflict with, or over-constrain, a path dependency (e.g. a retained `version` must be
        // satisfied by the local crate's version). Keys such as `package` and `features` are kept.
        for conflicting in ["version", "git", "registry", "branch", "tag", "rev"] {
            table.remove(conflicting);
        }
        table.insert("path", value(new_path));
    } else {
        *item = inline_path_dep(new_path, &[]);
    }

    Ok(doc.to_string())
}

fn dependency_package(item: &Item) -> Option<String> {
    item.as_table_like()
        .and_then(|table| table.get("package"))
        .and_then(|package| package.as_value())
        .and_then(|package| package.as_str())
        .map(str::to_string)
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
        return item.as_str().map(|s| DependencySpec::Version {
            version: s.to_string(),
            features: Vec::new(),
        });
    }

    let table = item.as_table_like()?;
    let features = table
        .get("features")
        .and_then(|item| item.as_array())
        .map(|array| {
            array
                .iter()
                .filter_map(|v| v.as_str())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    // A dependency with an explicit non-registry source (`git`) or a custom `registry` cannot be
    // reduced to a plain version or rewritten to a path without corrupting the spec, so it is
    // treated as unsupported regardless of any accompanying `path` or `version` keys.
    if table.contains_key("git") || table.contains_key("registry") {
        return Some(DependencySpec::Unsupported(item.to_string()));
    }

    if let Some(path) = table
        .get("path")
        .and_then(|item| item.as_value())
        .and_then(|value| value.as_str())
    {
        return Some(DependencySpec::Path {
            path: path.to_string(),
            features,
        });
    }

    table
        .get("version")
        .and_then(|item| item.as_value())
        .and_then(|value| value.as_str())
        .map(|value| DependencySpec::Version {
            version: value.to_string(),
            features,
        })
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
        DependencySpec::Version { version, features } => {
            if features.is_empty() {
                Ok(value(version.as_str()))
            } else {
                Ok(inline_version_dep(version, features))
            }
        }
        DependencySpec::Path { path, features } => Ok(inline_path_dep(path, features)),
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
            if key == "features"
                && let (Some(base_features), Some(update_features)) = (
                    base_table.get("features").and_then(|it| it.as_array()),
                    update_item.as_array(),
                )
            {
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
            inline_version_dep(self.version.as_str(), &self.features)
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

fn inline_version_dep(version: &str, features: &[String]) -> Item {
    let mut entry = InlineTable::new();
    entry.insert("version", Value::from(version));
    if !features.is_empty() {
        entry.insert(
            "features",
            Value::Array(features_to_array(features.to_vec())),
        );
    }
    Item::Value(Value::InlineTable(entry))
}

fn inline_path_dep(path: &str, features: &[String]) -> Item {
    let mut entry = InlineTable::new();
    entry.insert("path", Value::from(path));
    if !features.is_empty() {
        entry.insert(
            "features",
            Value::Array(features_to_array(features.to_vec())),
        );
    }
    Item::Value(Value::InlineTable(entry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn package_metadata_dependencies_are_not_package_dependencies() {
        let source = r#"
            [package]
            name = "example"

            [package.metadata.tool.dependencies]
            guest = { path = "../bridge/foo-agent-guest-client" }
        "#;

        let specs = collect_package_dependency_specs(source).unwrap();

        assert!(specs.is_empty());
    }

    #[test]
    fn target_specific_dependencies_are_collected_without_collapsing_names() {
        let source = r#"
            [package]
            name = "example"

            [dependencies]
            guest = "1"

            [target.wasm32-wasip2.dependencies]
            guest = { path = "../bridge/foo-agent-guest-client" }
        "#;

        let specs = collect_package_dependency_specs(source).unwrap();

        assert_eq!(specs.len(), 2);
        assert!(specs.iter().any(|spec| matches!(
            spec,
            DependencySpec::Path { path, .. } if path == "../bridge/foo-agent-guest-client"
        )));
    }

    #[test]
    fn dev_dependencies_are_not_build_dependencies() {
        let source = r#"
            [package]
            name = "example"

            [dev-dependencies]
            guest = { path = "../bridge/foo-agent-guest-client" }

            [target.wasm32-wasip2.dev-dependencies]
            target-guest = { workspace = true }
        "#;

        let specs = collect_package_dependency_specs(source).unwrap();
        let refs = collect_workspace_dependency_refs(source).unwrap();

        assert!(specs.is_empty());
        assert!(refs.is_empty());
    }

    #[test]
    fn target_specific_workspace_dependency_refs_are_collected() {
        let source = r#"
            [package]
            name = "example"

            [target.wasm32-wasip2.dependencies]
            guest = { workspace = true }

            [workspace.dependencies]
            guest = { path = "../bridge/foo-agent-guest-client" }
        "#;

        let refs = collect_workspace_dependency_refs(source).unwrap();

        assert_eq!(refs, BTreeSet::from(["guest".to_string()]));
    }

    #[test]
    fn find_package_dependencies_matches_by_key_and_package_alias() {
        let source = r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = { path = "../old" }
            aliased = { package = "foo-agent-guest-client", path = "../foo" }
            unrelated = "1"

            [build-dependencies]
            baz-agent-guest-client = "0.0.0"
        "#;

        let crate_names = BTreeSet::from([
            "bar-agent-guest-client".to_string(),
            "foo-agent-guest-client".to_string(),
            "baz-agent-guest-client".to_string(),
        ]);
        let matches = find_package_dependencies_by_crate_name(source, &crate_names).unwrap();

        let by_crate = matches
            .iter()
            .map(|m| (m.crate_name.as_str(), m))
            .collect::<BTreeMap<_, _>>();

        assert_eq!(by_crate.len(), 3);
        assert_eq!(
            by_crate["bar-agent-guest-client"].key,
            "bar-agent-guest-client"
        );
        assert_eq!(
            by_crate["bar-agent-guest-client"].location,
            MatchedDependencyLocation::Package(DependencyTable::Dependencies)
        );
        assert_eq!(by_crate["foo-agent-guest-client"].key, "aliased");
        assert_eq!(
            by_crate["baz-agent-guest-client"].location,
            MatchedDependencyLocation::Package(DependencyTable::BuildDependencies)
        );
    }

    #[test]
    fn git_or_registry_dependencies_are_matched_as_unsupported() {
        let source = r#"
            [dependencies]
            a-agent-guest-client = { git = "https://example.com/a.git", version = "1" }
            b-agent-guest-client = { git = "https://example.com/b.git", path = "../b" }
            c-agent-guest-client = { registry = "custom", version = "1" }
        "#;

        let crate_names = BTreeSet::from([
            "a-agent-guest-client".to_string(),
            "b-agent-guest-client".to_string(),
            "c-agent-guest-client".to_string(),
        ]);
        let matches = find_package_dependencies_by_crate_name(source, &crate_names).unwrap();

        assert_eq!(matches.len(), 3);
        for matched in matches {
            assert!(
                matches!(matched.spec, DependencySpec::Unsupported(_)),
                "{} should be unsupported, got {:?}",
                matched.crate_name,
                matched.spec
            );
        }
    }

    #[test]
    fn find_package_dependencies_matches_target_specific_table() {
        let source = r#"
            [package]
            name = "consumer"

            [target.wasm32-wasip2.dependencies]
            bar-agent-guest-client = { path = "../old" }

            [target.'cfg(unix)'.dev-dependencies]
            foo-agent-guest-client = { path = "../foo" }
        "#;

        let crate_names = BTreeSet::from([
            "bar-agent-guest-client".to_string(),
            "foo-agent-guest-client".to_string(),
        ]);
        let matches = find_package_dependencies_by_crate_name(source, &crate_names).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].crate_name, "bar-agent-guest-client");
        assert_eq!(
            matches[0].location,
            MatchedDependencyLocation::TargetSpecific {
                target: "wasm32-wasip2".to_string(),
                table: DependencyTable::Dependencies,
            }
        );
    }

    #[test]
    fn find_package_dependencies_ignores_workspace_refs_and_dev_dependencies() {
        let source = r#"
            [package]
            name = "consumer"

            [dependencies]
            bar-agent-guest-client = { workspace = true }

            [dev-dependencies]
            foo-agent-guest-client = { path = "../foo" }
        "#;

        let crate_names = BTreeSet::from([
            "bar-agent-guest-client".to_string(),
            "foo-agent-guest-client".to_string(),
        ]);
        let matches = find_package_dependencies_by_crate_name(source, &crate_names).unwrap();

        assert!(matches.is_empty());
    }

    #[test]
    fn find_workspace_dependencies_matches_by_package_alias() {
        let source = r#"
            [workspace]
            members = ["consumer"]

            [workspace.dependencies]
            bar = { package = "bar-agent-guest-client", path = "old/path" }
            other = { path = "other" }
        "#;

        let crate_names = BTreeSet::from(["bar-agent-guest-client".to_string()]);
        let matches = find_workspace_dependencies_by_crate_name(source, &crate_names).unwrap();

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].key, "bar");
        assert_eq!(matches[0].crate_name, "bar-agent-guest-client");
        assert_eq!(matches[0].location, MatchedDependencyLocation::Workspace);
    }

    #[test]
    fn set_dependency_path_replaces_bare_version_with_path() {
        let source = r#"
            [dependencies]
            bar-agent-guest-client = "0.0.0"
        "#;

        let updated = set_dependency_path(
            source,
            MatchedDependencyLocation::Package(DependencyTable::Dependencies),
            "bar-agent-guest-client",
            "../bridge/bar-agent-guest-client",
        )
        .unwrap();

        assert!(
            updated.contains(
                r#"bar-agent-guest-client = { path = "../bridge/bar-agent-guest-client" }"#
            )
        );
        assert!(!updated.contains("0.0.0"));
    }

    #[test]
    fn set_dependency_path_preserves_package_alias_and_features() {
        let source = r#"
            [dependencies]
            bar = { package = "bar-agent-guest-client", path = "../old", features = ["extra"] }
        "#;

        let updated = set_dependency_path(
            source,
            MatchedDependencyLocation::Package(DependencyTable::Dependencies),
            "bar",
            "../bridge/bar-agent-guest-client",
        )
        .unwrap();

        let doc: DocumentMut = updated.parse().unwrap();
        let item = doc["dependencies"]["bar"].as_table_like().unwrap();
        assert_eq!(
            item.get("package").unwrap().as_str().unwrap(),
            "bar-agent-guest-client"
        );
        assert_eq!(
            item.get("path").unwrap().as_str().unwrap(),
            "../bridge/bar-agent-guest-client"
        );
        assert!(item.get("features").is_some());
    }

    #[test]
    fn set_dependency_path_updates_workspace_dependency() {
        let source = r#"
            [workspace.dependencies]
            bar = { package = "bar-agent-guest-client", path = "old/path" }
        "#;

        let updated = set_dependency_path(
            source,
            MatchedDependencyLocation::Workspace,
            "bar",
            "bridge/bar-agent-guest-client",
        )
        .unwrap();

        let doc: DocumentMut = updated.parse().unwrap();
        let item = doc["workspace"]["dependencies"]["bar"]
            .as_table_like()
            .unwrap();
        assert_eq!(
            item.get("package").unwrap().as_str().unwrap(),
            "bar-agent-guest-client"
        );
        assert_eq!(
            item.get("path").unwrap().as_str().unwrap(),
            "bridge/bar-agent-guest-client"
        );
    }
}
