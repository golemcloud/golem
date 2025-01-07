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

use crate::factory::ServiceFactory;
use crate::model::{ApiDefinitionId, ApiDefinitionVersion, GolemError, GolemResult};
use crate::oss::model::OssContext;
use golem_common::uri::oss::uri::ComponentUri;
use golem_common::uri::oss::uri::{ResourceUri, WorkerUri};
use golem_common::uri::oss::url::ResourceUrl;
use golem_common::uri::oss::url::{ComponentUrl, WorkerUrl};
use golem_common::uri::oss::urn::ResourceUrn;
use golem_common::uri::oss::urn::{ComponentUrn, WorkerUrn};
// use crate::factory::OssServiceFactory;
use crate::oss::factory::OssServiceFactory;

async fn get_resource_by_urn(
    urn: ResourceUrn,
    factory: &OssServiceFactory,
) -> Result<GolemResult, GolemError> {
    let ctx = &OssContext::EMPTY;

    match urn {
        ResourceUrn::Component(c) => {
            factory
                .component_service()
                .get(ComponentUri::URN(c), None, None)
                .await
        }
        ResourceUrn::ComponentVersion(c) => {
            factory
                .component_service()
                .get(
                    ComponentUri::URN(ComponentUrn { id: c.id }),
                    Some(c.version),
                    None,
                )
                .await
        }
        ResourceUrn::Worker(w) => factory.worker_service().get(WorkerUri::URN(w), None).await,
        ResourceUrn::WorkerFunction(f) => {
            factory
                .worker_service()
                .get_function(
                    WorkerUri::URN(WorkerUrn {
                        id: f.id.into_target_worker_id(),
                    }),
                    &f.function,
                    None,
                )
                .await
        }
        ResourceUrn::ApiDefinition(ad) => {
            factory
                .api_definition_service()
                .get(
                    ApiDefinitionId(ad.id),
                    ApiDefinitionVersion(ad.version),
                    ctx,
                )
                .await
        }
        ResourceUrn::ApiDeployment(ad) => factory.api_deployment_service().get(ad.site).await,
    }
}

async fn get_resource_by_url(
    url: ResourceUrl,
    factory: &OssServiceFactory,
) -> Result<GolemResult, GolemError> {
    let ctx = &OssContext::EMPTY;

    match url {
        ResourceUrl::Component(c) => {
            factory
                .component_service()
                .get(ComponentUri::URL(c), None, None)
                .await
        }
        ResourceUrl::ComponentVersion(c) => {
            factory
                .component_service()
                .get(
                    ComponentUri::URL(ComponentUrl { name: c.name }),
                    Some(c.version),
                    None,
                )
                .await
        }
        ResourceUrl::Worker(w) => factory.worker_service().get(WorkerUri::URL(w), None).await,
        ResourceUrl::WorkerFunction(f) => {
            factory
                .worker_service()
                .get_function(
                    WorkerUri::URL(WorkerUrl {
                        component_name: f.component_name,
                        worker_name: Some(f.worker_name),
                    }),
                    &f.function,
                    None,
                )
                .await
        }
        ResourceUrl::ApiDefinition(ad) => {
            factory
                .api_definition_service()
                .get(
                    ApiDefinitionId(ad.name),
                    ApiDefinitionVersion(ad.version),
                    ctx,
                )
                .await
        }
        ResourceUrl::ApiDeployment(ad) => factory.api_deployment_service().get(ad.site).await,
    }
}

pub async fn get_resource_by_uri(
    uri: ResourceUri,
    factory: &OssServiceFactory,
) -> Result<GolemResult, GolemError> {
    match uri {
        ResourceUri::URN(urn) => get_resource_by_urn(urn, factory).await,
        ResourceUri::URL(url) => get_resource_by_url(url, factory).await,
    }
}
