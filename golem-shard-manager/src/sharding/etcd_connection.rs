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

//! etcd connections for the shard manager.
//!
//! Two clients, never cloned from one another: `etcd-client` turns its request timeout into a
//! tonic endpoint timeout that applies to every RPC on the channel, and `Client::clone` shares
//! that channel. The election RPCs block by design, so they need a channel with no timeout.

use crate::config::{EtcdConfig, EtcdTlsConfig};
use crate::sharding::error::ShardManagerError;
use etcd_client::{Certificate, Client, ConnectOptions, Identity, TlsOptions};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::path::Path;

fn invalid_config(message: String) -> ShardManagerError {
    ShardManagerError::InvalidConfig(message)
}

fn check_endpoints(config: &EtcdConfig) -> Result<(), ShardManagerError> {
    if config.endpoints.is_empty() {
        return Err(invalid_config(
            "etcd shard state persistence requires at least one endpoint".to_string(),
        ));
    }

    // Strict both ways, so that a mistake stops the startup with the setting's name. etcd-client
    // refuses an `http://` endpoint with TLS options only when it connects, without naming a
    // setting; an `https://` endpoint without them fails only at its first handshake; and a
    // scheme-less `host:port` fails at connect time with an opaque error.
    let tls = config.tls.is_some();
    for endpoint in &config.endpoints {
        let message = match (tls, endpoint.starts_with("https://")) {
            (true, true) => continue,
            (false, false) if endpoint.starts_with("http://") => continue,
            (true, false) => format!(
                "etcd endpoint {endpoint} must start with https://, because \
                 persistence.config.tls is set"
            ),
            (false, true) => format!(
                "etcd endpoint {endpoint} uses https://, which needs persistence.config.tls \
                 (at least ca_cert_file)"
            ),
            (false, false) => format!(
                "etcd endpoint {endpoint} must start with http://, or with https:// together with \
                 persistence.config.tls"
            ),
        };
        return Err(invalid_config(message));
    }

    Ok(())
}

fn check_credentials(config: &EtcdConfig) -> Result<(), ShardManagerError> {
    match (&config.username, &config.password) {
        (None, None) => Ok(()),
        (Some(_), Some(_)) if config.tls.is_some() => Ok(()),
        (Some(_), Some(_)) => Err(invalid_config(
            "persistence.config.username and persistence.config.password need \
             persistence.config.tls: over http:// the password, and then the auth token on every \
             request, would cross the network in plain text"
                .to_string(),
        )),
        _ => Err(invalid_config(
            "persistence.config.username and persistence.config.password must be set together"
                .to_string(),
        )),
    }
}

#[derive(Clone, Copy)]
enum Pem {
    Certificate,
    PrivateKey,
}

/// Reads a PEM file and checks that it holds what the setting needs, so that a wrong file stops
/// the startup with the setting's name instead of failing every TLS handshake later.
fn read_pem(setting: &str, path: &Path, expected: Pem) -> Result<Vec<u8>, ShardManagerError> {
    let bytes = std::fs::read(path).map_err(|err| {
        invalid_config(format!(
            "cannot read persistence.config.tls.{setting} {}: {err}",
            path.display()
        ))
    })?;
    let (found, kind) = match expected {
        Pem::Certificate => (
            CertificateDer::pem_slice_iter(&bytes).any(|item| item.is_ok()),
            "certificate",
        ),
        Pem::PrivateKey => (PrivateKeyDer::from_pem_slice(&bytes).is_ok(), "private key"),
    };
    if !found {
        return Err(invalid_config(format!(
            "persistence.config.tls.{setting} {} holds no PEM {kind}",
            path.display()
        )));
    }
    Ok(bytes)
}

fn tls_options(tls: &EtcdTlsConfig) -> Result<TlsOptions, ShardManagerError> {
    let ca = read_pem("ca_cert_file", &tls.ca_cert_file, Pem::Certificate)?;
    let mut options = TlsOptions::new().ca_certificate(Certificate::from_pem(ca));
    match (&tls.client_cert_file, &tls.client_key_file) {
        (Some(cert), Some(key)) => {
            let cert = read_pem("client_cert_file", cert, Pem::Certificate)?;
            let key = read_pem("client_key_file", key, Pem::PrivateKey)?;
            options = options.identity(Identity::from_pem(cert, key));
        }
        (None, None) => {}
        _ => {
            return Err(invalid_config(
                "persistence.config.tls.client_cert_file and \
                 persistence.config.tls.client_key_file must be set together"
                    .to_string(),
            ));
        }
    }
    if let Some(name) = &tls.server_name {
        options = options.domain_name(name);
    }
    Ok(options)
}

/// The options both etcd clients connect with, so that TLS and authentication cannot differ
/// between them. Reads and checks the TLS files: call it before the campaign.
pub(crate) fn connect_options(config: &EtcdConfig) -> Result<ConnectOptions, ShardManagerError> {
    check_endpoints(config)?;
    check_credentials(config)?;

    let mut options = ConnectOptions::new().with_connect_timeout(config.connect_timeout);
    if let Some(tls) = &config.tls {
        options = options.with_tls(tls_options(tls)?);
    }
    if let (Some(username), Some(password)) = (&config.username, &config.password) {
        // etcd drops a token that goes unused for its `--auth-token-ttl`, a JWT token at the end
        // of its lifetime, and every simple token when a member restarts. A request already sent
        // with such a token - a standby's campaign waits inside etcd for as long as the leader
        // lives - can only be repaired when it fails: etcd-client then logs in again and sends the
        // request once more. etcd checks the token before it applies a request, so the second
        // send cannot apply a write twice.
        options = options
            .with_user(username, password.expose())
            .with_auto_token_refresh(true);
    }
    Ok(options)
}

/// A client for ordinary key/value requests, bounded by `request_timeout`.
pub async fn connect_for_requests(config: &EtcdConfig) -> Result<Client, ShardManagerError> {
    let options = connect_options(config)?.with_timeout(config.request_timeout);
    Ok(Client::connect(&config.endpoints, Some(options)).await?)
}

/// A client for the leadership campaign and its lease keepalive.
pub async fn connect_for_election(config: &EtcdConfig) -> Result<Client, ShardManagerError> {
    Ok(Client::connect(&config.endpoints, Some(connect_options(config)?)).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EtcdPassword, EtcdTlsConfig};
    use golem_common::SafeDisplay;
    use std::path::PathBuf;
    use test_r::test;

    /// A self-signed certificate and its key as PEM files, in a directory that lives as long as
    /// this value. The checks here only parse the files, so one certificate serves as both the CA
    /// and the client certificate.
    struct PemFiles {
        dir: tempfile::TempDir,
    }

    impl PemFiles {
        fn new() -> Self {
            let rcgen::CertifiedKey { cert, signing_key } =
                rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(dir.path().join("cert.pem"), cert.pem()).unwrap();
            std::fs::write(dir.path().join("key.pem"), signing_key.serialize_pem()).unwrap();
            Self { dir }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn mtls(&self) -> EtcdTlsConfig {
            EtcdTlsConfig {
                ca_cert_file: self.path("cert.pem"),
                client_cert_file: Some(self.path("cert.pem")),
                client_key_file: Some(self.path("key.pem")),
                server_name: None,
            }
        }
    }

    fn config(endpoint: &str, tls: Option<EtcdTlsConfig>) -> EtcdConfig {
        EtcdConfig {
            endpoints: vec![endpoint.to_string()],
            tls,
            ..EtcdConfig::default()
        }
    }

    fn refusal(config: &EtcdConfig) -> String {
        connect_options(config)
            .map(|_| ())
            .expect_err("the configuration must be refused")
            .to_string()
    }

    #[test]
    fn plaintext_settings_build_options() {
        connect_options(&config("http://etcd:2379", None)).expect("plaintext is accepted");
    }

    #[test]
    fn tls_settings_with_valid_files_build_options() {
        let files = PemFiles::new();
        connect_options(&config("https://etcd:2379", Some(files.mtls())))
            .expect("mTLS is accepted");
        let server_only = EtcdTlsConfig {
            client_cert_file: None,
            client_key_file: None,
            server_name: Some("localhost".to_string()),
            ..files.mtls()
        };
        connect_options(&config("https://127.0.0.1:2379", Some(server_only)))
            .expect("TLS without a client certificate is accepted");
    }

    #[test]
    fn an_https_endpoint_without_tls_settings_is_invalid_config() {
        let message = refusal(&config("https://etcd:2379", None));
        assert!(
            message.contains("uses https://, which needs persistence.config.tls"),
            "{message}"
        );
    }

    #[test]
    fn an_http_endpoint_with_tls_settings_is_invalid_config() {
        let message = refusal(&config("http://etcd:2379", Some(PemFiles::new().mtls())));
        assert!(
            message.contains("must start with https://, because persistence.config.tls is set"),
            "{message}"
        );
    }

    #[test]
    fn an_endpoint_without_a_scheme_is_invalid_config() {
        let message = refusal(&config("etcd:2379", None));
        assert!(message.contains("http://"), "{message}");
    }

    #[test]
    fn a_client_certificate_without_its_key_is_refused_and_the_reverse() {
        let files = PemFiles::new();
        for tls in [
            EtcdTlsConfig {
                client_key_file: None,
                ..files.mtls()
            },
            EtcdTlsConfig {
                client_cert_file: None,
                ..files.mtls()
            },
        ] {
            let message = refusal(&config("https://etcd:2379", Some(tls)));
            assert!(message.contains("client_cert_file"), "{message}");
            assert!(message.contains("client_key_file"), "{message}");
        }
    }

    #[test]
    fn a_missing_file_is_refused_with_its_setting_name() {
        let files = PemFiles::new();
        let tls = EtcdTlsConfig {
            ca_cert_file: files.path("no-such-file.pem"),
            ..files.mtls()
        };
        let message = refusal(&config("https://etcd:2379", Some(tls)));
        assert!(message.contains("ca_cert_file"), "{message}");
        assert!(message.contains("no-such-file.pem"), "{message}");
    }

    #[test]
    fn a_file_without_a_certificate_is_invalid_config() {
        let files = PemFiles::new();
        let tls = EtcdTlsConfig {
            ca_cert_file: files.path("key.pem"),
            ..files.mtls()
        };
        let message = refusal(&config("https://etcd:2379", Some(tls)));
        assert!(message.contains("ca_cert_file"), "{message}");
    }

    #[test]
    fn a_key_file_without_a_key_is_invalid_config() {
        let files = PemFiles::new();
        let tls = EtcdTlsConfig {
            client_key_file: Some(files.path("cert.pem")),
            ..files.mtls()
        };
        let message = refusal(&config("https://etcd:2379", Some(tls)));
        assert!(message.contains("client_key_file"), "{message}");
    }

    #[test]
    fn a_user_without_a_password_is_refused_and_the_reverse() {
        for (username, password) in [
            (Some("golem".to_string()), None),
            (None, Some(EtcdPassword::from("secret"))),
        ] {
            let message = refusal(&EtcdConfig {
                username,
                password,
                ..config("http://etcd:2379", None)
            });
            assert!(
                message.contains(
                    "persistence.config.username and persistence.config.password must be set \
                     together"
                ),
                "{message}"
            );
        }
    }

    #[test]
    fn a_user_without_tls_is_invalid_config() {
        let message = refusal(&EtcdConfig {
            username: Some("golem".to_string()),
            password: Some(EtcdPassword::from("secret")),
            ..config("http://etcd:2379", None)
        });
        assert!(
            message.contains(
                "persistence.config.username and persistence.config.password need \
                 persistence.config.tls"
            ),
            "{message}"
        );
    }

    #[test]
    fn the_password_never_appears_in_debug_or_display_output() {
        let files = PemFiles::new();
        let config = EtcdConfig {
            username: Some("golem".to_string()),
            password: Some(EtcdPassword::from("s3cr3t-value")),
            ..config("https://etcd:2379", Some(files.mtls()))
        };
        connect_options(&config).expect("a user with a password is accepted");
        let debug = format!("{config:?}");
        let display = config.to_safe_string();
        assert!(!debug.contains("s3cr3t-value"), "{debug}");
        assert!(!display.contains("s3cr3t-value"), "{display}");
        assert!(display.contains("golem"), "{display}");
    }
}
