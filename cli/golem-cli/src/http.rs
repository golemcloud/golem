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

use crate::config::HttpClientConfig;
use anyhow::bail;
use std::borrow::Cow;

pub fn new_reqwest_client(config: &HttpClientConfig) -> anyhow::Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder();

    if config.allow_insecure {
        builder = builder.danger_accept_invalid_certs(true);
    }

    if let Some(timeout) = config.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(connect_timeout) = config.connect_timeout {
        builder = builder.connect_timeout(connect_timeout);
    }
    if let Some(read_timeout) = config.read_timeout {
        builder = builder.read_timeout(read_timeout);
    }

    Ok(builder.connection_verbose(true).build()?)
}

pub async fn check_http_response_success(
    response: reqwest::Response,
) -> anyhow::Result<reqwest::Response> {
    if !response.status().is_success() {
        let url = response.url().clone();
        let status = response.status();
        let bytes = response.bytes().await.ok();
        let error_payload = bytes
            .as_ref()
            .map(|bytes| String::from_utf8_lossy(bytes.as_ref()))
            .unwrap_or_else(|| Cow::from(""));

        bail!(
            "Received unexpected response for {}: {}\n{}",
            url,
            status,
            error_payload
        );
    }
    Ok(response)
}
