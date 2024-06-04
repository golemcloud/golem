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

use crate::clients::api_definition::ApiDefinitionClient;
use crate::clients::api_deployment::ApiDeploymentClient;
use crate::clients::component::ComponentClient;
use crate::clients::health_check::HealthCheckClient;
use crate::clients::worker::WorkerClient;
use crate::factory::{FactoryWithAuth, ServiceFactory};
use crate::model::GolemError;
use crate::oss::clients::api_definition::ApiDefinitionClientLive;
use crate::oss::clients::api_deployment::ApiDeploymentClientLive;
use crate::oss::clients::component::ComponentClientLive;
use crate::oss::clients::health_check::HealthCheckClientLive;
use crate::oss::clients::worker::WorkerClientLive;
use crate::oss::model::OssContext;
use golem_client::Context;
use url::Url;

#[derive(Debug, Clone)]
pub struct OssServiceFactory {
    pub component_url: Url,
    pub worker_url: Url,
    pub allow_insecure: bool,
}

impl OssServiceFactory {
    fn client(&self) -> Result<reqwest::Client, GolemError> {
        let mut builder = reqwest::Client::builder();
        if self.allow_insecure {
            builder = builder.danger_accept_invalid_certs(true);
        }

        Ok(builder.connection_verbose(true).build()?)
    }

    fn component_context(&self) -> Result<Context, GolemError> {
        Ok(Context {
            base_url: self.component_url.clone(),
            client: self.client()?,
        })
    }
    fn worker_context(&self) -> Result<Context, GolemError> {
        Ok(Context {
            base_url: self.worker_url.clone(),
            client: self.client()?,
        })
    }
}

impl ServiceFactory for OssServiceFactory {
    type SecurityContext = OssContext;
    type ProjectContext = OssContext;

    fn with_auth(
        &self,
        auth: &Self::SecurityContext,
    ) -> FactoryWithAuth<Self::ProjectContext, Self::SecurityContext> {
        FactoryWithAuth {
            auth: *auth,
            factory: Box::new(self.clone()),
        }
    }

    fn component_client(
        &self,
        _auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ComponentClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ComponentClientLive {
            client: golem_client::api::ComponentClientLive {
                context: self.component_context()?,
            },
        }))
    }

    fn worker_client(
        &self,
        _auth: &Self::SecurityContext,
    ) -> Result<Box<dyn WorkerClient + Send + Sync>, GolemError> {
        Ok(Box::new(WorkerClientLive {
            client: golem_client::api::WorkerClientLive {
                context: self.worker_context()?,
            },
            context: self.worker_context()?,
            allow_insecure: self.allow_insecure,
        }))
    }

    fn api_definition_client(
        &self,
        _auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ApiDefinitionClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ApiDefinitionClientLive {
            client: golem_client::api::ApiDefinitionClientLive {
                context: self.worker_context()?,
            },
        }))
    }

    fn api_deployment_client(
        &self,
        _auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ApiDeploymentClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ApiDeploymentClientLive {
            client: golem_client::api::ApiDeploymentClientLive {
                context: self.worker_context()?,
            },
        }))
    }

    fn health_check_clients(
        &self,
        _auth: &Self::SecurityContext,
    ) -> Result<Vec<Box<dyn HealthCheckClient + Send + Sync>>, GolemError> {
        Ok(vec![
            Box::new(HealthCheckClientLive {
                client: golem_client::api::HealthCheckClientLive {
                    context: self.component_context()?,
                },
            }),
            Box::new(HealthCheckClientLive {
                client: golem_client::api::HealthCheckClientLive {
                    context: self.worker_context()?,
                },
            }),
        ])
    }
}
