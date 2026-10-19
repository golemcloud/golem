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
use crate::sharding::error::{ShardManagerError, is_certificate_failure};
use crate::sharding::persistence::STATE_KEY;
use etcd_client::{Certificate, Client, ConnectOptions, GetOptions, Identity, TlsOptions};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{error, info, warn};
use x509_parser::time::ASN1Time;

/// How often a quiet etcd connection is probed. etcd closes a connection that pings faster than
/// its `--grpc-keepalive-min-time`, five seconds by default, with `too_many_pings`; one second of
/// margin keeps clear of it.
const KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(6);

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
    // setting; an `https://` endpoint without them fails only at its first handshake; and for a
    // scheme-less `host:port` it picks the scheme itself, from whether TLS options are set.
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

/// One TLS item as read at startup, and where it came from.
struct Item {
    pem: Vec<u8>,
    /// The setting and, for a file, its path: what a refusal names.
    source: String,
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
) -> Result<Option<Item>, ShardManagerError> {
    let (pem, source) = match (file, value) {
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
            (
                bytes,
                format!("persistence.config.tls.{file_setting} {}", path.display()),
            )
        }
        (None, Some(value)) => (
            value.to_vec(),
            format!("persistence.config.tls.{value_setting}"),
        ),
    };
    let (found, kind) = match expected {
        Pem::Certificate => (
            CertificateDer::pem_slice_iter(&pem).any(|item| item.is_ok()),
            "certificate",
        ),
        Pem::PrivateKey => {
            // rustls reads no encrypted key; say so instead of "no key".
            if pem
                .windows(ENCRYPTED_KEY_LABEL.len())
                .any(|window| window == ENCRYPTED_KEY_LABEL)
            {
                return Err(invalid_config(format!(
                    "{source} holds an encrypted private key, which is not supported: give the key \
                     unencrypted, as a Kubernetes TLS Secret holds it"
                )));
            }
            (PrivateKeyDer::from_pem_slice(&pem).is_ok(), "private key")
        }
    };
    if !found {
        return Err(invalid_config(format!("{source} holds no PEM {kind}")));
    }
    Ok(Some(Item { pem, source }))
}

const ENCRYPTED_KEY_LABEL: &[u8] = b"-----BEGIN ENCRYPTED PRIVATE KEY-----";

pub(crate) fn sha256_hex(der: &[u8]) -> String {
    hex::encode(Sha256::digest(der))
}

/// Logs each certificate of `item` - subject, end of validity and SHA-256 fingerprint - so that an
/// operator can see which certificate a replica loaded and when it expires, and returns them.
///
/// For this replica's own certificate (`own`), the first one must be valid now - not expired, and
/// not dated ahead of this node's clock: etcd would refuse it with a generic alert that looks like
/// a wrong CA. A CA item must hold at least one
/// certificate that has not expired; an expired one beside it is only reported, because a CA
/// bundle holds the old CA for a while during a rotation.
fn check_certificates(
    item: &Item,
    own: bool,
) -> Result<Vec<CertificateDer<'static>>, ShardManagerError> {
    let certificates: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&item.pem)
        .filter_map(Result::ok)
        .collect();
    let now = ASN1Time::now().timestamp();
    let mut valid = 0;
    for (index, der) in certificates.iter().enumerate() {
        let (_, certificate) = x509_parser::parse_x509_certificate(der).map_err(|err| {
            invalid_config(format!(
                "{} holds a certificate that cannot be parsed: {err}",
                item.source
            ))
        })?;
        let subject = certificate.subject().to_string();
        let not_before = certificate.validity().not_before;
        let not_after = certificate.validity().not_after;
        let expired = not_after.timestamp() < now;
        let sha256 = sha256_hex(der);
        if expired {
            warn!(source = item.source, subject, %not_after, sha256, "etcd TLS certificate has expired");
        } else {
            info!(source = item.source, subject, %not_after, sha256, "etcd TLS certificate");
            valid += 1;
        }
        if own && index == 0 && expired {
            return Err(invalid_config(format!(
                "{}: the certificate of {subject} expired at {not_after}",
                item.source
            )));
        }
        if own && index == 0 && not_before.timestamp() > now {
            return Err(invalid_config(format!(
                "{}: the certificate of {subject} is valid only from {not_before}; check this \
                 node's clock",
                item.source
            )));
        }
    }
    if !own && valid == 0 {
        return Err(invalid_config(format!(
            "{}: every certificate in it has expired",
            item.source
        )));
    }
    Ok(certificates)
}

/// Refuses a key that does not belong to its certificate, which etcd would otherwise reject
/// with a generic alert at the first handshake.
fn check_key_matches(
    certificates: Vec<CertificateDer<'static>>,
    cert: &Item,
    key: &Item,
) -> Result<(), ShardManagerError> {
    let key_der = PrivateKeyDer::from_pem_slice(&key.pem)
        .map_err(|err| invalid_config(format!("{}: {err}", key.source)))?;
    match CertifiedKey::from_der(
        certificates,
        key_der,
        &rustls::crypto::ring::default_provider(),
    ) {
        Ok(_) => Ok(()),
        Err(rustls::Error::InconsistentKeys(rustls::InconsistentKeys::KeyMismatch)) => {
            Err(invalid_config(format!(
                "{} does not belong to the certificate in {}",
                key.source, cert.source
            )))
        }
        Err(err) => Err(invalid_config(format!("{}: {err}", key.source))),
    }
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
    check_certificates(&ca, false)?;
    let mut options = TlsOptions::new().ca_certificate(Certificate::from_pem(ca.pem));
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
        (Some(cert), Some(key)) => {
            let certificates = check_certificates(&cert, true)?;
            check_key_matches(certificates, &cert, &key)?;
            options = options.identity(Identity::from_pem(cert.pem, key.pem));
        }
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

/// What etcd makes of the TLS files as they are on disk now.
enum FilesOnDisk {
    /// etcd completed the handshake: a restart that reads these files can connect.
    Accepted,
    /// etcd refused them too, or they cannot be used: a restart would not help.
    Refused(String),
    /// No answer, such as while etcd is down.
    Unknown(String),
}

/// Reads the TLS files again, with the startup checks, and tries them on a fresh connection that
/// sends no login - the handshake is the question, and a login costs etcd a password check.
async fn try_files_on_disk(config: &EtcdConfig) -> FilesOnDisk {
    let Some(tls) = &config.tls else {
        return FilesOnDisk::Unknown("persistence.config.tls is not set".to_string());
    };
    let options = match tls_options(tls, &mut Vec::new()) {
        Ok(tls) => ConnectOptions::new()
            .with_connect_timeout(config.connect_timeout)
            .with_timeout(config.request_timeout)
            .with_tls(tls),
        Err(err) => return FilesOnDisk::Refused(err.to_string()),
    };
    let client = match Client::connect(&config.endpoints, Some(options)).await {
        Ok(client) => client,
        Err(err) => return FilesOnDisk::Unknown(err.to_string()),
    };
    match client
        .kv_client()
        .get(STATE_KEY, Some(GetOptions::new().with_count_only()))
        .await
    {
        Ok(_) => FilesOnDisk::Accepted,
        Err(err) if is_certificate_failure(&err) => FilesOnDisk::Refused(err.to_string()),
        // etcd answered, so the handshake passed; with authentication enabled, it refuses the
        // read itself, which carries no login.
        Err(etcd_client::Error::GRpcStatus(status))
            if matches!(
                status.code(),
                tonic::Code::InvalidArgument
                    | tonic::Code::PermissionDenied
                    | tonic::Code::Unauthenticated
            ) =>
        {
            FilesOnDisk::Accepted
        }
        Err(err) => FilesOnDisk::Unknown(err.to_string()),
    }
}

/// A count-only read through `client`, bounded by `request_timeout` also on the election client,
/// which has no request timeout of its own; `None` when it passed or ran out of time.
async fn refusal_on(client: &Client, request_timeout: Duration) -> Option<etcd_client::Error> {
    let mut kv = client.kv_client();
    let read = kv.get(STATE_KEY, Some(GetOptions::new().with_count_only()));
    match tokio::time::timeout(request_timeout, read).await {
        Ok(Err(err)) if is_certificate_failure(&err) => Some(err),
        _ => None,
    }
}

/// Watches, while the replica runs, for etcd refusing a certificate. Such a refusal is retried like
/// a network fault, because etcd's own certificate can be fixed on etcd's side; but renewed files
/// take effect only on a restart.
///
/// Every `retry_max_delay` it reads through both clients: they hold separate connections, and the
/// one that reconnects first may be either. When etcd refuses a certificate, the TLS files have
/// changed since startup, and etcd accepts the files as they are on disk now, this returns an
/// error, which stops the replica so that its restart reads them. Otherwise a restart would not
/// help - the files are the same, or etcd refuses the new ones too, as when etcd's own
/// certificate has expired - so it logs the refusal and goes on.
pub(crate) async fn watch_refused_certificates(
    config: EtcdConfig,
    settings: EtcdConnectSettings,
    requests: Client,
    election: Client,
) -> anyhow::Result<()> {
    loop {
        tokio::time::sleep(config.retry_max_delay).await;
        let refused = match refusal_on(&requests, config.request_timeout).await {
            Some(err) => err,
            None => match refusal_on(&election, config.request_timeout).await {
                Some(err) => err,
                None => continue,
            },
        };
        if !settings.tls_files_changed() {
            error!(
                error = %refused,
                "etcd refuses the TLS certificates, and the TLS files have not changed; retrying"
            );
            continue;
        }
        match try_files_on_disk(&config).await {
            FilesOnDisk::Accepted => {
                return Err(
                    anyhow::Error::new(ShardManagerError::TlsHandshakeFailed(refused)).context(
                        "etcd refuses the TLS certificates read at startup, and the TLS files \
                         have changed since then and etcd accepts them: stopping, so that a \
                         restart reads them",
                    ),
                );
            }
            FilesOnDisk::Refused(files_error) => error!(
                error = %refused,
                files_error,
                "etcd refuses the TLS certificates read at startup and the TLS files on disk \
                 alike, so a restart would not help; retrying"
            ),
            FilesOnDisk::Unknown(files_error) => warn!(
                error = %refused,
                files_error,
                "etcd refuses the TLS certificates read at startup, and the TLS files on disk \
                 cannot be tried now; retrying"
            ),
        }
    }
}

/// A client for ordinary key/value requests, bounded by `request_timeout`.
///
/// The connection is probed only while a stream is open, never while idle: etcd counts a ping on a
/// connection with no open stream as a strike and closes the connection after the third, unless it
/// was started to permit them, which is not its default. The client library probes while idle
/// unless told not to, so this is set explicitly. Hyper sends a ping as soon as a stream opens
/// after an interval without a read, so the first request after 6 s of silence probes the
/// connection: a dead idle connection costs that one request (up to `request_timeout`) and is then
/// replaced. Reads retry onto the new connection; a write fails with an unknown outcome, as any
/// write in flight does.
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
        .with_timeout(config.request_timeout)
        .with_keep_alive(KEEP_ALIVE_INTERVAL, config.request_timeout)
        .with_keep_alive_while_idle(false);
    Ok(Client::connect(&config.endpoints, Some(options)).await?)
}

/// A client for the leadership campaign and its lease keepalive, with no request timeout.
///
/// A connection that goes silent without a close, as one through a node that lost power does,
/// would otherwise hold the watch of the election and the lease keepalive for good, because a
/// request timeout does not replace the connection. The probe finds it dead within an interval and
/// a timeout, and the client connects again. This client always has a stream open (the watch of
/// the election and the keepalive of the lease), so it is probed whenever it is in use, and it is
/// not probed while idle: a ping with no open stream is refused by etcd and closes the connection.
///
/// The connect itself is bounded: with a user, it waits for etcd's answer to the login, and a
/// login that is never answered - a connection cut off right after the handshake - would otherwise
/// hold the caller for as long as TCP takes to notice.
pub async fn connect_election_with(
    config: &EtcdConfig,
    settings: &EtcdConnectSettings,
) -> Result<Client, ShardManagerError> {
    let options = settings
        .options
        .clone()
        .with_keep_alive(KEEP_ALIVE_INTERVAL, config.request_timeout)
        .with_keep_alive_while_idle(false);
    let connect = Client::connect(&config.endpoints, Some(options));
    match tokio::time::timeout(config.connect_timeout + config.request_timeout, connect).await {
        Ok(client) => Ok(client?),
        Err(_) => Err(ShardManagerError::Timeout),
    }
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
        assert!(
            message.contains(&format!(
                "persistence.config.tls.ca_cert_file {} holds no PEM certificate",
                files.path("key.pem").display()
            )),
            "{message}"
        );
    }

    #[test]
    fn a_key_file_without_a_key_is_refused() {
        let files = PemFiles::new();
        let tls = EtcdTlsConfig {
            client_key_file: Some(files.path("cert.pem")),
            ..files.mtls()
        };
        let message = refusal(&config("https://etcd:2379", Some(tls)));
        assert!(
            message.contains(&format!(
                "persistence.config.tls.client_key_file {} holds no PEM private key",
                files.path("cert.pem").display()
            )),
            "{message}"
        );
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

    /// A PEM block with `label` around `der`.
    fn pem(label: &str, der: &[u8]) -> String {
        use base64::Engine;
        let body = base64::engine::general_purpose::STANDARD.encode(der);
        let lines: Vec<&str> = body
            .as_bytes()
            .chunks(64)
            .map(|line| std::str::from_utf8(line).unwrap())
            .collect();
        format!(
            "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
            lines.join("\n")
        )
    }

    /// One DER element.
    fn der(tag: u8, content: &[u8]) -> Vec<u8> {
        let mut element = vec![tag];
        if content.len() < 0x80 {
            element.push(content.len() as u8);
        } else {
            let length = content.len().to_be_bytes();
            let length = &length[length.iter().position(|byte| *byte != 0).unwrap()..];
            element.push(0x80 | length.len() as u8);
            element.extend_from_slice(length);
        }
        element.extend_from_slice(content);
        element
    }

    /// The content of one DER element.
    fn der_content(element: &[u8]) -> &[u8] {
        let header = match element[1] {
            short if short < 0x80 => 2,
            long => 2 + (long & 0x7f) as usize,
        };
        &element[header..]
    }

    /// The elements of a DER sequence's content, each with its header.
    fn der_elements(mut content: &[u8]) -> Vec<&[u8]> {
        let mut elements = Vec::new();
        while !content.is_empty() {
            let (length, header) = match content[1] {
                short if short < 0x80 => (short as usize, 2),
                long => {
                    let bytes = (long & 0x7f) as usize;
                    let length = content[2..2 + bytes]
                        .iter()
                        .fold(0, |length, byte| length << 8 | *byte as usize);
                    (length, 2 + bytes)
                }
            };
            elements.push(&content[..header + length]);
            content = &content[header + length..];
        }
        elements
    }

    /// An RSA key as PKCS#1 PEM (`RSA PRIVATE KEY`), and its PKCS#8 DER for rcgen.
    fn pkcs1_key() -> (String, Vec<u8>) {
        use rsa::pkcs1::EncodeRsaPrivateKey;
        use rsa::pkcs8::EncodePrivateKey;
        let key = rsa::RsaPrivateKey::new(&mut rsa::rand_core::OsRng, 2048).unwrap();
        (
            key.to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
                .unwrap()
                .to_string(),
            key.to_pkcs8_der().unwrap().as_bytes().to_vec(),
        )
    }

    /// An EC P-256 key as SEC1 PEM (`EC PRIVATE KEY`, with the curve named, as Go and so
    /// cert-manager write it), and its PKCS#8 DER for rcgen.
    fn sec1_key() -> (String, Vec<u8>) {
        const PRIME256V1: &[u8] = &[0x06, 0x08, 0x2A, 0x86, 0x48, 0xCE, 0x3D, 0x03, 0x01, 0x07];
        let pkcs8 = rcgen::KeyPair::generate().unwrap().serialize_der();
        // PrivateKeyInfo { version, algorithm, privateKey: OCTET STRING holding ECPrivateKey }
        let info = der_elements(der_content(&pkcs8));
        let ec_private_key = der_content(info[2]);
        // ECPrivateKey { version, privateKey, [0] parameters (left out here), [1] publicKey }
        let fields = der_elements(der_content(ec_private_key));
        let mut content = [fields[0], fields[1]].concat();
        content.extend(der(0xA0, PRIME256V1));
        for field in &fields[2..] {
            if field[0] != 0xA0 {
                content.extend_from_slice(field);
            }
        }
        (pem("EC PRIVATE KEY", &der(0x30, &content)), pkcs8)
    }

    /// A self-signed certificate for the key in `pkcs8`.
    fn certificate_for(pkcs8: Vec<u8>) -> String {
        let key = rcgen::KeyPair::try_from(&PrivateKeyDer::Pkcs8(pkcs8.into())).unwrap();
        rcgen::CertificateParams::new(vec!["golem-shard-manager".to_string()])
            .unwrap()
            .self_signed(&key)
            .unwrap()
            .pem()
    }

    #[test]
    fn pkcs1_and_sec1_keys_are_accepted() {
        let files = PemFiles::new();
        for (format, (key, pkcs8), label) in [
            ("pkcs1", pkcs1_key(), "-----BEGIN RSA PRIVATE KEY-----"),
            ("sec1", sec1_key(), "-----BEGIN EC PRIVATE KEY-----"),
        ] {
            assert!(key.starts_with(label), "{format}");
            std::fs::write(files.path(&format!("{format}.pem")), certificate_for(pkcs8)).unwrap();
            std::fs::write(files.path(&format!("{format}-key.pem")), key).unwrap();
            let tls = EtcdTlsConfig {
                client_cert_file: Some(files.path(&format!("{format}.pem"))),
                client_key_file: Some(files.path(&format!("{format}-key.pem"))),
                ..files.mtls()
            };
            if let Err(err) = EtcdConnectSettings::load(&config("https://etcd:2379", Some(tls))) {
                panic!("a {format} key must be accepted: {err}");
            }
        }
    }

    #[test]
    fn an_encrypted_key_is_refused() {
        let files = PemFiles::new();
        std::fs::write(
            files.path("encrypted-key.pem"),
            pem("ENCRYPTED PRIVATE KEY", b"an encrypted key"),
        )
        .unwrap();
        let message = refusal(&config(
            "https://etcd:2379",
            Some(EtcdTlsConfig {
                client_key_file: Some(files.path("encrypted-key.pem")),
                ..files.mtls()
            }),
        ));
        assert!(
            message.contains("holds an encrypted private key, which is not supported"),
            "{message}"
        );
    }

    #[test]
    fn a_key_that_does_not_belong_to_its_certificate_is_refused() {
        let files = PemFiles::new();
        std::fs::write(
            files.path("other-key.pem"),
            rcgen::KeyPair::generate().unwrap().serialize_pem(),
        )
        .unwrap();
        let message = refusal(&config(
            "https://etcd:2379",
            Some(EtcdTlsConfig {
                client_key_file: Some(files.path("other-key.pem")),
                ..files.mtls()
            }),
        ));
        assert!(
            message.contains(
                "does not belong to the certificate in persistence.config.tls.client_cert_file"
            ),
            "{message}"
        );
    }

    /// A self-signed certificate that expired in 2002, and its key.
    fn expired_certificate() -> (String, String) {
        certificate_valid_in(2001, 2002)
    }

    /// A self-signed certificate valid from 1 January of `from` to 1 January of `to`, and its key.
    fn certificate_valid_in(from: i32, to: i32) -> (String, String) {
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params =
            rcgen::CertificateParams::new(vec!["golem-shard-manager".to_string()]).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, "golem-shard-manager");
        params.not_before = rcgen::date_time_ymd(from, 1, 1);
        params.not_after = rcgen::date_time_ymd(to, 1, 1);
        (params.self_signed(&key).unwrap().pem(), key.serialize_pem())
    }

    #[test]
    fn an_expired_client_certificate_is_refused() {
        let files = PemFiles::new();
        let (cert, key) = expired_certificate();
        std::fs::write(files.path("expired.pem"), cert).unwrap();
        std::fs::write(files.path("expired-key.pem"), key).unwrap();
        let message = refusal(&config(
            "https://etcd:2379",
            Some(EtcdTlsConfig {
                client_cert_file: Some(files.path("expired.pem")),
                client_key_file: Some(files.path("expired-key.pem")),
                ..files.mtls()
            }),
        ));
        assert!(
            message.contains("persistence.config.tls.client_cert_file"),
            "{message}"
        );
        assert!(
            message.contains("the certificate of CN=golem-shard-manager expired at"),
            "{message}"
        );
    }

    #[test]
    // A node clock that is behind, or a certificate dated ahead: etcd would refuse it with the same
    // generic alert as an expired one.
    fn a_client_certificate_that_is_not_valid_yet_is_refused() {
        let files = PemFiles::new();
        let (cert, key) = certificate_valid_in(2120, 2121);
        std::fs::write(files.path("future.pem"), cert).unwrap();
        std::fs::write(files.path("future-key.pem"), key).unwrap();
        let message = refusal(&config(
            "https://etcd:2379",
            Some(EtcdTlsConfig {
                client_cert_file: Some(files.path("future.pem")),
                client_key_file: Some(files.path("future-key.pem")),
                ..files.mtls()
            }),
        ));
        assert!(
            message.contains("persistence.config.tls.client_cert_file"),
            "{message}"
        );
        assert!(
            message.contains("the certificate of CN=golem-shard-manager is valid only from"),
            "{message}"
        );
    }

    #[test]
    fn a_ca_whose_certificates_all_expired_is_refused_but_one_valid_is_enough() {
        let files = PemFiles::new();
        let (expired, _) = expired_certificate();
        std::fs::write(files.path("expired-ca.pem"), &expired).unwrap();
        let message = refusal(&config(
            "https://etcd:2379",
            Some(EtcdTlsConfig {
                ca_cert_file: Some(files.path("expired-ca.pem")),
                ..files.mtls()
            }),
        ));
        assert!(
            message.contains("every certificate in it has expired"),
            "{message}"
        );

        let bundle = format!(
            "{expired}{}",
            String::from_utf8(files.read("cert.pem")).unwrap()
        );
        std::fs::write(files.path("bundle.pem"), bundle).unwrap();
        EtcdConnectSettings::load(&config(
            "https://etcd:2379",
            Some(EtcdTlsConfig {
                ca_cert_file: Some(files.path("bundle.pem")),
                ..files.mtls()
            }),
        ))
        .expect("a CA bundle with one valid certificate is accepted");
    }

    #[test]
    fn an_inline_certificate_shows_its_fingerprint() {
        let files = PemFiles::new();
        let display = config("https://etcd:2379", Some(files.inline_mtls())).to_safe_string();
        let der = CertificateDer::pem_slice_iter(&files.read("cert.pem"))
            .next()
            .unwrap()
            .unwrap();
        assert!(
            display.contains(&format!(
                "tls ca cert: inline value, sha256 {}",
                sha256_hex(&der)
            )),
            "{display}"
        );
    }
}
