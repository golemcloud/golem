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
use std::path::{Path, PathBuf};

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
                 (at least ca_cert_file or ca_cert)"
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
    let has_password = match (&config.password, &config.password_file) {
        (Some(_), Some(_)) => {
            return Err(invalid_config(
                "persistence.config.password and persistence.config.password_file must not both \
                 be set"
                    .to_string(),
            ));
        }
        (None, None) => false,
        _ => true,
    };
    match (config.username.is_some(), has_password) {
        (false, false) => Ok(()),
        (true, true) if config.tls.is_some() => Ok(()),
        (true, true) => Err(invalid_config(
            "persistence.config.username and persistence.config.password need \
             persistence.config.tls: over http:// the password, and then the auth token on every \
             request, would cross the network in plain text"
                .to_string(),
        )),
        _ => Err(invalid_config(
            "persistence.config.username and persistence.config.password must be set together \
             (the password directly, or through persistence.config.password_file)"
                .to_string(),
        )),
    }
}

/// Reads `persistence.config.password_file` as it is, but for one trailing line ending, which an
/// editor or `echo` adds and a Kubernetes Secret value does not have.
fn read_password_file(path: &Path) -> Result<String, ShardManagerError> {
    let bytes = std::fs::read(path).map_err(|err| {
        invalid_config(format!(
            "cannot read persistence.config.password_file {}: {err}",
            path.display()
        ))
    })?;
    let mut password = String::from_utf8(bytes).map_err(|_| {
        invalid_config(format!(
            "persistence.config.password_file {} does not hold UTF-8 text",
            path.display()
        ))
    })?;
    if password.ends_with('\n') {
        password.pop();
        if password.ends_with('\r') {
            password.pop();
        }
    }
    Ok(password)
}

#[derive(Clone, Copy)]
enum Pem {
    Certificate,
    PrivateKey,
}

/// A TLS file as it was read at startup.
#[derive(Clone)]
struct ReadFile {
    path: PathBuf,
    content: Vec<u8>,
}

/// Reads one TLS item from its file or from its inline value - at most one of them may be set -
/// and checks that it holds what the setting needs, so that a wrong item stops the startup with
/// the setting's name instead of failing every TLS handshake later. A file read is recorded in
/// `read_files`.
fn read_item(
    (file_setting, file): (&str, Option<&Path>),
    (value_setting, value): (&str, Option<&[u8]>),
    expected: Pem,
    read_files: &mut Vec<ReadFile>,
) -> Result<Option<Vec<u8>>, ShardManagerError> {
    let (bytes, source) = match (file, value) {
        (Some(_), Some(_)) => {
            return Err(invalid_config(format!(
                "persistence.config.tls.{file_setting} and persistence.config.tls.{value_setting} \
                 must not both be set"
            )));
        }
        (None, None) => return Ok(None),
        (Some(path), None) => {
            let bytes = std::fs::read(path).map_err(|err| {
                invalid_config(format!(
                    "cannot read persistence.config.tls.{file_setting} {}: {err}",
                    path.display()
                ))
            })?;
            read_files.push(ReadFile {
                path: path.to_path_buf(),
                content: bytes.clone(),
            });
            (bytes, format!("{file_setting} {}", path.display()))
        }
        (None, Some(value)) => (value.to_vec(), value_setting.to_string()),
    };
    let (found, kind) = match expected {
        Pem::Certificate => (
            CertificateDer::pem_slice_iter(&bytes).any(|item| item.is_ok()),
            "certificate",
        ),
        Pem::PrivateKey => (PrivateKeyDer::from_pem_slice(&bytes).is_ok(), "private key"),
    };
    if !found {
        return Err(invalid_config(format!(
            "persistence.config.tls.{source} holds no PEM {kind}"
        )));
    }
    Ok(Some(bytes))
}

fn tls_options(
    tls: &EtcdTlsConfig,
    read_files: &mut Vec<ReadFile>,
) -> Result<TlsOptions, ShardManagerError> {
    let ca = read_item(
        ("ca_cert_file", tls.ca_cert_file.as_deref()),
        (
            "ca_cert",
            tls.ca_cert.as_ref().map(|value| value.as_slice()),
        ),
        Pem::Certificate,
        read_files,
    )?
    .ok_or_else(|| {
        invalid_config(
            "persistence.config.tls needs persistence.config.tls.ca_cert_file or \
             persistence.config.tls.ca_cert"
                .to_string(),
        )
    })?;
    let mut options = TlsOptions::new().ca_certificate(Certificate::from_pem(ca));
    let cert = read_item(
        ("client_cert_file", tls.client_cert_file.as_deref()),
        (
            "client_cert",
            tls.client_cert.as_ref().map(|value| value.as_slice()),
        ),
        Pem::Certificate,
        read_files,
    )?;
    let key = read_item(
        ("client_key_file", tls.client_key_file.as_deref()),
        (
            "client_key",
            tls.client_key.as_ref().map(|value| value.expose()),
        ),
        Pem::PrivateKey,
        read_files,
    )?;
    match (cert, key) {
        (Some(cert), Some(key)) => options = options.identity(Identity::from_pem(cert, key)),
        (None, None) => {}
        _ => {
            return Err(invalid_config(
                "a client certificate and its key must be set together: \
                 persistence.config.tls.client_cert_file or client_cert, and \
                 persistence.config.tls.client_key_file or client_key"
                    .to_string(),
            ));
        }
    }
    if let Some(name) = &tls.server_name {
        options = options.domain_name(name);
    }
    Ok(options)
}

/// What both etcd clients connect with - the TLS material and the credentials - read and checked
/// once, before the campaign. Both clients get the same certificates even when a file changes
/// between their connects, and [`Self::tls_files_changed`] tells whether a TLS file on disk now
/// differs from what was read.
#[derive(Clone)]
pub struct EtcdConnectSettings {
    options: ConnectOptions,
    tls_files: Vec<ReadFile>,
}

impl EtcdConnectSettings {
    pub fn load(config: &EtcdConfig) -> Result<Self, ShardManagerError> {
        check_endpoints(config)?;
        check_credentials(config)?;

        let mut tls_files = Vec::new();
        let mut options = ConnectOptions::new().with_connect_timeout(config.connect_timeout);
        if let Some(tls) = &config.tls {
            options = options.with_tls(tls_options(tls, &mut tls_files)?);
        }
        let password = match (&config.password, &config.password_file) {
            (Some(password), _) => Some(password.expose().to_string()),
            (None, Some(file)) => Some(read_password_file(file)?),
            (None, None) => None,
        };
        if let (Some(username), Some(password)) = (&config.username, password) {
            // etcd drops a token that goes unused for its `--auth-token-ttl`, a JWT token at the
            // end of its lifetime, and every simple token when a member restarts. A request already
            // sent with such a token - a standby's campaign waits inside etcd for as long as the
            // leader lives - can only be repaired when it fails: etcd-client then logs in again and
            // sends the request once more. etcd checks the token before it applies a request, so
            // the second send cannot apply a write twice.
            options = options
                .with_user(username, password)
                .with_auto_token_refresh(true);
        }
        Ok(Self { options, tls_files })
    }

    /// Whether a TLS file on disk now differs from what [`Self::load`] read; a file that can no
    /// longer be read counts as changed.
    pub fn tls_files_changed(&self) -> bool {
        self.tls_files
            .iter()
            .any(|file| std::fs::read(&file.path).map_or(true, |content| content != file.content))
    }
}

/// A client for ordinary key/value requests, bounded by `request_timeout`.
pub async fn connect_for_requests(config: &EtcdConfig) -> Result<Client, ShardManagerError> {
    connect_requests_with(config, &EtcdConnectSettings::load(config)?).await
}

/// [`connect_for_requests`] with settings already loaded.
pub async fn connect_requests_with(
    config: &EtcdConfig,
    settings: &EtcdConnectSettings,
) -> Result<Client, ShardManagerError> {
    let options = settings
        .options
        .clone()
        .with_timeout(config.request_timeout);
    Ok(Client::connect(&config.endpoints, Some(options)).await?)
}

/// A client for the leadership campaign and its lease keepalive, with no request timeout.
pub async fn connect_election_with(
    config: &EtcdConfig,
    settings: &EtcdConnectSettings,
) -> Result<Client, ShardManagerError> {
    Ok(Client::connect(&config.endpoints, Some(settings.options.clone())).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{EtcdPassword, EtcdTlsConfig, EtcdTlsKey};
    use golem_common::SafeDisplay;
    use golem_common::model::base64::Base64;
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
                ca_cert_file: Some(self.path("cert.pem")),
                ca_cert: None,
                client_cert_file: Some(self.path("cert.pem")),
                client_cert: None,
                client_key_file: Some(self.path("key.pem")),
                client_key: None,
                server_name: None,
            }
        }

        fn read(&self, name: &str) -> Vec<u8> {
            std::fs::read(self.path(name)).unwrap()
        }

        /// [`Self::mtls`] with every item as an inline value.
        fn inline_mtls(&self) -> EtcdTlsConfig {
            EtcdTlsConfig {
                ca_cert_file: None,
                ca_cert: Some(Base64(self.read("cert.pem"))),
                client_cert_file: None,
                client_cert: Some(Base64(self.read("cert.pem"))),
                client_key_file: None,
                client_key: Some(EtcdTlsKey::from(self.read("key.pem"))),
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
        EtcdConnectSettings::load(config)
            .map(|_| ())
            .expect_err("the configuration must be refused")
            .to_string()
    }

    #[test]
    fn plaintext_settings_build_options() {
        EtcdConnectSettings::load(&config("http://etcd:2379", None))
            .expect("plaintext is accepted");
    }

    #[test]
    fn tls_settings_with_valid_files_build_options() {
        let files = PemFiles::new();
        EtcdConnectSettings::load(&config("https://etcd:2379", Some(files.mtls())))
            .expect("mTLS is accepted");
        let server_only = EtcdTlsConfig {
            client_cert_file: None,
            client_key_file: None,
            server_name: Some("localhost".to_string()),
            ..files.mtls()
        };
        EtcdConnectSettings::load(&config("https://127.0.0.1:2379", Some(server_only)))
            .expect("TLS without a client certificate is accepted");
    }

    #[test]
    fn an_https_endpoint_without_tls_settings_is_refused() {
        let message = refusal(&config("https://etcd:2379", None));
        assert!(
            message.contains("uses https://, which needs persistence.config.tls"),
            "{message}"
        );
    }

    #[test]
    fn an_http_endpoint_with_tls_settings_is_refused() {
        let message = refusal(&config("http://etcd:2379", Some(PemFiles::new().mtls())));
        assert!(
            message.contains("must start with https://, because persistence.config.tls is set"),
            "{message}"
        );
    }

    #[test]
    fn an_endpoint_without_a_scheme_is_refused() {
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
            ca_cert_file: Some(files.path("no-such-file.pem")),
            ..files.mtls()
        };
        let message = refusal(&config("https://etcd:2379", Some(tls)));
        assert!(message.contains("ca_cert_file"), "{message}");
        assert!(message.contains("no-such-file.pem"), "{message}");
    }

    #[test]
    fn a_file_without_a_certificate_is_refused() {
        let files = PemFiles::new();
        let tls = EtcdTlsConfig {
            ca_cert_file: Some(files.path("key.pem")),
            ..files.mtls()
        };
        let message = refusal(&config("https://etcd:2379", Some(tls)));
        assert!(message.contains("ca_cert_file"), "{message}");
    }

    #[test]
    fn a_key_file_without_a_key_is_refused() {
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
    fn a_user_without_tls_is_refused() {
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
        EtcdConnectSettings::load(&config).expect("a user with a password is accepted");
        let debug = format!("{config:?}");
        let display = config.to_safe_string();
        assert!(!debug.contains("s3cr3t-value"), "{debug}");
        assert!(!display.contains("s3cr3t-value"), "{display}");
        assert!(display.contains("golem"), "{display}");
    }

    fn with_user(tls: Option<EtcdTlsConfig>) -> EtcdConfig {
        EtcdConfig {
            username: Some("golem".to_string()),
            ..config("https://etcd:2379", tls)
        }
    }

    #[test]
    fn a_password_file_is_read_without_its_trailing_line_ending() {
        let files = PemFiles::new();
        for (content, expected) in [
            ("s3cr3t\n", "s3cr3t"),
            ("s3cr3t\r\n", "s3cr3t"),
            ("s3cr3t", "s3cr3t"),
            (" 0123 \n\n", " 0123 \n"),
        ] {
            std::fs::write(files.path("password"), content).unwrap();
            assert_eq!(
                read_password_file(&files.path("password")).unwrap(),
                expected,
                "{content:?}"
            );
        }
        let config = EtcdConfig {
            password_file: Some(files.path("password")),
            ..with_user(Some(files.mtls()))
        };
        EtcdConnectSettings::load(&config).expect("a user with a password file is accepted");
    }

    #[test]
    fn a_password_and_a_password_file_together_are_refused() {
        let files = PemFiles::new();
        std::fs::write(files.path("password"), "s3cr3t").unwrap();
        let message = refusal(&EtcdConfig {
            password: Some(EtcdPassword::from("s3cr3t")),
            password_file: Some(files.path("password")),
            ..with_user(Some(files.mtls()))
        });
        assert!(
            message.contains(
                "persistence.config.password and persistence.config.password_file must not both \
                 be set"
            ),
            "{message}"
        );
    }

    #[test]
    fn an_unreadable_password_file_is_refused_with_its_setting_name() {
        let files = PemFiles::new();
        let message = refusal(&EtcdConfig {
            password_file: Some(files.path("no-such-password")),
            ..with_user(Some(files.mtls()))
        });
        assert!(
            message.contains("cannot read persistence.config.password_file"),
            "{message}"
        );
        assert!(message.contains("no-such-password"), "{message}");
    }

    #[test]
    fn inline_tls_values_build_options() {
        let files = PemFiles::new();
        EtcdConnectSettings::load(&config("https://etcd:2379", Some(files.inline_mtls())))
            .expect("inline values are accepted");
    }

    #[test]
    fn a_file_and_a_value_for_one_item_are_refused() {
        let files = PemFiles::new();
        for (tls, file_setting, value_setting) in [
            (
                EtcdTlsConfig {
                    ca_cert: files.inline_mtls().ca_cert,
                    ..files.mtls()
                },
                "ca_cert_file",
                "ca_cert",
            ),
            (
                EtcdTlsConfig {
                    client_cert: files.inline_mtls().client_cert,
                    ..files.mtls()
                },
                "client_cert_file",
                "client_cert",
            ),
            (
                EtcdTlsConfig {
                    client_key: files.inline_mtls().client_key,
                    ..files.mtls()
                },
                "client_key_file",
                "client_key",
            ),
        ] {
            let message = refusal(&config("https://etcd:2379", Some(tls)));
            assert!(
                message.contains(&format!(
                    "persistence.config.tls.{file_setting} and \
                     persistence.config.tls.{value_setting} must not both be set"
                )),
                "{message}"
            );
        }
    }

    #[test]
    fn tls_without_a_ca_is_refused() {
        let files = PemFiles::new();
        let message = refusal(&config(
            "https://etcd:2379",
            Some(EtcdTlsConfig {
                ca_cert_file: None,
                ..files.mtls()
            }),
        ));
        assert!(
            message.contains(
                "persistence.config.tls needs persistence.config.tls.ca_cert_file or \
                 persistence.config.tls.ca_cert"
            ),
            "{message}"
        );
    }

    #[test]
    fn an_inline_value_without_a_certificate_is_refused_with_its_setting_name() {
        let files = PemFiles::new();
        let message = refusal(&config(
            "https://etcd:2379",
            Some(EtcdTlsConfig {
                ca_cert: Some(Base64(b"not a certificate".to_vec())),
                ..files.inline_mtls()
            }),
        ));
        assert!(
            message.contains("persistence.config.tls.ca_cert holds no PEM certificate"),
            "{message}"
        );
    }

    #[test]
    fn an_inline_key_never_appears_in_debug_or_display_output() {
        let files = PemFiles::new();
        let config = config("https://etcd:2379", Some(files.inline_mtls()));
        let key = String::from_utf8(files.read("key.pem")).unwrap();
        let key_body = key.lines().nth(1).expect("a PEM body line");
        let debug = format!("{config:?}");
        let display = config.to_safe_string();
        assert!(!debug.contains(key_body), "{debug}");
        assert!(!display.contains(key_body), "{display}");
        assert!(
            display.contains("tls client key: inline value"),
            "{display}"
        );
    }

    #[test]
    fn a_changed_tls_file_is_noticed() {
        let files = PemFiles::new();
        let settings = EtcdConnectSettings::load(&config("https://etcd:2379", Some(files.mtls())))
            .expect("mTLS is accepted");
        assert!(!settings.tls_files_changed());

        let renewed = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        std::fs::write(files.path("cert.pem"), renewed.cert.pem()).unwrap();
        assert!(settings.tls_files_changed());
    }
}
