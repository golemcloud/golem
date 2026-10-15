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
//! connection has no connector, so every WebSocket connection made through this crate uses one
//! of these connectors instead.

use rustls::{ClientConfig, RootCertStore};
use std::sync::{Arc, LazyLock};
use tokio_tungstenite::Connector;

static WEBPKI_ROOTS_CONFIG: LazyLock<Arc<ClientConfig>> = LazyLock::new(|| {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    Arc::new(
        client_config(roots).expect("the ring provider supports the default TLS protocol versions"),
    )
});

/// A connector that uses the `ring` provider and verifies servers against the given roots.
pub fn rustls_connector(roots: RootCertStore) -> Result<Connector, rustls::Error> {
    Ok(Connector::Rustls(Arc::new(client_config(roots)?)))
}

/// A connector that uses the `ring` provider and verifies servers against the webpki roots,
/// the same roots tokio-tungstenite uses when a connection has no connector.
pub fn webpki_roots_connector() -> Connector {
    Connector::Rustls(WEBPKI_ROOTS_CONFIG.clone())
}

fn client_config(roots: RootCertStore) -> Result<ClientConfig, rustls::Error> {
    Ok(
        ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}
