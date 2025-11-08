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
use crate::config::{ClientConfig, HttpClientConfig, ProfileName};
use crate::error::{ContextInitHintError, HintError, NonSuccessfulExit};
use crate::log::{set_log_output, LogOutput, Output};
use crate::model::app::{AppBuildStep, ApplicationSourceMode, ComponentPresetName};
use crate::model::app::{ApplicationConfig, ComponentPresetSelector};
use crate::model::app_raw::Marker;
use crate::model::environment::EnvironmentReference;
use crate::model::format::Format;
use crate::model::{app_raw, AccountDetails, PluginReference};
use crate::wasm_rpc_stubgen::stub::RustDependencyOverride;
use anyhow::{anyhow, bail};
use colored::control::SHOULD_COLORIZE;
use golem_client::api::{
    AccountClientLive, AccountSummaryClientLive, AgentTypesClientLive, ApiCertificateClientLive,
    ApiDefinitionClientLive, ApiDeploymentClientLive, ApiDomainClientLive, ApiSecurityClientLive,
    ApplicationClientLive, ComponentClientLive, EnvironmentClientLive, GrantClientLive,
    HealthCheckClientLive, LimitsClientLive, LoginClientLive, PluginClientLive, TokenClientLive,
    WorkerClientLive,
};
use golem_client::{Context as ContextCloud, Security};
use golem_common::model::account::AccountId;
use golem_common::model::application::ApplicationName;
use golem_common::model::auth::TokenSecret;
use golem_common::model::component_metadata::ComponentMetadata;
use golem_common::model::environment::EnvironmentName;
use golem_rib_repl::ReplComponentDependencies;
use golem_templates::model::{ComposableAppGroupName, GuestLanguage};
use golem_templates::ComposableAppTemplate;
use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
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
    deploy_args: DeployArgs,
    // TODO: atomic:
    /*
    profile_name: ProfileName,
    profile: Profile,
    */
    environment_reference: Option<EnvironmentReference>,
    manifest_environment: Option<(EnvironmentName, app_raw::Environment)>,
    app_context_config: Option<ApplicationContextConfig>,
    http_batch_size: u64,
    auth_token_override: Option<Uuid>,
    client_config: ClientConfig,
    yes: bool,
    show_sensitive: bool,
    dev_mode: bool,
    server_no_limit_change: bool,
    should_colorize: bool,
    file_download_client: reqwest::Client,

    // Lazy initialized
    golem_clients: tokio::sync::OnceCell<GolemClients>,
    templates: std::sync::OnceLock<
        BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>>,
    >,
    selected_profile_and_environment_logging: std::sync::OnceLock<()>,
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
    ) -> anyhow::Result<Self> {
        let (environment_reference, env_ref_can_be_builtin_server) = {
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

        let preloaded_app = ApplicationContext::preload_sources_and_get_environments(
            ApplicationContextConfig::app_source_mode_from_global_flags(&global_flags),
        )?;

        if preloaded_app.loaded_with_warnings
            && log_output_for_help.is_none()
            && !InteractiveHandler::confirm_manifest_profile_warning(global_flags.yes)?
        {
            bail!(NonSuccessfulExit);
        }

        let app_source_mode = preloaded_app.source_mode;
        let manifest_environments = preloaded_app.environments;

        let manifest_environment: Option<(EnvironmentName, app_raw::Environment)> =
            match &environment_reference {
                Some(environment_reference) => {
                    match environment_reference {
                        EnvironmentReference::Environment { environment_name } => {
                            match &manifest_environments {
                                Some(environments) => match environments.get(environment_name) {
                                    Some(environment) => {
                                        Some((environment_name.clone(), environment.clone()))
                                    }
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
                                    if env_ref_can_be_builtin_server {
                                        None
                                    } else {
                                        bail!(ContextInitHintError::CannotSelectEnvironmentWithoutManifest {
                                        requested_environment_name: environment_name.clone()
                                    })
                                    }
                                }
                            }
                        }
                        EnvironmentReference::ApplicationEnvironment { .. } => {
                            // TODO: atomic
                            todo!()
                        }
                        EnvironmentReference::AccountApplicationEnvironment { .. } => {
                            // TODO: atomic
                            todo!()
                        }
                    }
                }
                None => match &manifest_environments {
                    Some(environments) => environments
                        .iter()
                        .find(|(_, env)| env.default == Some(Marker))
                        .map(|(name, env)| (name.clone(), env.clone())),
                    None => None,
                },
            };

        let mut yes = global_flags.yes;
        let mut deploy_args = DeployArgs::none();
        if let Some((_, environment)) = &manifest_environment {
            // TODO: atomic: use component presets
            /*
            if app_context_config.build_profile.is_none() {
                app_context_config.build_profile = manifest_profile
                    .build_profile
                    .as_ref()
                    .map(|build_profile| build_profile.as_str().into())

            }
            */

            if let Some(cli) = environment.cli.as_ref() {
                if cli.auto_confirm == Some(Marker) {
                    yes = true;
                }

                if cli.redeploy_agents == Some(Marker) {
                    deploy_args.redeploy_agents = true;
                }

                if cli.redeploy_http_api == Some(Marker) {
                    deploy_args.redeploy_http_api = true;
                }

                if cli.redeploy_all == Some(Marker) {
                    deploy_args.redeploy_all = true;
                }

                if cli.reset == Some(Marker) {
                    deploy_args.reset = true;
                }
            }
        }

        let format = global_flags
            .format
            .or(manifest_environment
                .as_ref()
                .and_then(|(_, env)| env.cli.as_ref())
                .and_then(|cli| cli.format))
            .unwrap_or_default();

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
            Some((_, environment)) => match environment.server.as_ref() {
                Some(server) => ClientConfig::from(server),
                None => {
                    ClientConfig::from(&app_raw::Server::Builtin(app_raw::BuiltinServer::Local))
                }
            },
            None => {
                // TODO: atomic: fallback to default profiles or command line args
                todo!()
            }
        };
        let file_download_client =
            new_reqwest_client(&client_config.file_download_http_client_config)?;

        let app_context_config = {
            manifest_environment
                .as_ref()
                .zip(manifest_environments)
                .map(
                    |((selected_environment_name, selected_environment), environments)| {
                        ApplicationContextConfig::new(
                            &global_flags,
                            environments,
                            ComponentPresetSelector {
                                environment: selected_environment_name.clone(),
                                presets: {
                                    let mut presets = selected_environment
                                        .component_presets
                                        .clone()
                                        .into_vec()
                                        .into_iter()
                                        .map(ComponentPresetName)
                                        .collect::<Vec<_>>();
                                    presets.extend(global_flags.preset.iter().cloned());
                                    presets
                                },
                            },
                        )
                    },
                )
        };

        Ok(Self {
            config_dir: global_flags.config_dir(),
            format,
            help_mode: log_output_for_help.is_some(),
            deploy_args,
            app_context_config,
            http_batch_size: global_flags.http_batch_size.unwrap_or(50),
            auth_token_override: global_flags.auth_token,
            environment_reference,
            manifest_environment,
            yes,
            dev_mode: global_flags.dev_mode,
            show_sensitive: global_flags.show_sensitive,
            server_no_limit_change: global_flags.server_no_limit_change,
            should_colorize: SHOULD_COLORIZE.should_colorize(),
            client_config,
            golem_clients: tokio::sync::OnceCell::new(),
            file_download_client,
            templates: std::sync::OnceLock::new(),
            selected_profile_and_environment_logging: std::sync::OnceLock::new(),
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
        self.log_selected_profile_and_environment_once();
        // TODO: atomic: &self.profile_name
        todo!()
    }

    pub fn available_profile_names(&self) -> &BTreeSet<ProfileName> {
        // TODO: atomic: &self.available_profile_names
        todo!()
    }

    pub fn application_name(&self) -> Option<ApplicationName> {
        // TODO: atomic
        todo!()
    }

    pub fn environment_reference(&self) -> Option<&EnvironmentReference> {
        self.log_selected_profile_and_environment_once();
        self.environment_reference.as_ref()
    }

    pub fn manifest_environment_name(&self) -> Option<&EnvironmentName> {
        self.log_selected_profile_and_environment_once();
        self.manifest_environment.as_ref().map(|(name, _)| name)
    }

    pub fn manifest_environment(&self) -> Option<&app_raw::Environment> {
        self.log_selected_profile_and_environment_once();
        self.manifest_environment.as_ref().map(|(_, env)| env)
    }

    pub fn http_batch_size(&self) -> u64 {
        self.http_batch_size
    }

    pub async fn golem_clients(&self) -> anyhow::Result<&GolemClients> {
        self.golem_clients
            .get_or_try_init(|| async {
                self.log_selected_profile_and_environment_once();

                let clients = GolemClients::new(
                    self.client_config.clone(),
                    self.auth_token_override,
                    // TODO: atomic
                    /*
                    &self.profile_name,
                    &self.profile.auth,
                    */
                    todo!(),
                    todo!(),
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
        Ok(self.golem_clients().await?.account_id().clone())
    }

    pub async fn auth_token(&self) -> anyhow::Result<TokenSecret> {
        Ok(self.golem_clients().await?.auth_token().clone())
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

    pub async fn unload_app_context(&self) {
        // TODO: atomic: this should also redo env preload
        // let mut state = self.app_context_state.write().await;
        // *state = ApplicationContextState::new(self.yes, self.app_context_config.app_source_mode());
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
        _email: &str,
    ) -> anyhow::Result<AccountDetails> {
        // TODO: atomic
        /*
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
        }*/
        todo!()
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

    fn log_selected_profile_and_environment_once(&self) {
        self.selected_profile_and_environment_logging
            .get_or_init(|| {
                // TODO: atomic
                /*
                if !self.help_mode {
                    log_action(
                        "Selected",
                        format!(
                            "profile: {}{}",
                            profile.name.0.log_color_highlight(),
                            environment
                                .as_ref()
                                .map(|environment| format!(
                                    ", environment: {}",
                                    environment.to_string().log_color_highlight()
                                ))
                                .unwrap_or_default()
                        ),
                    );
                }
                */
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

    pub account: AccountClientLive,
    pub account_summary: AccountSummaryClientLive,
    pub agent_types: AgentTypesClientLive,
    pub api_certificate: ApiCertificateClientLive,
    pub api_definition: ApiDefinitionClientLive,
    pub api_deployment: ApiDeploymentClientLive,
    pub api_domain: ApiDomainClientLive,
    pub api_security: ApiSecurityClientLive,
    pub application: ApplicationClientLive,
    pub component: ComponentClientLive,
    pub component_healthcheck: HealthCheckClientLive,
    pub environment: EnvironmentClientLive,
    pub grant: GrantClientLive,
    pub limits: LimitsClientLive,
    pub login: LoginClientLive,
    pub plugin: PluginClientLive,
    pub token: TokenClientLive,
    pub worker: WorkerClientLive,
    pub worker_invoke: WorkerClientLive,
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
                base_url: config.registry_url.clone(),
                security_token: Security::Empty,
            },
        });

        let authentication = auth
            .authenticate(token_override, auth_config, config_dir, profile_name)
            .await?;

        let security_token = Security::Bearer(authentication.0.secret.0.to_string());

        let registry_context = || ContextCloud {
            client: service_http_client.clone(),
            base_url: config.registry_url.clone(),
            security_token: security_token.clone(),
        };

        let registry_healthcheck_context = || ContextCloud {
            client: healthcheck_http_client,
            base_url: config.registry_url.clone(),
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

        let login_context = || ContextCloud {
            client: service_http_client.clone(),
            base_url: config.registry_url.clone(),
            security_token: security_token.clone(),
        };

        Ok(GolemClients {
            authentication,
            account: AccountClientLive {
                context: registry_context(),
            },
            account_summary: AccountSummaryClientLive {
                context: registry_context(),
            },
            agent_types: AgentTypesClientLive {
                context: registry_context(),
            },
            api_certificate: ApiCertificateClientLive {
                context: registry_context(),
            },
            api_definition: ApiDefinitionClientLive {
                context: registry_context(),
            },
            api_deployment: ApiDeploymentClientLive {
                context: registry_context(),
            },
            api_domain: ApiDomainClientLive {
                context: registry_context(),
            },
            api_security: ApiSecurityClientLive {
                context: registry_context(),
            },
            application: ApplicationClientLive {
                context: registry_context(),
            },
            component: ComponentClientLive {
                context: registry_context(),
            },
            component_healthcheck: HealthCheckClientLive {
                context: registry_healthcheck_context(),
            },
            environment: EnvironmentClientLive {
                context: registry_context(),
            },
            grant: GrantClientLive {
                context: registry_context(),
            },
            limits: LimitsClientLive {
                context: worker_context(),
            },
            login: LoginClientLive {
                context: login_context(),
            },
            plugin: PluginClientLive {
                context: registry_context(),
            },
            token: TokenClientLive {
                context: registry_context(),
            },
            worker: WorkerClientLive {
                context: worker_context(),
            },
            worker_invoke: WorkerClientLive {
                context: worker_invoke_context(),
            },
        })
    }

    pub fn account_id(&self) -> &AccountId {
        self.authentication.account_id()
    }

    pub fn auth_token(&self) -> &TokenSecret {
        &self.authentication.0.secret
    }
}

struct ApplicationContextConfig {
    app_manifest_path: Option<PathBuf>,
    disable_app_manifest_discovery: bool,
    environments: BTreeMap<EnvironmentName, app_raw::Environment>,
    component_presets: ComponentPresetSelector,
    golem_rust_override: RustDependencyOverride,
    wasm_rpc_client_build_offline: bool,
    dev_mode: bool,
    enable_wasmtime_fs_cache: bool,
}

impl ApplicationContextConfig {
    pub fn new(
        global_flags: &GolemCliGlobalFlags,
        environments: BTreeMap<EnvironmentName, app_raw::Environment>,
        component_presets: ComponentPresetSelector,
    ) -> Self {
        Self {
            app_manifest_path: global_flags.app_manifest_path.clone(),
            disable_app_manifest_discovery: global_flags.disable_app_manifest_discovery,
            environments,
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
            skip_up_to_date_checks: self.skip_up_to_date_checks,
            offline: config.wasm_rpc_client_build_offline,
            steps_filter: self.build_steps_filter.clone(),
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
                config.environments.clone(),
                config.component_presets.clone(),
                file_download_client.clone(),
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

// TODO: atomic: move to a http / client module?
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

// TODO: atomic: move to a http / client module?
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
