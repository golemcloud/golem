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
use crate::app::template::AppTemplateRepo;
use crate::client::{new_reqwest_client, GolemClients};
use crate::command::shared_args::PostDeployArgs;
use crate::command::GolemCliGlobalFlags;
use crate::command_handler::interactive::InteractiveHandler;
use crate::config::{
    ApplicationEnvironmentConfigId, AuthenticationConfig, AuthenticationConfigWithSource,
    AuthenticationSource, Config, NamedProfile, BUILTIN_LOCAL_URL,
};
use crate::config::{ClientConfig, ProfileName};
use crate::error::{ContextInitHintError, HintError, NonSuccessfulExit};
use crate::log::{log_action, set_log_output, LogColorize, LogOutput, Output};
use crate::model::app::RustDependencyOverride;
use crate::model::app::{ApplicationConfig, ComponentPresetSelector};
use crate::model::app::{
    ApplicationNameAndEnvironments, ApplicationSourceMode, ComponentPresetName, WithSource,
};
use crate::model::app_raw::{
    BuiltinServer, CustomServerAuth, DeploymentOptions, Environment, Marker, Server,
};
use crate::model::environment::{EnvironmentReference, SelectedManifestEnvironment};
use crate::model::format::Format;
use crate::model::repl::ReplLanguage;
use crate::model::text::plugin::PluginNameAndVersion;
use crate::model::text::server::ToFormattedServerContext;
use crate::SdkOverrides;
use anyhow::{anyhow, bail};
use colored::control::SHOULD_COLORIZE;
use golem_client::model::EnvironmentPluginGrantWithDetails;
use golem_common::cache::{BackgroundEvictionMode, Cache, FullCacheEvictionMode};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationName;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component::{ComponentDto, ComponentId, ComponentRevision};
use golem_common::model::environment::{EnvironmentId, EnvironmentName};
use golem_common::model::http_api_deployment::{
    HttpApiDeployment, HttpApiDeploymentId, HttpApiDeploymentRevision,
};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, enabled, Level};
use url::Url;

// Context is responsible for storing the CLI state,
// but NOT responsible for producing CLI output (except for context selection logging), those should be part of the CommandHandler(s)
pub struct Context {
    // Readonly
    config_dir: PathBuf,
    format: Format,
    help_mode: bool,
    post_deploy_args: PostDeployArgs,
    profile: NamedProfile,
    environment_reference: Option<EnvironmentReference>,
    manifest_environment: Option<SelectedManifestEnvironment>,
    manifest_environment_deployment_options: Option<DeploymentOptions>,
    app_context_config: Option<ApplicationContextConfig>,
    http_batch_size: u64,
    http_parallelism: usize,
    auth_token_override: Option<String>,
    client_config: ClientConfig,
    yes: bool,
    show_sensitive: bool,
    dev_mode: bool,
    server_no_limit_change: bool,
    should_colorize: bool,
    sdk_overrides: SdkOverrides,

    file_download_client: reqwest::Client,

    // Lazy initialized
    golem_clients: tokio::sync::OnceCell<GolemClients>,
    selected_context_logging: std::sync::OnceLock<()>,

    // Directly mutable
    app_context_state: tokio::sync::RwLock<ApplicationContextState>,
    caches: Caches,
}

impl Context {
    pub async fn new(
        global_flags: GolemCliGlobalFlags,
        log_output_for_help: Option<Output>,
    ) -> anyhow::Result<Self> {
        if matches!(
            global_flags.environment,
            Some(EnvironmentReference::Environment { .. })
        ) && (global_flags.local || global_flags.cloud)
        {
            bail!(ContextInitHintError::CannotUseShortEnvRefWithLocalOrCloudFlags);
        }

        let (environment_reference, env_ref_can_be_builtin_profile) = {
            if let Some(environment) = &global_flags.environment {
                (Some(environment.clone()), false)
            } else if global_flags.local {
                (
                    Some(EnvironmentReference::Environment {
                        environment_name: EnvironmentName("local".to_string()),
                    }),
                    true,
                )
            } else if global_flags.cloud {
                (
                    Some(EnvironmentReference::Environment {
                        environment_name: EnvironmentName("cloud".to_string()),
                    }),
                    true,
                )
            } else {
                (None, false)
            }
        };

        let preloaded_app = ApplicationContext::preload_application(
            ApplicationContextConfig::app_source_mode_from_global_flags(&global_flags),
        )?;

        if preloaded_app.loaded_with_warnings
            && log_output_for_help.is_none()
            && !InteractiveHandler::confirm_manifest_profile_warning(global_flags.yes)?
        {
            bail!(NonSuccessfulExit);
        }

        // TODO: FCL:
        //   - move logic to app and app build module
        //   - use cached meta
        //   - review expects, to anyhow
        //   - golem-temp: either make it non-configurable, or preload it?
        {
            /*if !preloaded_app.used_language_templates.is_empty() {
                let templates = composable_app_templates(global_flags.dev_mode);
                let default_group = ComposableAppGroupName::default();
                for language in preloaded_app.used_language_templates {
                    let common_template = templates
                        .get(&language)
                        .expect("missing builtin language template")
                        .get(&default_group)
                        .expect("missing builtin default language template group")
                        .common
                        .as_ref()
                        .expect("missing builtin common language template");

                    for dir in common_template
                        .on_demand_common_dir_paths
                        .iter()
                        .map(|path| {
                            TEMPLATES_DIR
                                .get_dir(path)
                                .expect("missing on demand common template dir")
                        })
                    {
                        for entry in dir.entries() {
                            log_warn(format!("TODO: {}", entry.path().display()));
                        }
                    }
                }
            }*/
        }

        let app_source_mode = preloaded_app.source_mode;
        let application_name_and_environments = preloaded_app.application_name_and_environments;

        let manifest_environment: Option<SelectedManifestEnvironment> = match &environment_reference
        {
            Some(environment_reference) => {
                match environment_reference {
                    EnvironmentReference::Environment { environment_name } => {
                        match &application_name_and_environments {
                            Some(ApplicationNameAndEnvironments {
                                application_name,
                                environments,
                            }) => match environments.get(environment_name) {
                                Some(environment) => Some(SelectedManifestEnvironment {
                                    application_name: application_name.value.clone(),
                                    environment_name: environment_name.clone(),
                                    environment: environment.clone(),
                                }),
                                None => {
                                    bail!(ContextInitHintError::EnvironmentNotFound {
                                        requested_environment_name: environment_name.clone(),
                                        manifest_environment_names: environments
                                            .keys()
                                            .cloned()
                                            .collect(),
                                    })
                                }
                            },
                            None => {
                                if env_ref_can_be_builtin_profile {
                                    None
                                } else {
                                    bail!(ContextInitHintError::CannotSelectEnvironmentWithoutManifest {
                                        requested_environment_name: environment_name.clone()
                                    })
                                }
                            }
                        }
                    }
                    EnvironmentReference::ApplicationEnvironment { .. } => None,
                    EnvironmentReference::AccountApplicationEnvironment { .. } => None,
                }
            }
            None => match &application_name_and_environments {
                Some(ApplicationNameAndEnvironments {
                    application_name,
                    environments,
                }) => environments
                    .iter()
                    .find(|(_, env)| env.default == Some(Marker))
                    .map(
                        |(environment_name, environment)| SelectedManifestEnvironment {
                            application_name: application_name.value.clone(),
                            environment_name: environment_name.clone(),
                            environment: environment.clone(),
                        },
                    ),
                None => None,
            },
        };

        let manifest_environment_deployment_options = manifest_environment.as_ref().map(|env| {
            let is_local = env.environment_name.0 == "local";
            match env.environment.deployment.as_ref() {
                Some(options) if is_local => options
                    .clone()
                    .with_defaults_from(DeploymentOptions::new_local()),
                Some(options) => options.clone(),
                None if is_local => DeploymentOptions::new_local(),
                None => DeploymentOptions::new_cloud(),
            }
        });

        let use_cloud_profile_for_env = manifest_environment
            .as_ref()
            .map(|env| {
                matches!(
                    &env.environment.server,
                    Some(Server::Builtin(BuiltinServer::Cloud))
                )
            })
            .unwrap_or_default();

        let profile = Self::load_profile(&global_flags, use_cloud_profile_for_env)?;

        let mut yes = global_flags.yes;
        let mut post_deploy_args = PostDeployArgs::none();
        if let Some(selected_manifest_environment) = &manifest_environment {
            if let Some(cli) = selected_manifest_environment.environment.cli.as_ref() {
                if cli.auto_confirm == Some(Marker) {
                    yes = true;
                }

                if cli.redeploy_agents == Some(Marker) {
                    post_deploy_args.redeploy_agents = true;
                }

                if cli.reset == Some(Marker) {
                    post_deploy_args.reset = true;
                }
            }
        }

        let format = global_flags
            .format
            .or(manifest_environment
                .as_ref()
                .and_then(|env| env.environment.cli.as_ref())
                .and_then(|cli| cli.format))
            .unwrap_or(profile.profile.config.default_format);

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

        let client_config = match &manifest_environment {
            Some(env) => match env.environment.server.as_ref() {
                Some(server) => ClientConfig::from(server),
                None => ClientConfig::from(&Server::Builtin(BuiltinServer::Local)),
            },
            None => ClientConfig::from(&profile.profile),
        };
        let file_download_client =
            new_reqwest_client(&client_config.file_download_http_client_config)?;

        let app_context_config = {
            manifest_environment
                .as_ref()
                .zip(application_name_and_environments)
                .map(
                    |(selected_environment, application_name_and_environments)| {
                        ApplicationContextConfig::new(
                            &global_flags,
                            application_name_and_environments,
                            ComponentPresetSelector {
                                environment: selected_environment.environment_name.clone(),
                                presets: {
                                    if global_flags.preset.is_empty() {
                                        selected_environment
                                            .environment
                                            .component_presets
                                            .clone()
                                            .into_vec()
                                            .into_iter()
                                            .map(ComponentPresetName)
                                            .collect::<Vec<_>>()
                                    } else {
                                        global_flags.preset.clone()
                                    }
                                },
                            },
                        )
                    },
                )
        };

        let template_sdk_overrides = SdkOverrides {
            golem_rust_path: global_flags
                .golem_rust_path
                .as_ref()
                .map(|p| p.to_string_lossy().to_string()),
            golem_rust_version: global_flags.golem_rust_version.clone(),
            ts_packages_path: global_flags.golem_ts_packages_path.clone(),
            ts_version: global_flags.golem_ts_version.clone(),
        };

        Ok(Self {
            config_dir: global_flags.config_dir(),
            format,
            help_mode: log_output_for_help.is_some(),
            post_deploy_args,
            profile,
            app_context_config,
            http_batch_size: global_flags.http_batch_size(),
            http_parallelism: global_flags.http_parallelism(),
            auth_token_override: global_flags.auth_token,
            environment_reference,
            manifest_environment,
            manifest_environment_deployment_options,
            yes,
            dev_mode: global_flags.dev_mode,
            show_sensitive: global_flags.show_sensitive,
            server_no_limit_change: global_flags.server_no_limit_change,
            should_colorize: SHOULD_COLORIZE.should_colorize(),
            sdk_overrides: template_sdk_overrides,
            client_config,
            golem_clients: tokio::sync::OnceCell::new(),
            file_download_client,
            selected_context_logging: std::sync::OnceLock::new(),
            app_context_state: tokio::sync::RwLock::new(ApplicationContextState::new(
                yes,
                app_source_mode,
            )),
            caches: Caches::new(),
        })
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub async fn repl_history_file(&self, language: ReplLanguage) -> anyhow::Result<PathBuf> {
        let app_ctx = self.app_context_lock().await;
        let history_file = match app_ctx.opt()? {
            Some(app_ctx) => app_ctx
                .application()
                .repl_history_file(language)
                .to_path_buf(),
            None => self
                .config_dir
                .join(format!(".{}_repl_history", language.id())),
        };
        debug!(
            history_file = %history_file.display(),
            "Selected {language} REPL history file"
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

    pub fn deploy_args(&self) -> &PostDeployArgs {
        &self.post_deploy_args
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

    pub fn environment_reference(&self) -> Option<&EnvironmentReference> {
        self.log_context_selection_once();
        self.environment_reference.as_ref()
    }

    pub fn manifest_environment(&self) -> Option<&SelectedManifestEnvironment> {
        self.log_context_selection_once();
        self.manifest_environment.as_ref()
    }

    pub fn manifest_environment_deployment_options(&self) -> Option<&DeploymentOptions> {
        self.log_context_selection_once();
        self.manifest_environment_deployment_options.as_ref()
    }

    pub fn caches(&self) -> &Caches {
        &self.caches
    }

    pub fn http_batch_size(&self) -> u64 {
        self.http_batch_size
    }

    pub fn http_parallelism(&self) -> usize {
        self.http_parallelism
    }

    pub async fn golem_clients(&self) -> anyhow::Result<&GolemClients> {
        self.golem_clients
            .get_or_try_init(|| async {
                let clients = GolemClients::new(
                    &self.client_config,
                    self.auth_token_override.clone(),
                    &self.auth_config()?,
                    self.config_dir(),
                )
                .await?;

                Ok(clients)
            })
            .await
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
        Ok(*self.golem_clients().await?.account_id())
    }

    pub async fn auth_token(&self) -> anyhow::Result<TokenSecret> {
        Ok(self.golem_clients().await?.auth_token().clone())
    }

    fn auth_config(&self) -> anyhow::Result<AuthenticationConfigWithSource> {
        match self.manifest_environment() {
            Some(env) => match &env.environment.server {
                Some(Server::Builtin(BuiltinServer::Local)) | None => {
                    Ok(AuthenticationConfigWithSource {
                        authentication: AuthenticationConfig::static_builtin_local(),
                        source: AuthenticationSource::ApplicationEnvironment(
                            ApplicationEnvironmentConfigId {
                                application_name: env.application_name.clone(),
                                environment_name: env.environment_name.clone(),
                                server_url: BUILTIN_LOCAL_URL.parse()?,
                            },
                        ),
                    })
                }
                Some(Server::Builtin(BuiltinServer::Cloud)) => {
                    if !self.profile.name.is_builtin_cloud() {
                        panic!("when using the builtin cloud environment, the selected profile must be the builtin cloud profile");
                    }
                    Ok(AuthenticationConfigWithSource {
                        authentication: self.profile.profile.auth.clone(),
                        source: AuthenticationSource::Profile(self.profile.name.clone()),
                    })
                }
                Some(Server::Custom(server)) => {
                    let app_env_cfg_id = ApplicationEnvironmentConfigId {
                        application_name: env.application_name.clone(),
                        environment_name: env.environment_name.clone(),
                        server_url: server.url.clone(),
                    };

                    let authentication = match &server.auth {
                        CustomServerAuth::OAuth2 { .. } => {
                            Config::get_application_environment(&self.config_dir, &app_env_cfg_id)?
                                .map(|cfg| AuthenticationConfig::OAuth2(cfg.auth))
                                .unwrap_or_default()
                        }
                        CustomServerAuth::Static { static_token } => {
                            AuthenticationConfig::static_token(static_token.clone())
                        }
                    };

                    Ok(AuthenticationConfigWithSource {
                        authentication,
                        source: AuthenticationSource::ApplicationEnvironment(app_env_cfg_id),
                    })
                }
            },
            _ => Ok(AuthenticationConfigWithSource {
                authentication: self.profile.profile.auth.clone(),
                source: AuthenticationSource::Profile(self.profile.name.clone()),
            }),
        }
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
            .init(&self.app_context_config, &self.file_download_client)
            .await?;
        Ok(state)
    }

    pub async fn task_result_marker_dir(&self) -> anyhow::Result<PathBuf> {
        let app_ctx = self.app_context_lock().await;
        let app_ctx = app_ctx.some_or_err()?;
        Ok(app_ctx.application().task_result_marker_dir())
    }

    pub fn app_template_repo(&self) -> anyhow::Result<&AppTemplateRepo> {
        AppTemplateRepo::get(self.dev_mode)
    }

    pub fn sdk_overrides(&self) -> &SdkOverrides {
        &self.sdk_overrides
    }

    fn log_context_selection_once(&self) {
        self.selected_context_logging.get_or_init(|| {
            if self.help_mode {
                return;
            }

            let (app, env, server, profile): (String, String, String, Option<String>) = {
                if let Some(env) = &self.manifest_environment {
                    (
                        env.application_name.0.clone(),
                        env.environment_name.0.clone(),
                        env.environment.to_formatted_server_context(),
                        None,
                    )
                } else if let Some(environment_ref) = &self.environment_reference {
                    match environment_ref {
                        EnvironmentReference::Environment { environment_name } => (
                            "-".to_string(),
                            environment_name.0.clone(),
                            self.profile.to_formatted_server_context(),
                            Some(self.profile.name.0.clone()),
                        ),
                        EnvironmentReference::ApplicationEnvironment {
                            application_name,
                            environment_name,
                        } => (
                            application_name.0.clone(),
                            environment_name.0.clone(),
                            self.profile.to_formatted_server_context(),
                            Some(self.profile.name.0.clone()),
                        ),
                        EnvironmentReference::AccountApplicationEnvironment {
                            account_email,
                            application_name,
                            environment_name,
                            ..
                        } => (
                            format!("{}/{}", account_email, application_name.0),
                            environment_name.0.clone(),
                            self.profile.to_formatted_server_context(),
                            Some(self.profile.name.0.clone()),
                        ),
                    }
                } else {
                    (
                        "-".to_string(),
                        "-".to_string(),
                        self.profile.to_formatted_server_context(),
                        Some(self.profile.name.0.clone()),
                    )
                }
            };

            let opt_profile_formatted = profile
                .map(|profile| format!(", profile: {}", profile.log_color_highlight()))
                .unwrap_or_default();

            log_action(
                "Selected",
                format!(
                    "app: {app}, env: {env}, server: {server}{opt_profile_formatted}",
                    app = app.log_color_highlight(),
                    env = env.log_color_highlight(),
                    server = server.log_color_highlight(),
                ),
            );
        });
    }

    fn load_profile(
        global_flags: &GolemCliGlobalFlags,
        force_use_cloud_profile: bool,
    ) -> anyhow::Result<NamedProfile> {
        let config = Config::from_dir(&global_flags.config_dir())?;

        let profile_name = force_use_cloud_profile
            .then(ProfileName::cloud)
            .or_else(|| global_flags.profile.clone())
            .or_else(|| global_flags.local.then(ProfileName::local))
            .or_else(|| global_flags.cloud.then(ProfileName::cloud))
            .or(config.default_profile)
            .unwrap_or_else(ProfileName::local);

        let Some(profile) = config.profiles.get(&profile_name) else {
            bail!(ContextInitHintError::ProfileNotFound {
                profile_name,
                available_profile_names: config.profiles.keys().cloned().collect()
            });
        };

        Ok(NamedProfile {
            name: profile_name,
            profile: profile.clone(),
        })
    }
}

struct ApplicationContextConfig {
    #[allow(unused)]
    app_manifest_path: Option<PathBuf>,
    #[allow(unused)]
    disable_app_manifest_discovery: bool,
    application_name: WithSource<ApplicationName>,
    environments: BTreeMap<EnvironmentName, Environment>,
    component_presets: ComponentPresetSelector,
    golem_rust_override: RustDependencyOverride,
    wasm_rpc_client_build_offline: bool,
    dev_mode: bool,
    enable_wasmtime_fs_cache: bool,
}

impl ApplicationContextConfig {
    pub fn new(
        global_flags: &GolemCliGlobalFlags,
        application_name_and_environments: ApplicationNameAndEnvironments,
        component_presets: ComponentPresetSelector,
    ) -> Self {
        Self {
            app_manifest_path: global_flags.app_manifest_path.clone(),
            disable_app_manifest_discovery: global_flags.disable_app_manifest_discovery,
            application_name: application_name_and_environments.application_name,
            environments: application_name_and_environments.environments,
            component_presets,
            golem_rust_override: RustDependencyOverride {
                path_override: global_flags.golem_rust_path.clone(),
                version_override: global_flags.golem_rust_version.clone(),
            },
            wasm_rpc_client_build_offline: global_flags.wasm_rpc_offline,
            dev_mode: global_flags.dev_mode,
            enable_wasmtime_fs_cache: global_flags.enable_wasmtime_fs_cache,
        }
    }

    pub fn app_source_mode_from_global_flags(
        global_flags: &GolemCliGlobalFlags,
    ) -> ApplicationSourceMode {
        Self::app_source_mode_from_flags(
            global_flags.disable_app_manifest_discovery,
            global_flags.app_manifest_path.as_deref(),
        )
    }

    fn app_source_mode_from_flags(
        disable_app_manifest_discovery: bool,
        app_manifest_path: Option<&Path>,
    ) -> ApplicationSourceMode {
        if disable_app_manifest_discovery {
            ApplicationSourceMode::None
        } else {
            match app_manifest_path {
                Some(root_manifest) if !disable_app_manifest_discovery => {
                    ApplicationSourceMode::ByRootManifest(root_manifest.to_path_buf())
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

    app_context: Option<Result<Option<ApplicationContext>, Arc<anyhow::Error>>>,
}

impl ApplicationContextState {
    pub fn new(yes: bool, source_mode: ApplicationSourceMode) -> Self {
        Self {
            yes,
            app_source_mode: Some(source_mode),
            silent_init: false,
            app_context: None,
        }
    }

    async fn init(
        &mut self,
        config: &Option<ApplicationContextConfig>,
        file_download_client: &reqwest::Client,
    ) -> anyhow::Result<()> {
        if self.app_context.is_some() {
            return Ok(());
        }

        let Some(config) = config else {
            self.app_context = Some(Ok(None));
            return Ok(());
        };

        let _log_output = self
            .silent_init
            .then(|| LogOutput::new(Output::TracingDebug));

        let app_config = ApplicationConfig {
            offline: config.wasm_rpc_client_build_offline,
            golem_rust_override: config.golem_rust_override.clone(),
            dev_mode: config.dev_mode,
            enable_wasmtime_fs_cache: config.enable_wasmtime_fs_cache,
        };

        debug!(app_config = ?app_config, "Initializing application context");

        self.app_context = Some(
            ApplicationContext::new(
                self.app_source_mode
                    .take()
                    .expect("ApplicationContextState.app_source_mode is not set"),
                app_config,
                config.application_name.clone(),
                config.environments.clone(),
                config.component_presets.clone(),
                file_download_client.clone(),
            )
            .await
            .map_err(Arc::new),
        );

        if !self.silent_init {
            if let Some(Ok(Some(app_ctx))) = &self.app_context {
                if app_ctx.loaded_with_warnings()
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

pub struct Caches {
    pub component_revision:
        Cache<(ComponentId, ComponentRevision), (), ComponentDto, Arc<anyhow::Error>>,
    pub http_api_deployment_revision: Cache<
        (HttpApiDeploymentId, HttpApiDeploymentRevision),
        (),
        HttpApiDeployment,
        Arc<anyhow::Error>,
    >,
    pub plugin_grants: Cache<
        EnvironmentId,
        (),
        HashMap<PluginNameAndVersion, EnvironmentPluginGrantWithDetails>,
        Arc<anyhow::Error>,
    >,
}

impl Default for Caches {
    fn default() -> Self {
        Self::new()
    }
}

impl Caches {
    pub fn new() -> Self {
        Self {
            component_revision: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "component_revision",
            ),
            http_api_deployment_revision: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "http_api_deployment_revision",
            ),
            plugin_grants: Cache::new(
                None,
                FullCacheEvictionMode::None,
                BackgroundEvictionMode::None,
                "plugin_grants",
            ),
        }
    }
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
