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
use crate::clients::component::ComponentClient;
use crate::clients::health_check::HealthCheckClient;
use crate::clients::plugin::PluginClient;
use crate::clients::worker::WorkerClient;
use crate::service::api_definition::{ApiDefinitionService, ApiDefinitionServiceLive};
use crate::service::api_deployment::{ApiDeploymentService, ApiDeploymentServiceLive};
use crate::service::api_security::{ApiSecuritySchemeService, ApiSecuritySchemeServiceLive};
use crate::service::component::{ComponentService, ComponentServiceLive};
use crate::service::deploy::{DeployService, DeployServiceLive};
use crate::service::project::ProjectResolver;
use crate::service::version::{VersionService, VersionServiceLive};
use crate::service::worker::{WorkerService, WorkerServiceLive};
use std::fmt::Display;
use std::sync::Arc;

pub trait ServiceFactory {
    type ProjectRef: Send + Sync + 'static;
    type ProjectContext: Display + Send + Sync + 'static;
    type PluginDefinition: Send + Sync + 'static;
    type PluginDefinitionWithoutOwner: Send + Sync + 'static;
    type PluginScope: Send + Sync + 'static;

    fn project_resolver(
        &self,
    ) -> Arc<dyn ProjectResolver<Self::ProjectRef, Self::ProjectContext> + Send + Sync>;

    fn file_download_client(
        &self,
    ) -> Box<dyn crate::clients::file_download::FileDownloadClient + Send + Sync>;

    fn component_client(
        &self,
    ) -> Box<dyn ComponentClient<ProjectContext = Self::ProjectContext> + Send + Sync>;

    fn component_service(
        &self,
    ) -> Arc<dyn ComponentService<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Arc::new(ComponentServiceLive {
            client: self.component_client(),
            file_download_client: self.file_download_client(),
        })
    }

    fn worker_client(&self) -> Arc<dyn WorkerClient + Send + Sync>;

    fn worker_service(
        &self,
    ) -> Arc<dyn WorkerService<ProjectContext = Self::ProjectContext> + Send + Sync>
    where
        Self: Send + Sync + Sized + 'static,
    {
        Arc::new(WorkerServiceLive {
            client: self.worker_client(),
            components: self.component_service(),
        })
    }

    fn api_definition_client(
        &self,
    ) -> Box<dyn ApiDefinitionClient<ProjectContext = Self::ProjectContext> + Send + Sync>;

    fn api_definition_service(
        &self,
    ) -> Arc<dyn ApiDefinitionService<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Arc::new(ApiDefinitionServiceLive {
            client: self.api_definition_client(),
        })
    }

    fn api_deployment_client(
        &self,
    ) -> Box<dyn ApiDeploymentClient<ProjectContext = Self::ProjectContext> + Send + Sync>;

    fn api_deployment_service(
        &self,
    ) -> Arc<dyn ApiDeploymentService<ProjectContext = Self::ProjectContext> + Send + Sync> {
        Arc::new(ApiDeploymentServiceLive {
            client: self.api_deployment_client(),
        })
    }

    fn api_security_scheme_client(
        &self,
    ) -> Box<
        dyn crate::clients::api_security::ApiSecurityClient<ProjectContext = Self::ProjectContext>
            + Send
            + Sync,
    >;

    fn api_security_scheme_service(
        &self,
    ) -> Arc<dyn ApiSecuritySchemeService<ProjectContext = Self::ProjectContext> + Send + Sync>
    {
        Arc::new(ApiSecuritySchemeServiceLive {
            client: self.api_security_scheme_client(),
        })
    }

    fn health_check_clients(&self) -> Vec<Arc<dyn HealthCheckClient + Send + Sync>>;

    fn version_service(&self) -> Arc<dyn VersionService + Send + Sync> {
        Arc::new(VersionServiceLive {
            clients: self.health_check_clients(),
        })
    }

    fn deploy_service(
        &self,
    ) -> Arc<dyn DeployService<ProjectContext = Self::ProjectContext> + Send + Sync>
    where
        Self: Send + Sync + Sized + 'static,
    {
        Arc::new(DeployServiceLive {
            component_service: self.component_service(),
            worker_service: self.worker_service(),
        })
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
    >;
}
