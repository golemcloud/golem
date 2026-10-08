// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

use crate::model::local_observability::{
    LOCAL_OBSERVABILITY_CAPABILITY_PATH, LOCAL_OBSERVABILITY_DASHBOARD_PATH,
    LOCAL_OBSERVABILITY_FAMILIES_PATH, LOCAL_OBSERVABILITY_SAMPLES_PATH,
    LOCAL_OBSERVABILITY_SERIES_PATH, LocalMetricDashboardRequest, LocalMetricDashboardResponse,
    LocalMetricFamiliesRequest, LocalMetricFamiliesResponse, LocalMetricSamplesRequest,
    LocalMetricSamplesResponse, LocalMetricSeriesRequest, LocalMetricSeriesResponse,
    LocalObservabilityCapability,
};
use anyhow::{Context, bail};
use reqwest::{Client, StatusCode, Url};
use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Clone)]
pub struct LocalObservabilityClient {
    http: Client,
    base_url: Url,
}

impl LocalObservabilityClient {
    pub fn new(http: Client, base_url: Url) -> Self {
        Self { http, base_url }
    }

    pub async fn capability(&self) -> anyhow::Result<Option<LocalObservabilityCapability>> {
        let response = self
            .http
            .get(self.endpoint(LOCAL_OBSERVABILITY_CAPABILITY_PATH)?)
            .bearer_auth(golem_client::LOCAL_WELL_KNOWN_TOKEN)
            .send()
            .await
            .context("failed to query local observability capability")?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        Ok(Some(decode_response(response).await?))
    }

    pub async fn families(
        &self,
        request: &LocalMetricFamiliesRequest,
    ) -> anyhow::Result<LocalMetricFamiliesResponse> {
        self.post(LOCAL_OBSERVABILITY_FAMILIES_PATH, request).await
    }

    pub async fn dashboard(
        &self,
        request: &LocalMetricDashboardRequest,
    ) -> anyhow::Result<LocalMetricDashboardResponse> {
        self.post(LOCAL_OBSERVABILITY_DASHBOARD_PATH, request).await
    }

    pub async fn series(
        &self,
        request: &LocalMetricSeriesRequest,
    ) -> anyhow::Result<LocalMetricSeriesResponse> {
        self.post(LOCAL_OBSERVABILITY_SERIES_PATH, request).await
    }

    pub async fn samples(
        &self,
        request: &LocalMetricSamplesRequest,
    ) -> anyhow::Result<LocalMetricSamplesResponse> {
        self.post(LOCAL_OBSERVABILITY_SAMPLES_PATH, request).await
    }

    async fn post<T: Serialize + ?Sized, U: DeserializeOwned>(
        &self,
        path: &str,
        request: &T,
    ) -> anyhow::Result<U> {
        let response = self
            .http
            .post(self.endpoint(path)?)
            .bearer_auth(golem_client::LOCAL_WELL_KNOWN_TOKEN)
            .json(request)
            .send()
            .await
            .with_context(|| format!("failed to query local observability endpoint {path}"))?;
        decode_response(response).await
    }

    fn endpoint(&self, path: &str) -> anyhow::Result<Url> {
        self.base_url
            .join(path)
            .with_context(|| format!("invalid local observability endpoint {path}"))
    }
}

async fn decode_response<T: DeserializeOwned>(response: reqwest::Response) -> anyhow::Result<T> {
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        bail!("local observability request failed with {status}: {body}");
    }
    response
        .json()
        .await
        .context("invalid local observability response")
}
