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

use poem_openapi::OpenApiService;

// This is only a very temporary solution (after trying a few open-api and swagger merges)
// Since the research work is taking time, this hack is going to exist until we find a formal solution
// TODO; Revisit and remove this file
type ApiServices = (
    golem_template_service::api::template::TemplateApi,
    golem_worker_service::api::worker::WorkerApi,
    golem_worker_service::api::api_definition_endpoints::RegisterApiDefinitionApi,
    golem_worker_service::api::healthcheck::HealthcheckApi
);
#[tokio::main]
async fn main() {
    let worker_services =  golem_worker_service::service::Services::noop();
    let template_services = golem_template_service::service::Services::noop();
    let api_service = make_open_api_service(&worker_services, &template_services);
    println!("{}", api_service.spec_yaml())
}

pub fn make_open_api_service(worker_services: &golem_worker_service::service::Services, template_services: &golem_template_service::service::Services) -> OpenApiService<ApiServices, ()> {
    OpenApiService::new(
        (
            golem_template_service::api::template::TemplateApi {
                template_service: template_services.template_service.clone(),
            },
            golem_worker_service::api::worker::WorkerApi {
                template_service: worker_services.template_service.clone(),
                worker_service: worker_services.worker_service.clone(),
            },
            golem_worker_service::api::api_definition_endpoints::RegisterApiDefinitionApi::new(
                worker_services.definition_service.clone(),
            ),
            golem_worker_service::api::healthcheck::HealthcheckApi,
        ),
        "Golem API",
        "2.0",
    )
}