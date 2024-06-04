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

use crate::model::Format;
use crate::oss::command::api_definition::ApiDefinitionSubcommand;
use crate::oss::command::api_deployment::ApiDeploymentSubcommand;
use crate::oss::command::component::ComponentSubCommand;
use crate::oss::command::worker::WorkerSubcommand;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier, PackageName};

pub mod api_definition;
pub mod api_deployment;
pub mod component;
pub mod worker;

#[derive(Subcommand, Debug)]
#[command()]
pub enum OssCommand {
    /// Upload and manage Golem components
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubCommand,
    },

    /// Manage Golem workers
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand,
    },

    /// Create a new Golem component from built-in examples
    #[command()]
    New {
        /// Name of the example to use
        #[arg(short, long)]
        example: ExampleName,

        /// The new component's name
        #[arg(short, long)]
        component_name: golem_examples::model::ComponentName,

        /// The package name of the generated component (in namespace:name format)
        #[arg(short, long)]
        package_name: Option<PackageName>,
    },

    /// Lists the built-in examples available for creating new components
    #[command()]
    ListExamples {
        /// The minimum language tier to include in the list
        #[arg(short, long)]
        min_tier: Option<GuestLanguageTier>,

        /// Filter examples by a given guest language
        #[arg(short, long)]
        language: Option<GuestLanguage>,
    },

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
        subcommand: ApiDefinitionSubcommand,
    },

    /// Manage Golem api deployments
    #[command()]
    ApiDeployment {
        #[command(subcommand)]
        subcommand: ApiDeploymentSubcommand,
    },
}

#[derive(Parser, Debug)]
#[command(author, version = option_env ! ("VERSION").unwrap_or(env ! ("CARGO_PKG_VERSION")), about, long_about, rename_all = "kebab-case")]
/// Command line interface for OSS version of Golem.
///
/// For Golem Cloud client see golem-cloud-cli instead: https://github.com/golemcloud/golem-cloud-cli
pub struct GolemOssCommand {
    #[command(flatten)]
    pub verbosity: Verbosity,

    #[arg(short = 'F', long, default_value = "text")]
    pub format: Format,

    #[arg(short = 'u', long)]
    /// Golem base url. Default: GOLEM_BASE_URL environment variable or http://localhost:9881.
    ///
    /// You can also specify different URLs for different services
    /// via GOLEM_COMPONENT_BASE_URL and GOLEM_WORKER_BASE_URL
    /// environment variables.
    pub golem_url: Option<String>,

    #[command(subcommand)]
    pub command: OssCommand,
}
