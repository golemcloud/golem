// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

pub mod cargo;
pub mod commands;
pub mod compilation;
pub mod make;
pub mod model;
pub mod rust;
pub mod stub;
pub mod wit;

use crate::commands::dependencies::UpdateCargoToml;
use crate::stub::StubDefinition;
use anyhow::Context;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use tempfile::TempDir;

#[derive(Parser, Debug)]
#[command(name = "wasm-rpc-stubgen", version)]
pub enum Command {
    /// Generate a Rust RPC stub crate for a WASM component
    Generate(GenerateArgs),
    /// Build an RPC stub for a WASM component
    Build(BuildArgs),
    /// Adds a generated stub as a dependency to another WASM component
    AddStubDependency(AddStubDependencyArgs),
    /// Compose a WASM component with a generated stub WASM
    Compose(ComposeArgs),
    /// Initializes a Golem-specific cargo-make configuration in a Cargo workspace for automatically
    /// generating stubs and composing results.
    InitializeWorkspace(InitializeWorkspaceArgs),
    /// TODO
    #[cfg(feature = "unstable-dec-dep")]
    App {
        #[command(subcommand)]
        subcommand: App,
    },
}

/// Generate a Rust RPC stub crate for a WASM component
///
/// The command creates a new Rust crate that is ready to be compiled with
#[derive(clap::Args, Debug)]
#[command(version, about, long_about = None)]
pub struct GenerateArgs {
    /// The root directory of the component's WIT definition to be called via RPC
    #[clap(short, long)]
    pub source_wit_root: PathBuf,
    /// The target path to generate a new stub crate to
    #[clap(short, long)]
    pub dest_crate_root: PathBuf,
    /// The world name to be used in the generated stub crate. If there is only a single world in the source root
    ///  package, no need to specify.
    #[clap(short, long)]
    pub world: Option<String>,
    /// The crate version of the generated stub crate
    #[clap(long, default_value = "0.0.1")]
    pub stub_crate_version: String,
    #[clap(flatten)]
    pub wasm_rpc_override: WasmRpcOverride,
    /// Always inline all the data types defined in the source WIT instead of copying and depending on
    /// it from the stub WIT. This is useful for example with ComponentizeJS currently where otherwise
    /// the original component's interface would be added as an import to the final WASM.
    #[clap(long, default_value_t = false)]
    pub always_inline_types: bool,
}

#[derive(clap::Args, Debug, Clone)]
#[group(required = false, multiple = false)]
#[derive(Default)]
pub struct WasmRpcOverride {
    /// The path to the `wasm-rpc` crate to be used in the generated stub crate. If not specified, the latest version of `wasm-rpc` will be used. It needs to be an **absolute path**.
    #[clap(long, group = "override")]
    pub wasm_rpc_path_override: Option<String>,
    /// The version of the `wasm-rpc` crate to be used in the generated stub crate. If not specified, the latest version of `wasm-rpc` will be used.
    #[clap(long, group = "override")]
    pub wasm_rpc_version_override: Option<String>,
}

/// Build an RPC stub for a WASM component
///
/// The resulting WASM component implements the **stub interface** corresponding to the source interface, found in the
/// target directory's
/// `wit/_stub.wit` file. This WASM component is to be composed together with another component that calls the original
/// interface via WASM RPC.
#[derive(clap::Args, Debug)]
#[command(version, about, long_about = None)]
pub struct BuildArgs {
    /// The root directory of the component's WIT definition to be called via RPC
    #[clap(short, long)]
    pub source_wit_root: PathBuf,
    /// The name of the stub WASM file to be generated
    #[clap(long)]
    pub dest_wasm: PathBuf,
    /// The directory name where the generated WIT files should be placed
    #[clap(long)]
    pub dest_wit_root: PathBuf,
    /// The world name to be used in the generated stub crate. If there is only a single world in the source root
    ///   package, no need to specify.
    #[clap(short, long)]
    pub world: Option<String>,
    /// The crate version of the generated stub crate
    #[clap(long, default_value = "0.0.1")]
    pub stub_crate_version: String,
    #[clap(flatten)]
    pub wasm_rpc_override: WasmRpcOverride,
    /// Always inline all the data types defined in the source WIT instead of copying and depending on
    /// it from the stub WIT. This is useful for example with ComponentizeJS currently where otherwise
    /// the original component's interface would be added as an import to the final WASM.
    #[clap(long, default_value_t = false)]
    pub always_inline_types: bool,
}

/// Adds a generated stub as a dependency to another WASM component
///
/// The command merges a generated RPC stub as a WIT dependency into another component's WIT root.
#[derive(clap::Args, Debug)]
#[command(version, about, long_about = None)]
pub struct AddStubDependencyArgs {
    /// The WIT root generated by either `generate` or `build` command
    #[clap(short, long)]
    pub stub_wit_root: PathBuf,
    /// The WIT root of the component where the stub should be added as a dependency
    #[clap(short, long)]
    pub dest_wit_root: PathBuf,
    /// This command would not do anything if it detects that it would change an existing WIT file's contents at
    /// the destination. With this flag, it can be forced to overwrite those files.
    #[clap(short, long)]
    pub overwrite: bool,
    /// Enables updating the Cargo.toml file in the parent directory of `dest-wit-root` with the copied
    /// dependencies.
    #[clap(short, long)]
    pub update_cargo_toml: bool,
}

/// Compose a WASM component with a generated stub WASM
///
/// The command composes a caller component's WASM (which uses the generated stub to call a remote worker) with the
/// generated stub WASM, writing out a composed WASM which no longer depends on the stub interface, ready to use.
#[derive(clap::Args, Debug)]
#[command(version, about, long_about = None)]
pub struct ComposeArgs {
    /// The WASM file of the caller component
    #[clap(long)]
    pub source_wasm: PathBuf,
    /// The WASM file of the generated stub. Multiple stubs can be listed.
    #[clap(long, required = true)]
    pub stub_wasm: Vec<PathBuf>,
    /// The name of the composed WASM file to be generated
    #[clap(long)]
    pub dest_wasm: PathBuf,
}

/// Initializes a Golem-specific cargo-make configuration in a Cargo workspace for automatically
/// generating stubs and composing results.
#[derive(clap::Args, Debug)]
#[command(version, about, long_about = None)]
pub struct InitializeWorkspaceArgs {
    /// List of subprojects to be called via RPC
    #[clap(long, required = true)]
    pub targets: Vec<String>,
    /// List of subprojects using the generated stubs for calling remote workers
    #[clap(long, required = true)]
    pub callers: Vec<String>,
    #[clap(flatten)]
    pub wasm_rpc_override: WasmRpcOverride,
}

#[derive(Subcommand, Debug)]
pub enum App {
    /// Creates open application model for component
    Init(DeclarativeInitArgs),
    /// Runs the pre-component-build steps (stub generation and adding wit dependencies)
    PreComponentBuild(DeclarativeBuildArgs),
    /// Runs component build steps
    ComponentBuild(DeclarativeBuildArgs),
    /// Runs the post-component-build steps (composing stubs)
    PostComponentBuild(DeclarativeBuildArgs),
    /// Runs all build steps (pre-component, component, post-component)
    Build(DeclarativeBuildArgs),
}

#[derive(clap::Args, Debug)]
#[command(version, about, long_about = None)]
pub struct DeclarativeInitArgs {
    #[clap(long, short, required = true)]
    pub component_name: String,
}

#[derive(clap::Args, Debug)]
#[command(version, about, long_about = None)]
pub struct DeclarativeBuildArgs {
    /// List of Open Application Model specifications for component dependencies, can be defined multiple times
    #[clap(long, short)]
    pub component: Vec<PathBuf>,
}

pub fn generate(args: GenerateArgs) -> anyhow::Result<()> {
    let stub_def = StubDefinition::new(
        &args.source_wit_root,
        &args.dest_crate_root,
        &args.world,
        &args.stub_crate_version,
        &args.wasm_rpc_override,
        args.always_inline_types,
    )
        .context("Failed to gather information for the stub generator. Make sure source_wit_root has a valid WIT file.")?;
    commands::generate::generate(&stub_def)
}

pub async fn build(args: BuildArgs) -> anyhow::Result<()> {
    let target_root = TempDir::new()?;
    let canonical_target_root = target_root.path().canonicalize()?;

    let stub_def = StubDefinition::new(
        &args.source_wit_root,
        &canonical_target_root,
        &args.world,
        &args.stub_crate_version,
        &args.wasm_rpc_override,
        args.always_inline_types,
    )
    .context("Failed to gather information for the stub generator")?;

    commands::generate::build(&stub_def, &args.dest_wasm, &args.dest_wit_root).await
}

pub fn add_stub_dependency(args: AddStubDependencyArgs) -> anyhow::Result<()> {
    commands::dependencies::add_stub_dependency(
        &args.stub_wit_root,
        &args.dest_wit_root,
        args.overwrite,
        if args.update_cargo_toml {
            UpdateCargoToml::Update
        } else {
            UpdateCargoToml::NoUpdate
        },
    )
}

pub async fn compose(args: ComposeArgs) -> anyhow::Result<()> {
    commands::composition::compose(&args.source_wasm, &args.stub_wasm, &args.dest_wasm).await
}

pub fn initialize_workspace(
    args: InitializeWorkspaceArgs,
    stubgen_command: &str,
    stubgen_prefix: &[&str],
) -> anyhow::Result<()> {
    make::initialize_workspace(
        &args.targets,
        &args.callers,
        args.wasm_rpc_override,
        stubgen_command,
        stubgen_prefix,
    )
}

pub async fn run_declarative_command(command: App) -> anyhow::Result<()> {
    match command {
        App::Init(args) => commands::declarative::init(args.component_name),
        App::PreComponentBuild(args) => {
            commands::declarative::pre_component_build(dec_build_args_to_config(args)).await
        }
        App::ComponentBuild(args) => {
            commands::declarative::component_build(dec_build_args_to_config(args))
        }
        App::PostComponentBuild(args) => {
            commands::declarative::post_component_build(dec_build_args_to_config(args)).await
        }
        App::Build(args) => commands::declarative::build(dec_build_args_to_config(args)).await,
    }
}

fn dec_build_args_to_config(args: DeclarativeBuildArgs) -> commands::declarative::Config {
    commands::declarative::Config {
        app_resolve_mode: {
            if args.component.is_empty() {
                commands::declarative::ApplicationResolveMode::Automatic
            } else {
                commands::declarative::ApplicationResolveMode::Explicit(args.component)
            }
        },
        skip_up_to_date_checks: true,
    }
}
