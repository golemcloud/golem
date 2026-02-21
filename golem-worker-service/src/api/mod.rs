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

pub mod agents;
pub mod common;
mod worker;

use crate::api::agents::AgentsApi;
use crate::api::worker::WorkerApi;
use crate::bootstrap::Services;
use golem_service_base::api::HealthcheckApi;
use poem_openapi::OpenApiService;

pub type Apis = (HealthcheckApi, WorkerApi, AgentsApi);

pub fn make_open_api_service(services: &Services) -> OpenApiService<Apis, ()> {
    OpenApiService::new(
        (
            HealthcheckApi,
            WorkerApi::new(
                services.component_service.clone(),
                services.worker_service.clone(),
                services.auth_service.clone(),
            ),
            AgentsApi::new(
                services.agents_service.clone(),
                services.auth_service.clone(),
            ),
        ),
        "Golem API",
        "1.0",
    )
}
