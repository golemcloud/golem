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

//! Non-fatal warnings produced during deployment validation.
//!
//! Unlike `DeployValidationError` (which causes the deployment to fail),
//! these warnings are surfaced in the [`CurrentDeployment`](super::deployment::CurrentDeployment)
//! response so the deploying user is notified about likely misconfigurations
//! without blocking the deployment.

use super::agent::{AgentTypeName, HttpMethod};
use super::component::ComponentId;
use crate::{declare_structs, declare_unions};
use std::fmt;

declare_unions! {
    pub enum DeployValidationWarning {
        /// A read-only `AgentMethod` is bound to an HTTP route whose method is
        /// not `GET`/`HEAD`. The route still works, but the worker-service
        /// will not emit HTTP cache headers for it.
        HttpApiReadOnlyMethodBoundToNonGetVerb(HttpApiReadOnlyMethodBoundToNonGetVerb),

        /// The read-only method's `cache-policy = ttl(d)` value rounds down to
        /// zero seconds. `Cache-Control: max-age=0` will be emitted, meaning
        /// every downstream request will revalidate via `ETag`. Likely not
        /// what the author intended.
        HttpApiReadOnlyTtlBelowOneSecond(HttpApiReadOnlyTtlBelowOneSecond)
    }
}

declare_structs! {
    pub struct HttpApiReadOnlyMethodBoundToNonGetVerb {
        pub component_id: ComponentId,
        pub agent_type: AgentTypeName,
        pub method_name: String,
        pub http_method: HttpMethod,
        pub path: String,
    }

    pub struct HttpApiReadOnlyTtlBelowOneSecond {
        pub component_id: ComponentId,
        pub agent_type: AgentTypeName,
        pub method_name: String,
        pub ttl_nanos: u64,
    }
}

impl fmt::Display for DeployValidationWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DeployValidationWarning::HttpApiReadOnlyMethodBoundToNonGetVerb(w) => write!(
                f,
                "HTTP route {http_method} {path} resolves to read-only method `{method_name}` of agent `{agent_type}`; only GET/HEAD requests get cache headers — this route will not emit any.",
                http_method = render_http_method(&w.http_method),
                path = w.path,
                method_name = w.method_name,
                agent_type = w.agent_type,
            ),
            DeployValidationWarning::HttpApiReadOnlyTtlBelowOneSecond(w) => write!(
                f,
                "Read-only method `{method_name}` of agent `{agent_type}` has a TTL of {ttl_nanos} ns, which rounds down to 0 seconds in `Cache-Control: max-age=...`. Downstream caches will revalidate on every request.",
                method_name = w.method_name,
                agent_type = w.agent_type,
                ttl_nanos = w.ttl_nanos,
            ),
        }
    }
}

fn render_http_method(method: &HttpMethod) -> &'static str {
    match method {
        HttpMethod::Get(_) => "GET",
        HttpMethod::Head(_) => "HEAD",
        HttpMethod::Post(_) => "POST",
        HttpMethod::Put(_) => "PUT",
        HttpMethod::Delete(_) => "DELETE",
        HttpMethod::Connect(_) => "CONNECT",
        HttpMethod::Options(_) => "OPTIONS",
        HttpMethod::Trace(_) => "TRACE",
        HttpMethod::Patch(_) => "PATCH",
        HttpMethod::Custom(_) => "<custom>",
    }
}
