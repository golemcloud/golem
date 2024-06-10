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

use crate::cloud::clients::CloudAuthentication;
use crate::cloud::command::{CloudCommand, GolemCloudCommand};
use crate::cloud::factory::CloudServiceFactory;
use crate::config::{CloudProfile, ProfileName};
use crate::examples;
use crate::factory::ServiceFactory;
use crate::model::GolemError;
use crate::stubgen::handle_stubgen;
use colored::Colorize;
use std::path::{Path, PathBuf};
use uuid::Uuid;

async fn get_auth(
    auth_token: Option<Uuid>,
    profile_name: &ProfileName,
    profile: &CloudProfile,
    config_dir: &Path,
    factory: &CloudServiceFactory,
) -> Result<CloudAuthentication, GolemError> {
    let auth = factory
        .auth()?
        .authenticate(auth_token, profile_name, profile, config_dir)
        .await?;

    let version_check = factory.version_service(&auth)?.check().await;

    if let Err(err) = version_check {
        eprintln!("{}", err.0.yellow())
    }

    Ok(auth)
}

pub async fn async_main(
    cmd: GolemCloudCommand,
    profile_name: ProfileName,
    profile: CloudProfile,
    config_dir: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let format = cmd.format.unwrap_or(profile.config.default_format);

    let factory = CloudServiceFactory::from_profile(&profile);

    let res = match cmd.command {
        CloudCommand::Component { subcommand } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(
                    factory.component_service(&auth)?.as_ref(),
                    factory.project_resolver(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::Worker { subcommand } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(
                    format,
                    factory.worker_service(&auth)?.as_ref(),
                    factory.project_resolver(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::Account {
            account_id,
            subcommand,
        } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(
                    account_id,
                    factory.account_service(&auth)?.as_ref(),
                    factory.grant_service(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::Token {
            account_id,
            subcommand,
        } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(account_id, factory.token_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::Project { subcommand } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(factory.project_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::Share {
            project_ref,
            recipient_account_id,
            project_policy_id,
            project_actions,
        } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            factory
                .project_grant_service(&auth)?
                .grant(
                    project_ref,
                    recipient_account_id,
                    project_policy_id,
                    project_actions,
                )
                .await
        }
        CloudCommand::ProjectPolicy { subcommand } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(factory.project_policy_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::New {
            example,
            package_name,
            component_name,
        } => examples::process_new(example, component_name, package_name),
        CloudCommand::ListExamples { min_tier, language } => {
            examples::process_list_examples(min_tier, language)
        }
        #[cfg(feature = "stubgen")]
        CloudCommand::Stubgen { subcommand } => handle_stubgen(subcommand).await,
        CloudCommand::ApiDefinition { subcommand } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(
                    factory.api_definition_service(&auth)?.as_ref(),
                    factory.project_resolver(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::ApiDeployment { subcommand } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(
                    factory.api_deployment_service(&auth)?.as_ref(),
                    factory.project_resolver(&auth)?.as_ref(),
                )
                .await
        }
        CloudCommand::Certificate { subcommand } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(factory.certificate_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::Domain { subcommand } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            subcommand
                .handle(factory.domain_service(&auth)?.as_ref())
                .await
        }
        CloudCommand::Profile { subcommand } => subcommand.handle(&config_dir).await,
        CloudCommand::Init {} => crate::init::init_profile(None, &config_dir).await,
    };

    match res {
        Ok(res) => {
            res.print(format);
            Ok(())
        }
        Err(err) => Err(Box::new(err)),
    }
}
