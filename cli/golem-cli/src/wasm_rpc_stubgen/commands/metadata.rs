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

use crate::fs;
use crate::wasm_metadata::{AddMetadata, AddMetadataField};
use anyhow::Context;
use std::path::Path;
use wit_parser::PackageName;

/// Writes a name and a version metadata section based on the provided `root_package-name`
/// to the WASM read from `source`, saving the result to `target`
#[allow(clippy::field_reassign_with_default)]
pub fn add_metadata(
    source: &impl AsRef<Path>,
    root_package_name: PackageName,
    target: &impl AsRef<Path>,
) -> anyhow::Result<()> {
    let wasm = fs::read(source).context("Failed reading linked WASM")?;

    let mut metadata = AddMetadata::default();
    metadata.name = AddMetadataField::Set(format!(
        "{}:{}",
        root_package_name.namespace, root_package_name.name
    ));
    metadata.version = match &root_package_name.version {
        None => AddMetadataField::Clear,
        Some(v) => AddMetadataField::Set(crate::wasm_metadata::Version::new(v.to_string())),
    };

    let updated_wasm = metadata
        .to_wasm(&wasm)
        .context("Adding name and version metadata to the linked WASM")?;

    fs::create_dir_all(fs::parent_or_err(target.as_ref())?)?;

    fs::write(target, &updated_wasm).context("Failed writing final linked WASM")?;
    Ok(())
}
