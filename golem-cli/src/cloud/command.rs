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

use crate::cloud::command::account::AccountSubcommand;
use crate::cloud::command::certificate::CertificateSubcommand;
use crate::cloud::command::domain::DomainSubcommand;
use crate::cloud::command::policy::ProjectPolicySubcommand;
use crate::cloud::command::project::ProjectSubcommand;
use crate::cloud::command::token::TokenSubcommand;
use crate::cloud::model::{
    AccountId, CloudComponentIdOrName, ProjectAction, ProjectPolicyId, ProjectRef,
};
use crate::command::api_definition::ApiDefinitionSubcommand;
use crate::command::api_deployment::ApiDeploymentSubcommand;
use crate::command::component::ComponentSubCommand;
use crate::command::profile::ProfileSubCommand;
use crate::command::worker::WorkerSubcommand;
use crate::model::Format;
use clap::{Parser, Subcommand};
use clap_verbosity_flag::Verbosity;
use golem_examples::model::{ExampleName, GuestLanguage, GuestLanguageTier, PackageName};
use uuid::Uuid;

pub mod account;
pub mod certificate;
pub mod domain;
pub mod policy;
pub mod project;
pub mod token;

#[derive(Subcommand, Debug)]
#[command()]
pub enum CloudCommand {
    /// Upload and manage Golem components
    #[command()]
    Component {
        #[command(subcommand)]
        subcommand: ComponentSubCommand<ProjectRef, CloudComponentIdOrName>,
    },

    /// Manage Golem workers
    #[command()]
    Worker {
        #[command(subcommand)]
        subcommand: WorkerSubcommand<CloudComponentIdOrName>,
    },

    /// Manage accounts
    #[command()]
    Account {
        /// The account ID to operate on
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: AccountSubcommand,
    },

    /// Manage access tokens
    #[command()]
    Token {
        /// The account ID to operate on
        #[arg(short = 'A', long)]
        account_id: Option<AccountId>,

        #[command(subcommand)]
        subcommand: TokenSubcommand,
    },

    /// Manage projects
    #[command()]
    Project {
        #[command(subcommand)]
        subcommand: ProjectSubcommand,
    },

    /// Share a project with another account
    #[command()]
    Share {
        /// Project to be shared
        #[command(flatten)]
        project_ref: ProjectRef,

        /// User account the project will be shared with
        #[arg(long)]
        recipient_account_id: AccountId,

        /// The sharing policy's identifier. If not provided, use `--project-actions` instead
        #[arg(long, required = true, conflicts_with = "project_actions")]
        project_policy_id: Option<ProjectPolicyId>,

        /// A list of actions to be granted to the recipient account. If not provided, use `--project-policy-id` instead
        #[arg(
            short = 'A',
            long,
            required = true,
            conflicts_with = "project_policy_id"
        )]
        project_actions: Option<Vec<ProjectAction>>,
    },

    /// Manage project sharing policies
    #[command()]
    ProjectPolicy {
        #[command(subcommand)]
        subcommand: ProjectPolicySubcommand,
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
        subcommand: ApiDefinitionSubcommand<ProjectRef>,
    },

    /// Manage Golem api deployments
    #[command()]
    ApiDeployment {
        #[command(subcommand)]
        subcommand: ApiDeploymentSubcommand<ProjectRef>,
    },

    #[command()]
    Certificate {
        #[command(subcommand)]
        subcommand: CertificateSubcommand,
    },

    #[command()]
    Domain {
        #[command(subcommand)]
        subcommand: DomainSubcommand,
    },

    /// Manage profiles
    #[command()]
    Profile {
        #[command(subcommand)]
        subcommand: ProfileSubCommand,
    },

    /// Interactively creates default profile
    #[command()]
    Init {},
}

#[derive(Parser, Debug)]
#[command(author, version, about = "Command line interface for Golem Cloud.", long_about = None, rename_all = "kebab-case")]
pub struct GolemCloudCommand {
    #[arg(short = 'T', long)]
    pub auth_token: Option<Uuid>,

    #[command(flatten)]
    pub verbosity: Verbosity,

    #[arg(short = 'F', long)]
    pub format: Option<Format>,

    #[command(subcommand)]
    pub command: CloudCommand,
}
