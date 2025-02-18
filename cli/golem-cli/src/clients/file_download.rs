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

use crate::model::GolemError;
use async_trait::async_trait;
use url::Url;

#[async_trait]
pub trait FileDownloadClient {
    async fn download_file(&self, url: Url) -> Result<Vec<u8>, GolemError>;
}

pub struct FileDownloadClientLive {
    pub client: reqwest::Client,
}

#[async_trait]
impl FileDownloadClient for FileDownloadClientLive {
    async fn download_file(&self, url: Url) -> Result<Vec<u8>, GolemError> {
        let response = self.client.get(url).send().await?;
        let bytes = response.bytes().await?;
        Ok(bytes.to_vec())
    }
}
