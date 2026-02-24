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
use toml_edit::{value, DocumentMut, Item, Table};

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
        doc[table_name][name] = value(version.as_str());
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
}
