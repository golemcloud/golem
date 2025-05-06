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

use crate::app::context::ApplicationContext;
use crate::auth::{Auth, CloudAuthentication};
use crate::cloud::{AccountId, CloudAuthenticationConfig};
use crate::command::GolemCliGlobalFlags;
use crate::config::{
    ClientConfig, HttpClientConfig, NamedProfile, Profile, ProfileKind, ProfileName,
};
use crate::error::HintError;
use crate::log::{set_log_output, LogOutput, Output};
use crate::model::app::{AppBuildStep, ApplicationSourceMode};
use crate::model::app::{ApplicationConfig, BuildProfileName as AppBuildProfileName};
use crate::model::{Format, HasFormatConfig};
use crate::wasm_rpc_stubgen::stub::RustDependencyOverride;
use anyhow::anyhow;
use futures_util::future::BoxFuture;
use golem_client::api::ApiDefinitionClientLive as ApiDefinitionClientOss;
use golem_client::api::ApiDeploymentClientLive as ApiDeploymentClientOss;
use golem_client::api::ApiSecurityClientLive as ApiSecurityClientOss;
use golem_client::api::ComponentClientLive as ComponentClientOss;
use golem_client::api::HealthCheckClientLive as HealthCheckClientOss;
use golem_client::api::PluginClientLive as PluginClientOss;
use golem_client::api::WorkerClientLive as WorkerClientOss;
use golem_client::Context as ContextOss;
use golem_cloud_client::api::AccountSummaryClientLive as AccountSummaryClientCloud;
use golem_cloud_client::api::ApiCertificateClientLive as ApiCertificateClientCloud;
use golem_cloud_client::api::ApiDefinitionClientLive as ApiDefinitionClientCloud;
use golem_cloud_client::api::ApiDeploymentClientLive as ApiDeploymentClientCloud;
use golem_cloud_client::api::ApiDomainClientLive as ApiDomainClientCloud;
use golem_cloud_client::api::ApiSecurityClientLive as ApiSecurityClientCloud;
use golem_cloud_client::api::ComponentClientLive as ComponentClientCloud;
use golem_cloud_client::api::GrantClientLive as GrantClientCloud;
use golem_cloud_client::api::LimitsClientLive as LimitsClientCloud;
use golem_cloud_client::api::LoginClientLive as LoginClientCloud;
use golem_cloud_client::api::PluginClientLive as PluginClientCloud;
use golem_cloud_client::api::ProjectClientLive as ProjectClientCloud;
use golem_cloud_client::api::ProjectGrantClientLive as ProjectGrantClientCloud;
use golem_cloud_client::api::ProjectPolicyClientLive as ProjectPolicyClientCloud;
use golem_cloud_client::api::TokenClientLive as TokenClientCloud;
use golem_cloud_client::api::WorkerClientLive as WorkerClientCloud;
use golem_cloud_client::api::{AccountClientLive as AccountClientCloud, LoginClientLive};
use golem_cloud_client::{Context as ContextCloud, Security};
use golem_rib_repl::ReplDependencies;
use golem_templates::model::{ComposableAppGroupName, GuestLanguage};
use golem_templates::ComposableAppTemplate;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::debug;
use url::Url;
use uuid::Uuid;

// Context is responsible for storing the CLI state,
// but NOT responsible for producing CLI output, those should be part of the CommandHandler(s)
pub struct Context {
    // Readonly
    config_dir: PathBuf,
    format: Format,
    profile_name: ProfileName,
    profile_kind: ProfileKind,
    profile: Profile,
    app_context_config: ApplicationContextConfig,
    http_batch_size: u64,
    auth_token_override: Option<Uuid>,
    client_config: ClientConfig,
    yes: bool,
    #[allow(unused)]
    start_local_server: Box<dyn Fn() -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>,

    // Lazy initialized
    clients: tokio::sync::OnceCell<Clients>,
    templates: std::sync::OnceLock<
        BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>>,
    >,

    // Directly mutable
    app_context_state: tokio::sync::RwLock<ApplicationContextState>,
    rib_repl_state: tokio::sync::RwLock<RibReplState>,
}

impl Context {
    pub fn new(
        global_flags: &GolemCliGlobalFlags,
        profile: NamedProfile,
        start_local_server: Box<dyn Fn() -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>,
    ) -> Self {
        let format = global_flags
            .format
            .unwrap_or(profile.profile.format().unwrap_or(Format::Text));
        let log_output = match format {
            Format::Json => Output::Stderr,
            Format::Yaml => Output::Stderr,
            Format::Text => Output::Stdout,
        };
        set_log_output(log_output);

        let client_config = ClientConfig::from(&profile.profile);

        Self {
            config_dir: global_flags.config_dir(),
            format: global_flags.format.unwrap_or(Format::Text),
            profile_name: profile.name,
            profile_kind: profile.profile.kind(),
            profile: profile.profile,
            app_context_config: ApplicationContextConfig {
                build_profile: global_flags
                    .build_profile
                    .as_ref()
                    .map(|bp| bp.0.clone().into()),
                app_manifest_path: global_flags.app_manifest_path.clone(),
                disable_app_manifest_discovery: global_flags.disable_app_manifest_discovery,
                golem_rust_override: RustDependencyOverride {
                    path_override: global_flags.golem_rust_path.clone(),
                    version_override: global_flags.golem_rust_version.clone(),
                },
                wasm_rpc_client_build_offline: global_flags.wasm_rpc_offline,
            },
            http_batch_size: global_flags.http_batch_size.unwrap_or(50),
            auth_token_override: global_flags.auth_token,
            yes: global_flags.yes,
            start_local_server,
            client_config,
            clients: tokio::sync::OnceCell::new(),
            templates: std::sync::OnceLock::new(),
            app_context_state: tokio::sync::RwLock::default(),
            rib_repl_state: tokio::sync::RwLock::default(),
        }
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

    pub async fn silence_app_context_init(&self) {
        let mut state = self.app_context_state.write().await;
        state.silent_init = true;
    }

    pub fn profile_kind(&self) -> ProfileKind {
        self.profile_kind
    }

    pub fn profile_name(&self) -> &ProfileName {
        &self.profile_name
    }

    pub fn build_profile(&self) -> Option<&AppBuildProfileName> {
        self.app_context_config.build_profile.as_ref()
    }

    pub fn http_batch_size(&self) -> u64 {
        self.http_batch_size
    }

    pub async fn clients(&self) -> anyhow::Result<&Clients> {
        self.clients
            .get_or_try_init(|| async {
                let clients = Clients::new(
                    self.client_config.clone(),
                    self.auth_token_override,
                    &self.profile_name,
                    match &self.profile {
                        Profile::Golem(_) => None,
                        Profile::GolemCloud(profile) => profile.auth.as_ref(),
                    },
                    self.config_dir(),
                )
                .await?;

                self.start_local_server_if_needed(&clients).await?;

                Ok(clients)
            })
            .await
    }

    #[cfg(feature = "server-commands")]
    async fn start_local_server_if_needed(&self, clients: &Clients) -> anyhow::Result<()> {
        if !self.profile_name.is_builtin_local() {
            return Ok(());
        };

        let GolemClients::Oss(clients) = &clients.golem else {
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
    async fn start_local_server_if_needed(&self, _clients: &Clients) -> anyhow::Result<()> {
        Ok(())
    }

    pub async fn golem_clients(&self) -> anyhow::Result<&GolemClients> {
        Ok(&self.clients().await?.golem)
    }

    pub async fn file_download_client(&self) -> anyhow::Result<reqwest::Client> {
        Ok(self.clients().await?.file_download.clone())
    }

    pub async fn golem_clients_cloud(&self) -> anyhow::Result<&GolemClientsCloud> {
        match &self.clients().await?.golem {
            GolemClients::Oss(_) => Err(anyhow!(HintError::ExpectedCloudProfile)),
            GolemClients::Cloud(clients) => Ok(clients),
        }
    }

    pub fn worker_service_url(&self) -> &Url {
        &self.client_config.worker_url
    }

    pub fn allow_insecure(&self) -> bool {
        self.client_config.service_http_client_config.allow_insecure
    }

    pub async fn auth_token(&self) -> anyhow::Result<Option<String>> {
        match self.golem_clients().await? {
            GolemClients::Oss(_) => Ok(None),
            GolemClients::Cloud(clients) => Ok(Some(clients.auth_token())),
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
    ) -> tokio::sync::RwLockWriteGuard<'_, ApplicationContextState> {
        let mut state = self.app_context_state.write().await;
        state.init(&self.app_context_config);
        state
    }

    pub async fn unload_app_context(&self) {
        let mut state = self.app_context_state.write().await;
        *state = ApplicationContextState::default();
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
            panic!("{} can be set only once, was already set", name);
        }
        if state.app_context.is_some() {
            panic!("cannot change {} after application context init", name);
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

    pub async fn set_rib_repl_dependencies(&self, dependencies: ReplDependencies) {
        let mut rib_repl_state = self.rib_repl_state.write().await;
        rib_repl_state.dependencies = dependencies;
    }

    pub async fn get_rib_repl_dependencies(&self) -> ReplDependencies {
        let rib_repl_state = self.rib_repl_state.read().await;
        ReplDependencies {
            component_dependencies: rib_repl_state.dependencies.component_dependencies.clone(),
        }
    }

    pub fn templates(
        &self,
    ) -> &BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>> {
        self.templates
            .get_or_init(golem_templates::all_composable_app_templates)
    }
}

pub struct Clients {
    pub golem: GolemClients,
    pub file_download: reqwest::Client,
}

impl Clients {
    pub async fn new(
        config: ClientConfig,
        token_override: Option<Uuid>,
        profile_name: &ProfileName,
        auth_config: Option<&CloudAuthenticationConfig>,
        config_dir: &Path,
    ) -> anyhow::Result<Self> {
        let healthcheck_http_client = new_reqwest_client(&config.health_check_http_client_config)?;
        let local_healthcheck_http_client =
            new_reqwest_client(&config.local_health_check_http_client_config)?;
        let service_http_client = new_reqwest_client(&config.service_http_client_config)?;
        let invoke_http_client = new_reqwest_client(&config.invoke_http_client_config)?;
        let file_download_http_client =
            new_reqwest_client(&config.file_download_http_client_config)?;

        match &config.cloud_url {
            Some(cloud_url) => {
                let auth = Auth::new(LoginClientLive {
                    context: ContextCloud {
                        client: service_http_client.clone(),
                        base_url: cloud_url.clone(),
                        security_token: Security::Empty,
                    },
                });

                let authentication = auth
                    .authenticate(token_override, profile_name, auth_config, config_dir)
                    .await?;
                let security_token = Security::Bearer(authentication.0.secret.value.to_string());

                let component_context = || ContextCloud {
                    client: service_http_client.clone(),
                    base_url: config.component_url.clone(),
                    security_token: security_token.clone(),
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
                    base_url: cloud_url.clone(),
                    security_token: security_token.clone(),
                };

                let login_context = || ContextCloud {
                    client: service_http_client.clone(),
                    base_url: cloud_url.clone(),
                    security_token: security_token.clone(),
                };

                Ok(Clients {
                    golem: GolemClients::Cloud(GolemClientsCloud {
                        authentication,
                        account: AccountClientCloud {
                            context: cloud_context(),
                        },
                        account_summary: AccountSummaryClientCloud {
                            context: worker_context(),
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
                    }),
                    file_download: file_download_http_client,
                })
            }
            None => {
                let component_context = || ContextOss {
                    client: service_http_client.clone(),
                    base_url: config.component_url.clone(),
                };

                let component_healthcheck_context = || ContextOss {
                    client: {
                        if profile_name.is_builtin_local() {
                            local_healthcheck_http_client.clone()
                        } else {
                            healthcheck_http_client.clone()
                        }
                    },
                    base_url: config.component_url.clone(),
                };

                let worker_context = || ContextOss {
                    client: service_http_client.clone(),
                    base_url: config.worker_url.clone(),
                };

                let worker_invoke_context = || ContextOss {
                    client: invoke_http_client.clone(),
                    base_url: config.worker_url.clone(),
                };

                Ok(Clients {
                    golem: GolemClients::Oss(GolemClientsOss {
                        api_definition: ApiDefinitionClientOss {
                            context: worker_context(),
                        },
                        api_deployment: ApiDeploymentClientOss {
                            context: worker_context(),
                        },
                        api_security: ApiSecurityClientOss {
                            context: worker_context(),
                        },
                        component: ComponentClientOss {
                            context: component_context(),
                        },
                        component_healthcheck: HealthCheckClientOss {
                            context: component_healthcheck_context(),
                        },
                        plugin: PluginClientOss {
                            context: component_context(),
                        },
                        worker: WorkerClientOss {
                            context: worker_context(),
                        },
                        worker_invoke: WorkerClientOss {
                            context: worker_invoke_context(),
                        },
                    }),
                    file_download: file_download_http_client,
                })
            }
        }
    }
}

pub enum GolemClients {
    Oss(GolemClientsOss),
    Cloud(GolemClientsCloud),
}

pub struct GolemClientsOss {
    pub api_definition: ApiDefinitionClientOss,
    pub api_deployment: ApiDeploymentClientOss,
    pub api_security: ApiSecurityClientOss,
    pub component: ComponentClientOss,
    pub component_healthcheck: HealthCheckClientOss,
    pub plugin: PluginClientOss,
    pub worker: WorkerClientOss,
    pub worker_invoke: WorkerClientOss,
}

pub struct GolemClientsCloud {
    authentication: CloudAuthentication,

    pub account: AccountClientCloud,
    pub account_summary: AccountSummaryClientCloud,
    pub api_certificate: ApiCertificateClientCloud,
    pub api_definition: ApiDefinitionClientCloud,
    pub api_deployment: ApiDeploymentClientCloud,
    pub api_domain: ApiDomainClientCloud,
    pub api_security: ApiSecurityClientCloud,
    pub component: ComponentClientCloud,
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

impl GolemClientsCloud {
    pub fn account_id(&self) -> AccountId {
        self.authentication.account_id()
    }

    pub fn auth_token(&self) -> String {
        self.authentication.0.secret.value.to_string()
    }
}

struct ApplicationContextConfig {
    build_profile: Option<AppBuildProfileName>,
    app_manifest_path: Option<PathBuf>,
    disable_app_manifest_discovery: bool,
    golem_rust_override: RustDependencyOverride,
    wasm_rpc_client_build_offline: bool,
}

#[derive(Default)]
pub struct ApplicationContextState {
    pub silent_init: bool,
    pub skip_up_to_date_checks: bool,
    skip_up_to_date_checks_was_set: bool,
    pub build_steps_filter: HashSet<AppBuildStep>,
    build_steps_filter_was_set: bool,

    app_context: Option<Result<Option<ApplicationContext>, Arc<anyhow::Error>>>,
}

impl ApplicationContextState {
    fn init(&mut self, config: &ApplicationContextConfig) {
        if self.app_context.is_some() {
            return;
        }

        let _log_output = self
            .silent_init
            .then(|| LogOutput::new(Output::TracingDebug));

        let config = ApplicationConfig {
            app_source_mode: {
                match &config.app_manifest_path {
                    Some(path) => ApplicationSourceMode::Explicit(path.clone()),
                    None => {
                        if config.disable_app_manifest_discovery {
                            ApplicationSourceMode::None
                        } else {
                            ApplicationSourceMode::Automatic
                        }
                    }
                }
            },
            skip_up_to_date_checks: self.skip_up_to_date_checks,
            profile: config.build_profile.as_ref().map(|p| p.to_string().into()),
            offline: config.wasm_rpc_client_build_offline,
            steps_filter: self.build_steps_filter.clone(),
            golem_rust_override: config.golem_rust_override.clone(),
        };

        debug!(config = ?config, "Initializing application context");

        self.app_context = Some(ApplicationContext::new(config).map_err(Arc::new))
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
    dependencies: ReplDependencies,
}

impl Default for RibReplState {
    fn default() -> Self {
        Self {
            dependencies: ReplDependencies {
                component_dependencies: vec![],
            },
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
