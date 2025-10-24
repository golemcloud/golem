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

use crate::app::context::ApplicationContext;
use crate::auth::{Auth, Authentication};
use crate::command::shared_args::DeployArgs;
use crate::command::GolemCliGlobalFlags;
use crate::command_handler::interactive::InteractiveHandler;
use crate::config::AuthenticationConfig;
use crate::config::{ClientConfig, Config, HttpClientConfig, NamedProfile, Profile, ProfileName};
use crate::error::{ContextInitHintError, HintError, NonSuccessfulExit};
use crate::log::{log_action, set_log_output, LogColorize, LogOutput, Output};
use crate::model::app::{AppBuildStep, ApplicationSourceMode};
use crate::model::app::{ApplicationConfig, BuildProfileName as AppBuildProfileName};
use crate::model::text::fmt::log_error;
use crate::model::{app_raw, Format, ProjectReference};
use crate::model::{AccountDetails, AccountId, PluginReference};
use crate::wasm_rpc_stubgen::stub::RustDependencyOverride;
use anyhow::{anyhow, bail, Context as AnyhowContext};
use colored::control::SHOULD_COLORIZE;
use futures_util::future::BoxFuture;
use golem_client::api::AgentTypesClientLive as AgentTypesClientCloud;
use golem_client::api::ApiCertificateClientLive as ApiCertificateClientCloud;
use golem_client::api::ApiDefinitionClientLive as ApiDefinitionClientCloud;
use golem_client::api::ApiDeploymentClientLive as ApiDeploymentClientCloud;
use golem_client::api::ApiDomainClientLive as ApiDomainClientCloud;
use golem_client::api::ApiSecurityClientLive as ApiSecurityClientCloud;
use golem_client::api::ComponentClientLive as ComponentClientCloud;
use golem_client::api::GrantClientLive as GrantClientCloud;
use golem_client::api::HealthCheckClientLive;
use golem_client::api::LimitsClientLive as LimitsClientCloud;
use golem_client::api::LoginClientLive as LoginClientCloud;
use golem_client::api::PluginClientLive as PluginClientCloud;
use golem_client::api::ProjectClientLive as ProjectClientCloud;
use golem_client::api::ProjectGrantClientLive as ProjectGrantClientCloud;
use golem_client::api::ProjectPolicyClientLive as ProjectPolicyClientCloud;
use golem_client::api::TokenClientLive as TokenClientCloud;
use golem_client::api::WorkerClientLive as WorkerClientCloud;
use golem_client::api::{AccountClient, AccountSummaryClientLive as AccountSummaryClientCloud};
use golem_client::api::{AccountClientLive as AccountClientCloud, LoginClientLive};
use golem_client::{Context as ContextCloud, Security};
use golem_common::model::component_metadata::ComponentMetadata;
use golem_rib_repl::ReplComponentDependencies;
use golem_templates::model::{ComposableAppGroupName, GuestLanguage};
use golem_templates::ComposableAppTemplate;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;
use tracing::{debug, enabled, Level};
use url::Url;
use uuid::Uuid;

// Context is responsible for storing the CLI state,
// but NOT responsible for producing CLI output, those should be part of the CommandHandler(s)
pub struct Context {
    // Readonly
    config_dir: PathBuf,
    format: Format,
    help_mode: bool,
    local_server_auto_start: bool,
    deploy_args: DeployArgs,
    profile_name: ProfileName,
    profile: Profile,
    available_profile_names: BTreeSet<ProfileName>,
    app_context_config: ApplicationContextConfig,
    http_batch_size: u64,
    auth_token_override: Option<Uuid>,
    project: Option<ProjectReference>,
    client_config: ClientConfig,
    yes: bool,
    show_sensitive: bool,
    dev_mode: bool,
    server_no_limit_change: bool,
    should_colorize: bool,
    #[allow(unused)]
    start_local_server: Box<dyn Fn() -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>,

    file_download_client: reqwest::Client,

    // Lazy initialized
    golem_clients: tokio::sync::OnceCell<GolemClients>,
    templates: std::sync::OnceLock<
        BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>>,
    >,
    selected_profile_logging: std::sync::OnceLock<()>,
    http_api_reset: tokio::sync::OnceCell<()>,
    http_deployment_reset: tokio::sync::OnceCell<()>,

    // Directly mutable
    app_context_state: tokio::sync::RwLock<ApplicationContextState>,
    rib_repl_state: tokio::sync::RwLock<RibReplState>,
}

impl Context {
    pub async fn new(
        global_flags: GolemCliGlobalFlags,
        log_output_for_help: Option<Output>,
        start_local_server_yes: Arc<tokio::sync::RwLock<bool>>,
        start_local_server: Box<dyn Fn() -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>,
    ) -> anyhow::Result<Self> {
        let format = global_flags.format;
        let http_batch_size = global_flags.http_batch_size;
        let auth_token = global_flags.auth_token;
        let config_dir = global_flags.config_dir();
        let local_server_auto_start = global_flags.local_server_auto_start;
        let show_sensitive = global_flags.show_sensitive;
        let server_no_limit_change = global_flags.server_no_limit_change;

        let mut yes = global_flags.yes;
        let dev_mode = global_flags.dev_mode;
        let mut deploy_args = DeployArgs::none();

        let mut app_context_config = ApplicationContextConfig::new(global_flags);

        let preloaded_app = ApplicationContext::preload_sources_and_get_profiles(
            app_context_config.app_source_mode(),
        )?;

        if preloaded_app.loaded_with_warnings
            && log_output_for_help.is_none()
            && !InteractiveHandler::confirm_manifest_profile_warning(yes)?
        {
            bail!(NonSuccessfulExit);
        }

        let app_source_mode = preloaded_app.source_mode;
        let manifest_profiles = preloaded_app.profiles.unwrap_or_default();

        let (available_profile_names, profile, manifest_profile) = load_merged_profiles(
            &config_dir,
            app_context_config.requested_profile_name.as_ref(),
            manifest_profiles,
        )?;

        debug!(profile_name=%profile.name, manifest_profile=?manifest_profile, "Loaded profiles");

        if let Some(manifest_profile) = &manifest_profile {
            if app_context_config.build_profile.is_none() {
                app_context_config.build_profile = manifest_profile
                    .build_profile
                    .as_ref()
                    .map(|build_profile| build_profile.as_str().into())
            }

            if manifest_profile.auto_confirm == Some(true) {
                yes = true;
                *start_local_server_yes.write().await = true;
            }

            if manifest_profile.redeploy_agents == Some(true) {
                deploy_args.redeploy_agents = true;
            }

            if manifest_profile.redeploy_http_api == Some(true) {
                deploy_args.redeploy_http_api = true;
            }

            if manifest_profile.redeploy_all == Some(true) {
                deploy_args.redeploy_all = true;
            }

            if manifest_profile.reset == Some(true) {
                deploy_args.reset = true;
            }
        }

        let project = match manifest_profile.as_ref().and_then(|m| m.project.as_ref()) {
            Some(project) => Some(
                ProjectReference::from_str(project.as_str())
                    .map_err(|err| anyhow!("{}", err))
                    .with_context(|| {
                        anyhow!(
                            "Failed to parse project for manifest profile {}",
                            profile.name.0.log_color_highlight()
                        )
                    })?,
            ),
            None => None,
        };

        let format = format.unwrap_or(profile.profile.config.default_format);

        let log_output = log_output_for_help.unwrap_or_else(|| {
            if enabled!(Level::ERROR) {
                match format {
                    Format::Json | Format::PrettyJson | Format::Yaml | Format::PrettyYaml => {
                        Output::Stderr
                    }
                    Format::Text => Output::Stdout,
                }
            } else {
                Output::BufferedUntilErr
            }
        });

        set_log_output(log_output);

        let client_config = ClientConfig::from(&profile.profile);
        let file_download_client =
            new_reqwest_client(&client_config.file_download_http_client_config)?;

        Ok(Self {
            config_dir,
            format,
            help_mode: log_output_for_help.is_some(),
            local_server_auto_start,
            deploy_args,
            profile_name: profile.name,
            profile: profile.profile,
            available_profile_names,
            app_context_config,
            http_batch_size: http_batch_size.unwrap_or(50),
            auth_token_override: auth_token,
            project,
            yes,
            dev_mode,
            show_sensitive,
            server_no_limit_change,
            should_colorize: SHOULD_COLORIZE.should_colorize(),
            start_local_server,
            client_config,
            golem_clients: tokio::sync::OnceCell::new(),
            file_download_client,
            templates: std::sync::OnceLock::new(),
            selected_profile_logging: std::sync::OnceLock::new(),
            http_api_reset: tokio::sync::OnceCell::new(),
            http_deployment_reset: tokio::sync::OnceCell::new(),
            app_context_state: tokio::sync::RwLock::new(ApplicationContextState::new(
                yes,
                app_source_mode,
            )),
            rib_repl_state: tokio::sync::RwLock::default(),
        })
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub async fn rib_repl_history_file(&self) -> anyhow::Result<PathBuf> {
        let app_ctx = self.app_context_lock().await;
        let history_file = match app_ctx.opt()? {
            Some(app_ctx) => app_ctx.application.rib_repl_history_file().to_path_buf(),
            None => self.config_dir.join(".rib_repl_history"),
        };
        debug!(
            history_file = %history_file.display(),
            "Selected Rib REPL history file"
        );
        Ok(history_file)
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn yes(&self) -> bool {
        self.yes
    }

    pub fn dev_mode(&self) -> bool {
        self.dev_mode
    }

    pub fn show_sensitive(&self) -> bool {
        self.show_sensitive
    }

    pub fn deploy_args(&self) -> &DeployArgs {
        &self.deploy_args
    }

    pub fn server_no_limit_change(&self) -> bool {
        self.server_no_limit_change
    }

    pub fn should_colorize(&self) -> bool {
        self.should_colorize
    }

    pub async fn silence_app_context_init(&self) {
        let mut state = self.app_context_state.write().await;
        state.silent_init = true;
    }

    pub fn profile_name(&self) -> &ProfileName {
        self.log_selected_profile_once();
        &self.profile_name
    }

    pub fn available_profile_names(&self) -> &BTreeSet<ProfileName> {
        &self.available_profile_names
    }

    pub fn build_profile(&self) -> Option<&AppBuildProfileName> {
        self.app_context_config.build_profile.as_ref()
    }

    pub fn profile_project(&self) -> Option<&ProjectReference> {
        self.log_selected_profile_once();
        self.project.as_ref()
    }

    pub fn http_batch_size(&self) -> u64 {
        self.http_batch_size
    }

    pub async fn golem_clients(&self) -> anyhow::Result<&GolemClients> {
        self.golem_clients
            .get_or_try_init(|| async {
                self.log_selected_profile_once();

                let clients = GolemClients::new(
                    self.client_config.clone(),
                    self.auth_token_override,
                    &self.profile_name,
                    &self.profile.auth,
                    self.config_dir(),
                )
                .await?;

                if self.local_server_auto_start {
                    self.start_local_server_if_needed(&clients).await?;
                }

                Ok(clients)
            })
            .await
    }

    #[cfg(feature = "server-commands")]
    async fn start_local_server_if_needed(&self, clients: &GolemClients) -> anyhow::Result<()> {
        if !self.profile_name.is_builtin_local() {
            return Ok(());
        };

        // NOTE: explicitly calling the trait method to avoid unused imports when compiling with
        //       default features
        if golem_client::api::HealthCheckClient::healthcheck(&clients.component_healthcheck)
            .await
            .is_ok()
        {
            return Ok(());
        }

        (self.start_local_server)().await?;

        Ok(())
    }

    #[cfg(not(feature = "server-commands"))]
    async fn start_local_server_if_needed(&self, _clients: &GolemClients) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn file_download_client(&self) -> &reqwest::Client {
        &self.file_download_client
    }

    pub fn worker_service_url(&self) -> &Url {
        &self.client_config.worker_url
    }

    pub fn allow_insecure(&self) -> bool {
        self.client_config.service_http_client_config.allow_insecure
    }

    pub async fn account_id(&self) -> anyhow::Result<AccountId> {
        Ok(self.golem_clients().await?.account_id())
    }

    pub async fn auth_token(&self) -> anyhow::Result<String> {
        Ok(self.golem_clients().await?.auth_token())
    }

    pub async fn app_context_lock(
        &self,
    ) -> tokio::sync::RwLockReadGuard<'_, ApplicationContextState> {
        {
            let state = self.app_context_state.read().await;
            if state.app_context.is_some() {
                return state;
            }
        }

        {
            let _init = self.app_context_lock_mut().await;
        }

        self.app_context_state.read().await
    }

    pub async fn app_context_lock_mut(
        &self,
    ) -> anyhow::Result<tokio::sync::RwLockWriteGuard<'_, ApplicationContextState>> {
        let mut state = self.app_context_state.write().await;
        state
            .init(
                &self.available_profile_names,
                &self.app_context_config,
                self.file_download_client.clone(),
            )
            .await?;
        Ok(state)
    }

    pub async fn unload_app_context(&self) {
        let mut state = self.app_context_state.write().await;
        *state = ApplicationContextState::new(self.yes, self.app_context_config.app_source_mode());
    }

    async fn set_app_ctx_init_config<T>(
        &self,
        name: &str,
        value_mut: fn(&mut ApplicationContextState) -> &mut T,
        was_set_mut: fn(&mut ApplicationContextState) -> &mut bool,
        value: T,
    ) {
        let mut state = self.app_context_state.write().await;
        if *was_set_mut(&mut state) {
            panic!("{name} can be set only once, was already set");
        }
        if state.app_context.is_some() {
            panic!("cannot change {name} after application context init");
        }
        *value_mut(&mut state) = value;
        *was_set_mut(&mut state) = true;
    }

    pub async fn set_skip_up_to_date_checks(&self, skip: bool) {
        self.set_app_ctx_init_config(
            "skip_up_to_date_checks",
            |ctx| &mut ctx.skip_up_to_date_checks,
            |ctx| &mut ctx.skip_up_to_date_checks_was_set,
            skip,
        )
        .await
    }

    pub async fn set_steps_filter(&self, steps_filter: HashSet<AppBuildStep>) {
        self.set_app_ctx_init_config(
            "steps_filter",
            |ctx| &mut ctx.build_steps_filter,
            |ctx| &mut ctx.build_steps_filter_was_set,
            steps_filter,
        )
        .await;
    }

    pub async fn task_result_marker_dir(&self) -> anyhow::Result<PathBuf> {
        let app_ctx = self.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        Ok(app_ctx.application.task_result_marker_dir())
    }

    pub async fn set_rib_repl_state(&self, state: RibReplState) {
        let mut rib_repl_state = self.rib_repl_state.write().await;
        *rib_repl_state = state;
    }

    pub async fn get_rib_repl_dependencies(&self) -> ReplComponentDependencies {
        let rib_repl_state = self.rib_repl_state.read().await;
        ReplComponentDependencies {
            component_dependencies: rib_repl_state.dependencies.component_dependencies.clone(),
            custom_instance_spec: rib_repl_state.dependencies.custom_instance_spec.clone(),
        }
    }

    pub async fn get_rib_repl_component_metadata(&self) -> ComponentMetadata {
        let rib_repl_state = self.rib_repl_state.read().await;
        rib_repl_state.component_metadata.clone()
    }

    pub fn templates(
        &self,
        dev_mode: bool,
    ) -> &BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>> {
        self.templates
            .get_or_init(|| golem_templates::all_composable_app_templates(dev_mode))
    }

    pub async fn select_account_by_email_or_error(
        &self,
        email: &str,
    ) -> anyhow::Result<AccountDetails> {
        let mut result = self
            .golem_clients()
            .await?
            .account
            .find_accounts(Some(email))
            .await?
            .values;

        if result.len() == 1 {
            Ok(result.remove(0).into())
        } else {
            log_error("referenced account could not be found");
            bail!(NonSuccessfulExit)
        }
    }

    pub async fn resolve_plugin_reference(
        &self,
        reference: PluginReference,
    ) -> anyhow::Result<(AccountId, String, String)> {
        match reference {
            PluginReference::FullyQualified {
                account_email,
                name,
                version,
            } => {
                let account_id = self
                    .select_account_by_email_or_error(&account_email)
                    .await?
                    .account_id;

                Ok((account_id, name, version))
            }
            PluginReference::RelativeToCurrentAccount { name, version } => {
                let account_id = self.account_id().await?;

                Ok((account_id, name, version))
            }
        }
    }

    fn log_selected_profile_once(&self) {
        self.selected_profile_logging.get_or_init(|| {
            if !self.help_mode {
                log_action(
                    "Selected",
                    format!(
                        "profile: {}{}",
                        self.profile_name.0.log_color_highlight(),
                        self.project
                            .as_ref()
                            .map(|project| format!(
                                ", project: {}",
                                project.to_string().log_color_highlight()
                            ))
                            .unwrap_or_default()
                    ),
                );
            }
        });
    }

    pub async fn reset_http_api_definitions_once<F, Fut>(&self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        self.http_api_reset.get_or_try_init(f).await?;
        Ok(())
    }

    pub async fn reset_http_deployments_once<F, Fut>(&self, f: F) -> anyhow::Result<()>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = anyhow::Result<()>>,
    {
        self.http_deployment_reset.get_or_try_init(f).await?;
        Ok(())
    }
}

pub struct GolemClients {
    authentication: Authentication,

    pub account: AccountClientCloud,
    pub account_summary: AccountSummaryClientCloud,
    pub agent_types: AgentTypesClientCloud,
    pub api_certificate: ApiCertificateClientCloud,
    pub api_definition: ApiDefinitionClientCloud,
    pub api_deployment: ApiDeploymentClientCloud,
    pub api_domain: ApiDomainClientCloud,
    pub api_security: ApiSecurityClientCloud,
    pub component: ComponentClientCloud,
    pub component_healthcheck: HealthCheckClientLive,
    pub grant: GrantClientCloud,
    pub limits: LimitsClientCloud,
    pub login: LoginClientCloud,
    pub plugin: PluginClientCloud,
    pub project: ProjectClientCloud,
    pub project_grant: ProjectGrantClientCloud,
    pub project_policy: ProjectPolicyClientCloud,
    pub token: TokenClientCloud,
    pub worker: WorkerClientCloud,
    pub worker_invoke: WorkerClientCloud,
}

impl GolemClients {
    pub async fn new(
        config: ClientConfig,
        token_override: Option<Uuid>,
        profile_name: &ProfileName,
        auth_config: &AuthenticationConfig,
        config_dir: &Path,
    ) -> anyhow::Result<Self> {
        let healthcheck_http_client = new_reqwest_client(&config.health_check_http_client_config)?;

        let service_http_client = new_reqwest_client(&config.service_http_client_config)?;
        let invoke_http_client = new_reqwest_client(&config.invoke_http_client_config)?;

        let auth = Auth::new(LoginClientLive {
            context: ContextCloud {
                client: service_http_client.clone(),
                base_url: config.cloud_url.clone(),
                security_token: Security::Empty,
            },
        });

        let authentication = auth
            .authenticate(token_override, auth_config, config_dir, profile_name)
            .await?;

        let security_token = Security::Bearer(authentication.0.secret.value.to_string());

        let component_context = || ContextCloud {
            client: service_http_client.clone(),
            base_url: config.component_url.clone(),
            security_token: security_token.clone(),
        };

        let component_healthcheck_context = || ContextCloud {
            client: healthcheck_http_client,
            base_url: config.component_url.clone(),
            security_token: Security::Empty,
        };

        let worker_context = || ContextCloud {
            client: service_http_client.clone(),
            base_url: config.worker_url.clone(),
            security_token: security_token.clone(),
        };

        let worker_invoke_context = || ContextCloud {
            client: invoke_http_client.clone(),
            base_url: config.worker_url.clone(),
            security_token: security_token.clone(),
        };

        let cloud_context = || ContextCloud {
            client: service_http_client.clone(),
            base_url: config.cloud_url.clone(),
            security_token: security_token.clone(),
        };

        let login_context = || ContextCloud {
            client: service_http_client.clone(),
            base_url: config.cloud_url.clone(),
            security_token: security_token.clone(),
        };

        Ok(GolemClients {
            authentication,
            account: AccountClientCloud {
                context: cloud_context(),
            },
            account_summary: AccountSummaryClientCloud {
                context: worker_context(),
            },
            agent_types: AgentTypesClientCloud {
                context: component_context(),
            },
            api_certificate: ApiCertificateClientCloud {
                context: worker_context(),
            },
            api_definition: ApiDefinitionClientCloud {
                context: worker_context(),
            },
            api_deployment: ApiDeploymentClientCloud {
                context: worker_context(),
            },
            api_domain: ApiDomainClientCloud {
                context: worker_context(),
            },
            api_security: ApiSecurityClientCloud {
                context: worker_context(),
            },
            component: ComponentClientCloud {
                context: component_context(),
            },
            component_healthcheck: HealthCheckClientLive {
                context: component_healthcheck_context(),
            },
            grant: GrantClientCloud {
                context: cloud_context(),
            },
            limits: LimitsClientCloud {
                context: worker_context(),
            },
            login: LoginClientCloud {
                context: login_context(),
            },
            plugin: PluginClientCloud {
                context: component_context(),
            },
            project: ProjectClientCloud {
                context: cloud_context(),
            },
            project_grant: ProjectGrantClientCloud {
                context: cloud_context(),
            },
            project_policy: ProjectPolicyClientCloud {
                context: cloud_context(),
            },
            token: TokenClientCloud {
                context: cloud_context(),
            },
            worker: WorkerClientCloud {
                context: worker_context(),
            },
            worker_invoke: WorkerClientCloud {
                context: worker_invoke_context(),
            },
        })
    }

    pub fn account_id(&self) -> AccountId {
        self.authentication.account_id()
    }

    pub fn auth_token(&self) -> String {
        self.authentication.0.secret.value.to_string()
    }
}

struct ApplicationContextConfig {
    requested_profile_name: Option<ProfileName>,
    build_profile: Option<AppBuildProfileName>,
    app_manifest_path: Option<PathBuf>,
    disable_app_manifest_discovery: bool,
    golem_rust_override: RustDependencyOverride,
    wasm_rpc_client_build_offline: bool,
    dev_mode: bool,
    enable_wasmtime_fs_cache: bool,
}

impl ApplicationContextConfig {
    pub fn new(global_flags: GolemCliGlobalFlags) -> Self {
        Self {
            requested_profile_name: {
                if global_flags.local {
                    Some(ProfileName::local())
                } else if global_flags.cloud {
                    Some(ProfileName::cloud())
                } else {
                    global_flags.profile.clone()
                }
            },
            build_profile: global_flags.build_profile.map(|bp| bp.0.into()),
            app_manifest_path: global_flags.app_manifest_path,
            disable_app_manifest_discovery: global_flags.disable_app_manifest_discovery,
            golem_rust_override: RustDependencyOverride {
                path_override: global_flags.golem_rust_path,
                version_override: global_flags.golem_rust_version,
            },
            wasm_rpc_client_build_offline: global_flags.wasm_rpc_offline,
            dev_mode: global_flags.dev_mode,
            enable_wasmtime_fs_cache: global_flags.enable_wasmtime_fs_cache,
        }
    }

    pub fn app_source_mode(&self) -> ApplicationSourceMode {
        if self.disable_app_manifest_discovery {
            ApplicationSourceMode::None
        } else {
            match &self.app_manifest_path {
                Some(root_manifest) if !self.disable_app_manifest_discovery => {
                    ApplicationSourceMode::ByRootManifest(root_manifest.clone())
                }
                _ => ApplicationSourceMode::Automatic,
            }
        }
    }
}

#[derive()]
pub struct ApplicationContextState {
    yes: bool,
    app_source_mode: Option<ApplicationSourceMode>,
    pub silent_init: bool,
    pub skip_up_to_date_checks: bool,
    skip_up_to_date_checks_was_set: bool,
    pub build_steps_filter: HashSet<AppBuildStep>,
    build_steps_filter_was_set: bool,

    app_context: Option<Result<Option<ApplicationContext>, Arc<anyhow::Error>>>,
}

impl ApplicationContextState {
    pub fn new(yes: bool, source_mode: ApplicationSourceMode) -> Self {
        Self {
            yes,
            app_source_mode: Some(source_mode),
            silent_init: false,
            skip_up_to_date_checks: false,
            skip_up_to_date_checks_was_set: false,
            build_steps_filter: HashSet::new(),
            build_steps_filter_was_set: false,
            app_context: None,
        }
    }

    async fn init(
        &mut self,
        available_profile_names: &BTreeSet<ProfileName>,
        config: &ApplicationContextConfig,
        file_download_client: reqwest::Client,
    ) -> anyhow::Result<()> {
        if self.app_context.is_some() {
            return Ok(());
        }

        let _log_output = self
            .silent_init
            .then(|| LogOutput::new(Output::TracingDebug));

        let app_config = ApplicationConfig {
            skip_up_to_date_checks: self.skip_up_to_date_checks,
            build_profile: config.build_profile.as_ref().map(|p| p.to_string().into()),
            offline: config.wasm_rpc_client_build_offline,
            steps_filter: self.build_steps_filter.clone(),
            golem_rust_override: config.golem_rust_override.clone(),
            dev_mode: config.dev_mode,
            enable_wasmtime_fs_cache: config.enable_wasmtime_fs_cache,
        };

        debug!(app_config = ?app_config, "Initializing application context");

        self.app_context = Some(
            ApplicationContext::new(
                available_profile_names,
                self.app_source_mode
                    .take()
                    .expect("ApplicationContextState.app_source_mode is not set"),
                app_config,
                file_download_client,
            )
            .await
            .map_err(Arc::new),
        );

        if !self.silent_init {
            if let Some(Ok(Some(app_ctx))) = &self.app_context {
                if app_ctx.loaded_with_warnings
                    && !InteractiveHandler::confirm_manifest_app_warning(self.yes)?
                {
                    bail!(NonSuccessfulExit)
                }
            }
        }

        Ok(())
    }

    pub fn opt(&self) -> anyhow::Result<Option<&ApplicationContext>> {
        match &self.app_context {
            Some(Ok(None)) => Ok(None),
            Some(Ok(Some(app_ctx))) => Ok(Some(app_ctx)),
            Some(Err(err)) => Err(anyhow!(err.clone())),
            None => unreachable!("Uninitialized application context"),
        }
    }

    pub fn opt_mut(&mut self) -> anyhow::Result<Option<&mut ApplicationContext>> {
        match &mut self.app_context {
            Some(Ok(None)) => Ok(None),
            Some(Ok(Some(app_ctx))) => Ok(Some(app_ctx)),
            Some(Err(err)) => Err(anyhow!(err.clone())),
            None => unreachable!("Uninitialized application context"),
        }
    }

    pub fn some_or_err(&self) -> anyhow::Result<&ApplicationContext> {
        match &self.app_context {
            Some(Ok(None)) => Err(anyhow!(HintError::NoApplicationManifestFound)),
            Some(Ok(Some(app_ctx))) => Ok(app_ctx),
            Some(Err(err)) => Err(anyhow!(err.clone())),
            None => unreachable!("Uninitialized application context"),
        }
    }

    pub fn some_or_err_mut(&mut self) -> anyhow::Result<&mut ApplicationContext> {
        match &mut self.app_context {
            Some(Ok(None)) => Err(anyhow!(HintError::NoApplicationManifestFound)),
            Some(Ok(Some(app_ctx))) => Ok(app_ctx),
            Some(Err(err)) => Err(anyhow!(err.clone())),
            None => unreachable!("Uninitialized application context"),
        }
    }
}

pub struct RibReplState {
    pub dependencies: ReplComponentDependencies,
    pub component_metadata: ComponentMetadata,
}

impl Default for RibReplState {
    fn default() -> Self {
        Self {
            dependencies: ReplComponentDependencies {
                component_dependencies: vec![],
                custom_instance_spec: vec![],
            },
            component_metadata: ComponentMetadata::default(),
        }
    }
}

fn new_reqwest_client(config: &HttpClientConfig) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();

    if config.allow_insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }

    if let Some(timeout) = config.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(connect_timeout) = config.connect_timeout {
        builder = builder.connect_timeout(connect_timeout);
    }
    if let Some(read_timeout) = config.read_timeout {
        builder = builder.read_timeout(read_timeout);
    }

    Ok(builder.connection_verbose(true).build()?)
}

/// Finds the requested or the default profile in the global CLI config
/// and in the application manifest. The global config gets overrides applied from
/// the manifest profile.
///
/// NOTE: Both of the profiles are returned, because currently the set of properties
///       are different. Eventually they should converge, but that will need breaking
///       config changes and migration.
fn load_merged_profiles(
    config_dir: &Path,
    profile_name: Option<&ProfileName>,
    manifest_profiles: BTreeMap<ProfileName, app_raw::Profile>,
) -> anyhow::Result<(
    BTreeSet<ProfileName>,
    NamedProfile,
    Option<app_raw::Profile>,
)> {
    let mut available_profile_names = BTreeSet::new();

    let mut config = Config::from_dir(config_dir)?;
    available_profile_names.extend(config.profiles.keys().cloned());
    available_profile_names.extend(manifest_profiles.keys().cloned());

    // Use the requested profile name or the manifest default profile if none was requested
    // and there is a manifest default one
    let profile_name = match profile_name {
        Some(profile_name) => Some(profile_name),
        None => manifest_profiles
            .iter()
            .find_map(|(profile_name, profile)| {
                (profile.default == Some(true)).then_some(profile_name)
            }),
    };

    let global_profile = match profile_name {
        Some(profile_name) => config
            .profiles
            .remove(profile_name)
            .map(|profile| NamedProfile {
                name: profile_name.clone(),
                profile,
            }),
        None => Some(Config::get_default_profile(config_dir)?),
    };

    // If we did not find a global (maybe default) profile then switch back to the previously
    // calculated requested or manifest default profile.
    let profile_name = global_profile
        .as_ref()
        .map(|profile| Some(profile.name.clone()))
        .unwrap_or(profile_name.cloned());

    let manifest_profile = profile_name
        .as_ref()
        .and_then(|profile_name| manifest_profiles.get(profile_name));

    let (profile, manifest_profile) = match (global_profile, manifest_profile) {
        (Some(mut profile), Some(manifest_profile)) => {
            let profile_name = &profile.name;
            {
                let profile = &mut profile.profile;

                if let Some(format) = &manifest_profile.format {
                    profile.config.default_format = *format;
                }

                // TODO: should we allow these? or show only warn logs?
                if manifest_profile.url.is_some() {
                    bail!(
                        "Cannot override {} for global profile {} in application manifest!",
                        "url".log_color_highlight(),
                        profile_name.0.log_color_highlight()
                    );
                }
                if manifest_profile.worker_url.is_some() {
                    bail!(
                        "Cannot override {} for global profile {} in application manifest!",
                        "worker url".log_color_highlight(),
                        profile_name.0.log_color_highlight()
                    );
                }
            }
            (profile, Some(manifest_profile.clone()))
        }
        (Some(profile), None) => (profile, None),
        (None, Some(manifest_profile)) => {
            // If we only found manifest profile, then it must be found by name
            let profile_name = profile_name.unwrap();

            let profile = {
                let mut profile = Profile::default_local_profile();

                if let Some(format) = &manifest_profile.format {
                    profile.config.default_format = *format
                }

                if let Some(url) = &manifest_profile.url {
                    profile.custom_url = Some(url.clone());
                }
                if let Some(worker_url) = &manifest_profile.worker_url {
                    profile.custom_worker_url = Some(worker_url.clone());
                }
                profile
            };
            (
                NamedProfile {
                    name: profile_name,
                    profile,
                },
                Some(manifest_profile.clone()),
            )
        }
        (None, None) => {
            // If no profile is found, then its name must be defined, as otherwise the global
            // default should have returned
            let profile_name = profile_name.as_ref().unwrap().clone();
            bail!(ContextInitHintError::ProfileNotFound {
                profile_name,
                manifest_profile_names: manifest_profiles.keys().cloned().collect(),
            });
        }
    };

    Ok((available_profile_names, profile, manifest_profile))
}

pub async fn check_http_response_success(
    response: reqwest::Response,
) -> anyhow::Result<reqwest::Response> {
    if !response.status().is_success() {
        let url = response.url().clone();
        let status = response.status();
        let bytes = response.bytes().await.ok();
        let error_payload = bytes
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(bytes.as_ref()))
            .unwrap_or_else(|| Cow::from(""));

        bail!(
            "Received unexpected response for {}: {}\n{}",
            url,
            status,
            error_payload
        );
    }
    Ok(response)
}

#[cfg(test)]
mod test {
    use crate::context::Context;
    use std::marker::PhantomData;
    use test_r::test;

    struct CheckSend<T: Send>(PhantomData<T>);
    struct CheckSync<T: Sync>(PhantomData<T>);

    #[test]
    fn test_context_is_send_sync() {
        let _ = CheckSend::<Context>(PhantomData);
        let _ = CheckSync::<Context>(PhantomData);
    }
}
