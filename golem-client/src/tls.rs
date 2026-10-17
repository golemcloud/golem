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

//! WebSocket TLS connectors that name their rustls `CryptoProvider` explicitly.
//!
//! When a process links rustls with more than one crypto provider and installs no process
//! default, `rustls::ClientConfig::builder()` panics. tokio-tungstenite calls it when a
//! `wss://` connection has no connector, so every `wss://` connection made through this crate
//! uses a connector from this module instead. A `ws://` connection needs no TLS configuration.

use rustls::ClientConfig;
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::{Arc, LazyLock};
use tokio_tungstenite::Connector;
use tokio_tungstenite::tungstenite::http::Uri;

static PLATFORM_VERIFIER_CONFIG: LazyLock<Result<Arc<ClientConfig>, rustls::Error>> =
    LazyLock::new(|| {
        Ok(Arc::new(
            ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()?
                .with_platform_verifier()?
                .with_no_client_auth(),
        ))
    });

/// A connector that uses the `ring` provider and verifies servers with the operating system's
/// certificate verifier, the same verifier that reqwest uses for REST calls. On Linux, this
/// verifier reads the roots from `SSL_CERT_FILE` and `SSL_CERT_DIR` when they are set.
pub fn platform_verifier_connector() -> Result<Connector, rustls::Error> {
    PLATFORM_VERIFIER_CONFIG.clone().map(Connector::Rustls)
}

/// The connector for a WebSocket connection to `uri`. A connector that the caller supplies is
/// kept. Without one, a `wss://` connection gets [`platform_verifier_connector`] and a `ws://`
/// connection gets none, so a plain connection does not need the system CA certificates.
pub fn connector_for(
    uri: &Uri,
    connector: Option<Connector>,
) -> Result<Option<Connector>, rustls::Error> {
    match connector {
        Some(connector) => Ok(Some(connector)),
        None if uri.scheme_str() == Some("wss") => platform_verifier_connector().map(Some),
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn plain_websocket_without_connector_gets_no_tls_configuration() {
        let uri: Uri = "ws://127.0.0.1:9006/v1/agents/invoke-agent-session"
            .parse()
            .unwrap();
        assert!(connector_for(&uri, None).unwrap().is_none());
    }

    #[test]
    fn connector_from_the_caller_is_kept() {
        let uri: Uri = "ws://127.0.0.1:9006/v1/agents/invoke-agent-session"
            .parse()
            .unwrap();
        assert!(matches!(
            connector_for(&uri, Some(Connector::Plain)).unwrap(),
            Some(Connector::Plain)
        ));
    }
}
