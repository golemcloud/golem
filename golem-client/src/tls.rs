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
//! connection has no connector, so every WebSocket connection made through this crate uses a
//! connector from this module instead.

use rustls::ClientConfig;
use rustls_platform_verifier::BuilderVerifierExt;
use std::sync::{Arc, LazyLock};
use tokio_tungstenite::Connector;

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
