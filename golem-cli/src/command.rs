// Copyright 2024-2025 Golem Cloud
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

pub mod api_definition;
pub mod api_deployment;
pub mod api_security;
pub mod component;
pub mod plugin;
pub mod profile;
pub mod worker;

use crate::command::api_security::ApiSecuritySchemeSubcommand;
use crate::completion;
use crate::config::ProfileName;
use crate::diagnose::{self, diagnose};
use crate::examples;
use crate::init::{init_profile, CliKind, DummyProfileAuth};
use crate::model::{ComponentUriArg, GolemError, GolemResult};
use crate::oss::model::OssContext;
use api_definition::ApiDefinitionSubcommand;
use api_deployment::ApiDeploymentSubcommand;
use clap::{self, Command, Subcommand};
use component::ComponentSubCommand;
use golem_common::uri::oss::uri::ComponentUri;
use golem_wasm_rpc_stubgen::App;
use plugin::PluginSubcommand;
use profile::{ProfileSubCommand, UniversalProfileAdd};
use std::future::Future;
use std::path::PathBuf;
use worker::WorkerSubcommand;

pub trait ComponentRefSplit<ProjectRef> {
    fn split(self) -> (ComponentUri, Option<ProjectRef>);
}

impl ComponentRefSplit<OssContext> for ComponentUriArg {
    fn split(self) -> (ComponentUri, Option<OssContext>) {
        (self.uri, None)
    }
}

pub trait CliCommand<Ctx>: Subcommand {
    fn run(self, ctx: Ctx) -> impl Future<Output = Result<GolemResult, GolemError>>;
}

#[derive(Subcommand, Debug)]
pub enum Zip<A: Subcommand, B: Subcommand> {
    #[command(flatten)]
    First(A),
    #[command(flatten)]
    Second(B),
}

impl<Ctx, A, B> CliCommand<Ctx> for Zip<A, B>
where
    A: CliCommand<Ctx>,
    B: CliCommand<Ctx>,
{
    async fn run(self, ctx: Ctx) -> Result<GolemResult, GolemError> {
        match self {
            Zip::First(a) => a.run(ctx).await,
            Zip::Second(b) => b.run(ctx).await,
        }
    }
}

#[derive(Subcommand)]
pub enum EmptyCommand {}

impl<Ctx> CliCommand<Ctx> for EmptyCommand {
    async fn run(self, _ctx: Ctx) -> Result<GolemResult, GolemError> {
        Ok(GolemResult::Str("".to_string()))
    }
}

/// convenience function to get both the clap::Command and the parsed struct in one pass
pub fn command_and_parsed<T: clap::Parser>() -> (Command, T) {
    let mut command = T::command();

    let mut matches = command.clone().get_matches();
    let res = <T as clap::FromArgMatches>::from_arg_matches_mut(&mut matches)
        .map_err(|e| e.format(&mut command));
    match res {
        Ok(t) => (command, t),
        Err(e) => e.exit(),
    }
}

/// Commands that are supported by both the OSS and Cloud version and have the same implementation
#[derive(Debug, Subcommand)]
pub enum StaticSharedCommand {
    /// Diagnose required tooling
    #[command()]
    Diagnose {
        #[command(flatten)]
        command: diagnose::cli::Command,
    },
    /// Create a new Golem component from built-in examples
    #[command(flatten)]
    Examples(golem_examples::cli::Command),
}

impl<Ctx> CliCommand<Ctx> for StaticSharedCommand {
    async fn run(self, _ctx: Ctx) -> Result<GolemResult, GolemError> {
        match self {
            StaticSharedCommand::Diagnose { command } => {
                diagnose(command);
                Ok(GolemResult::Str("".to_string()))
            }
            StaticSharedCommand::Examples(golem_examples::cli::Command::ListExamples {
                min_tier,
                language,
            }) => examples::process_list_examples(min_tier, language),
            StaticSharedCommand::Examples(golem_examples::cli::Command::New {
                name_or_language,
                package_name,
                component_name,
            }) => examples::process_new(
                name_or_language.example_name(),
                component_name,
                package_name,
            ),
        }
    }
}

/// Commands that are supported by both the OSS and Cloud version
#[derive(Subcommand, Debug)]
#[command()]
pub enum SharedCommand<
    ProjectRef: clap::Args,
    ComponentRef: clap::Args,
    WorkerRef: clap::Args,
    PluginScopeRef: clap::Args,
    ProfileAdd: clap::Args,
> {
    /// Build components with application manifests
    #[group(skip)]
    App {
        #[clap(flatten)]
        command: App,
    },
    /// Upload and manage Golem components
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubCommand<ProjectRef, ComponentRef>,
    },

    /// Manage Golem workers
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand<ComponentRef, WorkerRef>,
    },

    /// Manage Golem api definitions
    #[command()]
    ApiDefinition {
        #[command(subcommand)]
        subcommand: ApiDefinitionSubcommand<ProjectRef>,
    },

    /// Manage Golem api deployments
    #[command()]
    ApiDeployment {
        #[command(subcommand)]
        subcommand: ApiDeploymentSubcommand<ProjectRef>,
    },

    /// Manage Api Security Schemes
    #[command()]
    ApiSecurityScheme {
        #[command(subcommand)]
        subcommand: ApiSecuritySchemeSubcommand<ProjectRef>,
    },

    /// Manage plugins
    #[command()]
    Plugin {
        #[command(subcommand)]
        subcommand: PluginSubcommand<PluginScopeRef>,
    },

    /// Manage profiles
    #[command()]
    Profile {
        #[command(subcommand)]
        subcommand: ProfileSubCommand<ProfileAdd>,
    },

    /// Interactively creates default profile
    #[command()]
    Init {},

    /// Generate shell completions
    #[command()]
    Completion {
        #[arg(long = "generate", value_enum)]
        generator: clap_complete::Shell,
    },
}

/// Context before the user has initialized the profile.
pub struct NoProfileCommandContext {
    pub config_dir: PathBuf,
    pub command: Command,
    pub cli_kind: CliKind,
}

impl NoProfileCommandContext {
    // \! is an experimental type. Once stable, use in the return type.
    pub fn fail_uninitialized(&self) -> Result<GolemResult, GolemError> {
        Err(GolemError(
            "Your Golem CLI is not configured. Please run `golem-cli init`".to_owned(),
        ))
    }
}

impl<
        ProjectRef: clap::Args,
        ComponentRef: clap::Args,
        WorkerRef: clap::Args,
        PluginScopeRef: clap::Args,
        ProfileAdd: clap::Args + Into<UniversalProfileAdd>,
    > CliCommand<NoProfileCommandContext>
    for SharedCommand<ProjectRef, ComponentRef, WorkerRef, PluginScopeRef, ProfileAdd>
{
    async fn run(self, ctx: NoProfileCommandContext) -> Result<GolemResult, GolemError> {
        match self {
            SharedCommand::Init {} => {
                let profile_name = ProfileName::default(ctx.cli_kind);

                init_profile(
                    ctx.cli_kind,
                    profile_name,
                    &ctx.config_dir,
                    &DummyProfileAuth,
                )
                .await?;

                Ok(GolemResult::Str("Profile created".to_string()))
            }
            SharedCommand::Profile { subcommand } => {
                subcommand
                    .handle(ctx.cli_kind, &ctx.config_dir, &DummyProfileAuth)
                    .await
            }
            SharedCommand::Completion { generator } => {
                completion::print_completion(ctx.command, generator);
                Ok(GolemResult::Str("".to_string()))
            }
            _ => ctx.fail_uninitialized(),
        }
    }
}
