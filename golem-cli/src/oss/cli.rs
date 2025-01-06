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

use super::model::OssContext;
use crate::command::profile::UniversalProfileAdd;
use crate::command::worker::OssWorkerUriArg;
use crate::command::{self, NoProfileCommandContext};
use crate::command::{CliCommand, SharedCommand, StaticSharedCommand};
use crate::completion;
use crate::config::{OssProfile, ProfileName};
use crate::factory::ServiceFactory;
use crate::init::{init_profile, CliKind, DummyProfileAuth};
use crate::model::app_ext::GolemComponentExtensions;
use crate::model::{ComponentUriArg, GolemError, GolemResult, OssPluginScopeArgs};
use crate::model::{ComponentUrisArg, Format};
use crate::oss::factory::OssServiceFactory;
use crate::oss::resource;
use crate::{check_for_newer_server_version, VERSION};
use clap::Parser;
use clap::{Command, Subcommand};
use clap_verbosity_flag::Verbosity;
use colored::Colorize;
use golem_client::model::{
    PluginDefinitionDefaultPluginOwnerDefaultPluginScope,
    PluginDefinitionWithoutOwnerDefaultPluginScope,
};
use golem_client::DefaultComponentOwner;
use golem_common::model::plugin::DefaultPluginScope;
use golem_common::uri::oss::uri::ResourceUri;
use std::path::PathBuf;

pub async fn run_with_profile<
    ProfileAdd: clap::Args + Into<UniversalProfileAdd>,
    ExtraCommands: CliCommand<OssCommandContext>,
>(
    format: Format,
    config_dir: PathBuf,
    profile: OssProfile,
    command: Command,
    parsed: GolemOssCli<ProfileAdd, ExtraCommands>,
    cli_kind: CliKind,
) -> Result<GolemResult, GolemError> {
    let factory = OssServiceFactory::from_profile(&profile)?;

    check_for_newer_server_version(factory.version_service().as_ref(), VERSION).await;

    let ctx = OssCommandContext {
        format,
        factory,
        config_dir,
        command,
        cli_kind,
    };

    parsed.command.run(ctx).await
}

pub async fn run_without_profile<
    ProfileAdd: clap::Args + Into<UniversalProfileAdd>,
    ExtraCommands: CliCommand<NoProfileCommandContext>,
>(
    config_dir: PathBuf,
    command: Command,
    parsed: GolemOssCli<ProfileAdd, ExtraCommands>,
    cli_kind: CliKind,
) -> Result<GolemResult, GolemError> {
    let ctx = NoProfileCommandContext {
        config_dir,
        command,
        cli_kind,
    };

    parsed.command.run(ctx).await
}

/// Commands only available in OSS
#[derive(Subcommand, Debug)]
#[command()]
pub enum OssOnlyCommand {
    /// Get resource by URI
    ///
    /// Use resource URN or URL to get resource metadata.
    #[command()]
    Get {
        #[arg(value_name = "URI")]
        uri: ResourceUri,
    },
}

impl CliCommand<NoProfileCommandContext> for OssOnlyCommand {
    async fn run(self, ctx: NoProfileCommandContext) -> Result<GolemResult, GolemError> {
        match self {
            OssOnlyCommand::Get { .. } => ctx.fail_uninitialized(),
        }
    }
}

/// Shared command with oss-specific arguments.
pub type OssSpecializedSharedCommand<ProfileAdd> = SharedCommand<
    OssContext,
    ComponentUriArg,
    ComponentUrisArg,
    OssWorkerUriArg,
    OssPluginScopeArgs,
    ProfileAdd,
>;

#[derive(Parser, Debug)]
#[command(author, version = crate::VERSION, about, long_about, rename_all = "kebab-case")]
/// Command line interface for OSS version of Golem.
pub struct GolemOssCli<ProfileAdd: clap::Args, ExtraCommand: Subcommand> {
    #[command(flatten)]
    pub verbosity: Verbosity,

    #[arg(short = 'F', long, global = true, default_value = "text")]
    pub format: Option<Format>,

    #[command(subcommand)]
    pub command: command::Zip<
        StaticSharedCommand,
        command::Zip<
            OssSpecializedSharedCommand<ProfileAdd>,
            command::Zip<OssOnlyCommand, ExtraCommand>,
        >,
    >,
}

/// Full context after the user has initialized the profile.
pub struct OssCommandContext {
    pub format: Format,
    pub factory: OssServiceFactory,
    pub config_dir: PathBuf,
    pub command: Command,
    pub cli_kind: CliKind,
}

impl CliCommand<OssCommandContext> for OssOnlyCommand {
    async fn run(self, ctx: OssCommandContext) -> Result<GolemResult, GolemError> {
        match self {
            OssOnlyCommand::Get { uri } => {
                let factory = ctx.factory;

                resource::get_resource_by_uri(uri, &factory).await
            }
        }
    }
}

impl<ProfileAdd: clap::Args + Into<UniversalProfileAdd>> CliCommand<OssCommandContext>
    for OssSpecializedSharedCommand<ProfileAdd>
{
    async fn run(self, ctx: OssCommandContext) -> Result<GolemResult, GolemError> {
        match self {
            SharedCommand::App { command } => {
                golem_wasm_rpc_stubgen::run_app_command::<GolemComponentExtensions>(
                    {
                        // TODO: it would be nice to use the same logic which is used by default for handling help,
                        //       and that way include the current context (bin name and parent commands),
                        //       but that seems to be using errors, error formating and exit directly;
                        //       and quite different code path compared to calling print_help
                        let mut clap_command = ctx.command;
                        clap_command
                            .find_subcommand_mut("app")
                            .unwrap()
                            .clone()
                            .override_usage(format!(
                                "{} [OPTIONS] [COMMAND]",
                                "golem-cli app".bold()
                            ))
                    },
                    command,
                )
                .await
                .map(|_| GolemResult::Empty)
                .map_err(Into::into)
            }
            SharedCommand::Component { subcommand } => {
                let factory = ctx.factory;

                subcommand
                    .handle(
                        ctx.format,
                        factory.component_service(),
                        factory.deploy_service(),
                        factory.project_resolver().as_ref(),
                    )
                    .await
            }
            SharedCommand::Worker { subcommand } => {
                let factory = ctx.factory;

                subcommand
                    .handle(
                        ctx.format,
                        factory.worker_service(),
                        factory.project_resolver(),
                    )
                    .await
            }
            SharedCommand::ApiDefinition { subcommand } => {
                let factory = ctx.factory;

                subcommand
                    .handle(
                        factory.api_definition_service().as_ref(),
                        factory.project_resolver().as_ref(),
                    )
                    .await
            }
            SharedCommand::ApiDeployment { subcommand } => {
                let factory = ctx.factory;

                subcommand
                    .handle(
                        factory.api_deployment_service().as_ref(),
                        factory.project_resolver().as_ref(),
                    )
                    .await
            }
            SharedCommand::ApiSecurityScheme { subcommand } => {
                let factory = ctx.factory;

                subcommand
                    .handle(
                        factory.api_security_scheme_service().as_ref(),
                        factory.project_resolver().as_ref(),
                    )
                    .await
            }
            SharedCommand::Profile { subcommand } => {
                subcommand
                    .handle(ctx.cli_kind, &ctx.config_dir, &DummyProfileAuth)
                    .await
            }
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
            SharedCommand::Completion { generator } => {
                completion::print_completion(ctx.command, generator);
                Ok(GolemResult::Empty)
            }
            SharedCommand::Plugin { subcommand } => {
                let factory = ctx.factory;

                subcommand
                    .handle::<PluginDefinitionDefaultPluginOwnerDefaultPluginScope, PluginDefinitionWithoutOwnerDefaultPluginScope, OssContext, DefaultPluginScope, DefaultComponentOwner, OssContext>(
                        ctx.format,
                        factory.plugin_client(),
                        factory.project_resolver(),
                        factory.component_service(),
                    )
                    .await
            }
        }
    }
}
