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
use golem_common::api::Page;
use golem_common::api::certificate::{CertificateResponseView, UpdateCertificateRequest};
use golem_common::model::auth::AuthCtx;
use golem_common::model::certificate::CertificateId;
use golem_common::recorded_http_api_request;
use golem_service_base::api_tags::ApiTags;
use golem_service_base::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;

pub struct CertificatesApi {}

#[OpenApi(prefix_path = "/v1/certificates", tag = ApiTags::ApiCertificate)]
impl CertificatesApi {
    /// Get a certificate by id
    #[oai(
        path = "/:certificate_id",
        method = "get",
        operation_id = "get_certificate"
    )]
    async fn get_certificate(
        &self,
        certificate_id: Path<CertificateId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<CertificateResponseView>> {
        let record = recorded_http_api_request!(
            "get_certificate",
            certificate_id = certificate_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_certificate_internal(certificate_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_certificate_internal(
        &self,
        _certificate_id: CertificateId,
        _token: AuthCtx,
    ) -> ApiResult<Json<CertificateResponseView>> {
        todo!()
    }

    /// Get all revisions of a certificate
    #[oai(
        path = "/:certificate_id/revisions",
        method = "get",
        operation_id = "get_certificate_revisions"
    )]
    async fn get_certificate_revisions(
        &self,
        certificate_id: Path<CertificateId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<CertificateResponseView>>> {
        let record = recorded_http_api_request!(
            "get_certificate_revisions",
            certificate_id = certificate_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_certificate_revisions_internal(certificate_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_certificate_revisions_internal(
        &self,
        _certificate_id: CertificateId,
        _token: AuthCtx,
    ) -> ApiResult<Json<Page<CertificateResponseView>>> {
        todo!()
    }

    /// Update a certificate
    #[oai(
        path = "/:certificate_id",
        method = "patch",
        operation_id = "update_certificate"
    )]
    async fn update_certificate(
        &self,
        certificate_id: Path<CertificateId>,
        payload: Json<UpdateCertificateRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<CertificateResponseView>> {
        let record = recorded_http_api_request!(
            "update_certificate",
            certificate_id = certificate_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .update_certificate_internal(certificate_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn update_certificate_internal(
        &self,
        _certificate_id: CertificateId,
        _payload: UpdateCertificateRequest,
        _token: AuthCtx,
    ) -> ApiResult<Json<CertificateResponseView>> {
        todo!()
    }
}
