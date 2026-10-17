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
//! etcd with authentication enabled.
//!
//! Each test starts its own etcd container, so the shared etcd of `etcd_backed` is never changed.

use etcd_client::{Permission, TlsOptions};
use golem_shard_manager::config::{
    EtcdConfig, EtcdPassword, EtcdTlsConfig, GrpcApiConfig, PersistenceConfig, ShardManagerConfig,
};
use golem_shard_manager::{Deployment, RunDetails, STATE_KEY};
use std::path::PathBuf;
use std::time::Duration;
use test_r::test;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;

const LEASE_TTL: Duration = Duration::from_secs(2);
const REQUEST_TIMEOUT: Duration = Duration::from_millis(500);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(20);
/// A replica whose etcd connection cannot work must say so within this time, not wait forever.
const REFUSAL_TIMEOUT: Duration = Duration::from_secs(15);
const USER: &str = "golem";
const PASSWORD: &str = "s3cret-etcd-password";

/// The shard manager binary installs the ring provider in `main`; a test process must do it itself.
fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/etcd-tls")
        .join(name)
}

/// One etcd server in its own container, plaintext with authentication or TLS with client
/// certificates.
struct SecuredEtcd {
    _container: ContainerAsync<GenericImage>,
    url: String,
}

impl SecuredEtcd {
    /// TLS on the client port, and etcd requires a client certificate signed by the test CA.
    async fn with_mtls() -> Self {
        let mut flags = base_flags("https");
        flags.extend(
            [
                "--cert-file=/certs/server.pem",
                "--key-file=/certs/server-key.pem",
                "--trusted-ca-file=/certs/ca.pem",
                "--client-cert-auth",
            ]
            .map(String::from),
        );
        let image = GenericImage::new("gcr.io/etcd-development/etcd", "v3.5.17")
            .with_exposed_port(2379.tcp())
            .with_wait_for(WaitFor::message_on_stderr("ready to serve client requests"))
            .with_copy_to("/certs/server.pem", fixture("server.pem"))
            .with_copy_to("/certs/server-key.pem", fixture("server-key.pem"))
            .with_copy_to("/certs/ca.pem", fixture("ca.pem"))
            .with_cmd(flags);
        let container = image.start().await.expect("Cannot start the TLS etcd");
        let port = container.get_host_port_ipv4(2379).await.unwrap();
        Self {
            _container: container,
            url: format!("https://localhost:{port}"),
        }
    }

    /// Plaintext, with authentication enabled: user `USER` may read and write `/golem/` only.
    async fn with_auth(extra_flags: &[&str]) -> Self {
        let mut flags = base_flags("http");
        flags.extend(extra_flags.iter().map(|flag| flag.to_string()));
        let image = GenericImage::new("gcr.io/etcd-development/etcd", "v3.5.17")
            .with_exposed_port(2379.tcp())
            .with_wait_for(WaitFor::message_on_stderr("ready to serve client requests"))
            .with_cmd(flags);
        let container = image.start().await.expect("Cannot start the auth etcd");
        let port = container.get_host_port_ipv4(2379).await.unwrap();
        let url = format!("http://127.0.0.1:{port}");

        let client = connect_with_retry(&url).await;
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

        Self {
            _container: container,
            url,
        }
    }
}

fn base_flags(scheme: &str) -> Vec<String> {
    [
        "/usr/local/bin/etcd".to_string(),
        "--name=golem-test-etcd".to_string(),
        "--data-dir=/etcd-data".to_string(),
        format!("--listen-client-urls={scheme}://0.0.0.0:2379"),
        format!("--advertise-client-urls={scheme}://127.0.0.1:2379"),
        "--listen-peer-urls=http://0.0.0.0:2380".to_string(),
        "--initial-advertise-peer-urls=http://0.0.0.0:2380".to_string(),
        "--initial-cluster=golem-test-etcd=http://0.0.0.0:2380".to_string(),
        "--initial-cluster-state=new".to_string(),
    ]
    .to_vec()
}

/// The port forward can come up slightly after etcd logs that it is ready.
async fn connect_with_retry(url: &str) -> etcd_client::Client {
    timeout(STARTUP_TIMEOUT, async {
        loop {
            if let Ok(client) = etcd_client::Client::connect([url], None).await
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

fn mtls() -> EtcdTlsConfig {
    EtcdTlsConfig {
        ca_cert_file: fixture("ca.pem"),
        client_cert_file: Some(fixture("client.pem")),
        client_key_file: Some(fixture("client-key.pem")),
        server_name: None,
    }
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

fn auth_config(url: &str, password: &str) -> ShardManagerConfig {
    config(EtcdConfig {
        endpoints: vec![url.to_string()],
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
    tokio::time::sleep(LEASE_TTL).await;
    leader
        .leadership
        .as_ref()
        .expect("A distributed leader has a leadership handle")
        .step_down()
        .await
        .expect("A graceful step-down must work on a secured etcd");

    let standby = timeout(LEASE_TTL, standby_details)
        .await
        .expect("A revoke hands over well inside one lease TTL")
        .expect("The standby task ended without a result")
        .expect("The standby should start");
    assert!(standby.leadership.is_some());
}

async fn inspection_client_tls(url: &str) -> etcd_client::Client {
    let tls = TlsOptions::new()
        .ca_certificate(etcd_client::Certificate::from_pem(
            std::fs::read(fixture("ca.pem")).unwrap(),
        ))
        .identity(etcd_client::Identity::from_pem(
            std::fs::read(fixture("client.pem")).unwrap(),
            std::fs::read(fixture("client-key.pem")).unwrap(),
        ));
    timeout(STARTUP_TIMEOUT, async {
        loop {
            let options = etcd_client::ConnectOptions::new().with_tls(tls.clone());
            if let Ok(client) = etcd_client::Client::connect([url], Some(options)).await
                && client.clone().status().await.is_ok()
            {
                return client;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("The TLS etcd did not answer")
}

async fn inspection_client_auth(url: &str) -> etcd_client::Client {
    let options = etcd_client::ConnectOptions::new().with_user("root", "root-password");
    etcd_client::Client::connect([url], Some(options))
        .await
        .expect("Cannot connect as root")
}

#[test]
async fn a_replica_elects_writes_and_hands_over_over_mtls() {
    install_crypto_provider();
    let etcd = SecuredEtcd::with_mtls().await;
    let inspect = inspection_client_tls(&etcd.url).await;
    elect_write_and_hand_over(tls_config(&etcd.url, mtls()), inspect).await;
}

#[test]
async fn a_server_name_checks_the_certificate_of_an_ip_endpoint() {
    install_crypto_provider();
    let etcd = SecuredEtcd::with_mtls().await;
    let inspect = inspection_client_tls(&etcd.url).await;
    let ip_url = etcd.url.replace("localhost", "127.0.0.1");
    let config = tls_config(
        &ip_url,
        EtcdTlsConfig {
            server_name: Some("localhost".to_string()),
            ..mtls()
        },
    );
    elect_write_and_hand_over(config, inspect).await;
}

#[test]
async fn a_replica_without_a_client_certificate_does_not_start() {
    install_crypto_provider();
    let etcd = SecuredEtcd::with_mtls().await;
    let _ready = inspection_client_tls(&etcd.url).await;
    let message = refused(tls_config(
        &etcd.url,
        EtcdTlsConfig {
            client_cert_file: None,
            client_key_file: None,
            ..mtls()
        },
    ))
    .await;
    assert!(
        message.to_lowercase().contains("certificate") || message.contains("tls"),
        "The refusal should point at the certificate, but was: {message}"
    );
}

#[test]
async fn a_replica_that_trusts_another_ca_does_not_start() {
    install_crypto_provider();
    let etcd = SecuredEtcd::with_mtls().await;
    let _ready = inspection_client_tls(&etcd.url).await;
    let message = refused(tls_config(
        &etcd.url,
        EtcdTlsConfig {
            ca_cert_file: fixture("other-ca.pem"),
            ..mtls()
        },
    ))
    .await;
    assert!(
        message.to_lowercase().contains("certificate"),
        "The refusal should point at the certificate, but was: {message}"
    );
}

#[test]
async fn a_replica_elects_writes_and_hands_over_with_etcd_auth() {
    install_crypto_provider();
    let etcd = SecuredEtcd::with_auth(&[]).await;
    let inspect = inspection_client_auth(&etcd.url).await;
    elect_write_and_hand_over(auth_config(&etcd.url, PASSWORD), inspect).await;
}

#[test]
async fn a_replica_with_a_wrong_password_does_not_start() {
    install_crypto_provider();
    let etcd = SecuredEtcd::with_auth(&[]).await;
    let message = refused(auth_config(&etcd.url, "not-the-password")).await;
    assert!(
        message.to_lowercase().contains("authentication failed")
            || message.to_lowercase().contains("auth"),
        "The refusal should name the authentication, but was: {message}"
    );
}

#[test]
// etcd drops a token that is not used for `--auth-token-ttl`. A leader idle for longer than that
// must still be able to step down, which needs a valid token for the revoke of its lease.
async fn an_idle_leader_still_holds_a_valid_etcd_token() {
    install_crypto_provider();
    let etcd = SecuredEtcd::with_auth(&["--auth-token-ttl=2"]).await;
    let (_leader, leader_details) = spawn_replica(auth_config(&etcd.url, PASSWORD));
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
async fn a_standby_that_waited_longer_than_the_token_ttl_takes_over() {
    install_crypto_provider();
    let etcd = SecuredEtcd::with_auth(&["--auth-token-ttl=2"]).await;
    let mut config = auth_config(&etcd.url, PASSWORD);
    if let PersistenceConfig::Etcd(etcd) = &mut config.persistence {
        // Long enough that a campaign key left behind by a failed attempt shows as a late
        // hand-over, instead of expiring within the wait below.
        etcd.leader_lease_ttl = Duration::from_secs(6);
    }
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

    let standby = timeout(Duration::from_secs(2), standby_details)
        .await
        .expect("The standby should take over well inside one lease TTL")
        .expect("The standby task ended without a result")
        .expect("The standby should take over");
    assert!(standby.leadership.is_some());
}
