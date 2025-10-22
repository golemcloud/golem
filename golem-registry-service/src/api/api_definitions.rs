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
use golem_service_base::model::auth::AuthCtx;
use crate::services::auth::AuthService;
use golem_common::api::Page;
use golem_common::api::api_definition::{
    HttpApiDefinitionResponseView, UpdateHttpApiDefinitionRequest,
};
use golem_common::model::api_definition::ApiDefinitionId;
use golem_common::model::api_definition::ApiDefinitionRevision;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ApiDefinitionsApi {
    auth_service: Arc<AuthService>,
}

#[OpenApi(
    prefix_path = "/v1/api-definitions",
    tag = ApiTags::RegistryService,
    tag = ApiTags::ApiDefinition
)]
impl ApiDefinitionsApi {
    pub fn new(auth_service: Arc<AuthService>) -> Self {
        Self { auth_service }
    }

    /// Get api-definition by id
    #[oai(
        path = "/:api_definition_id",
        method = "get",
        operation_id = "get_api_definition"
    )]
    async fn get_api_definition(
        &self,
        api_definition_id: Path<ApiDefinitionId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        let record = recorded_http_api_request!(
            "get_api_definition",
            api_definition_id = api_definition_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_api_definition_internal(api_definition_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_api_definition_internal(
        &self,
        _api_definition_id: ApiDefinitionId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        todo!()
    }

    /// Get revisions of the api definition
    #[oai(
        path = "/:api_definition_id/revisions",
        method = "get",
        operation_id = "get_api_definition_revisions"
    )]
    async fn get_api_definition_revisions(
        &self,
        api_definition_id: Path<ApiDefinitionId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<HttpApiDefinitionResponseView>>> {
        let record = recorded_http_api_request!(
            "get_api_definition_revisions",
            api_definition_id = api_definition_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_api_definition_revisions_internal(api_definition_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_api_definition_revisions_internal(
        &self,
        _api_definition_id: ApiDefinitionId,
        _auth: AuthCtx,
    ) -> ApiResult<Json<Page<HttpApiDefinitionResponseView>>> {
        todo!()
    }

    /// Get specific revision of an api definition
    #[oai(
        path = "/:api_definition_id/revisions/:revision",
        method = "get",
        operation_id = "get_api_definition_revision"
    )]
    async fn get_api_definition_revision(
        &self,
        api_definition_id: Path<ApiDefinitionId>,
        revision: Path<ApiDefinitionRevision>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        let record = recorded_http_api_request!(
            "get_api_definition_revisions",
            api_definition_id = api_definition_id.0.to_string(),
            revision = revision.0.0
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .get_api_definition_revision_internal(api_definition_id.0, revision.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_api_definition_revision_internal(
        &self,
        _api_definition_id: ApiDefinitionId,
        _revision: ApiDefinitionRevision,
        _auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        todo!()
    }

    /// update api-definition
    #[oai(
        path = "/:api_definition_id",
        method = "patch",
        operation_id = "update_api_definition"
    )]
    async fn update_api_definition(
        &self,
        api_definition_id: Path<ApiDefinitionId>,
        payload: Json<UpdateHttpApiDefinitionRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        let record = recorded_http_api_request!(
            "update_api_definition",
            api_definition_id = api_definition_id.0.to_string(),
        );

        let auth = self.auth_service.authenticate_token(token.secret()).await?;

        let response = self
            .update_api_definition_internal(api_definition_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_api_definition_internal(
        &self,
        _api_definition_id: ApiDefinitionId,
        _payload: UpdateHttpApiDefinitionRequest,
        _auth: AuthCtx,
    ) -> ApiResult<Json<HttpApiDefinitionResponseView>> {
        todo!()
    }
}
