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

pub mod debugging;
pub mod errors;

use self::debugging::DebuggingApi;
use crate::debug_context::DebugContext;
use crate::services::debug_service::DebugServiceDefault;
use golem_service_base::api::HealthcheckApi;
use golem_worker_executor::services::{All, HasExtraDeps};
use poem_openapi::OpenApiService;
use std::sync::Arc;

pub type Apis = (HealthcheckApi, DebuggingApi);

pub fn make_open_api_service(services: &All<DebugContext>) -> OpenApiService<Apis, ()> {
    OpenApiService::new(
        (
            HealthcheckApi,
            // TODO: DebugService should be part of DebugConectx::ExtraDeps, but currently not possible as it causes
            // issues with cyclic wiring of All<DebugContext>
            DebuggingApi::new(
                Arc::new(DebugServiceDefault::new(services.clone())),
                services.extra_deps().auth_service(),
            ),
        ),
        "Golem API",
        "1.0",
    )
}
