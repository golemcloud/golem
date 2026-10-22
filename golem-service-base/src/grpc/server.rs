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

use crate::tls::material::TlsSources;
use crate::tls::pem::{PemSource, TlsError};
use golem_common::SafeDisplay;
use golem_common::model::Empty;
use golem_common::model::base64::Base64;
use serde::{Deserialize, Serialize};
use std::fmt::Write;
use std::path::{Path, PathBuf};
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

/// Each item comes from a PEM file or an inline value (the PEM, Base64 on one line), not both.
/// Files are polled and a renewed certificate is used for new connections; an inline value
/// changes only with a restart.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EnabledGrpcServerTlsConfig {
    /// server-specific certificate — issued by cluster CA
    #[serde(default)]
    pub server_cert: Option<Base64>,
    #[serde(default)]
    pub server_cert_file: Option<PathBuf>,
    /// private key for server_cert
    #[serde(default)]
    pub server_key: Option<Base64>,
    #[serde(default)]
    pub server_key_file: Option<PathBuf>,
    /// CA certificate used to validate client certificates (PEM)
    #[serde(default)]
    pub client_ca_cert: Option<Base64>,
    #[serde(default)]
    pub client_ca_cert_file: Option<PathBuf>,
}

impl SafeDisplay for EnabledGrpcServerTlsConfig {
    fn to_safe_string(&self) -> String {
        let mut result = String::new();
        write_item(
            &mut result,
            "server_cert",
            self.server_cert.as_ref(),
            self.server_cert_file.as_deref(),
            true,
        );
        write_item(
            &mut result,
            "server_key",
            self.server_key.as_ref(),
            self.server_key_file.as_deref(),
            false,
        );
        write_item(
            &mut result,
            "client_ca_cert",
            self.client_ca_cert.as_ref(),
            self.client_ca_cert_file.as_deref(),
            true,
        );
        result
    }
}

/// One line per item: the file path when a file is set, else the fingerprint of the inline value
/// (or `*******` for a private key).
fn write_item(
    result: &mut String,
    name: &str,
    value: Option<&Base64>,
    file: Option<&Path>,
    show_fingerprint: bool,
) {
    use sha2::{Digest, Sha256};

    if let Some(file) = file {
        let _ = writeln!(result, "{name}_file: {}", file.display());
    } else if let (true, Some(value)) = (show_fingerprint, value) {
        let _ = writeln!(
            result,
            "{name}_sha256: {}",
            hex::encode(Sha256::digest(&value.0))
        );
    } else if value.is_some() {
        let _ = writeln!(result, "{name}: *******");
    } else {
        let _ = writeln!(result, "{name}: <not set>");
    }
}

impl EnabledGrpcServerTlsConfig {
    pub fn sources(&self) -> Result<TlsSources, TlsError> {
        Ok(TlsSources {
            cert: PemSource::from_config(
                "server_cert",
                self.server_cert.as_ref(),
                self.server_cert_file.as_deref(),
            )?,
            key: PemSource::from_config(
                "server_key",
                self.server_key.as_ref(),
                self.server_key_file.as_deref(),
            )?,
            ca: PemSource::from_config(
                "client_ca_cert",
                self.client_ca_cert.as_ref(),
                self.client_ca_cert_file.as_deref(),
            )?,
        })
    }

    pub fn to_tonic(&self) -> Result<ServerTlsConfig, TlsError> {
        use tonic::transport::{Certificate, Identity};

        let sources = self.sources()?;
        let identity = Identity::from_pem(sources.cert.read()?, sources.key.read()?);
        let client_ca = Certificate::from_pem(sources.ca.read()?);

        Ok(ServerTlsConfig::new()
            .identity(identity)
            .client_ca_root(client_ca))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::pem::TlsError;
    use serde_json::json;
    use test_r::test;

    fn parse(value: serde_json::Value) -> EnabledGrpcServerTlsConfig {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn server_tls_settings_accept_files() {
        let config = parse(json!({
            "server_cert_file": "/x/tls.crt",
            "server_key_file": "/x/tls.key",
            "client_ca_cert_file": "/x/ca.crt",
        }));
        let sources = config.sources().unwrap();
        assert!(sources.cert.is_file());
        assert!(sources.key.is_file());
        assert!(sources.ca.is_file());
    }

    #[test]
    fn server_tls_settings_refuse_an_item_set_twice() {
        let both = parse(json!({
            "server_cert": "",
            "server_cert_file": "/x/tls.crt",
            "server_key": "",
            "client_ca_cert": "",
        }));
        assert!(matches!(
            both.sources(),
            Err(TlsError::BothSet { name }) if name == "server_cert"
        ));

        let neither = parse(json!({
            "server_cert": "",
            "client_ca_cert": "",
        }));
        assert!(matches!(
            neither.sources(),
            Err(TlsError::NeitherSet { name }) if name == "server_key"
        ));
    }

    #[test]
    fn tls_display_names_files_and_hides_keys() {
        let files = parse(json!({
            "server_cert_file": "/x/tls.crt",
            "server_key_file": "/x/tls.key",
            "client_ca_cert_file": "/x/ca.crt",
        }));
        let shown = files.to_safe_string();
        assert!(shown.contains("server_key_file: /x/tls.key"), "{shown}");
        assert!(shown.contains("server_cert_file: /x/tls.crt"), "{shown}");

        let inline = parse(json!({
            "server_cert": "",
            "server_key": "",
            "client_ca_cert": "",
        }));
        let shown = inline.to_safe_string();
        assert!(shown.contains("server_key: *******"), "{shown}");
        assert!(shown.contains("server_cert_sha256:"), "{shown}");
    }
}
