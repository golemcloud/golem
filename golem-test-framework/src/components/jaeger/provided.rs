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

use crate::components::jaeger::{Jaeger, wait_for_startup};
use async_trait::async_trait;
use std::fmt::{Debug, Formatter};
use std::time::Duration;
use tracing::info;

/// A `Jaeger` implementation backed by an externally-managed
/// collector. Mirrors the `Provided*` pattern used elsewhere in the
/// test framework (`ProvidedRedis`, etc.): the test process does not
/// start a process or container — it consumes endpoints supplied by
/// the caller via env vars.
pub struct ProvidedJaeger {
    otlp_http_endpoint: String,
    query_url: String,
}

impl ProvidedJaeger {
    pub async fn new(otlp_http_endpoint: String, query_url: String) -> Self {
        info!("Using provided Jaeger: otlp={otlp_http_endpoint}, query={query_url}");
        wait_for_startup(&query_url, Duration::from_secs(30)).await;
        Self {
            otlp_http_endpoint,
            query_url,
        }
    }
}

#[async_trait]
impl Jaeger for ProvidedJaeger {
    fn otlp_http_endpoint(&self) -> String {
        self.otlp_http_endpoint.clone()
    }

    fn query_url(&self) -> String {
        self.query_url.clone()
    }

    async fn kill(&self) {
        // Externally managed; lifecycle is the caller's responsibility.
    }
}

impl Debug for ProvidedJaeger {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ProvidedJaeger {{ otlp_http_endpoint: {:?}, query_url: {:?} }}",
            self.otlp_http_endpoint, self.query_url
        )
    }
}
