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

use std::collections::BTreeMap;

use super::{PredicateValue, RetryContext, RetryProperties};
use crate::model::agent::parse_agent_id_parts;
use crate::model::{OwnedAgentId, RdbmsPoolKey};

impl RetryProperties {
    /// Creates an empty property bag.
    pub fn new() -> Self {
        Self {
            entries: BTreeMap::new(),
        }
    }

    /// Inserts or updates a property. Returns `&mut Self` for chaining.
    pub fn set(&mut self, key: impl Into<String>, value: PredicateValue) -> &mut Self {
        self.entries.insert(key.into(), value);
        self
    }

    /// Returns the value associated with the given key, if present.
    pub fn get(&self, key: &str) -> Option<&PredicateValue> {
        self.entries.get(key)
    }

    /// Returns `true` if the property bag contains the given key.
    pub fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Iterates over all key-value pairs in the property bag.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &PredicateValue)> {
        self.entries.iter()
    }
}
impl RetryContext {
    /// Builds retry properties for an outgoing HTTP request.
    pub fn http(method: &str, uri: &str) -> RetryProperties {
        let mut props = Self::base(method, uri);
        decompose_uri(&mut props, uri);
        props
    }

    /// Builds retry properties for an HTTP request that received a response or error.
    pub fn http_with_response(
        method: &str,
        uri: &str,
        status_code: Option<u16>,
        error_type: &str,
    ) -> RetryProperties {
        let mut props = Self::http(method, uri);
        if let Some(status_code) = status_code {
            props.set("status-code", PredicateValue::Integer(status_code as i64));
        }
        props.set("error-type", PredicateValue::Text(error_type.to_string()));
        props
    }

    /// Builds retry properties for a worker-to-worker RPC call.
    pub fn rpc(verb: &str, target: &OwnedAgentId, function: &str) -> RetryProperties {
        let noun_uri = format!("worker://{}/{}", target.component_id(), target.agent_name());
        let mut props = Self::base(verb, &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props
            .set("function", PredicateValue::Text(function.to_string()))
            .set(
                "target-component-id",
                PredicateValue::Text(target.component_id().to_string()),
            );

        if let Ok((agent_type_name, _, _)) = parse_agent_id_parts(&target.agent_name()) {
            props.set(
                "target-agent-type",
                PredicateValue::Text(agent_type_name.to_string()),
            );
        }

        props
    }

    /// Builds retry properties for a key-value store operation.
    pub fn kv(verb: &str, bucket: &str) -> RetryProperties {
        let noun_uri = format!("kv://{bucket}");
        let mut props = Self::base(verb, &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props
    }

    /// Builds retry properties for a blob storage operation.
    pub fn blobstore(verb: &str, container: &str) -> RetryProperties {
        let noun_uri = format!("blobstore://{container}");
        let mut props = Self::base(verb, &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props
    }

    /// Builds retry properties for an RDBMS operation.
    pub fn rdbms(verb: &str, pool_key: &RdbmsPoolKey) -> RetryProperties {
        let noun_uri = pool_key.masked_address();
        let mut props = Self::base(verb, &noun_uri);
        decompose_uri(&mut props, &noun_uri);

        if let Ok(url) = url::Url::parse(&noun_uri) {
            props.set(
                "db-type",
                PredicateValue::Text(url.scheme().to_ascii_lowercase()),
            );
        }

        props
    }

    /// Builds retry properties for a DNS resolution attempt.
    pub fn dns(hostname: &str) -> RetryProperties {
        let noun_uri = format!("dns://{hostname}");
        let mut props = Self::base("resolve", &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props
    }

    /// Builds retry properties for a Golem platform API call.
    pub fn golem_api(verb: &str) -> RetryProperties {
        let noun_uri = "golem://api";
        let mut props = Self::base(verb, noun_uri);
        decompose_uri(&mut props, noun_uri);
        props
    }

    /// Builds retry properties for a WASM trap (deterministic or transient).
    pub fn trap(trap_type: &str, function: Option<&str>) -> RetryProperties {
        let function = function.unwrap_or("unknown");
        let noun_uri = format!("wasm://{function}");
        let mut props = Self::base("trap", &noun_uri);
        decompose_uri(&mut props, &noun_uri);
        props.set("trap-type", PredicateValue::Text(trap_type.to_string()));
        props
    }

    /// Builds retry properties for a custom operation with a verb and noun URI.
    pub fn custom(verb: &str, noun_uri: &str) -> RetryProperties {
        let mut props = Self::base(verb, noun_uri);
        decompose_uri(&mut props, noun_uri);
        props
    }

    fn base(verb: &str, noun_uri: &str) -> RetryProperties {
        let mut props = RetryProperties::new();
        props
            .set("verb", PredicateValue::Text(verb.to_string()))
            .set("noun-uri", PredicateValue::Text(noun_uri.to_string()));
        props
    }
}

/// Parses a URI and adds its scheme, host, port, and path as individual retry properties.
fn decompose_uri(props: &mut RetryProperties, uri: &str) {
    if let Ok(parsed) = url::Url::parse(uri) {
        props.set(
            "uri-scheme",
            PredicateValue::Text(parsed.scheme().to_string()),
        );
        if let Some(host) = parsed.host_str() {
            props.set("uri-host", PredicateValue::Text(host.to_string()));
        }
        if let Some(port) = parsed.port() {
            props.set("uri-port", PredicateValue::Integer(port as i64));
        }
        props.set("uri-path", PredicateValue::Text(parsed.path().to_string()));
    }
}
