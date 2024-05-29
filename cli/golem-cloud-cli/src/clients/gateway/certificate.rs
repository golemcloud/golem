// Copyright 2024 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use async_trait::async_trait;
use golem_cloud_worker_client::model::{Certificate, CertificateRequest};
use uuid::Uuid;

use crate::model::{GolemError, ProjectId};

#[async_trait]
pub trait CertificateClient {
    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<&Uuid>,
    ) -> Result<Vec<Certificate>, GolemError>;

    async fn create(&self, certificate: CertificateRequest) -> Result<Certificate, GolemError>;

    async fn delete(
        &self,
        project_id: ProjectId,
        certificate_id: &Uuid,
    ) -> Result<String, GolemError>;
}

pub struct CertificateClientLive<
    C: golem_cloud_worker_client::api::ApiCertificateClient + Sync + Send,
> {
    pub client: C,
}

#[async_trait]
impl<C: golem_cloud_worker_client::api::ApiCertificateClient + Sync + Send> CertificateClient
    for CertificateClientLive<C>
{
    async fn get(
        &self,
        project_id: ProjectId,
        certificate_id: Option<&Uuid>,
    ) -> Result<Vec<Certificate>, GolemError> {
        Ok(self.client.get(&project_id.0, certificate_id).await?)
    }

    async fn create(&self, certificate: CertificateRequest) -> Result<Certificate, GolemError> {
        Ok(self.client.post(&certificate).await?)
    }

    async fn delete(
        &self,
        project_id: ProjectId,
        certificate_id: &Uuid,
    ) -> Result<String, GolemError> {
        Ok(self.client.delete(&project_id.0, certificate_id).await?)
    }
}
