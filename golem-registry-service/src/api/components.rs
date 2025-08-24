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

use super::ApiResult;
use crate::model::auth::AuthCtx;
use crate::services::auth::AuthService;
use crate::services::component::ComponentService;
use futures::TryStreamExt;
use golem_common::api::Page;
use golem_common::api::component::UpdateComponentRequestMetadata;
use golem_common::model::ComponentId;
use golem_common::model::component::Component;
use golem_common::model::component::ComponentRevision;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use golem_service_base::poem::TempFileUpload;
use poem::Body;
use poem_openapi::param::Path;
use poem_openapi::payload::{Binary, Json};
use poem_openapi::types::multipart::{JsonField, Upload};
use poem_openapi::{Multipart, OpenApi};
use std::sync::Arc;
use tracing::Instrument;

pub struct ComponentsApi {
    component_service: Arc<ComponentService>,
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/components",
    tag = ApiTags::RegistryService,
    tag = ApiTags::Component
)]
impl ComponentsApi {
    pub fn new(component_service: Arc<ComponentService>, auth_service: Arc<AuthService>) -> Self {
        Self {
            component_service,
            auth_service,
        }
    }

    /// Get a component by id
    #[oai(
        path = "/:component_id",
        method = "get",
        operation_id = "get_component"
    )]
    async fn get_component(
        &self,
        component_id: Path<ComponentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record =
            recorded_http_api_request!("get_component", component_id = component_id.0.to_string());

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_component_internal(component_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_component_internal(
        &self,
        component_id: ComponentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        let component: Component = self
            .component_service
            .get_component(&component_id)
            .await?
            .into();
        Ok(Json(component))
    }

    /// Get all revisions for a component
    #[oai(
        path = "/:component_id/revisions",
        method = "get",
        operation_id = "get_component_revisions"
    )]
    async fn get_component_revisions(
        &self,
        component_id: Path<ComponentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<Component>>> {
        let record = recorded_http_api_request!(
            "get_component_revisions",
            component_id = component_id.0.to_string()
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_component_revisions_internal(component_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_component_revisions_internal(
        &self,
        _component_id: ComponentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<Component>>> {
        todo!()
    }

    /// Get specific revision of a component
    #[oai(
        path = "/:component_id/revisions/:revision",
        method = "get",
        operation_id = "get_component_revision"
    )]
    async fn get_component_revision(
        &self,
        component_id: Path<ComponentId>,
        revision: Path<ComponentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!(
            "get_component_revision",
            component_id = component_id.0.to_string(),
            revision = revision.0.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_component_revision_internal(component_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_component_revision_internal(
        &self,
        component_id: ComponentId,
        revision: ComponentRevision,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        let component: Component = self
            .component_service
            .get_component_revision(&component_id, revision)
            .await?
            .into();

        Ok(Json(component))
    }

    /// Get the component wasm binary of a specific revision
    #[oai(
        path = "/:component_id/revisions/:revision/wasm",
        method = "get",
        operation_id = "get_component_wasm"
    )]
    async fn get_component_wasm(
        &self,
        component_id: Path<ComponentId>,
        revision: Path<ComponentRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Binary<Body>> {
        let record = recorded_http_api_request!(
            "get_component_wasm",
            component_id = component_id.0.to_string(),
            revision = revision.0.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_component_wasm_internal(component_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_component_wasm_internal(
        &self,
        component_id: ComponentId,
        revision: ComponentRevision,
        _auth: AuthCtx,
    ) -> ApiResult<Binary<Body>> {
        let result = self
            .component_service
            .download_component_wasm(&component_id, revision)
            .await?;
        let body =
            Body::from_bytes_stream(result.map_err(|e| std::io::Error::other(e.to_string())));
        Ok(Binary(body))
    }

    /// Update a component
    ///
    /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    #[oai(
        path = "/:component_id",
        method = "patch",
        operation_id = "update_component"
    )]
    async fn update_component(
        &self,
        component_id: Path<ComponentId>,
        payload: UpdateComponentRequest,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!(
            "update_component",
            component_id = component_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_component_internal(component_id.0, payload, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_component_internal(
        &self,
        component_id: ComponentId,
        payload: UpdateComponentRequest,
        auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        let data = if let Some(upload) = payload.new_component_wasm {
            Some(upload.into_vec().await?)
        } else {
            None
        };

        let new_files_archive = payload.new_files.map(|f| f.into_file());

        let metadata = payload.metadata.0;

        let component: Component = self
            .component_service
            .update(
                &component_id,
                metadata.previous_version,
                data,
                metadata.component_type,
                metadata.removed_files.unwrap_or_default(),
                new_files_archive,
                metadata.new_file_options.unwrap_or_default(),
                metadata.dynamic_linking,
                metadata.env,
                metadata.agent_types,
                &auth,
            )
            .await?
            .into();

        Ok(Json(component))
    }
}

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
struct UpdateComponentRequest {
    metadata: JsonField<UpdateComponentRequestMetadata>,
    new_component_wasm: Option<Upload>,
    new_files: Option<TempFileUpload>,
}
