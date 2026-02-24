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

use crate::edit::json::{collect_object_entries, update_object_entries};
use std::collections::BTreeMap;

pub fn merge_dependencies(
    source: &str,
    dependencies: &[(String, String)],
    dev_dependencies: &[(String, String)],
) -> anyhow::Result<String> {
    let mut output = source.to_string();
    if !dependencies.is_empty() {
        output = update_object_entries(&output, "dependencies", dependencies)?;
    }
    if !dev_dependencies.is_empty() {
        output = update_object_entries(&output, "devDependencies", dev_dependencies)?;
    }
    Ok(output)
}

pub fn collect_versions(
    source: &str,
    names: &[&str],
) -> anyhow::Result<BTreeMap<String, Option<String>>> {
    let mut collected = collect_object_entries(source, "dependencies", names)?;
    let dev = collect_object_entries(source, "devDependencies", names)?;
    for (name, version) in dev {
        if version.is_some() {
            collected.insert(name, version);
        }
    }
    Ok(collected)
}
