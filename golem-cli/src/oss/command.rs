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

use crate::command::api_definition::ApiDefinitionSubcommand;
use crate::command::api_deployment::ApiDeploymentSubcommand;
use crate::command::component::ComponentSubCommand;
use crate::command::profile::ProfileSubCommand;
use crate::command::worker::{OssWorkerUriArg, WorkerSubcommand};
use crate::completion;
use crate::completion::PrintCompletion;
use crate::diagnose;
use crate::model::{ComponentUriArg, Format, HasFormatConfig, HasVerbosity};
use crate::oss::model::OssContext;
use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use clap_verbosity_flag::Verbosity;
use golem_common::uri::oss::uri::ResourceUri;

#[derive(Subcommand, Debug)]
#[command()]
pub enum OssCommand<ProfileAdd: clap::Args> {
    /// Upload and manage Golem components
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubCommand<OssContext, ComponentUriArg>,
    },

    /// Manage Golem workers
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand<ComponentUriArg, OssWorkerUriArg>,
    },

    /// Get resource by URI
    ///
    /// Use resource URN or URL to get resource metadata.
    #[command()]
    Get {
        #[arg(value_name = "URI")]
        uri: ResourceUri,
    },

    /// Create a new Golem component from built-in examples
    #[command(flatten)]
    Examples(golem_examples::cli::Command),

    /// WASM RPC stub generator
    #[cfg(feature = "stubgen")]
    Stubgen {
        #[command(subcommand)]
        subcommand: golem_wasm_rpc_stubgen::Command,
    },

    /// Manage Golem api definitions
    #[command()]
    ApiDefinition {
        #[command(subcommand)]
        subcommand: ApiDefinitionSubcommand<OssContext>,
    },

    /// Manage Golem api deployments
    #[command()]
    ApiDeployment {
        #[command(subcommand)]
        subcommand: ApiDeploymentSubcommand<OssContext>,
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

    /// Diagnose required tooling
    #[command()]
    Diagnose {
        #[command(flatten)]
        command: diagnose::cli::Command,
    },
}

#[derive(Parser, Debug)]
#[command(author, version = crate::VERSION, about, long_about, rename_all = "kebab-case")]
/// Command line interface for OSS version of Golem.
pub struct GolemOssCommand<ProfileAdd: clap::Args> {
    #[command(flatten)]
    pub verbosity: Verbosity,

    #[arg(short = 'F', long, global = true)]
    pub format: Option<Format>,

    #[command(subcommand)]
    pub command: OssCommand<ProfileAdd>,
}

impl<ProfileAdd: clap::Args> HasFormatConfig for GolemOssCommand<ProfileAdd> {
    fn format(&self) -> Option<Format> {
        self.format
    }
}

impl<ProfileAdd: clap::Args> HasVerbosity for GolemOssCommand<ProfileAdd> {
    fn verbosity(&self) -> Verbosity {
        self.verbosity.clone()
    }
}

impl<ProfileAdd: clap::Args> PrintCompletion for GolemOssCommand<ProfileAdd> {
    fn print_completion(shell: Shell) {
        completion::print_completion(GolemOssCommand::<ProfileAdd>::command(), shell)
    }
}
