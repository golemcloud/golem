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
use crate::log::{log_action, LogColorize, LogIndent};
use crate::wasm_rpc_stubgen::cargo::generate_client_cargo_toml;
use crate::wasm_rpc_stubgen::compilation::compile;
use crate::wasm_rpc_stubgen::naming;
use crate::wasm_rpc_stubgen::rust::generate_stub_source;
use crate::wasm_rpc_stubgen::stub::StubDefinition;
use crate::wasm_rpc_stubgen::wit_generate::{
    add_dependencies_to_stub_wit_dir, generate_client_wit_to_target,
};
use crate::wasm_rpc_stubgen::wit_resolve::ResolvedWitDir;
use anyhow::{anyhow, Context};
use fs_extra::dir::CopyOptions;
use heck::ToSnakeCase;
use std::path::{Path, PathBuf};

pub fn generate_client(stub_def: &StubDefinition) -> anyhow::Result<()> {
    let _ = generate_client_wit_dir(stub_def)?;
    generate_client_cargo_toml(stub_def).context("Failed to generate the Cargo.toml file")?;
    generate_stub_source(stub_def).context("Failed to generate the client Rust source")?;
    Ok(())
}

pub async fn build(
    stub_def: &StubDefinition,
    dest_wasm: &Path,
    dest_wit_root: &Path,
    offline: bool,
) -> anyhow::Result<()> {
    let wasm_path = generate_and_build_client(stub_def, offline).await?;

    fs::copy(wasm_path, dest_wasm).context("Failed to copy the WASM file to the destination")?;
    fs::create_dir_all(dest_wit_root).context("Failed to create the target WIT root directory")?;

    fs_extra::dir::copy(
        stub_def.config.client_root.join(naming::wit::WIT_DIR),
        dest_wit_root,
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .context("Failed to copy the generated WIT files to the destination")?;

    Ok(())
}

pub fn generate_and_copy_client_wit(
    stub_def: &StubDefinition,
    dest_wit_root: &Path,
) -> anyhow::Result<()> {
    let _ = generate_client_wit_dir(stub_def)?;
    fs::create_dir_all(dest_wit_root).context("Failed to create the target WIT root directory")?;
    fs_extra::dir::copy(
        stub_def.config.client_root.join(naming::wit::WIT_DIR),
        dest_wit_root,
        &CopyOptions::new().content_only(true).overwrite(true),
    )
    .context("Failed to copy the generated WIT files to the destination")?;
    Ok(())
}

pub async fn generate_and_build_client(
    stub_def: &StubDefinition,
    offline: bool,
) -> anyhow::Result<PathBuf> {
    let _ = generate_client_wit_dir(stub_def)?;
    generate_client_cargo_toml(stub_def).context("Failed to generate the Cargo.toml file")?;
    generate_stub_source(stub_def).context("Failed to generate the client Rust source")?;

    compile(
        &stub_def
            .config
            .client_root
            .canonicalize()
            .with_context(|| {
                anyhow!(
                    "Failed to canonicalize client target root {}",
                    stub_def.config.client_root.log_color_error_highlight()
                )
            })?,
        offline,
    )
    .await
    .context("Failed to compile the generated client")?;

    let wasm_path = stub_def
        .config
        .client_root
        .join("target")
        .join("wasm32-wasip1")
        .join("release")
        .join(format!(
            "{}.wasm",
            stub_def.client_crate_name().to_snake_case()
        ));
    Ok(wasm_path)
}

pub fn generate_client_wit_dir(stub_def: &StubDefinition) -> anyhow::Result<ResolvedWitDir> {
    log_action(
        "Generating",
        format!(
            "client WIT directory to {}",
            stub_def.config.client_root.log_color_highlight()
        ),
    );
    let _indent = LogIndent::new();
    generate_client_wit_to_target(stub_def).context("Failed to generate the client wit file")?;
    add_dependencies_to_stub_wit_dir(stub_def).context("Failed to copy the dependent wit files")?;
    stub_def
        .resolve_client_wit()
        .context("Failed to resolve the result WIT root")
}
