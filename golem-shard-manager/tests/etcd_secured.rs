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
use golem_shard_manager::{Deployment, RunDetails, STATE_KEY};
use golem_test_framework::components::etcd::docker_etcd::{DockerEtcd, DockerEtcdOptions};
use rcgen::{
    BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose, IsCa,
    Issuer, KeyPair, KeyUsagePurpose,
};
use std::path::PathBuf;
use std::time::Duration;
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
    ca_pem: String,
    server_pem: String,
    server_key_pem: String,
}

impl TestPki {
    fn new() -> Self {
        let ca = test_ca("golem test CA");
        let (server_pem, server_key_pem) =
            signed_by(&ca, "localhost", ExtendedKeyUsagePurpose::ServerAuth);
        let (client_pem, client_key_pem) = signed_by(
            &ca,
            "golem-shard-manager",
            ExtendedKeyUsagePurpose::ClientAuth,
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
            server_pem,
            server_key_pem,
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
}

fn test_ca(name: &str) -> CertifiedIssuer<'static, KeyPair> {
    let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
    CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
}

/// A certificate for `name` and its private key, both PEM.
fn signed_by(
    ca: &CertifiedIssuer<'static, KeyPair>,
    name: &str,
    usage: ExtendedKeyUsagePurpose,
) -> (String, String) {
    let mut params = CertificateParams::new(vec![name.to_string()]).unwrap();
    params.distinguished_name.push(DnType::CommonName, name);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![usage];
    let key = KeyPair::generate().unwrap();
    let issuer: &Issuer<'static, KeyPair> = ca;
    let cert = params.signed_by(&key, issuer).unwrap();
    (cert.pem(), key.serialize_pem())
}

/// TLS on the client port, and etcd requires a client certificate signed by the test CA.
async fn mtls_etcd(pki: &TestPki) -> DockerEtcd {
    DockerEtcd::new_with_options(pki.etcd_options().with_client_cert_auth(pki.ca_pem.clone())).await
}

/// TLS on the client port without client certificates, and authentication enabled: user `USER`
/// may read and write `/golem/` only.
async fn auth_etcd(pki: &TestPki, options: DockerEtcdOptions) -> DockerEtcd {
    let etcd = DockerEtcd::new_with_options(options).await;
    let client = connect_with_retry(
        &etcd.client_url(),
        ConnectOptions::new().with_tls(pki.server_tls()),
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
    let (started, details) = oneshot::channel();
    let handle = tokio::spawn(async move {
        let mut join_set: JoinSet<anyhow::Result<()>> = JoinSet::new();
        match golem_shard_manager::run(
            &config,
            Deployment::Standalone {
                shutdown: CancellationToken::new(),
            },
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
