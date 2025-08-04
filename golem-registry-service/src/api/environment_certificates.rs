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
use golem_common_next::api::Page;
use golem_common_next::api::certificate::{CertificateResponseView, CreateCertificateRequest};
use golem_common_next::model::auth::AuthCtx;
use golem_common_next::model::certificate::CertificateName;
use golem_common_next::model::environment::EnvironmentId;
use golem_common_next::recorded_http_api_request;
use golem_service_base_next::api_tags::ApiTags;
use golem_service_base_next::model::auth::GolemSecurityScheme;
use poem_openapi::OpenApi;
use poem_openapi::param::Path;
use poem_openapi::payload::Json;
use tracing::Instrument;

pub struct EnvironmentCertificatesApi {}

#[OpenApi(prefix_path = "/v1/envs", tag = ApiTags::Environment, tag = ApiTags::ApiCertificate)]
impl EnvironmentCertificatesApi {
    /// Get all certificates in this environment
    #[oai(
        path = "/:environment_id/certificates",
        method = "get",
        operation_id = "get_environment_certificates"
    )]
    async fn get_environment_certificates(
        &self,
        environment_id: Path<EnvironmentId>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<Page<CertificateResponseView>>> {
        let record = recorded_http_api_request!(
            "get_environment_certificates",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_certificates_internal(environment_id.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_certificates_internal(
        &self,
        _environment_id: EnvironmentId,
        _token: AuthCtx,
    ) -> ApiResult<Json<Page<CertificateResponseView>>> {
        todo!()
    }

    /// Get a certificate in this environment
    #[oai(
        path = "/:environment_id/certificates/:certificate_name",
        method = "get",
        operation_id = "get_environment_certificate"
    )]
    async fn get_environment_certificate(
        &self,
        environment_id: Path<EnvironmentId>,
        certificate_name: Path<CertificateName>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<CertificateResponseView>> {
        let record = recorded_http_api_request!(
            "get_environment_certificate",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .get_environment_certificate_internal(environment_id.0, certificate_name.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn get_environment_certificate_internal(
        &self,
        _environment_id: EnvironmentId,
        _certificate_name: CertificateName,
        _token: AuthCtx,
    ) -> ApiResult<Json<CertificateResponseView>> {
        todo!()
    }

    /// Creates a new certificate
    ///
    /// A certificate is associated with a given Golem project and domain, and consists of
    /// a key pair.
    ///
    /// The created certificate will be associated with a certificate ID returned by this endpoint.
    #[oai(
        path = "/:environment_id/certificates",
        method = "post",
        operation_id = "create_environment_certificate"
    )]
    async fn create_environment_certificate(
        &self,
        environment_id: Path<EnvironmentId>,
        payload: Json<CreateCertificateRequest>,
        token: GolemSecurityScheme,
    ) -> ApiResult<Json<CertificateResponseView>> {
        let record = recorded_http_api_request!(
            "create_environment_certificate",
            environment_id = environment_id.0.to_string(),
        );

        let auth = AuthCtx::new(token.secret());

        let response = self
            .create_environment_certificate_internal(environment_id.0, payload.0, auth)
            .instrument(record.span.clone())
            .await;

        record.result(response)
    }

    async fn create_environment_certificate_internal(
        &self,
        _environment_id: EnvironmentId,
        _payload: CreateCertificateRequest,
        _token: AuthCtx,
    ) -> ApiResult<Json<CertificateResponseView>> {
        todo!()
    }
}
