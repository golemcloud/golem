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

/// The entry point where a request was decided. Each surface renders a denial with its own
/// renderer, and denial metrics and audit records carry its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Surface {
    /// The REST API of the worker service and the registry service.
    Rest,
    /// The HTTP routes of deployed agents, Durable Streams reads included.
    HttpRoute,
    /// The MCP endpoint.
    Mcp,
    /// The invocation attach protocol.
    Attach,
    /// Typed internal calls, such as calls between agents.
    Internal,
    /// Host functions that a guest calls.
    Host,
}

impl Surface {
    /// A stable, bounded label for metrics and audit records.
    pub fn label(self) -> &'static str {
        match self {
            Surface::Rest => "rest",
            Surface::HttpRoute => "http-route",
            Surface::Mcp => "mcp",
            Surface::Attach => "attach",
            Surface::Internal => "internal",
            Surface::Host => "host",
        }
    }
}
