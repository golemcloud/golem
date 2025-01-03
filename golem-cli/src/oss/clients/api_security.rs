// Copyright 2024-2025 Golem Cloud
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

use crate::clients::api_security::ApiSecurityClient;
use crate::model::{ApiSecurityScheme, GolemError};
use crate::oss::model::OssContext;
use golem_client::model::{Provider, SecuritySchemeData};
use tracing::info;

#[derive(Clone)]
pub struct ApiSecurityClientLive<C: golem_client::api::ApiSecurityClient + Sync + Send> {
    pub client: C,
}

#[async_trait]
impl<C: golem_client::api::ApiSecurityClient + Sync + Send> ApiSecurityClient
    for ApiSecurityClientLive<C>
{
    type ProjectContext = OssContext;

    async fn create(
        &self,
        id: String,
        provider_type: Provider,
        client_id: String,
        client_secret: String,
        scopes: Vec<String>,
        redirect_url: String,
        _project: &Self::ProjectContext,
    ) -> Result<ApiSecurityScheme, GolemError> {
        info!("Creating security scheme {}", id);

        let result = self
            .client
            .create(&SecuritySchemeData {
                scheme_identifier: id,
                provider_type,
                client_id,
                client_secret,
                scopes,
                redirect_url,
            })
            .await
            .map_err(GolemError::from)?;

        Ok(ApiSecurityScheme::from(result))
    }

    async fn get(
        &self,
        id: &str,
        _project: &Self::ProjectContext,
    ) -> Result<ApiSecurityScheme, GolemError> {
        info!("Getting api security scheme for {id}");

        let result = self.client.get(id).await.map_err(GolemError::from)?;

        Ok(ApiSecurityScheme::from(result))
    }
}
