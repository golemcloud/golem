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

use crate::auth::{Auth, CloudAuthentication};
use crate::cloud::{AccountId, CloudAuthenticationConfig};
use crate::command::GolemCliGlobalFlags;
use crate::config::{
    ClientConfig, HttpClientConfig, NamedProfile, Profile, ProfileKind, ProfileName,
};
use crate::error::HintError;
use crate::model::app_ext::GolemComponentExtensions;
use crate::model::Format;
use anyhow::anyhow;
use golem_client::api::ApiDefinitionClientLive as ApiDefinitionClientOss;
use golem_client::api::ApiDeploymentClientLive as ApiDeploymentClientOss;
use golem_client::api::ApiSecurityClientLive as ApiSecurityClientOss;
use golem_client::api::ComponentClientLive as ComponentClientOss;
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
use golem_templates::model::{ComposableAppGroupName, GuestLanguage};
use golem_templates::ComposableAppTemplate;
use golem_wasm_rpc_stubgen::commands::app::{ApplicationContext, ApplicationSourceMode};
use golem_wasm_rpc_stubgen::log::{set_log_output, LogOutput, Output};
use golem_wasm_rpc_stubgen::model::app::AppBuildStep;
use golem_wasm_rpc_stubgen::model::app::BuildProfileName as AppBuildProfileName;
use golem_wasm_rpc_stubgen::stub::WasmRpcOverride;
use std::collections::{BTreeMap, HashSet};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use tracing::debug;
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

    // Lazy initialized
    clients: tokio::sync::OnceCell<Clients>,
    templates: std::sync::OnceLock<
        BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>>,
    >,

    // Directly mutable
    app_context_state: tokio::sync::RwLock<ApplicationContextState>,
}

impl Context {
    pub fn new(global_flags: &GolemCliGlobalFlags, profile: NamedProfile) -> Self {
        // TODO: handle format override from profile
        let format = global_flags.format.unwrap_or(Format::Text);
        let log_output = match format {
            Format::Json => Output::Stderr,
            Format::Yaml => Output::Stderr,
            Format::Text => Output::Stdout,
        };
        set_log_output(log_output);

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
                wasm_rpc_override: WasmRpcOverride {
                    wasm_rpc_path_override: global_flags.wasm_rpc_path.clone(),
                    wasm_rpc_version_override: global_flags.wasm_rpc_version.clone(),
                },
            },
            http_batch_size: global_flags.http_batch_size.unwrap_or(50),
            clients: tokio::sync::OnceCell::new(),
            templates: std::sync::OnceLock::new(),
            app_context_state: tokio::sync::RwLock::default(),
        }
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub async fn silence_app_context_init(&self) {
        let mut state = self.app_context_state.write().await;
        state.silent_init = true;
    }

    pub fn profile_kind(&self) -> ProfileKind {
        self.profile_kind
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
                Clients::new(
                    ClientConfig::from(&self.profile),
                    None, // TODO: token override
                    &self.profile_name,
                    match &self.profile {
                        Profile::Golem(_) => None,
                        Profile::GolemCloud(profile) => profile.auth.as_ref(),
                    },
                    self.config_dir(),
                )
                .await
            })
            .await
    }

    pub async fn golem_clients(&self) -> anyhow::Result<&GolemClients> {
        Ok(&self.clients().await?.golem)
    }

    pub async fn golem_clients_cloud(&self) -> anyhow::Result<&GolemClientsCloud> {
        match &self.clients().await?.golem {
            GolemClients::Oss(_) => Err(anyhow!(HintError::ExpectedCloudProfile)),
            GolemClients::Cloud(clients) => Ok(clients),
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

    pub fn templates(
        &self,
    ) -> &BTreeMap<GuestLanguage, BTreeMap<ComposableAppGroupName, ComposableAppTemplate>> {
        self.templates
            .get_or_init(golem_templates::all_composable_app_templates)
    }
}

// TODO: add healthcheck clients
pub struct Clients {
    pub golem: GolemClients,
    pub file_download_http_client: reqwest::Client,
}

impl Clients {
    pub async fn new(
        config: ClientConfig,
        token_override: Option<Uuid>,
        profile_name: &ProfileName,
        auth_config: Option<&CloudAuthenticationConfig>,
        config_dir: &Path,
    ) -> anyhow::Result<Self> {
        let service_http_client = new_reqwest_client(&config.service_http_client_config)?;
        let file_download_http_client =
            new_reqwest_client(&config.file_download_http_client_config)?;

        match &config.cloud_url {
            Some(cloud_url) => {
                let auth = Auth {
                    login_client: LoginClientLive {
                        context: ContextCloud {
                            client: service_http_client.clone(),
                            base_url: cloud_url.clone(),
                            security_token: Security::Empty,
                        },
                    },
                };

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
                    }),
                    file_download_http_client,
                })
            }
            None => {
                let component_context = || ContextOss {
                    client: service_http_client.clone(),
                    base_url: config.component_url.clone(),
                };

                let worker_context = || ContextOss {
                    client: service_http_client.clone(),
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
                        plugin: PluginClientOss {
                            context: component_context(),
                        },
                        worker: WorkerClientOss {
                            context: worker_context(),
                        },
                    }),
                    file_download_http_client,
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
    pub plugin: PluginClientOss,
    pub worker: WorkerClientOss,
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
}

impl GolemClientsCloud {
    pub fn account_id(&self) -> AccountId {
        self.authentication.account_id()
    }
}

struct ApplicationContextConfig {
    build_profile: Option<AppBuildProfileName>,
    app_manifest_path: Option<PathBuf>,
    disable_app_manifest_discovery: bool,
    wasm_rpc_override: WasmRpcOverride,
}

#[derive(Default)]
pub struct ApplicationContextState {
    pub silent_init: bool,
    pub skip_up_to_date_checks: bool,
    skip_up_to_date_checks_was_set: bool,
    pub build_steps_filter: HashSet<AppBuildStep>,
    build_steps_filter_was_set: bool,

    app_context: Option<anyhow::Result<Option<ApplicationContext<GolemComponentExtensions>>>>,
}

impl ApplicationContextState {
    fn init(&mut self, config: &ApplicationContextConfig) {
        if self.app_context.is_some() {
            return;
        }

        let _log_output = self
            .silent_init
            .then(|| LogOutput::new(Output::TracingDebug));

        let config = golem_wasm_rpc_stubgen::commands::app::Config {
            app_source_mode: {
                match &config.app_manifest_path {
                    Some(path) => ApplicationSourceMode::Explicit(vec![path.clone()]),
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
            offline: false, // TODO:
            extensions: PhantomData::<GolemComponentExtensions>,
            steps_filter: self.build_steps_filter.clone(),
            wasm_rpc_override: config.wasm_rpc_override.clone(),
        };

        debug!(config = ?config, "Initializing application context");

        self.app_context = Some(ApplicationContext::new(config))
    }

    pub fn opt(&self) -> anyhow::Result<Option<&ApplicationContext<GolemComponentExtensions>>> {
        match &self.app_context {
            Some(Ok(None)) => Ok(None),
            Some(Ok(Some(app_ctx))) => Ok(Some(app_ctx)),
            Some(Err(err)) => Err(anyhow!("{err}")),
            None => panic!("Uninitialized application context"),
        }
    }

    pub fn opt_mut(
        &mut self,
    ) -> anyhow::Result<Option<&mut ApplicationContext<GolemComponentExtensions>>> {
        match &mut self.app_context {
            Some(Ok(None)) => Ok(None),
            Some(Ok(Some(app_ctx))) => Ok(Some(app_ctx)),
            Some(Err(err)) => Err(anyhow!("{err}")),
            None => panic!("Uninitialized application context"),
        }
    }

    pub fn some_or_err(&self) -> anyhow::Result<&ApplicationContext<GolemComponentExtensions>> {
        match &self.app_context {
            Some(Ok(None)) => Err(anyhow!(HintError::NoApplicationManifestFound)),
            Some(Ok(Some(app_ctx))) => Ok(app_ctx),
            Some(Err(err)) => Err(anyhow!("{err}")),
            None => panic!("Uninitialized application context"),
        }
    }

    pub fn some_or_err_mut(
        &mut self,
    ) -> anyhow::Result<&mut ApplicationContext<GolemComponentExtensions>> {
        match &mut self.app_context {
            Some(Ok(None)) => Err(anyhow!(HintError::NoApplicationManifestFound)),
            Some(Ok(Some(app_ctx))) => Ok(app_ctx),
            Some(Err(err)) => Err(anyhow!("{err}")),
            None => panic!("Uninitialized application context"),
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
