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
use super::model::UpdateComponentRequest;
use crate::model::component::{Component, InitialComponentFilesArchiveAndPermissions};
use golem_common::api::Page;
use golem_common::model::auth::AuthCtx;
use golem_common::model::{ComponentId, Revision};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem::Body;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::{Binary, Json};
use tracing::Instrument;
use crate::services::component::ComponentService;
use std::sync::Arc;

pub struct ComponentsApi {
    component_service: Arc<ComponentService>
}

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Component)]
impl ComponentsApi {
    pub fn new(
        component_service: Arc<ComponentService>
    ) -> Self {
        Self {
            component_service
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

        let auth = AuthCtx::new(token.secret());

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
        let component = self.component_service.get_component(&component_id).await?;
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

        let auth = AuthCtx::new(token.secret());

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
        revision: Path<Revision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Component>> {
        let record = recorded_http_api_request!(
            "get_component_revision",
            component_id = component_id.0.to_string(),
            revision = revision.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_component_revision_internal(component_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_component_revision_internal(
        &self,
        component_id: ComponentId,
        revision: Revision,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        let component = self.component_service.get_component_revision(&component_id, revision).await?;
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
        revision: Path<Revision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Binary<Body>> {
        let record = recorded_http_api_request!(
            "get_component_wasm",
            component_id = component_id.0.to_string(),
            revision = revision.0.to_string()
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_component_wasm_internal(component_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_component_wasm_internal(
        &self,
        _component_id: ComponentId,
        _revision: Revision,
        _auth: AuthCtx,
    ) -> ApiResult<Binary<Body>> {
        todo!()
    }

    // /// Update a component
    // ///
    // /// The request body is encoded as multipart/form-data containing metadata and the WASM binary.
    // #[oai(
    //     path = "/:component_id",
    //     method = "patch",
    //     operation_id = "update_component"
    // )]
    // async fn update_component(
    //     &self,
    //     component_id: Path<ComponentId>,
    //     payload: UpdateComponentRequest,
    //     token: GolemSecurityScheme,
    // ) -> ApiResult<Json<Component>> {
    //     let record = recorded_http_api_request!(
    //         "update_component",
    //         component_id = component_id.0.to_string(),
    //     );

    //     let auth = AuthCtx::new(token.secret());

    //     let response = self
    //         .update_component_internal(component_id.0, payload, auth)
    //         .instrument(record.span.clone())
    //         .await;

    //     record.result(response)
    // }

    // async fn update_component_internal(
    //     &self,
    //     component_id: ComponentId,
    //     payload: UpdateComponentRequest,
    //     _auth: AuthCtx,
    // ) -> ApiResult<Json<Component>> {
    //     let data = if let Some(upload) = payload.component {
    //          Some(upload.into_vec().await?)
    //     } else {
    //         None
    //     };

    //     let files_archive = payload.files_archive.map(|f| f.into_file());

    //     let files = files_archive
    //         .zip(payload.files)
    //         .map(
    //             |(archive, permissions)| InitialComponentFilesArchiveAndPermissions {
    //                 archive,
    //                 files: permissions.values,
    //             },
    //         );


    //     let files_archive = payload.files_archive.map(|f| f.into_file());

    //     let response = self
    //         .component_service
    //         .update(
    //             &component_id,
    //             payload.component_type,
    //             data,
    //             files,
    //             payload.dynamic_linking.map(|f| f.0),
    //             &auth,
    //             env,
    //             payload
    //                 .agent_types
    //                 .map(|types| types.0.types)
    //                 .unwrap_or_default(),
    //         )
    //         .await?;

    // }
}
