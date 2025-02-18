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

use crate::cloud::factory::CloudServiceFactory;
use crate::cloud::model::ProjectRef;
use golem_cli::cloud::{AccountId, ProjectId};
use golem_cli::factory::ServiceFactory;
use golem_cli::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult};
use golem_common::uri::cloud::uri::{
    ApiDefinitionUri, ComponentUri, ProjectUri, ResourceUri, ToOssUri, WorkerUri,
};
use golem_common::uri::cloud::url::{ComponentUrl, ProjectUrl, ResourceUrl, WorkerUrl};
use golem_common::uri::cloud::urn::{ComponentUrn, ResourceUrn, WorkerUrn};

pub async fn get_resource_by_uri(
    uri: ResourceUri,
    factory: &CloudServiceFactory,
) -> Result<GolemResult, GolemError> {
    match uri {
        ResourceUri::URN(urn) => get_resource_by_urn(urn, factory).await,
        ResourceUri::URL(url) => get_resource_by_url(url, factory).await,
    }
}

async fn get_resource_by_urn(
    urn: ResourceUrn,
    factory: &CloudServiceFactory,
) -> Result<GolemResult, GolemError> {
    match urn {
        ResourceUrn::Account(a) => {
            factory
                .account_service()
                .get(Some(AccountId { id: a.id.value }))
                .await
        }
        ResourceUrn::Project(p) => {
            factory
                .project_service()
                .as_ref()
                .get(ProjectUri::URN(p))
                .await
        }
        ResourceUrn::Component(c) => {
            let (c, p) = ComponentUri::URN(c).to_oss_uri();
            let p = resolve_project_id(factory, p).await?;

            factory.component_service().as_ref().get(c, None, p).await
        }
        ResourceUrn::ComponentVersion(cv) => {
            let (c, p) = ComponentUri::URN(ComponentUrn { id: cv.id }).to_oss_uri();
            let p = resolve_project_id(factory, p).await?;

            factory
                .component_service()
                .get(c, Some(cv.version), p)
                .await
        }
        ResourceUrn::Worker(w) => {
            let (w, p) = WorkerUri::URN(w).to_oss_uri();
            let p = resolve_project_id(factory, p).await?;

            factory.worker_service().get(w, p).await
        }
        ResourceUrn::WorkerFunction(wf) => {
            let (w, p) = WorkerUri::URN(WorkerUrn {
                id: wf.id.into_target_worker_id(),
            })
            .to_oss_uri();
            let p = resolve_project_id(factory, p).await?;

            factory
                .worker_service()
                .get_function(w, &wf.function, p)
                .await
        }
        ResourceUrn::ApiDefinition(d) => {
            let (_, p) = ApiDefinitionUri::URN(d.clone()).to_oss_uri();
            let p = factory
                .project_service()
                .resolve_urn_or_default(ProjectRef {
                    uri: p.map(ProjectUri::URL),
                    explicit_name: false,
                })
                .await?;
            let p = ProjectId(p.id.0);

            factory
                .api_definition_service()
                .get(ApiDefinitionId(d.id), ApiDefinitionVersion(d.version), &p)
                .await
        }
        ResourceUrn::ApiDeployment(d) => factory.api_deployment_service().get(d.site).await,
    }
}

async fn get_resource_by_url(
    url: ResourceUrl,
    factory: &CloudServiceFactory,
) -> Result<GolemResult, GolemError> {
    match url {
        ResourceUrl::Account(a) => {
            factory
                .account_service()
                .as_ref()
                .get(Some(AccountId { id: a.name }))
                .await
        }
        ResourceUrl::Project(p) => {
            factory
                .project_service()
                .as_ref()
                .get(ProjectUri::URL(p))
                .await
        }
        ResourceUrl::Component(c) => {
            let (c, p) = ComponentUri::URL(c).to_oss_uri();
            let p = resolve_project_id(factory, p).await?;

            factory.component_service().as_ref().get(c, None, p).await
        }
        ResourceUrl::ComponentVersion(cv) => {
            let (c, p) = ComponentUri::URL(ComponentUrl {
                name: cv.name.clone(),
                project: cv.project.clone(),
            })
            .to_oss_uri();
            let p = resolve_project_id(factory, p).await?;

            factory
                .component_service()
                .as_ref()
                .get(c, Some(cv.version), p)
                .await
        }
        ResourceUrl::Worker(w) => {
            let (w, p) = WorkerUri::URL(w).to_oss_uri();
            let p = resolve_project_id(factory, p).await?;

            factory.worker_service().get(w, p).await
        }
        ResourceUrl::WorkerFunction(wf) => {
            let (w, p) = WorkerUri::URL(WorkerUrl {
                component_name: wf.component_name.clone(),
                worker_name: Some(wf.worker_name.clone()),
                project: wf.project.clone(),
            })
            .to_oss_uri();
            let p = resolve_project_id(factory, p).await?;

            factory
                .worker_service()
                .get_function(w, &wf.function, p)
                .await
        }
        ResourceUrl::ApiDefinition(d) => {
            let (_, p) = ApiDefinitionUri::URL(d.clone()).to_oss_uri();
            let p = factory
                .project_service()
                .resolve_urn_or_default(ProjectRef {
                    uri: p.map(ProjectUri::URL),
                    explicit_name: false,
                })
                .await?;
            let p = ProjectId(p.id.0);

            factory
                .api_definition_service()
                .get(ApiDefinitionId(d.name), ApiDefinitionVersion(d.version), &p)
                .await
        }
        ResourceUrl::ApiDeployment(d) => factory.api_deployment_service().get(d.site).await,
    }
}

async fn resolve_project_id(
    factory: &CloudServiceFactory,
    p: Option<ProjectUrl>,
) -> Result<Option<ProjectId>, GolemError> {
    Ok(factory
        .project_service()
        .resolve_urn(ProjectRef {
            uri: p.map(ProjectUri::URL),
            explicit_name: false,
        })
        .await?
        .map(|p| ProjectId(p.id.0)))
}
