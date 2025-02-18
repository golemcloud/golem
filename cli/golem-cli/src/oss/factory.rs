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

use crate::clients::api_definition::ApiDefinitionClient;
use crate::clients::api_deployment::ApiDeploymentClient;
use crate::clients::api_security::ApiSecurityClient;
use crate::clients::component::ComponentClient;
use crate::clients::file_download;
use crate::clients::health_check::HealthCheckClient;
use crate::clients::plugin::PluginClient;
use crate::clients::worker::WorkerClient;
use crate::config::{HttpClientConfig, OssProfile};
use crate::factory::ServiceFactory;
use crate::model::GolemError;
use crate::oss::clients::api_definition::ApiDefinitionClientLive;
use crate::oss::clients::api_deployment::ApiDeploymentClientLive;
use crate::oss::clients::api_security::ApiSecurityClientLive;
use crate::oss::clients::component::ComponentClientLive;
use crate::oss::clients::health_check::HealthCheckClientLive;
use crate::oss::clients::plugin::PluginClientLive;
use crate::oss::clients::worker::WorkerClientLive;
use crate::oss::model::OssContext;
use crate::service::project::{ProjectResolver, ProjectResolverOss};
use golem_client::model::{
    PluginDefinitionDefaultPluginOwnerDefaultPluginScope,
    PluginDefinitionWithoutOwnerDefaultPluginScope,
};
use golem_client::Context;
use golem_common::model::plugin::DefaultPluginScope;
use itertools::Itertools;
use std::sync::Arc;
use tracing::warn;
use url::Url;

#[derive(Debug, Clone)]
pub struct OssServiceFactoryConfig {
    pub component_url: Url,
    pub worker_url: Url,
    pub service_http_client_config: HttpClientConfig,
    pub health_check_http_client_config: HttpClientConfig,
    pub file_download_http_client_config: HttpClientConfig,
    pub allow_insecure: bool,
}

#[derive(Debug, Clone)]
pub struct OssServiceFactory {
    config: OssServiceFactoryConfig,
    http_client_service: reqwest::Client,
    http_client_health_check: reqwest::Client,
    http_client_file_download: reqwest::Client,
}

impl OssServiceFactory {
    pub fn new(config: OssServiceFactoryConfig) -> Result<Self, GolemError> {
        let service_http_client = make_reqwest_client(&config.service_http_client_config)?;
        let healthcheck_http_client = make_reqwest_client(&config.health_check_http_client_config)?;
        let file_download_http_client =
            make_reqwest_client(&config.file_download_http_client_config)?;

        Ok(Self {
            config,
            http_client_service: service_http_client,
            http_client_health_check: healthcheck_http_client,
            http_client_file_download: file_download_http_client,
        })
    }

    pub fn from_profile(profile: &OssProfile) -> Result<Self, GolemError> {
        let component_url = profile.url.clone();
        let worker_url = profile
            .worker_url
            .clone()
            .unwrap_or_else(|| component_url.clone());
        let allow_insecure = profile.allow_insecure;

        OssServiceFactory::new(OssServiceFactoryConfig {
            component_url,
            worker_url,
            service_http_client_config: HttpClientConfig::new_for_service_calls(allow_insecure),
            health_check_http_client_config: HttpClientConfig::new_for_health_check(allow_insecure),
            file_download_http_client_config: HttpClientConfig::new_for_file_download(
                allow_insecure,
            ),
            allow_insecure,
        })
    }

    fn component_context(&self) -> Context {
        Context {
            client: self.http_client_service.clone(),
            base_url: self.config.component_url.clone(),
        }
    }

    fn component_context_health_check(&self) -> Context {
        Context {
            client: self.http_client_health_check.clone(),
            base_url: self.config.component_url.clone(),
        }
    }

    fn worker_context(&self) -> Context {
        Context {
            client: self.http_client_service.clone(),
            base_url: self.config.worker_url.clone(),
        }
    }

    fn worker_context_health_check(&self) -> Context {
        Context {
            client: self.http_client_health_check.clone(),
            base_url: self.config.worker_url.clone(),
        }
    }
}

impl ServiceFactory for OssServiceFactory {
    type ProjectRef = OssContext;
    type ProjectContext = OssContext;
    type PluginDefinition = PluginDefinitionDefaultPluginOwnerDefaultPluginScope;
    type PluginDefinitionWithoutOwner = PluginDefinitionWithoutOwnerDefaultPluginScope;
    type PluginScope = DefaultPluginScope;

    fn project_resolver(
        &self,
    ) -> Arc<dyn ProjectResolver<Self::ProjectRef, Self::ProjectContext> + Send + Sync> {
        Arc::new(ProjectResolverOss::DUMMY)
    }

    fn file_download_client(&self) -> Box<dyn file_download::FileDownloadClient + Send + Sync> {
        Box::new(file_download::FileDownloadClientLive {
            client: self.http_client_file_download.clone(),
        })
    }

    fn component_client(
        &self,
    ) -> Box<dyn ComponentClient<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Box::new(ComponentClientLive {
            client: golem_client::api::ComponentClientLive {
                context: self.component_context(),
            },
        })
    }

    fn worker_client(&self) -> Arc<dyn WorkerClient + Send + Sync> {
        Arc::new(WorkerClientLive {
            client: golem_client::api::WorkerClientLive {
                context: self.worker_context(),
            },
            context: self.worker_context(),
            allow_insecure: self.config.allow_insecure,
        })
    }

    fn api_definition_client(
        &self,
    ) -> Box<dyn ApiDefinitionClient<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Box::new(ApiDefinitionClientLive {
            client: golem_client::api::ApiDefinitionClientLive {
                context: self.worker_context(),
            },
        })
    }

    fn api_deployment_client(
        &self,
    ) -> Box<dyn ApiDeploymentClient<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Box::new(ApiDeploymentClientLive {
            client: golem_client::api::ApiDeploymentClientLive {
                context: self.worker_context(),
            },
        })
    }

    fn api_security_scheme_client(
        &self,
    ) -> Box<dyn ApiSecurityClient<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Box::new(ApiSecurityClientLive {
            client: golem_client::api::ApiSecurityClientLive {
                context: self.worker_context(),
            },
        })
    }

    fn health_check_clients(&self) -> Vec<Arc<dyn HealthCheckClient + Send + Sync>> {
        let contexts = vec![
            self.component_context_health_check(),
            self.worker_context_health_check(),
        ];

        let contexts_count = contexts.len();

        let unique_contexts: Vec<_> = contexts
            .iter()
            .unique_by(|context| context.base_url.clone())
            .collect();

        if contexts_count != unique_contexts.len() {
            warn!(
                "Health check client contexts are not unique, contexts count: {}, unique count: {}",
                contexts_count,
                unique_contexts.len()
            )
        }

        contexts
            .into_iter()
            .map(|context| -> Arc<dyn HealthCheckClient + Send + Sync> {
                Arc::new(HealthCheckClientLive {
                    client: golem_client::api::HealthCheckClientLive { context },
                })
            })
            .collect()
    }

    fn plugin_client(
        &self,
    ) -> Arc<
        dyn PluginClient<
                PluginDefinition = Self::PluginDefinition,
                PluginDefinitionWithoutOwner = Self::PluginDefinitionWithoutOwner,
                PluginScope = Self::PluginScope,
                ProjectContext = Self::ProjectContext,
            > + Send
            + Sync,
    > {
        Arc::new(PluginClientLive {
            client: golem_client::api::PluginClientLive {
                context: self.worker_context(),
            },
        })
    }
}

pub fn make_reqwest_client(config: &HttpClientConfig) -> Result<reqwest::Client, GolemError> {
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
