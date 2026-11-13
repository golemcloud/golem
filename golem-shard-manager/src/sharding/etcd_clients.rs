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

//! The two etcd clients of a replica, behind handles that follow renewed TLS files.
//!
//! A `Client` is built once from the TLS files and keeps those certificates for every connection
//! it makes later, such as after etcd restarts. A holder that keeps a `Client` therefore keeps the
//! old certificate. Holders keep a [`RequestClient`] or an [`ElectionClient`] instead, and take
//! the current `Client` from it each time they connect: [`EtcdClients`] builds new clients when
//! the files change and puts them behind the handles. A connection that is open goes on with the
//! client it was made with, so a reload never interrupts a campaign, a keepalive or a watch.

use crate::config::EtcdConfig;
use crate::sharding::error::{ShardManagerError, is_certificate_failure};
use crate::sharding::etcd_connection::{
    EtcdConnectSettings, ProbeRefusal, ReadFile, connect_election_with, connect_requests_with,
    probe_handshake,
};
use arc_swap::ArcSwap;
use etcd_client::Client;
use golem_service_base::tls::material::TLS_RELOAD_INTERVAL;
use std::sync::{Arc, Mutex};
use tracing::{error, info, warn};

macro_rules! client_handle {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Clone)]
        pub struct $name(Arc<ArcSwap<Client>>);

        impl $name {
            /// A handle that holds `client` until [`EtcdClients`] swaps it.
            pub fn fixed(client: Client) -> Self {
                Self(Arc::new(ArcSwap::from_pointee(client)))
            }

            /// The client to connect with now.
            pub fn current(&self) -> Client {
                Client::clone(&self.0.load())
            }

            pub(crate) fn swap(&self, client: Client) {
                self.0.store(Arc::new(client));
            }
        }
    };
}

client_handle! {
    /// The current client for ordinary requests, which have a request timeout. `current()` is an
    /// atomic load and a cheap clone of the client. Clones share the handle.
    RequestClient
}

client_handle! {
    /// The current client for the campaign, the lease keepalive and the election watch, which have
    /// no request timeout. `current()` is an atomic load and a
    /// cheap clone of the client. Clones share the handle.
    ElectionClient
}

/// What one look at the TLS files came to.
#[derive(Debug, PartialEq, Eq)]
pub enum TlsReload {
    /// The files are as they were when the clients in use were built.
    Unchanged,
    /// New clients were built from changed files, etcd accepted them, and they are in use.
    Reloaded,
    /// Changed files that were not taken into use, and why.
    Rejected(String),
}

/// A set of files that was not taken into use, and how it was turned down, so that each turn-down
/// is reported once and not at every poll.
struct Rejection {
    files: Vec<ReadFile>,
    /// The ways of turning down these files that were logged already.
    reported: Vec<RejectionKind>,
    /// Whether these files were counted as a failure already.
    counted: bool,
}

/// What to do about a rejection: log it, count it, or neither because it was reported already.
struct Report {
    log: bool,
    count: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RejectionKind {
    /// The files cannot be used: they do not parse, or are not valid now.
    Unusable,
    /// etcd refuses the certificates.
    Refused,
    /// etcd did not answer.
    NoAnswer,
}

/// TLS files that differ from what the clients in use were built from: the files as they are now,
/// and the settings read and checked from them.
struct ChangedFiles {
    files: Vec<ReadFile>,
    settings: Result<EtcdConnectSettings, ShardManagerError>,
}

/// Rebuilds both etcd clients from the TLS files (and the password file) when they change.
pub struct EtcdClients {
    config: Arc<EtcdConfig>,
    /// What the clients in use were built from. Never held across an await.
    settings: Mutex<Arc<EtcdConnectSettings>>,
    /// The last turned-down set of files. Never held across an await.
    rejected: Mutex<Option<Rejection>>,
    requests: RequestClient,
    election: ElectionClient,
}

impl EtcdClients {
    pub fn new(
        config: EtcdConfig,
        settings: EtcdConnectSettings,
        requests: RequestClient,
        election: ElectionClient,
    ) -> Arc<Self> {
        Arc::new(Self {
            config: Arc::new(config),
            settings: Mutex::new(Arc::new(settings)),
            rejected: Mutex::new(None),
            requests,
            election,
        })
    }

    /// Whether any TLS setting names a file, which can be renewed. Inline values change only with
    /// a restart.
    pub fn has_tls_files(&self) -> bool {
        self.settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .has_tls_files()
    }

    /// Looks at the TLS files once. When they differ from what the clients in use were built from,
    /// reads and checks them, builds both clients again, asks etcd through the new request client
    /// whether it accepts them, and only then puts the new clients in use.
    ///
    /// A set that cannot be used is looked at again at every call, because it can become usable
    /// without its bytes changing: etcd may start to trust its CA, and a certificate that is not
    /// valid yet becomes valid with time. Only the log and the failure counter are limited to one
    /// per distinct set of files.
    ///
    /// The file reads and the checks run on Tokio's blocking pool; the probe and the client builds
    /// run here.
    pub async fn reload_if_files_changed(&self) -> TlsReload {
        let (files, settings) = match self.look_at_files().await {
            Ok(None) => {
                *self
                    .rejected
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                return TlsReload::Unchanged;
            }
            Ok(Some(ChangedFiles { files, settings })) => (files, settings),
            Err(err) => {
                error!(
                    error = %err,
                    "Reading the etcd TLS files failed; the etcd clients keep the files they have, \
                     and the files are read again at the next poll"
                );
                return TlsReload::Rejected(format!("reading the TLS files failed: {err}"));
            }
        };

        let settings = match settings {
            Ok(settings) => settings,
            Err(err) => {
                let reason = err.to_string();
                let report = self.first_report(files, RejectionKind::Unusable);
                if report.log {
                    error!(
                        error = reason,
                        "The changed etcd TLS files cannot be used; the etcd clients keep the \
                         files they have"
                    );
                }
                if report.count {
                    golem_service_base::metrics::tls::record_reload_failure("etcd client");
                }
                return TlsReload::Rejected(reason);
            }
        };

        let (requests, election) = match self.connect(&settings).await {
            Ok(clients) => clients,
            Err(ProbeRefusal::Refused(reason)) => {
                let report = self.first_report(files, RejectionKind::Refused);
                if report.log {
                    warn!(
                        error = reason,
                        "etcd refuses the changed TLS files; the etcd clients keep the files they \
                         have"
                    );
                }
                if report.count {
                    golem_service_base::metrics::tls::record_reload_failure("etcd client");
                }
                return TlsReload::Rejected(format!("etcd refuses the new files: {reason}"));
            }
            Err(ProbeRefusal::NoAnswer(reason)) => {
                if self.first_report(files, RejectionKind::NoAnswer).log {
                    warn!(
                        error = reason,
                        "etcd did not answer on the changed TLS files; the etcd clients keep the \
                         files they have, and the files are tried again"
                    );
                }
                return TlsReload::Rejected(format!("etcd did not answer: {reason}"));
            }
        };

        self.requests.swap(requests);
        self.election.swap(election);
        settings.log_certificates();
        *self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Arc::new(settings);
        *self
            .rejected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
        info!("etcd TLS files reloaded; new connections use them");
        TlsReload::Reloaded
    }

    /// The TLS files when they differ from what the clients in use were built from; `None` when
    /// they do not. Reads the files on Tokio's blocking pool. A panic there is the `JoinError`.
    async fn look_at_files(&self) -> Result<Option<ChangedFiles>, tokio::task::JoinError> {
        let in_use = self
            .settings
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            if !in_use.tls_files_changed() {
                return None;
            }
            Some(ChangedFiles {
                files: EtcdConnectSettings::snapshot_of_files(&config),
                settings: EtcdConnectSettings::load_quietly(&config),
            })
        })
        .await
    }

    /// Builds both clients from `settings` and probes etcd with the request client. The two are
    /// built as at startup, and never cloned from one another.
    async fn connect(
        &self,
        settings: &EtcdConnectSettings,
    ) -> Result<(Client, Client), ProbeRefusal> {
        let bound = self.config.connect_timeout + self.config.request_timeout;
        // With a user the connect logs in, which asks etcd, and the TLS handshake with it.
        let requests = match tokio::time::timeout(
            bound,
            connect_requests_with(&self.config, settings),
        )
        .await
        {
            Ok(Ok(client)) => client,
            Ok(Err(err)) => return Err(refusal_of(err)),
            Err(_) => {
                return Err(ProbeRefusal::NoAnswer(format!(
                    "no connection within {bound:?}"
                )));
            }
        };
        probe_handshake(&requests, self.config.request_timeout).await?;
        let election = connect_election_with(&self.config, settings)
            .await
            .map_err(refusal_of)?;
        Ok((requests, election))
    }

    /// What is new about turning down `files` this way. The memory is keyed on the file bytes
    /// alone: a way that was logged for them is not logged again, they are counted at most once,
    /// and a poll that finds etcd silent never replaces what is remembered. New bytes start over.
    fn first_report(&self, files: Vec<ReadFile>, kind: RejectionKind) -> Report {
        let mut rejected = self
            .rejected
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if rejected.as_ref().is_none_or(|known| known.files != files) {
            *rejected = Some(Rejection {
                files,
                reported: Vec::new(),
                counted: false,
            });
        }
        let known = rejected.as_mut().expect("set above");
        let log = !known.reported.contains(&kind);
        if log {
            known.reported.push(kind);
        }
        let count = kind != RejectionKind::NoAnswer && !known.counted;
        known.counted |= count;
        Report { log, count }
    }

    /// Runs for the life of the process: every [`TLS_RELOAD_INTERVAL`] it looks at the files, so
    /// that a renewed certificate is in use by the next connection without a restart.
    pub async fn reload_on_change(self: Arc<Self>) -> anyhow::Result<()> {
        let interval = self
            .config
            .test_tls_reload_interval
            .unwrap_or(TLS_RELOAD_INTERVAL);
        loop {
            tokio::time::sleep(interval).await;
            self.reload_if_files_changed().await;
        }
    }
}

fn refusal_of(err: ShardManagerError) -> ProbeRefusal {
    match err {
        ShardManagerError::EtcdError(err) if is_certificate_failure(&err) => {
            ProbeRefusal::Refused(err.to_string())
        }
        other => ProbeRefusal::NoAnswer(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EtcdTlsConfig;
    use crate::sharding::etcd_connection::{connect_election_with, connect_requests_with};
    use std::path::PathBuf;
    use std::time::Duration;
    use test_r::test;

    /// A self-signed certificate and its key as PEM files: one certificate serves as the CA and
    /// as the client certificate.
    struct PemFiles {
        dir: tempfile::TempDir,
    }

    impl PemFiles {
        fn new() -> Self {
            let files = Self {
                dir: tempfile::tempdir().unwrap(),
            };
            files.renew();
            files
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn renew(&self) {
            let rcgen::CertifiedKey { cert, signing_key } =
                rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
            std::fs::write(self.path("cert.pem"), cert.pem()).unwrap();
            std::fs::write(self.path("key.pem"), signing_key.serialize_pem()).unwrap();
        }
    }

    fn failures() -> u64 {
        golem_service_base::metrics::tls::RELOAD_FAILURES_TOTAL
            .with_label_values(&["etcd client"])
            .get()
    }

    /// Clients for an etcd that is not there: building a client without a user makes no call.
    async fn clients_over(files: &PemFiles) -> Arc<EtcdClients> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let config = EtcdConfig {
            endpoints: vec!["https://127.0.0.1:1".to_string()],
            tls: Some(EtcdTlsConfig {
                ca_cert_file: Some(files.path("cert.pem")),
                ca_cert: None,
                client_cert_file: Some(files.path("cert.pem")),
                client_cert: None,
                client_key_file: Some(files.path("key.pem")),
                client_key: None,
                server_name: None,
            }),
            connect_timeout: Duration::from_millis(300),
            request_timeout: Duration::from_millis(300),
            ..EtcdConfig::default()
        };
        let settings = EtcdConnectSettings::load(&config).unwrap();
        let requests = connect_requests_with(&config, &settings).await.unwrap();
        let election = connect_election_with(&config, &settings).await.unwrap();
        EtcdClients::new(
            config,
            settings,
            RequestClient::fixed(requests),
            ElectionClient::fixed(election),
        )
    }

    #[test]
    async fn unchanged_files_are_left_alone() {
        let files = PemFiles::new();
        let clients = clients_over(&files).await;
        assert!(clients.has_tls_files());
        assert_eq!(
            clients.reload_if_files_changed().await,
            TlsReload::Unchanged
        );
    }

    #[test]
    async fn a_rejected_set_of_files_is_counted_once_and_a_silent_etcd_is_not_counted() {
        let files = PemFiles::new();
        let original_key = std::fs::read(files.path("key.pem")).unwrap();
        let clients = clients_over(&files).await;
        let before = failures();

        std::fs::write(files.path("key.pem"), "not a key").unwrap();
        assert!(matches!(
            clients.reload_if_files_changed().await,
            TlsReload::Rejected(_)
        ));
        // Looked at again, but reported and counted once.
        assert!(matches!(
            clients.reload_if_files_changed().await,
            TlsReload::Rejected(_)
        ));
        assert_eq!(failures() - before, 1);

        // Other content is another set of files.
        std::fs::write(files.path("key.pem"), "still not a key").unwrap();
        assert!(matches!(
            clients.reload_if_files_changed().await,
            TlsReload::Rejected(_)
        ));
        assert_eq!(failures() - before, 2);

        // Back to the files the clients were built from.
        std::fs::write(files.path("key.pem"), original_key).unwrap();
        assert_eq!(
            clients.reload_if_files_changed().await,
            TlsReload::Unchanged
        );
        assert_eq!(failures() - before, 2);

        // Valid files that etcd does not answer on: tried again at every look, and not counted.
        files.renew();
        for _ in 0..2 {
            match clients.reload_if_files_changed().await {
                TlsReload::Rejected(reason) => {
                    assert!(reason.contains("etcd did not answer"), "{reason}")
                }
                other => panic!("expected a rejection, got {other:?}"),
            }
        }
        assert_eq!(failures() - before, 2);
    }

    #[test]
    async fn a_flapping_etcd_does_not_make_the_same_files_reported_again() {
        let files = PemFiles::new();
        let clients = clients_over(&files).await;
        let same = EtcdConnectSettings::snapshot_of_files(&clients.config);
        let report = |kind| clients.first_report(same.clone(), kind);

        let first = report(RejectionKind::Refused);
        assert!(first.log && first.count);
        let silent = report(RejectionKind::NoAnswer);
        assert!(silent.log && !silent.count);
        let again = report(RejectionKind::Refused);
        assert!(!again.log && !again.count);
        let still = report(RejectionKind::NoAnswer);
        assert!(!still.log && !still.count);

        // Other bytes start over.
        std::fs::write(files.path("key.pem"), "other bytes").unwrap();
        let other = EtcdConnectSettings::snapshot_of_files(&clients.config);
        let fresh = clients.first_report(other, RejectionKind::Refused);
        assert!(fresh.log && fresh.count);
    }
}
