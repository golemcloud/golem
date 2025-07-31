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

use futures::{stream, StreamExt, TryStreamExt};
use golem_common_next::model::agent::AgentTypes;
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::model::component::VersionedComponentId;
use golem_common_next::model::error::{ErrorBody, ErrorsBody};
use golem_common_next::model::plugin::{PluginInstallationCreation, PluginInstallationUpdate};
use golem_common_next::model::{
    ComponentFilePathWithPermissionsList, Empty, PluginInstallationId, ProjectId,
};
use golem_common_next::model::{ComponentId, ComponentType};
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use golem_service_base_next::model::{BatchPluginInstallationUpdates};
use golem_service_base_next::poem::TempFileUpload;
use poem::Body;
use poem_openapi::param::{Path, Query};
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::*;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::Instrument;
use super::ApiResult;

pub struct EnvironmentComponentsApi { }

#[OpenApi(prefix_path = "/v1/envs/{environment_id}/components", tag = ApiTags::Component)]
impl EnvironmentComponentsApi {
    /// Create a new component
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    /// If the component type is not specified, it will be considered as a `Durable` component.
    #[oai(path = "/", method = "post", operation_id = "create_component")]
    async fn create_component(
        &self,
        payload: CreateComponentRequest,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let auth = AuthCtx::new(token.secret());
        let record = recorded_http_api_request!(
            "create_component",
            component_name = payload.query.0.component_name.to_string(),
            project_id = payload.query.0.project_id.as_ref().map(|v| v.to_string()),
        );
        let response = self
            .create_component_internal(payload, auth)
            .instrument(record.span.clone())
            .await;
        record.result(response)
    }

    async fn create_component_internal(
        &self,
        payload: UploadPayload,
        auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        todo!()
    }
}
