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

use crate::context::check_http_response_success;
use crate::log::{log_action, LogColorize};
use anyhow::{anyhow, Context};
use base64::prelude::*;
use std::path::PathBuf;
use url::Url;

pub struct RemoteComponents {
    client: reqwest::Client,
    temp_dir: PathBuf,
    offline: bool,
}

impl RemoteComponents {
    pub fn new(client: reqwest::Client, target: PathBuf, offline: bool) -> Self {
        Self {
            client,
            temp_dir: target,
            offline,
        }
    }

    pub async fn get_from_url(&self, url: &Url) -> anyhow::Result<PathBuf> {
        let parent_dir = self.temp_dir.join("remote_components");
        crate::fs::create_dir_all(&parent_dir)?;

        let url_hash = BASE64_STANDARD.encode(url.to_string());
        let path = parent_dir.join(format!("{url_hash}.wasm"));

        if std::fs::exists(&path)? {
            log_action(
                "Skipping",
                format!(
                    "download of remote WASM component: {}, using a previously downloaded version",
                    url.as_str().log_color_highlight()
                ),
            );

            Ok(path)
        } else if !self.offline {
            log_action(
                "Downloading",
                format!(
                    "remote WASM component: {}",
                    url.as_str().log_color_highlight()
                ),
            );

            let response =
                self.client.get(url.clone()).send().await.with_context(|| {
                    anyhow!("Failed to download remote component WASM: {}", url)
                })?;

            let response = check_http_response_success(response).await?;

            let bytes = response
                .bytes()
                .await
                .with_context(|| anyhow!("Failed to download remote component WASM: {}", url))?;

            std::fs::write(&path, bytes)?;
            Ok(path)
        } else {
            Err(anyhow!(
                "Offline mode is enabled, but the remote component '{}' is not downloaded yet",
                url
            ))
        }
    }
}
