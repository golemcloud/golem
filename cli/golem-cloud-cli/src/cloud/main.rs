use crate::cloud::clients::CloudAuthentication;
use crate::cloud::command::{CloudCommand, GolemCloudCommand};
use crate::cloud::factory::{CloudProfileAuth, CloudServiceFactory};
use crate::cloud::model::ProjectRef;
use colored::Colorize;
use golem_cli::cloud::{AccountId, ProjectId};
use golem_cli::command::profile::UniversalProfileAdd;
use golem_cli::config::{CloudProfile, ProfileName};
use golem_cli::diagnose::diagnose;
use golem_cli::examples;
use golem_cli::factory::ServiceFactory;
use golem_cli::init::{CliKind, PrintCompletion, ProfileAuth};
use golem_cli::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult};
use golem_cli::stubgen::handle_stubgen;
use golem_common::uri::cloud::uri::{
    ApiDefinitionUri, ComponentUri, ProjectUri, ResourceUri, ToOssUri, WorkerUri,
};
use golem_common::uri::cloud::url::{ComponentUrl, ProjectUrl, ResourceUrl, WorkerUrl};
use golem_common::uri::cloud::urn::{ComponentUrn, ResourceUrn, WorkerUrn};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub async fn get_auth(
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
        CloudCommand::Get { uri } => {
            let auth = get_auth(
                cmd.auth_token,
                &profile_name,
                &profile,
                &config_dir,
                &factory,
            )
            .await?;

            get_resource_by_uri(&auth, uri, &factory).await
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
            subcommand
                .handle(cli_kind, &config_dir, &CloudProfileAuth())
                .await
        }
        CloudCommand::Init {} => init(cli_kind, &config_dir, &CloudProfileAuth()).await,
        CloudCommand::Completion { generator } => {
            print_completion.print_completion(generator);
            Ok(GolemResult::Str("".to_string()))
        }
        CloudCommand::Diagnose { command } => {
            diagnose(command);
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

async fn init(
    cli_kind: CliKind,
    config_dir: &Path,
    profile_auth: &(dyn ProfileAuth + Send + Sync),
) -> Result<GolemResult, GolemError> {
    let res = golem_cli::init::init_profile(
        cli_kind,
        ProfileName::default(cli_kind),
        config_dir,
        profile_auth,
    )
    .await?;

    if res.auth_required {
        profile_auth.auth(&res.profile_name, config_dir).await?
    }

    Ok(GolemResult::Str("Profile created.".to_string()))
}

async fn resolve_project_id(
    auth: &CloudAuthentication,
    factory: &CloudServiceFactory,
    p: Option<ProjectUrl>,
) -> Result<Option<ProjectId>, GolemError> {
    Ok(factory
        .project_service(auth)?
        .resolve_urn(ProjectRef {
            uri: p.map(ProjectUri::URL),
            explicit_name: false,
        })
        .await?
        .map(|p| ProjectId(p.id.0)))
}

async fn get_resource_by_urn(
    auth: &CloudAuthentication,
    urn: ResourceUrn,
    factory: &CloudServiceFactory,
) -> Result<GolemResult, GolemError> {
    match urn {
        ResourceUrn::Account(a) => {
            factory
                .account_service(auth)?
                .as_ref()
                .get(Some(AccountId { id: a.id.value }))
                .await
        }
        ResourceUrn::Project(p) => {
            factory
                .project_service(auth)?
                .as_ref()
                .get(ProjectUri::URN(p))
                .await
        }
        ResourceUrn::Component(c) => {
            let (c, p) = ComponentUri::URN(c).to_oss_uri();
            let p = resolve_project_id(auth, factory, p).await?;

            factory
                .component_service(auth)?
                .as_ref()
                .get(c, None, p)
                .await
        }
        ResourceUrn::ComponentVersion(cv) => {
            let (c, p) = ComponentUri::URN(ComponentUrn { id: cv.id }).to_oss_uri();
            let p = resolve_project_id(auth, factory, p).await?;

            factory
                .component_service(auth)?
                .as_ref()
                .get(c, Some(cv.version), p)
                .await
        }
        ResourceUrn::Worker(w) => {
            let (w, p) = WorkerUri::URN(w).to_oss_uri();
            let p = resolve_project_id(auth, factory, p).await?;

            factory.worker_service(auth)?.get(w, p).await
        }
        ResourceUrn::WorkerFunction(wf) => {
            let (w, p) = WorkerUri::URN(WorkerUrn { id: wf.id }).to_oss_uri();
            let p = resolve_project_id(auth, factory, p).await?;

            factory
                .worker_service(auth)?
                .get_function(w, &wf.function, p)
                .await
        }
        ResourceUrn::ApiDefinition(d) => {
            let (_, p) = ApiDefinitionUri::URN(d.clone()).to_oss_uri();
            let p = factory
                .project_service(auth)?
                .resolve_urn_or_default(ProjectRef {
                    uri: p.map(ProjectUri::URL),
                    explicit_name: false,
                })
                .await?;
            let p = ProjectId(p.id.0);

            factory
                .api_definition_service(auth)?
                .get(ApiDefinitionId(d.id), ApiDefinitionVersion(d.version), &p)
                .await
        }
        ResourceUrn::ApiDeployment(d) => factory.api_deployment_service(auth)?.get(d.site).await,
    }
}

async fn get_resource_by_url(
    auth: &CloudAuthentication,
    url: ResourceUrl,
    factory: &CloudServiceFactory,
) -> Result<GolemResult, GolemError> {
    match url {
        ResourceUrl::Account(a) => {
            factory
                .account_service(auth)?
                .as_ref()
                .get(Some(AccountId { id: a.name }))
                .await
        }
        ResourceUrl::Project(p) => {
            factory
                .project_service(auth)?
                .as_ref()
                .get(ProjectUri::URL(p))
                .await
        }
        ResourceUrl::Component(c) => {
            let (c, p) = ComponentUri::URL(c).to_oss_uri();
            let p = resolve_project_id(auth, factory, p).await?;

            factory
                .component_service(auth)?
                .as_ref()
                .get(c, None, p)
                .await
        }
        ResourceUrl::ComponentVersion(cv) => {
            let (c, p) = ComponentUri::URL(ComponentUrl {
                name: cv.name.clone(),
                project: cv.project.clone(),
            })
            .to_oss_uri();
            let p = resolve_project_id(auth, factory, p).await?;

            factory
                .component_service(auth)?
                .as_ref()
                .get(c, Some(cv.version), p)
                .await
        }
        ResourceUrl::Worker(w) => {
            let (w, p) = WorkerUri::URL(w).to_oss_uri();
            let p = resolve_project_id(auth, factory, p).await?;

            factory.worker_service(auth)?.get(w, p).await
        }
        ResourceUrl::WorkerFunction(wf) => {
            let (w, p) = WorkerUri::URL(WorkerUrl {
                component_name: wf.component_name.clone(),
                worker_name: wf.worker_name.clone(),
                project: wf.project.clone(),
            })
            .to_oss_uri();
            let p = resolve_project_id(auth, factory, p).await?;

            factory
                .worker_service(auth)?
                .get_function(w, &wf.function, p)
                .await
        }
        ResourceUrl::ApiDefinition(d) => {
            let (_, p) = ApiDefinitionUri::URL(d.clone()).to_oss_uri();
            let p = factory
                .project_service(auth)?
                .resolve_urn_or_default(ProjectRef {
                    uri: p.map(ProjectUri::URL),
                    explicit_name: false,
                })
                .await?;
            let p = ProjectId(p.id.0);

            factory
                .api_definition_service(auth)?
                .get(ApiDefinitionId(d.name), ApiDefinitionVersion(d.version), &p)
                .await
        }
        ResourceUrl::ApiDeployment(d) => factory.api_deployment_service(auth)?.get(d.site).await,
    }
}

async fn get_resource_by_uri(
    auth: &CloudAuthentication,
    uri: ResourceUri,
    factory: &CloudServiceFactory,
) -> Result<GolemResult, GolemError> {
    match uri {
        ResourceUri::URN(urn) => get_resource_by_urn(auth, urn, factory).await,
        ResourceUri::URL(url) => get_resource_by_url(auth, url, factory).await,
    }
}
