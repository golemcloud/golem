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

use cargo_component::config::{CargoArguments, Config};
use cargo_component::{load_component_metadata, load_metadata, run_cargo_command};
use cargo_component_core::terminal::{Color, Terminal, Verbosity};
use std::path::Path;

pub async fn compile(root: &Path, offline: bool) -> anyhow::Result<()> {
    let current_dir = std::env::current_dir()?;
    std::env::set_current_dir(root)?;

    let cargo_args = CargoArguments {
        release: true,
        manifest_path: Some(root.join("Cargo.toml")),
        offline,
        ..Default::default()
    };

    let config = Config::new(Terminal::new(Verbosity::Verbose, Color::Auto), None).await?;
    let client = config.client(None, offline).await?;

    let metadata = load_metadata(cargo_args.manifest_path.as_deref())?;
    let packages =
        load_component_metadata(&metadata, cargo_args.packages.iter(), cargo_args.workspace)?;

    let mut spawn_args = vec!["build".to_string(), "--release".to_string()];
    if offline {
        spawn_args.push("--offline".to_string());
    }

    run_cargo_command(
        client,
        &config,
        &metadata,
        &packages,
        Some("build"),
        &cargo_args,
        &spawn_args,
    )
    .await?;

    std::env::set_current_dir(current_dir)?;
    Ok(())
}
