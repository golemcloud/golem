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

use crate::fs::{create_dir_all, PathExtra};
use anyhow::Context;
use std::fs;
use std::path::Path;
use wasm_metadata::AddMetadata;
use wit_parser::PackageName;

/// Writes a name and a version metadata section based on the provided `root_package-name`
/// to the WASM read from `source`, saving the result to `target`
pub fn add_metadata(
    source: &impl AsRef<Path>,
    root_package_name: PackageName,
    target: &impl AsRef<Path>,
) -> anyhow::Result<()> {
    let wasm = fs::read(source)
        .with_context(|| format!("Reading linked WASM from {:?}", source.as_ref()))?;

    let metadata = AddMetadata {
        name: Some(format!(
            "{}:{}",
            root_package_name.namespace, root_package_name.name
        )),
        version: root_package_name
            .version
            .map(|v| wasm_metadata::Version::new(v.to_string())),
        ..Default::default()
    };

    let updated_wasm = metadata
        .to_wasm(&wasm)
        .context("Adding name and version metadata to the linked WASM")?;

    create_dir_all(PathExtra::new(target).parent()?)
        .with_context(|| format!("Failed to create target dir for {:?}", target.as_ref()))?;

    fs::write(target, &updated_wasm)
        .with_context(|| format!("Writing final linked WASM to {:?}", target.as_ref()))?;
    Ok(())
}
