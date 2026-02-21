// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_common::SafeDisplay;
use golem_common::model::Empty;
use golem_common::model::base64::Base64;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use tonic::transport::ServerTlsConfig;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "config")]
pub enum GrpcServerTlsConfig {
    Enabled(EnabledGrpcServerTlsConfig),
    Disabled(Empty),
}

impl GrpcServerTlsConfig {
    pub fn disabled() -> Self {
        Self::Disabled(Empty {})
    }
}

impl SafeDisplay for GrpcServerTlsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        match self {
            Self::Enabled(inner) => {
                let _ = writeln!(&mut result, "Enabled:");
                let _ = writeln!(&mut result, "{}", inner.to_safe_string_indented());
            }
            Self::Disabled(_) => {
                let _ = writeln!(&mut result, "Disabled");
            }
        }
        result
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnabledGrpcServerTlsConfig {
    /// server-specific certificate — issued by cluster CA
    pub server_cert: Base64,
    /// private key for server_cert
    pub server_key: Base64,
    /// CA certificate used to validate client certificates (PEM)
    pub client_ca_cert: Base64,
}

impl SafeDisplay for EnabledGrpcServerTlsConfig {
    fn to_safe_string(&self) -> String {
        use sha2::{Digest, Sha256};

        fn fingerprint(data: &[u8]) -> String {
            let hash = Sha256::digest(data);
            hex::encode(hash)
        }

        let mut result = String::new();
        let _ = writeln!(
            &mut result,
            "server_cert_sha256: {}",
            fingerprint(&self.server_cert.0)
        );
        let _ = writeln!(&mut result, "server_key: *******");
        let _ = writeln!(
            &mut result,
            "client_ca_cert: {}",
            fingerprint(&self.client_ca_cert.0)
        );
        result
    }
}

impl EnabledGrpcServerTlsConfig {
    pub fn to_tonic(&self) -> ServerTlsConfig {
        use tonic::transport::{Certificate, Identity};

        let identity = Identity::from_pem(&self.server_cert.0, &self.server_key.0);
        let client_ca = Certificate::from_pem(&self.client_ca_cert.0);

        ServerTlsConfig::new()
            .identity(identity)
            .client_ca_root(client_ca)
    }
}
