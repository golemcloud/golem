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
use golem_common::api::Page;
use golem_common::model::auth::AuthCtx;
use golem_common::model::component::Component;
use golem_common::model::{ComponentId, Revision};
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem::Body;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::{Binary, Json};
use tracing::Instrument;

pub struct ComponentsApi {}

#[OpenApi(prefix_path = "/v1/components", tag = ApiTags::Component)]
impl ComponentsApi {
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
        _component_id: ComponentId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        todo!()
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

        let auth = AuthCtx::new(token.secret());

        let response = self
            .update_component_internal(component_id.0, payload, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_component_internal(
        &self,
        _component_id: ComponentId,
        _payload: UpdateComponentRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Component>> {
        todo!()
    }

    /// Get the component wasm binary
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
}
