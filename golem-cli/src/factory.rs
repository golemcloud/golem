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
use crate::model::GolemError;
use crate::service::api_definition::{ApiDefinitionService, ApiDefinitionServiceLive};
use crate::service::api_deployment::{ApiDeploymentService, ApiDeploymentServiceLive};
use crate::service::component::{ComponentService, ComponentServiceLive};
use crate::service::version::{VersionService, VersionServiceLive};
use crate::service::worker::{
    ComponentServiceBuilder, WorkerClientBuilder, WorkerService, WorkerServiceLive,
};
use std::fmt::Display;

pub trait ServiceFactory {
    type SecurityContext: Clone + Send + Sync + 'static;
    type ProjectContext: Display + Send + Sync + 'static;

    fn with_auth(
        &self,
        auth: &Self::SecurityContext,
    ) -> FactoryWithAuth<Self::ProjectContext, Self::SecurityContext>;
    fn component_client(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ComponentClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    >;
    fn component_service(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ComponentService<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ComponentServiceLive {
            client: self.component_client(auth)?,
        }))
    }
    fn worker_client(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<Box<dyn WorkerClient + Send + Sync>, GolemError>;
    fn worker_service(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn WorkerService<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    >
    where
        Self: Send + Sync + Sized + 'static,
    {
        Ok(Box::new(WorkerServiceLive {
            client: self.worker_client(auth)?,
            components: self.component_service(auth)?,
            client_builder: Box::new(self.with_auth(auth)),
            component_service_builder: Box::new(self.with_auth(auth)),
        }))
    }
    fn api_definition_client(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ApiDefinitionClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    >;
    fn api_definition_service(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ApiDefinitionService<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ApiDefinitionServiceLive {
            client: self.api_definition_client(auth)?,
        }))
    }
    fn api_deployment_client(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ApiDeploymentClient<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    >;
    fn api_deployment_service(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<
        Box<dyn ApiDeploymentService<ProjectContext = Self::ProjectContext> + Send + Sync>,
        GolemError,
    > {
        Ok(Box::new(ApiDeploymentServiceLive {
            client: self.api_deployment_client(auth)?,
        }))
    }

    fn health_check_clients(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<Vec<Box<dyn HealthCheckClient + Send + Sync>>, GolemError>;
    fn version_service(
        &self,
        auth: &Self::SecurityContext,
    ) -> Result<Box<dyn VersionService + Send + Sync>, GolemError> {
        Ok(Box::new(VersionServiceLive {
            clients: self.health_check_clients(auth)?,
        }))
    }
}

pub struct FactoryWithAuth<
    PC: Send + Sync + 'static,
    SecurityContext: Clone + Send + Sync + 'static,
> {
    pub auth: SecurityContext,
    pub factory: Box<
        dyn ServiceFactory<ProjectContext = PC, SecurityContext = SecurityContext> + Send + Sync,
    >,
}

impl<PC: Display + Send + Sync, S: Clone + Send + Sync> WorkerClientBuilder
    for FactoryWithAuth<PC, S>
{
    fn build(&self) -> Result<Box<dyn WorkerClient + Send + Sync>, GolemError> {
        self.factory.worker_client(&self.auth)
    }
}

impl<PC: Display + Send + Sync, S: Clone + Send + Sync> ComponentServiceBuilder<PC>
    for FactoryWithAuth<PC, S>
{
    fn build(
        &self,
    ) -> Result<Box<dyn ComponentService<ProjectContext = PC> + Send + Sync>, GolemError> {
        self.factory.component_service(&self.auth)
    }
}
