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

use crate::api::common::{ApiEndpointError, ApiTags};
use crate::model::{ApiDomain, DomainRequest};
use crate::service::api_domain::ApiDomainService;
use golem_common::model::auth::AuthCtx;
use golem_common::model::ProjectId;
use golem_common::recorded_http_api_request;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::param::Query;
use poem_openapi::payload::Json;
use poem_openapi::*;
use std::sync::Arc;
use tracing::Instrument;

pub struct ApiDomainApi {
    domain_service: Arc<dyn ApiDomainService + Sync + Send>,
}

#[OpenApi(prefix_path = "/v1/api/domains", tag = ApiTags::ApiDomain)]
impl ApiDomainApi {
    pub fn new(domain_service: Arc<dyn ApiDomainService + Sync + Send>) -> Self {
        Self { domain_service }
    }

    /// Create or update an API domain
    #[oai(path = "/", method = "put", operation_id = "create_or_update_domain")]
    async fn create_or_update(
        &self,
        payload: Json<DomainRequest>,
        token: GolemSecurityScheme,
    ) -> Result<Json<ApiDomain>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!(
            "create_or_update_domain",
            domain_name = payload.0.domain_name.to_string(),
            project_id = payload.0.project_id.to_string()
        );
        let response = self
            .domain_service
            .create_or_update(&payload.0, &AuthCtx::new(token))
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(|err| err.into());

        record.result(response)
    }

    /// Get all API domains
    ///
    /// Returns a list of API domains for the given project.
    #[oai(path = "/", method = "get", operation_id = "get_domains")]
    async fn get(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        token: GolemSecurityScheme,
    ) -> Result<Json<Vec<ApiDomain>>, ApiEndpointError> {
        let token = token.secret();
        let record =
            recorded_http_api_request!("get_domains", project_id = project_id_query.0.to_string());
        let response = self
            .domain_service
            .get(&project_id_query.0, &AuthCtx::new(token))
            .instrument(record.span.clone())
            .await
            .map(Json)
            .map_err(|err| err.into());

        record.result(response)
    }

    /// Delete an API domain
    #[oai(path = "/", method = "delete", operation_id = "delete_domain")]
    async fn delete(
        &self,
        #[oai(name = "project-id")] project_id_query: Query<ProjectId>,
        #[oai(name = "domain")] domain_query: Query<String>,
        token: GolemSecurityScheme,
    ) -> Result<Json<String>, ApiEndpointError> {
        let token = token.secret();
        let record = recorded_http_api_request!(
            "delete_domain",
            domain_name = domain_query.0,
            project_id = project_id_query.0.to_string()
        );
        let response = self
            .domain_service
            .delete(&project_id_query.0, &domain_query.0, &AuthCtx::new(token))
            .instrument(record.span.clone())
            .await
            .map(|_| Json("API domain deleted".to_string()))
            .map_err(|err| err.into());

        record.result(response)
    }
}
