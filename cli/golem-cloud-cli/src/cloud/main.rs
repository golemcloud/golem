use crate::cloud::clients::CloudAuthentication;
use crate::cloud::command::{CloudCommand, GolemCloudCommand};
use crate::cloud::factory::CloudServiceFactory;
use async_trait::async_trait;
use colored::Colorize;
use golem_cli::command::profile::UniversalProfileAdd;
use golem_cli::config::{CloudProfile, Config, Profile, ProfileName};
use golem_cli::examples;
use golem_cli::factory::ServiceFactory;
use golem_cli::init::{CliKind, PrintCompletion, ProfileAuth};
use golem_cli::model::{GolemError, GolemResult};
use golem_cli::stubgen::handle_stubgen;
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

pub async fn async_main<ProfileAdd: Into<UniversalProfileAdd> + clap::Args>(
    cmd: GolemCloudCommand<ProfileAdd>,
    profile_name: ProfileName,
    profile: CloudProfile,
    cli_kind: CliKind,
    config_dir: PathBuf,
    print_completion: Box<dyn PrintCompletion>,
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
                    format,
                    factory.component_service(&auth)?,
                    factory.deploy_service(&auth)?,
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
                    factory.worker_service(&auth)?,
                    factory.project_resolver(&auth)?,
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
            project_actions_or_policy_id,
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
                    project_actions_or_policy_id.project_policy_id,
                    project_actions_or_policy_id.project_actions,
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
        CloudCommand::Examples(golem_examples::cli::Command::New {
            name_or_language,
            package_name,
            component_name,
        }) => examples::process_new(
            name_or_language.example_name(),
            component_name,
            package_name,
        ),
        CloudCommand::Examples(golem_examples::cli::Command::ListExamples {
            min_tier,
            language,
        }) => examples::process_list_examples(min_tier, language),
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
        CloudCommand::Profile { subcommand } => {
            subcommand.handle(cli_kind, &config_dir, &factory).await
        }
        CloudCommand::Init {} => init(cli_kind, &config_dir, &factory).await,
        CloudCommand::Completion { generator } => {
            print_completion.print_completion(generator);
            Ok(GolemResult::Str("".to_string()))
        }
    };

    match res {
        Ok(res) => {
            res.print(format);
            Ok(())
        }
        Err(err) => Err(Box::new(err)),
    }
}

#[async_trait]
impl ProfileAuth for CloudServiceFactory {
    async fn auth(&self, profile_name: &ProfileName, config_dir: &Path) -> Result<(), GolemError> {
        let profile = Config::get_profile(profile_name, config_dir)
            .ok_or(GolemError(format!("Can't find profile '{profile_name}'")))?;

        match profile {
            Profile::Golem(_) => Ok(()),
            Profile::GolemCloud(profile) => {
                let _ = get_auth(None, profile_name, &profile, config_dir, self).await?;
                Ok(())
            }
        }
    }
}

async fn init(
    cli_kind: CliKind,
    config_dir: &Path,
    profile_auth: &(dyn ProfileAuth + Send + Sync),
) -> Result<GolemResult, GolemError> {
    let res =
        golem_cli::init::init_profile(cli_kind, ProfileName::default(cli_kind), config_dir).await?;

    if res.auth_required {
        profile_auth.auth(&res.profile_name, config_dir).await?
    }

    Ok(GolemResult::Str("Profile created.".to_string()))
}
