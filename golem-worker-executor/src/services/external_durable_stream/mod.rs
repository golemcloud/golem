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
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};

struct LocalhostResolver;

impl Resolve for LocalhostResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            if !protocol::is_localhost_name(name.as_str()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "Local HTTP requires a localhost name",
                )
                .into());
            }
            let addresses: Addrs = Box::new(
                [
                    SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
                    SocketAddr::from((Ipv6Addr::LOCALHOST, 0)),
                ]
                .into_iter(),
            );
            Ok(addresses)
        })
    }
}

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
    local_client: reqwest::Client,
}

impl DefaultExternalDurableStreamService {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()?,
            local_client: reqwest::Client::builder()
                .no_proxy()
                .dns_resolver(LocalhostResolver)
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .build()?,
        })
    }

    fn client(&self, url: &reqwest::Url) -> &reqwest::Client {
        // Validation restricts HTTP to localhost names and literal loopback IPs.
        // HTTPS always retains the normal client's DNS and proxy behavior.
        if url.scheme() == "http" {
            &self.local_client
        } else {
            &self.client
        }
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
        let url = validate_read(request, max_bytes)?;
        protocol::read_batch(self.client(&url), request, bearer, max_bytes).await
    }

    async fn append_batch(
        &self,
        request: &DurableStreamAppendRequest,
        bearer: Option<&str>,
        max_bytes: usize,
    ) -> Result<DurableStreamAppendReceipt, DurableStreamError> {
        let url = preflight_append(request, max_bytes)?;
        protocol::append_batch(self.client(&url), request, bearer, max_bytes).await
    }
}
