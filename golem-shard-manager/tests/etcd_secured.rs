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

//! Distributed mode against an etcd that requires TLS with client certificates, and against an
//! etcd with TLS and authentication enabled.
//!
//! Each test starts its own etcd container, so the shared etcd of `etcd_backed` is never changed.

use crate::Tracing;
use etcd_client::{ConnectOptions, Permission, TlsOptions};
use golem_common::model::base64::Base64;
use golem_shard_manager::config::{
    EtcdConfig, EtcdPassword, EtcdTlsConfig, GrpcApiConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_shard_manager::{
    Deployment, LEADER_ELECTION_NAME, LeaderElection, RunDetails, STATE_KEY, ShardManagerError,
};
use golem_test_framework::components::etcd::docker_etcd::{DockerEtcd, DockerEtcdOptions};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use test_r::{inherit_test_dep, test, test_dep};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

inherit_test_dep!(Tracing);

/// Long enough that a hand-over which waits for a lease to expire, instead of a revoke, misses
/// [`HANDOVER_BOUND`] by seconds, even on a slow runner.
const LEASE_TTL: Duration = Duration::from_secs(10);
/// A revoke hands over within this time; waiting out a lease takes most of [`LEASE_TTL`].
const HANDOVER_BOUND: Duration = Duration::from_secs(5);
/// Time for a second replica to start and queue its campaign behind the leader.
const STANDBY_SETTLE: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// A replica whose etcd connection cannot work must say so within this time, not wait forever.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(15);
const USER: &str = "golem";
const PASSWORD: &str = "s3cret-etcd-password";

#[derive(Debug)]
pub struct CryptoProvider;

/// The shard manager binary installs the ring provider in `main`; a test process must do it itself.
#[test_dep(scope = PerWorker)]
fn crypto_provider() -> CryptoProvider {
    let _ = rustls::crypto::ring::default_provider().install_default();
    CryptoProvider
}

/// A test CA, an etcd server certificate and a client certificate signed by it, and a second CA
/// that signed neither, written as PEM files to a directory that lives as long as this value.
///
/// The server certificate names `localhost` only, not `127.0.0.1`, so a connection to the IP
/// address needs `server_name`.
struct TestPki {
    dir: tempfile::TempDir,
    ca: CertifiedIssuer<'static, KeyPair>,
    ca_pem: String,
    server_pem: String,
    server_key_pem: String,
}

impl TestPki {
    fn new() -> Self {
        Self::with_server_certificate(false)
    }

    /// A PKI whose etcd server certificate expired in 2002.
    fn with_expired_server_certificate() -> Self {
        Self::with_server_certificate(true)
    }

    fn with_server_certificate(expired: bool) -> Self {
        let ca = test_ca("golem test CA");
        let (server_pem, server_key_pem) = signed_by(
            &ca,
            "localhost",
            ExtendedKeyUsagePurpose::ServerAuth,
            if expired {
                Validity::Expired
            } else {
                Validity::Long
            },
        );
        let (client_pem, client_key_pem) = signed_by(
            &ca,
            "golem-shard-manager",
            ExtendedKeyUsagePurpose::ClientAuth,
            Validity::Long,
        );
        let dir = tempfile::tempdir().expect("Cannot create a directory for the test certificates");
        for (name, pem) in [
            ("ca.pem", ca.pem()),
            ("client.pem", client_pem),
            ("client-key.pem", client_key_pem),
            ("other-ca.pem", test_ca("another test CA").pem()),
        ] {
            std::fs::write(dir.path().join(name), pem).expect("Cannot write a test certificate");
        }
        Self {
            dir,
            ca_pem: ca.pem(),
            ca,
            server_pem,
            server_key_pem,
        }
    }

    /// Options for an etcd that requires client certificates, with a new server certificate from
    /// the same CA: valid, as after a renewal, or expired, as when etcd missed one.
    fn new_mtls_etcd_options(&self, validity: Validity) -> DockerEtcdOptions {
        let (server_pem, server_key_pem) = signed_by(
            &self.ca,
            "localhost",
            ExtendedKeyUsagePurpose::ServerAuth,
            validity,
        );
        DockerEtcdOptions::new()
            .with_tls(server_pem, server_key_pem)
            .with_client_cert_auth(self.ca_pem.clone())
    }

    /// Writes a new client certificate from the same CA and its key to `<name>.pem` and
    /// `<name>-key.pem`, as cert-manager renews a mounted Secret, and returns the TLS settings
    /// that read them.
    fn write_client_certificate(&self, name: &str, validity: Validity) -> EtcdTlsConfig {
        let (cert, key) = signed_by(
            &self.ca,
            "golem-shard-manager",
            ExtendedKeyUsagePurpose::ClientAuth,
            validity,
        );
        let cert_path = self.path(&format!("{name}.pem"));
        let key_path = self.path(&format!("{name}-key.pem"));
        std::fs::write(&cert_path, cert).expect("Cannot write a test certificate");
        std::fs::write(&key_path, key).expect("Cannot write a test key");
        EtcdTlsConfig {
            client_cert_file: Some(cert_path),
            client_key_file: Some(key_path),
            ..self.mtls()
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    /// CA, client certificate and key; no server name.
    fn mtls(&self) -> EtcdTlsConfig {
        EtcdTlsConfig {
            ca_cert_file: Some(self.path("ca.pem")),
            ca_cert: None,
            client_cert_file: Some(self.path("client.pem")),
            client_cert: None,
            client_key_file: Some(self.path("client-key.pem")),
            client_key: None,
            server_name: None,
        }
    }

    /// TLS that checks etcd's certificate against the test CA, with no client certificate.
    fn server_tls(&self) -> TlsOptions {
        TlsOptions::new().ca_certificate(etcd_client::Certificate::from_pem(&self.ca_pem))
    }

    /// [`Self::server_tls`] with the test client certificate.
    fn client_tls(&self) -> TlsOptions {
        self.server_tls().identity(etcd_client::Identity::from_pem(
            std::fs::read(self.path("client.pem")).unwrap(),
            std::fs::read(self.path("client-key.pem")).unwrap(),
        ))
    }

    fn etcd_options(&self) -> DockerEtcdOptions {
        DockerEtcdOptions::new().with_tls(self.server_pem.clone(), self.server_key_pem.clone())
    }

    /// [`Self::etcd_options`], and etcd requires a client certificate signed by the test CA.
    fn mtls_etcd_options(&self) -> DockerEtcdOptions {
        self.etcd_options()
            .with_client_cert_auth(self.ca_pem.clone())
    }
}

fn test_ca(name: &str) -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
}

/// How long a test certificate is valid.
#[derive(Clone, Copy)]
enum Validity {
    /// rcgen's default: for as long as any test runs.
    Long,
    /// Expired in 2002.
    Expired,
    /// Until this moment.
    Until(SystemTime),
}

/// A certificate for `name` and its private key, both PEM.
fn signed_by(
    ca: &CertifiedIssuer<'static, KeyPair>,
    name: &str,
    usage: ExtendedKeyUsagePurpose,
    validity: Validity,
) -> (String, String) {
    let mut params = CertificateParams::new(vec![name.to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    match validity {
        Validity::Long => {}
        Validity::Expired => {
            params.not_before = rcgen::date_time_ymd(2001, 1, 1);
            params.not_after = rcgen::date_time_ymd(2002, 1, 1);
        }
        Validity::Until(end) => {
            params.not_after = rcgen::date_time_ymd(1970, 1, 1)
                + end
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .expect("A test certificate ends after 1970");
        }
    }
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![usage];
    let key = KeyPair::generate().unwrap();
    let issuer: &Issuer<'static, KeyPair> = ca;
    let cert = params.signed_by(&key, issuer).unwrap();
    (cert.pem(), key.serialize_pem())
}

/// TLS on the client port, and etcd requires a client certificate signed by the test CA.
async fn mtls_etcd(pki: &TestPki) -> DockerEtcd {
    DockerEtcd::new_with_options(pki.mtls_etcd_options()).await
}

/// TLS on the client port, and authentication enabled: user `USER` may read and write `/golem/`
/// only. The setup presents the test client certificate, so `options` may also require one.
async fn auth_etcd(pki: &TestPki, options: DockerEtcdOptions) -> DockerEtcd {
    let etcd = DockerEtcd::new_with_options(options).await;
    let client = connect_with_retry(
        &etcd.client_url(),
        ConnectOptions::new().with_tls(pki.client_tls()),
    )
    .await;
    let mut auth = client.auth_client();
    auth.role_add(USER).await.unwrap();
    auth.role_grant_permission(USER, Permission::read_write("/golem/").with_prefix())
        .await
        .unwrap();
    auth.user_add(USER, PASSWORD, None).await.unwrap();
    auth.user_grant_role(USER, USER).await.unwrap();
    auth.user_add("root", "root-password", None).await.unwrap();
    auth.user_grant_role("root", "root").await.unwrap();
    auth.auth_enable().await.unwrap();
    etcd
}

/// An etcd client for the test's own reads and setup; it waits until etcd answers, because the
/// port forward can come up slightly after etcd logs that it is ready.
async fn connect_with_retry(url: &str, options: ConnectOptions) -> etcd_client::Client {
    timeout(STARTUP_TIMEOUT, async {
        loop {
            if let Ok(client) = etcd_client::Client::connect([url], Some(options.clone())).await
                && client.clone().status().await.is_ok()
            {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("etcd did not answer")
}

/// A root client of an etcd from [`auth_etcd`].
async fn inspection_client_auth(url: &str, pki: &TestPki) -> etcd_client::Client {
    connect_with_retry(
        url,
        ConnectOptions::new()
            .with_tls(pki.server_tls())
            .with_user("root", "root-password"),
    )
    .await
}

/// A client of an etcd from [`mtls_etcd`].
async fn inspection_client_tls(url: &str, pki: &TestPki) -> etcd_client::Client {
    connect_with_retry(url, ConnectOptions::new().with_tls(pki.client_tls())).await
}

fn config(etcd: EtcdConfig) -> ShardManagerConfig {
    ShardManagerConfig {
        number_of_shards: 16,
        http_port: 0,
        grpc: GrpcApiConfig {
            port: 0,
            ..GrpcApiConfig::default()
        },
        persistence: PersistenceConfig::Etcd(EtcdConfig {
            leader_lease_ttl: LEASE_TTL,
            request_timeout: REQUEST_TIMEOUT,
            ..etcd
        }),
        ..ShardManagerConfig::default()
    }
}

fn tls_config(url: &str, tls: EtcdTlsConfig) -> ShardManagerConfig {
    config(EtcdConfig {
        endpoints: vec![url.to_string()],
        tls: Some(tls),
        ..EtcdConfig::default()
    })
}

fn auth_config(url: &str, password: &str, pki: &TestPki) -> ShardManagerConfig {
    config(EtcdConfig {
        endpoints: vec![url.to_string()],
        tls: Some(EtcdTlsConfig {
            ca_cert_file: Some(pki.path("ca.pem")),
            ca_cert: None,
            client_cert_file: None,
            client_cert: None,
            client_key_file: None,
            client_key: None,
            server_name: None,
        }),
        username: Some(USER.to_string()),
        password: Some(EtcdPassword::from(password)),
        ..EtcdConfig::default()
    })
}

/// Starts a replica in the background and reports its details the moment `run()` returns.
fn spawn_replica(
    config: ShardManagerConfig,
) -> (
    JoinHandle<anyhow::Result<()>>,
    oneshot::Receiver<anyhow::Result<RunDetails>>,
) {
    spawn_replica_with_shutdown(config, CancellationToken::new())
}

/// [`spawn_replica`], stopped by `shutdown` as a SIGTERM stops the binary.
fn spawn_replica_with_shutdown(
    config: ShardManagerConfig,
    shutdown: CancellationToken,
) -> (
    JoinHandle<anyhow::Result<()>>,
    oneshot::Receiver<anyhow::Result<RunDetails>>,
) {
    let (started, details) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        match golem_shard_manager::run(
            &config,
            Deployment::Standalone { shutdown },
            prometheus::Registry::new(),
            &mut join_set,
        )
        .await
        {
            Ok(details) => {
                let _ = started.send(Ok(details));
            }
            Err(err) => {
                join_set.abort_all();
                let _ = started.send(Err(err));
                return Ok(());
            }
        }
        while let Some(result) = join_set.join_next().await {
            result??;
        }
        Ok(())
    });
    (handle, details)
}

async fn started(details: oneshot::Receiver<anyhow::Result<RunDetails>>) -> RunDetails {
    timeout(STARTUP_TIMEOUT, details)
        .await
        .expect("The replica should finish starting")
        .expect("The replica task ended without a result")
        .expect("The replica should start")
}

/// The replica's startup must fail, and quickly: a connection that cannot work stays broken.
async fn refused(config: ShardManagerConfig) -> String {
    let (replica, details) = spawn_replica(config);
    let result = timeout(REFUSAL_TIMEOUT, details).await;
    replica.abort();
    let err = result
        .expect("The replica neither started nor failed: it retries a connection that cannot work")
        .expect("The replica task ended without a result")
        .map(|_| ())
        .expect_err("The replica must not start");
    format!("{err:#}")
}

async fn state_is_written(client: &mut etcd_client::Client) {
    timeout(STARTUP_TIMEOUT, async {
        while client
            .get(STATE_KEY, None)
            .await
            .expect("Cannot read the state")
            .kvs()
            .is_empty()
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("The leader should write its state");
}

/// Elect, write, hand over: the whole distributed path on one secured etcd.
async fn elect_write_and_hand_over(config: ShardManagerConfig, mut inspect: etcd_client::Client) {
    let (_leader, leader_details) = spawn_replica(config.clone());
    let leader = started(leader_details).await;
    state_is_written(&mut inspect).await;

    let (_standby, standby_details) = spawn_replica(config);
    tokio::time::sleep(STANDBY_SETTLE).await;
    leader
        .leadership
        .as_ref()
        .expect("A distributed leader has a leadership handle")
        .step_down()
        .await
        .expect("A graceful step-down must work on a secured etcd");

    let standby = timeout(HANDOVER_BOUND, standby_details)
        .await
        .expect("A revoke hands over well inside one lease TTL")
        .expect("The standby task ended without a result")
        .expect("The standby should start");
    assert!(standby.leadership.is_some());
}

#[test]
async fn a_replica_elects_writes_and_hands_over_over_mtls(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = mtls_etcd(&pki).await;
    let inspect = inspection_client_tls(&etcd.client_url(), &pki).await;
    elect_write_and_hand_over(tls_config(&etcd.client_url(), pki.mtls()), inspect).await;
}

#[test]
async fn a_server_name_checks_the_certificate_of_an_ip_endpoint(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = mtls_etcd(&pki).await;
    let inspect = inspection_client_tls(&etcd.client_url(), &pki).await;
    let ip_url = etcd.client_url().replace("localhost", "127.0.0.1");
    let config = tls_config(
        &ip_url,
        EtcdTlsConfig {
            server_name: Some("localhost".to_string()),
            ..pki.mtls()
        },
    );
    elect_write_and_hand_over(config, inspect).await;
}

#[test]
// The server certificate does not name 127.0.0.1, so without `server_name` the check fails: this
// is what makes the test above prove that `server_name` is used.
async fn an_ip_endpoint_without_a_server_name_does_not_start(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = mtls_etcd(&pki).await;
    let _ready = inspection_client_tls(&etcd.client_url(), &pki).await;
    let ip_url = etcd.client_url().replace("localhost", "127.0.0.1");
    let message = refused(tls_config(&ip_url, pki.mtls())).await;
    assert!(
        message.contains("NotValidForName"),
        "The refusal should name the certificate's names, but was: {message}"
    );
}

#[test]
async fn a_replica_without_a_client_certificate_does_not_start(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = mtls_etcd(&pki).await;
    let _ready = inspection_client_tls(&etcd.client_url(), &pki).await;
    let message = refused(tls_config(
        &etcd.client_url(),
        EtcdTlsConfig {
            client_cert_file: None,
            client_key_file: None,
            ..pki.mtls()
        },
    ))
    .await;
    assert!(
        message.contains("received fatal alert: CertificateRequired"),
        "The refusal should be etcd's demand for a client certificate, but was: {message}"
    );
}

#[test]
async fn a_replica_that_trusts_another_ca_does_not_start(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = mtls_etcd(&pki).await;
    let _ready = inspection_client_tls(&etcd.client_url(), &pki).await;
    let message = refused(tls_config(
        &etcd.client_url(),
        EtcdTlsConfig {
            ca_cert_file: Some(pki.path("other-ca.pem")),
            ..pki.mtls()
        },
    ))
    .await;
    assert!(
        message.contains("invalid peer certificate: UnknownIssuer"),
        "The refusal should be the unknown issuer of etcd's certificate, but was: {message}"
    );
}

#[test]
async fn a_replica_elects_writes_and_hands_over_with_etcd_auth(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.etcd_options()).await;
    let inspect = inspection_client_auth(&etcd.client_url(), &pki).await;
    let mut config = auth_config(&etcd.client_url(), PASSWORD, &pki);
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        // The leader compacts after each pass; a retention of one revision makes the first pass
        // compact, as the user whose role covers `/golem/` only.
        etcd.compaction_retention_revisions = 1;
    }
    elect_write_and_hand_over(config, inspect.clone()).await;
    history_is_compacted(inspect).await;
}

/// A failed compaction is only logged, so this looks at etcd: once its history is compacted, a
/// read at the first revision is refused with `OutOfRange`.
async fn history_is_compacted(mut client: etcd_client::Client) {
    let first_revision = Some(etcd_client::GetOptions::new().with_revision(1));
    timeout(STARTUP_TIMEOUT, async {
        loop {
            match client.get(STATE_KEY, first_revision.clone()).await {
                Err(etcd_client::Error::GRpcStatus(status))
                    if status.code() == tonic::Code::OutOfRange =>
                {
                    return;
                }
                _ => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
    })
    .await
    .expect("The leader should compact etcd's history as the limited user");
}

#[test]
// The other way to give each setting: the CA as an inline value instead of a file, and the
// password from a file, as a Kubernetes Secret mounted as files provides it.
async fn a_replica_with_an_inline_ca_and_a_password_file_elects_writes_and_hands_over(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.etcd_options()).await;
    let inspect = inspection_client_auth(&etcd.client_url(), &pki).await;
    std::fs::write(pki.path("password"), format!("{PASSWORD}\n")).unwrap();
    let mut config = auth_config(&etcd.client_url(), PASSWORD, &pki);
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        etcd.password = None;
        etcd.password_file = Some(pki.path("password"));
        let tls = etcd.tls.as_mut().expect("the auth config has TLS");
        tls.ca_cert_file = None;
        tls.ca_cert = Some(Base64(pki.ca_pem.clone().into_bytes()));
    }
    elect_write_and_hand_over(config, inspect).await;
}

#[test]
async fn a_replica_with_a_wrong_password_does_not_start(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.etcd_options()).await;
    let message = refused(auth_config(&etcd.client_url(), "not-the-password", &pki)).await;
    assert!(
        message.contains("etcdserver: authentication failed, invalid user ID or password"),
        "The refusal should be etcd's authentication failure, but was: {message}"
    );
}

#[test]
// etcd drops a token that is not used for `--auth-token-ttl`. A leader idle for longer than that
// must still be able to step down, which needs a valid token for the revoke of its lease.
async fn an_idle_leader_still_holds_a_valid_etcd_token(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.etcd_options().with_flag("--auth-token-ttl=2")).await;
    let (_leader, leader_details) = spawn_replica(auth_config(&etcd.client_url(), PASSWORD, &pki));
    let leader = started(leader_details).await;

    tokio::time::sleep(Duration::from_secs(4)).await;

    leader
        .leadership
        .as_ref()
        .expect("A distributed leader has a leadership handle")
        .step_down()
        .await
        .expect("The step-down needs a token that is still valid");
}

#[test]
// A standby's campaign waits inside etcd with the token it was sent with, and etcd checks that token
// again when the key ahead of it goes away. A standby that waited longer than `--auth-token-ttl` must
// still take over at once when the leader steps down, not one lease TTL later.
async fn a_standby_that_waited_longer_than_the_token_ttl_takes_over(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.etcd_options().with_flag("--auth-token-ttl=2")).await;
    let config = auth_config(&etcd.client_url(), PASSWORD, &pki);
    let (_leader, leader_details) = spawn_replica(config.clone());
    let leader = started(leader_details).await;

    let (_standby, standby_details) = spawn_replica(config);
    tokio::time::sleep(Duration::from_secs(5)).await;

    leader
        .leadership
        .as_ref()
        .expect("A distributed leader has a leadership handle")
        .step_down()
        .await
        .expect("The step-down needs a token that is still valid");

    let standby = timeout(HANDOVER_BOUND, standby_details)
        .await
        .expect("The standby should take over well inside one lease TTL")
        .expect("The standby task ended without a result")
        .expect("The standby should take over");
    assert!(standby.leadership.is_some());
}

/// A host port for an etcd container that must keep its port across a restart.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|listener| listener.local_addr())
        .expect("Cannot find a free port")
        .port()
}

#[test]
// With a user, etcd-client logs in as it connects, so the connect itself meets an etcd that is not
// there yet. The replica must wait for etcd, as it does without a user, instead of exiting.
async fn a_replica_with_a_user_waits_for_an_etcd_that_starts_later(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.etcd_options().with_host_port(free_port())).await;
    etcd.stop().await;

    let (_replica, details) = spawn_replica(auth_config(&etcd.client_url(), PASSWORD, &pki));
    tokio::time::sleep(Duration::from_secs(5)).await;
    etcd.start_again().await;

    let replica = started(details).await;
    assert!(replica.leadership.is_some());
}

#[test]
// An expired etcd certificate is a fault on etcd's side that nothing in this replica's settings can
// fix: the startup waits, as for an etcd that is down, and goes on once etcd has a valid one.
async fn a_replica_waits_while_etcds_certificate_has_expired(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::with_expired_server_certificate();
    let port = free_port();
    let expired = DockerEtcd::new_with_options(pki.mtls_etcd_options().with_host_port(port)).await;
    let (_replica, mut details) = spawn_replica(tls_config(&expired.client_url(), pki.mtls()));
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert!(
        details.try_recv().is_err(),
        "While etcd's certificate has expired, the replica must wait, neither starting nor stopping"
    );

    expired.stop().await;
    let _renewed = DockerEtcd::new_with_options(
        pki.new_mtls_etcd_options(Validity::Long)
            .with_host_port(port),
    )
    .await;
    started(details).await;
}

#[test]
// With a user, etcd-client logs in while it connects, so the first TLS handshake is in the connect
// itself, not in the first read: a refused certificate must stop the startup there too.
async fn a_replica_with_a_user_that_trusts_another_ca_does_not_start(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.etcd_options()).await;
    let mut config = auth_config(&etcd.client_url(), PASSWORD, &pki);
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        etcd.tls
            .as_mut()
            .expect("the auth config has TLS")
            .ca_cert_file = Some(pki.path("other-ca.pem"));
    }
    let message = refused(config).await;
    assert!(
        message.contains("TLS handshake with etcd failed"),
        "The refusal should be the handshake, but was: {message}"
    );
    assert!(
        message.contains("invalid peer certificate: UnknownIssuer"),
        "The refusal should be the unknown issuer of etcd's certificate, but was: {message}"
    );
}

#[test]
// While a replica runs, a refused certificate is retried, because etcd's certificate can be fixed
// on etcd's side. Renewed files take effect only on a restart, so once the files have changed, the
// replica stops, and a new start reads them.
async fn a_running_replica_stops_on_a_refused_certificate_only_when_its_files_changed(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let old_pki = TestPki::new();
    let port = free_port();
    let old_etcd =
        DockerEtcd::new_with_options(old_pki.mtls_etcd_options().with_host_port(port)).await;
    let mut config = tls_config(&old_etcd.client_url(), old_pki.mtls());
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        // The certificate check runs every `retry_max_delay`.
        etcd.retry_max_delay = Duration::from_secs(1);
    }
    let (_leader, leader_details) = spawn_replica(config.clone());
    started(leader_details).await;
    let (_standby, mut standby_details) = spawn_replica(config.clone());
    tokio::time::sleep(STANDBY_SETTLE).await;

    // etcd comes back on the same address with a certificate from another CA, which also signs
    // the client certificates it accepts.
    old_etcd.stop().await;
    let new_pki = TestPki::new();
    let _new_etcd =
        DockerEtcd::new_with_options(new_pki.mtls_etcd_options().with_host_port(port)).await;

    assert!(
        timeout(Duration::from_secs(4), &mut standby_details)
            .await
            .is_err(),
        "With unchanged files, the standby must keep retrying"
    );

    for name in ["ca.pem", "client.pem", "client-key.pem"] {
        std::fs::copy(new_pki.path(name), old_pki.path(name)).unwrap();
    }
    let stopped = timeout(STARTUP_TIMEOUT, standby_details)
        .await
        .expect("With changed files, the standby must stop")
        .expect("The standby task ended without a result")
        .map(|_| ())
        .expect_err("The standby must not become a leader with the old certificates");
    let message = format!("{stopped:#}");
    assert!(
        message.contains("the TLS files have changed since then"),
        "The standby should stop because its files changed, but stopped with: {message}"
    );

    let (_restarted, restarted_details) = spawn_replica(config);
    started(restarted_details).await;
}

#[test]
// etcd's election service reports a denied write as `Unknown` instead of `PermissionDenied`. A role
// that may not write `/golem/` does not change while the replica runs, so the replica must stop
// with the reason instead of campaigning forever.
async fn a_user_who_may_only_read_does_not_start(_tracing: &Tracing, _crypto: &CryptoProvider) {
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.etcd_options()).await;
    let root = inspection_client_auth(&etcd.client_url(), &pki).await;
    let mut auth = root.auth_client();
    auth.role_add("reader").await.unwrap();
    auth.role_grant_permission("reader", Permission::read("/golem/").with_prefix())
        .await
        .unwrap();
    auth.user_add("reader", PASSWORD, None).await.unwrap();
    auth.user_grant_role("reader", "reader").await.unwrap();

    let mut config = auth_config(&etcd.client_url(), PASSWORD, &pki);
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        etcd.username = Some("reader".to_string());
    }
    let message = refused(config).await;
    assert!(
        message.contains("etcdserver: permission denied"),
        "The refusal should be etcd's permission check, but was: {message}"
    );
}

#[test]
// An etcd restart closes every connection; the leader's keepalive must reconnect over TLS with the
// certificates it holds and keep the lease, or the deployment loses its leader on each etcd
// restart.
async fn a_tls_leader_keeps_its_leadership_across_an_etcd_restart(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let etcd =
        DockerEtcd::new_with_options(pki.mtls_etcd_options().with_host_port(free_port())).await;
    let (leader, leader_details) = spawn_replica(tls_config(&etcd.client_url(), pki.mtls()));
    let elected = started(leader_details).await;

    etcd.restart().await;
    // Longer than one lease TTL: the lease survives only if the keepalive renewed it after the
    // restart.
    tokio::time::sleep(LEASE_TTL + Duration::from_secs(2)).await;

    assert!(
        !leader.is_finished(),
        "The leader must still run after the etcd restart"
    );
    assert!(
        !elected
            .leadership
            .as_ref()
            .expect("A distributed leader has a leadership handle")
            .has_stepped_down()
    );
    let mut inspect = inspection_client_tls(&etcd.client_url(), &pki).await;
    let leader_keys = inspect
        .get(
            "/golem/shard-manager/leader",
            Some(etcd_client::GetOptions::new().with_prefix()),
        )
        .await
        .expect("Cannot read the election keys");
    assert_eq!(
        leader_keys.kvs().len(),
        1,
        "The leader's election key must survive the restart"
    );
}

/// A TLS and HTTP/2 server that takes every request and never answers it: an etcd that accepted a
/// connection and then stalls, as one cut off by a network partition right after the handshake.
struct SilentEtcd {
    url: String,
    requests: Arc<AtomicUsize>,
    server: JoinHandle<()>,
}

impl SilentEtcd {
    async fn start(pki: &TestPki) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let certificates = CertificateDer::pem_slice_iter(pki.server_pem.as_bytes())
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let key = PrivateKeyDer::from_pem_slice(pki.server_key_pem.as_bytes()).unwrap();
        let mut tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(certificates, key)
            .unwrap();
        tls.alpn_protocols = vec![b"h2".to_vec()];
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
        let requests = Arc::new(AtomicUsize::new(0));
        let counter = requests.clone();
        let server = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            while let Ok((tcp, _)) = listener.accept().await {
                let acceptor = acceptor.clone();
                let counter = counter.clone();
                connections.spawn(async move {
                    let Ok(tls) = acceptor.accept(tcp).await else {
                        return;
                    };
                    let Ok(mut connection) = h2::server::handshake(tls).await else {
                        return;
                    };
                    // Held, never answered; accepting keeps the connection itself alive.
                    let mut held = Vec::new();
                    while let Some(Ok(request)) = connection.accept().await {
                        counter.fetch_add(1, Ordering::SeqCst);
                        held.push(request);
                    }
                });
            }
        });
        Self {
            url,
            requests,
            server,
        }
    }

    async fn wait_for_requests(&self, count: usize) {
        timeout(STARTUP_TIMEOUT, async {
            while self.requests.load(Ordering::SeqCst) < count {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("The replica should send its login to the silent etcd");
    }
}

impl Drop for SilentEtcd {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[test]
// The election client has no request timeout, because a campaign waits inside etcd for as long as
// the leader lives. Its connect logs in, and a login that etcd never answers must still end; the
// startup then tries again.
async fn the_election_connect_gives_up_on_a_login_that_etcd_never_answers(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let silent = SilentEtcd::start(&pki).await;
    let mut config = auth_config(&silent.url, PASSWORD, &pki);
    let PersistenceConfig::Etcd(etcd) = &mut config.persistence else {
        unreachable!("the auth config is an etcd config");
    };
    etcd.connect_timeout = Duration::from_secs(1);
    let etcd = etcd.clone();

    let result = timeout(
        Duration::from_secs(10),
        LeaderElection::connect(&etcd, LEADER_ELECTION_NAME),
    )
    .await
    .expect("The election connect must give up on a login that etcd never answers");

    assert!(
        matches!(result, Err(ShardManagerError::Timeout)),
        "{:?}",
        result.map(|_| ())
    );
    assert!(silent.requests.load(Ordering::SeqCst) >= 1);

    let logins = silent.requests.load(Ordering::SeqCst);
    let (_replica, mut details) = spawn_replica(config);
    silent.wait_for_requests(logins + 3).await;
    assert!(
        details.try_recv().is_err(),
        "The replica must keep trying, neither starting nor stopping"
    );
}

#[test]
// A pod that is stopped while its startup waits for an etcd login must stop at once, not when the
// orchestrator gives up and kills it.
async fn a_shutdown_ends_a_startup_that_waits_for_an_etcd_login(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let silent = SilentEtcd::start(&pki).await;
    let mut config = auth_config(&silent.url, PASSWORD, &pki);
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        // Longer than the bound below: only the shutdown can end the wait in time. The request
        // timeout may be at most half the lease TTL.
        etcd.request_timeout = Duration::from_secs(8);
        etcd.leader_lease_ttl = Duration::from_secs(20);
    }
    let shutdown = CancellationToken::new();
    let (_replica, details) = spawn_replica_with_shutdown(config, shutdown.clone());
    silent.wait_for_requests(1).await;

    shutdown.cancel();
    let err = timeout(Duration::from_secs(2), details)
        .await
        .expect("A shutdown must end the startup at once")
        .expect("The replica task ended without a result")
        .map(|_| ())
        .expect_err("The replica must not start");
    assert!(
        matches!(
            err.downcast_ref::<ShardManagerError>(),
            Some(ShardManagerError::ShutdownRequested)
        ),
        "{err:#}"
    );
}

/// A TCP relay in front of etcd whose connections the test can cut one by one, numbered in the
/// order they were opened: a way to break one of a replica's two etcd connections and keep the
/// other.
struct TcpRelay {
    url: String,
    connections: Arc<std::sync::Mutex<Vec<tokio::task::AbortHandle>>>,
    server: JoinHandle<()>,
}

impl TcpRelay {
    async fn start(etcd: &DockerEtcd) -> Self {
        let target: u16 = etcd
            .client_url()
            .rsplit(':')
            .next()
            .and_then(|port| port.parse().ok())
            .expect("etcd's client URL ends with its port");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!(
            "https://localhost:{}",
            listener.local_addr().unwrap().port()
        );
        let connections = Arc::new(std::sync::Mutex::new(Vec::new()));
        let opened = connections.clone();
        let server = tokio::spawn(async move {
            let mut relayed = JoinSet::new();
            while let Ok((mut inbound, _)) = listener.accept().await {
                let handle = relayed.spawn(async move {
                    if let Ok(mut outbound) =
                        tokio::net::TcpStream::connect(("127.0.0.1", target)).await
                    {
                        let _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await;
                    }
                });
                opened.lock().unwrap().push(handle);
            }
        });
        Self {
            url,
            connections,
            server,
        }
    }

    async fn wait_for_connections(&self, count: usize) {
        timeout(STARTUP_TIMEOUT, async {
            while self.connections.lock().unwrap().len() < count {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("The replica should open its etcd connections through the relay");
    }

    /// Closes the connection opened as number `index`, counted from 0.
    fn cut(&self, index: usize) {
        self.connections.lock().unwrap()[index].abort();
    }
}

impl Drop for TcpRelay {
    fn drop(&mut self) {
        self.server.abort();
    }
}

#[test]
// A replica's two etcd clients hold separate connections, and one can break while the other stays
// up - behind a Service, when the etcd member under one of them restarts. A standby whose
// certificate expired after its files were renewed must stop, so that its restart reads them, also
// when only its election connection is refused: that is the connection it needs to take over.
async fn a_standby_stops_when_only_its_election_connection_meets_a_refused_certificate(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    // Client certificates and a user: the check of the renewed files sends no login, so etcd
    // refuses its read after the handshake, and that answer must still count as accepted files.
    let pki = TestPki::new();
    let etcd = auth_etcd(&pki, pki.mtls_etcd_options()).await;
    let with_user = |url: &str, tls: EtcdTlsConfig| {
        config(EtcdConfig {
            endpoints: vec![url.to_string()],
            tls: Some(tls),
            username: Some(USER.to_string()),
            password: Some(EtcdPassword::from(PASSWORD)),
            // The certificate check runs every `retry_max_delay`.
            retry_max_delay: Duration::from_secs(1),
            ..EtcdConfig::default()
        })
    };
    let (_leader, leader_details) = spawn_replica(with_user(&etcd.client_url(), pki.mtls()));
    started(leader_details).await;

    let relay = TcpRelay::start(&etcd).await;
    let expiry = SystemTime::now() + Duration::from_secs(8);
    let config = with_user(
        &relay.url,
        pki.write_client_certificate("standby", Validity::Until(expiry)),
    );
    let (_standby, mut standby_details) = spawn_replica(config);
    // The startup read opens the request connection, then the campaign the election connection.
    relay.wait_for_connections(2).await;

    let until_expired = expiry.duration_since(SystemTime::now()).unwrap_or_default();
    tokio::time::sleep(until_expired + Duration::from_secs(2)).await;
    assert!(
        standby_details.try_recv().is_err(),
        "The standby keeps waiting on its open connections after its certificate expired"
    );
    pki.write_client_certificate("standby", Validity::Long);
    relay.cut(1);

    let stopped = timeout(STARTUP_TIMEOUT, standby_details)
        .await
        .expect("A standby refused on its election connection, with changed files, must stop")
        .expect("The standby task ended without a result")
        .map(|_| ())
        .expect_err("The standby must not become a leader with its expired certificate");
    let message = format!("{stopped:#}");
    assert!(
        message.contains("the TLS files have changed since then"),
        "The standby should stop because its files changed, but stopped with: {message}"
    );
}

#[test]
// A refused etcd certificate is etcd's to fix. A replica whose files were renewed must not stop for
// it: its restart would present the new files to the same etcd, be refused at once, and loop.
async fn a_replica_with_renewed_files_keeps_running_while_etcds_certificate_has_expired(
    _tracing: &Tracing,
    _crypto: &CryptoProvider,
) {
    let pki = TestPki::new();
    let port = free_port();
    let etcd = DockerEtcd::new_with_options(pki.mtls_etcd_options().with_host_port(port)).await;
    let mut config = tls_config(&etcd.client_url(), pki.mtls());
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        etcd.retry_max_delay = Duration::from_secs(1);
    }
    let (_leader, leader_details) = spawn_replica(config.clone());
    started(leader_details).await;
    let (_standby, mut standby_details) = spawn_replica(config);
    tokio::time::sleep(STANDBY_SETTLE).await;

    pki.write_client_certificate("client", Validity::Long);
    etcd.stop().await;
    let expired = DockerEtcd::new_with_options(
        pki.new_mtls_etcd_options(Validity::Expired)
            .with_host_port(port),
    )
    .await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(
        standby_details.try_recv().is_err(),
        "While etcd's certificate has expired, the standby must keep running"
    );

    expired.stop().await;
    let _fixed = DockerEtcd::new_with_options(
        pki.new_mtls_etcd_options(Validity::Long)
            .with_host_port(port),
    )
    .await;
    let standby = timeout(STARTUP_TIMEOUT, standby_details)
        .await
        .expect("Once etcd is fixed, the standby should lead without a restart")
        .expect("The standby task ended without a result")
        .expect("The standby should lead");
    assert!(standby.leadership.is_some());
}
