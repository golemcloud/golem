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

mod protocol;

pub(crate) use protocol::{
    append_memory_reservation, preflight_append, read_memory_reservation, validate_read,
};

use async_trait::async_trait;
use golem_common::model::oplog::payload::external_durable_stream::{
    DurableStreamAppendReceipt, DurableStreamAppendRequest, DurableStreamBatch, DurableStreamError,
    DurableStreamReadRequest,
};

/// One finite HTTP attempt. Authorization, memory admission, quotas and durable completion
/// belong to the caller; this service never retries or changes the producer identity.
#[async_trait]
pub trait ExternalDurableStreamService: Send + Sync {
    async fn read_batch(
        &self,
        request: &DurableStreamReadRequest,
        bearer: Option<&str>,
        max_bytes: usize,
    ) -> Result<DurableStreamBatch, DurableStreamError>;

    async fn append_batch(
        &self,
        request: &DurableStreamAppendRequest,
        bearer: Option<&str>,
        max_bytes: usize,
    ) -> Result<DurableStreamAppendReceipt, DurableStreamError>;
}

pub struct DefaultExternalDurableStreamService {
    client: reqwest::Client,
}

impl DefaultExternalDurableStreamService {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()?,
        })
    }
}

#[async_trait]
impl ExternalDurableStreamService for DefaultExternalDurableStreamService {
    async fn read_batch(
        &self,
        request: &DurableStreamReadRequest,
        bearer: Option<&str>,
        max_bytes: usize,
    ) -> Result<DurableStreamBatch, DurableStreamError> {
        protocol::read_batch(&self.client, request, bearer, max_bytes).await
    }

    async fn append_batch(
        &self,
        request: &DurableStreamAppendRequest,
        bearer: Option<&str>,
        max_bytes: usize,
    ) -> Result<DurableStreamAppendReceipt, DurableStreamError> {
        protocol::append_batch(&self.client, request, bearer, max_bytes).await
    }
}
