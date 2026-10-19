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

use crate::sharding::leader_election::LeaseLost;
use crate::sharding::model::ExecutorId;
use golem_common::retriable_error::IsRetriableError;
use golem_service_base::error::worker_executor::WorkerExecutorError;
use golem_service_base::repo::RepoError;
use std::fmt::Debug;

#[derive(thiserror::Error, Debug)]
pub enum ShardManagerError {
    #[error("No source IP for pod")]
    NoSourceIpForPod,
    #[error("Failed to resolve address for pod")]
    FailedAddressResolveForPod,
    #[error("Timeout")]
    Timeout,
    #[error("gRPC: error status: {0}")]
    GrpcError(#[from] tonic::Status),
    #[error("No result")]
    NoResult,
    #[error("Worker execution error: {0}")]
    WorkerExecutionError(WorkerExecutorError),
    #[error("Persistence serialization error {0}")]
    SerializationError(String),
    #[error("Concurrent modification: the persisted shard state was changed by another writer")]
    ConcurrentModification,
    #[error("No shard lease for executor {executor_id}")]
    ShardLeaseNotFound { executor_id: ExecutorId },
    #[error(
        "Leadership lost: the election key {leader_key} is no longer held at creation revision \
         {create_revision}"
    )]
    LeadershipLost {
        leader_key: String,
        create_revision: i64,
    },
    #[error("Leadership lease lost while campaigning: {0}")]
    LeaseLostWhileCampaigning(#[source] LeaseLost),
    #[error("Shutdown requested")]
    ShutdownRequested,
    #[error("DB error {0}")]
    RepoError(#[from] RepoError),
    #[error("etcd error {0}")]
    EtcdError(#[from] etcd_client::Error),
    #[error("Migration error {0}")]
    MigrationError(#[from] anyhow::Error),
    #[error("IO error {0}")]
    IoError(#[from] std::io::Error),
    /// A setting that cannot work, found at startup; the message names the setting.
    #[error("Invalid configuration: {0}")]
    InvalidConfig(String),
    #[error("Internal error: {0}")]
    Internal(String),
    /// A certificate refused during startup; the source says which and why.
    #[error("TLS handshake with etcd failed")]
    TlsHandshakeFailed(#[source] etcd_client::Error),
}

impl ShardManagerError {
    /// A second copy of this error, for the fail-stop slot: a refused write has to reach both the
    /// caller whose request it was and the loop that must end the process because of it.
    ///
    /// Every variant a caller *matches* on - a lost fence, a revision conflict, a shutdown - is
    /// reproduced exactly. The ones whose payload cannot be duplicated (`anyhow`, `io`, `RepoError`,
    /// `etcd_client`) degrade to [`ShardManagerError::Internal`] carrying the same message, which
    /// is what a log line or a gRPC error body would have shown of them anyway.
    pub(crate) fn duplicate(&self) -> Self {
        match self {
            Self::NoSourceIpForPod => Self::NoSourceIpForPod,
            Self::FailedAddressResolveForPod => Self::FailedAddressResolveForPod,
            Self::Timeout => Self::Timeout,
            Self::GrpcError(status) => Self::GrpcError(status.clone()),
            Self::NoResult => Self::NoResult,
            Self::SerializationError(message) => Self::SerializationError(message.clone()),
            Self::ConcurrentModification => Self::ConcurrentModification,
            Self::ShardLeaseNotFound { executor_id } => Self::ShardLeaseNotFound {
                executor_id: *executor_id,
            },
            Self::LeadershipLost {
                leader_key,
                create_revision,
            } => Self::LeadershipLost {
                leader_key: leader_key.clone(),
                create_revision: *create_revision,
            },
            Self::ShutdownRequested => Self::ShutdownRequested,
            Self::InvalidConfig(message) => Self::InvalidConfig(message.clone()),
            Self::Internal(message) => Self::Internal(message.clone()),
            other => Self::Internal(other.to_string()),
        }
    }
}

/// etcd's election service runs on etcd's in-process client, which loses the gRPC code: a write
/// that the user's role does not allow comes back as `Unknown` with etcd's text instead of
/// `PermissionDenied`. The role does not change while the replica runs, so a retry could only loop.
fn is_denied_by_election(status: &tonic::Status) -> bool {
    status.code() == tonic::Code::Unknown && status.message() == "etcdserver: permission denied"
}

/// etcd's answer to a token from before a change to its users or roles.
const STALE_AUTH_TOKEN: &str = "etcdserver: revision of auth store is old";

/// Whether etcd refused the auth token because its users or roles changed since the login. A JWT
/// token carries the revision of etcd's auth store at login, so after any change - a user added, a
/// password changed - etcd refuses it: `InvalidArgument`, or `Unknown` through the election
/// service, which loses the code. etcd-client renews a token only when etcd calls it invalid, so
/// the caller logs in again itself.
pub(crate) fn is_stale_auth_token(err: &ShardManagerError) -> bool {
    matches!(
        err,
        ShardManagerError::EtcdError(etcd_client::Error::GRpcStatus(status))
            if matches!(status.code(), tonic::Code::InvalidArgument | tonic::Code::Unknown)
                && status.message() == STALE_AUTH_TOKEN
    )
}

/// The alerts with which a TLS peer refuses a certificate.
const CERTIFICATE_ALERTS: [rustls::AlertDescription; 7] = [
    rustls::AlertDescription::BadCertificate,
    rustls::AlertDescription::UnsupportedCertificate,
    rustls::AlertDescription::CertificateRevoked,
    rustls::AlertDescription::CertificateExpired,
    rustls::AlertDescription::CertificateUnknown,
    rustls::AlertDescription::UnknownCA,
    rustls::AlertDescription::CertificateRequired,
];

/// The TLS error under an etcd error: rustls's own, or only its text.
enum TlsError<'a> {
    Typed(&'a rustls::Error),
    /// A refusal that etcd sends after the handshake - a missing or rejected client certificate
    /// under TLS 1.3 - is read through h2, which keeps only the text of the I/O error's inner
    /// error.
    Text(String),
}

fn tls_error(err: &etcd_client::Error) -> Option<TlsError<'_>> {
    // etcd-client's error does not expose its cause, so the walk starts at what it wraps.
    let start: &(dyn std::error::Error + 'static) = match err {
        etcd_client::Error::GRpcStatus(status) => status,
        etcd_client::Error::TransportError(error) => error,
        _ => return None,
    };
    let mut current = Some(start);
    while let Some(error) = current {
        // tokio-rustls wraps a rustls error in an `InvalidData` I/O error, whose `source` skips
        // the wrapped error itself.
        let io_inner = error
            .downcast_ref::<std::io::Error>()
            .filter(|io| io.kind() == std::io::ErrorKind::InvalidData)
            .and_then(|io| io.get_ref());
        let rustls_error = error
            .downcast_ref::<rustls::Error>()
            .or_else(|| io_inner.and_then(|inner| inner.downcast_ref::<rustls::Error>()));
        if let Some(rustls_error) = rustls_error {
            return Some(TlsError::Typed(rustls_error));
        }
        if let Some(inner) = io_inner {
            return Some(TlsError::Text(inner.to_string()));
        }
        current = error.source();
    }
    None
}

/// Whether an etcd error is a refused certificate: one that etcd rejected, or one of etcd's that
/// this replica rejected. Other TLS errors - a damaged record, a non-TLS reply from a proxy during
/// an outage - are network faults like any other, and stay retriable.
pub(crate) fn is_certificate_failure(err: &etcd_client::Error) -> bool {
    match tls_error(err) {
        Some(TlsError::Typed(
            rustls::Error::InvalidCertificate(_) | rustls::Error::NoCertificatesPresented,
        )) => true,
        Some(TlsError::Typed(rustls::Error::AlertReceived(alert))) => {
            CERTIFICATE_ALERTS.contains(alert)
        }
        Some(TlsError::Typed(_)) | None => false,
        // The text is rustls's own for the alert.
        Some(TlsError::Text(text)) => CERTIFICATE_ALERTS
            .iter()
            .any(|alert| text == rustls::Error::AlertReceived(*alert).to_string()),
    }
}

/// When this replica refused etcd's certificate because it is not valid now - expired, or dated
/// ahead of this node's clock - what is wrong with it. Such a certificate is etcd's to renew, and
/// nothing in this replica's settings can fix it. An alert from etcd about this replica's own
/// certificate is not one of these.
pub(crate) fn etcd_certificate_not_valid_now(err: &etcd_client::Error) -> Option<String> {
    use rustls::CertificateError;

    let date = |time: rustls::pki_types::UnixTime| {
        i64::try_from(time.as_secs())
            .ok()
            .and_then(|seconds| x509_parser::time::ASN1Time::from_timestamp(seconds).ok())
            .map_or_else(
                || format!("{} (UNIX)", time.as_secs()),
                |date| date.to_string(),
            )
    };
    match tls_error(err)? {
        TlsError::Typed(rustls::Error::InvalidCertificate(error)) => match error {
            CertificateError::ExpiredContext { not_after, .. } => Some(format!(
                "etcd's TLS certificate expired at {}",
                date(*not_after)
            )),
            CertificateError::NotValidYetContext { not_before, .. } => Some(format!(
                "etcd's TLS certificate is valid only from {}; check this node's clock",
                date(*not_before)
            )),
            CertificateError::Expired => Some("etcd's TLS certificate has expired".to_string()),
            CertificateError::NotValidYet => {
                Some("etcd's TLS certificate is not valid yet; check this node's clock".to_string())
            }
            _ => None,
        },
        _ => None,
    }
}

impl IsRetriableError for ShardManagerError {
    fn is_retriable(&self) -> bool {
        match self {
            ShardManagerError::NoSourceIpForPod => false,
            ShardManagerError::FailedAddressResolveForPod => false,
            ShardManagerError::Timeout => true,
            ShardManagerError::GrpcError(status) => status.is_retriable(),
            ShardManagerError::NoResult => true,
            ShardManagerError::WorkerExecutionError(_) => true, // TODO: can we define which ones are retryable?
            ShardManagerError::SerializationError(_) => false,
            // Retrying a compare-and-swap with the same, now stale, previous revision can never
            // succeed: recovery is a re-read followed by re-deriving the change, which is a
            // different operation. Reporting this as retriable would turn a conflict into a spin.
            ShardManagerError::ConcurrentModification => false,
            // The executor holds no lease at all, so the same request can only be refused again;
            // recovery is a fresh registration, which is a different call.
            ShardManagerError::ShardLeaseNotFound { .. } => false,
            // Another replica holds the leadership now; no retry here can take it back.
            ShardManagerError::LeadershipLost { .. } => false,
            // A campaigner holds nothing yet: a fresh lease and a new campaign is full recovery.
            ShardManagerError::LeaseLostWhileCampaigning(_) => true,
            // Retrying would be the process refusing the stop it was just asked for.
            ShardManagerError::ShutdownRequested => false,
            ShardManagerError::RepoError(_) => false,
            // A refused certificate is retried like a network fault while the replica runs:
            // etcd's certificate can be fixed on etcd's side. Before the campaign it becomes
            // `TlsHandshakeFailed` instead, and a watch stops a replica whose files changed.
            ShardManagerError::EtcdError(err) => match err {
                etcd_client::Error::GRpcStatus(status) if is_denied_by_election(status) => false,
                etcd_client::Error::GRpcStatus(status) => status.is_retriable(),
                etcd_client::Error::TransportError(_)
                | etcd_client::Error::IoError(_)
                | etcd_client::Error::EndpointError(_) => true,
                // A catch-all is required regardless: `etcd_client::Error` has a
                // `#[cfg(feature = "tls-openssl")]` variant. Everything else - bad URI, bad
                // arguments, bad metadata - is a configuration bug, not a transient failure.
                _ => false,
            },
            ShardManagerError::MigrationError(_) => false,
            ShardManagerError::IoError(_) => false,
            // The configuration does not change while the process runs.
            ShardManagerError::InvalidConfig(_) => false,
            ShardManagerError::Internal(_) => false,
            // Startup stops on it: the files were just read, so a retry presents the same ones.
            ShardManagerError::TlsHandshakeFailed(_) => false,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

#[derive(thiserror::Error, Debug)]
pub enum HealthCheckError {
    #[error("gRPC: error status: {0}")]
    GrpcError(tonic::Status),
    #[error("gRPC: transport error: {0}")]
    GrpcTransportError(#[source] tonic::transport::Error),
    #[error("gRPC: {0}")]
    GrpcOther(&'static str),
    #[cfg(feature = "kubernetes")]
    #[error("K8s: connect error: {0}")]
    K8sConnectError(#[source] kube::Error),
    #[cfg(feature = "kubernetes")]
    #[error("K8s: pod not found")]
    K8sPodNotFound,
    #[cfg(feature = "kubernetes")]
    #[error("K8s: pod terminated")]
    K8sPodTerminated,
    #[cfg(feature = "kubernetes")]
    #[error("K8s: pod is not ready")]
    K8sPodNotReady,
    #[cfg(feature = "kubernetes")]
    #[error("K8s: no pod status")]
    K8sNoPodStatus,
    #[cfg(feature = "kubernetes")]
    #[error("K8s: no pod name")]
    K8sNoPodName,
}

impl IsRetriableError for HealthCheckError {
    fn is_retriable(&self) -> bool {
        match self {
            HealthCheckError::GrpcError(status) => status.is_retriable(),
            HealthCheckError::GrpcTransportError(_) => true,
            HealthCheckError::GrpcOther(_) => true,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sConnectError(_) => true,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sPodNotFound => false,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sPodTerminated => false,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sPodNotReady => true,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sNoPodStatus => true,
            #[cfg(feature = "kubernetes")]
            HealthCheckError::K8sNoPodName => false,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::{
        ShardManagerError, etcd_certificate_not_valid_now, is_certificate_failure,
        is_stale_auth_token,
    };
    use crate::sharding::model::ExecutorId;
    use golem_common::retriable_error::IsRetriableError;

    #[test]
    // `with_retries` re-invokes with the same arguments, so a retry of any of these can only
    // spin: the revision stays stale, the leadership stays lost, the stop stays requested.
    fn the_fail_stop_errors_are_not_retriable() {
        assert!(!ShardManagerError::ConcurrentModification.is_retriable());
        assert!(
            !ShardManagerError::LeadershipLost {
                leader_key: "/golem/shard-manager/leader/abc".to_string(),
                create_revision: 7,
            }
            .is_retriable()
        );
        assert!(!ShardManagerError::ShutdownRequested.is_retriable());
        assert!(
            !ShardManagerError::ShardLeaseNotFound {
                executor_id: ExecutorId(uuid::Uuid::from_u128(1)),
            }
            .is_retriable()
        );
    }

    /// An etcd error as tonic reports a connection that failed: `UNAVAILABLE`, with the
    /// connector's error, and under it `cause`, as its source.
    fn connect_failure(
        cause: impl std::error::Error + Send + Sync + 'static,
    ) -> etcd_client::Error {
        let status = tonic::Status::from_error(Box::new(tonic::ConnectError(Box::new(cause))));
        etcd_client::Error::GRpcStatus(status)
    }

    /// A rustls error as tokio-rustls reports it.
    fn tls(error: rustls::Error) -> std::io::Error {
        std::io::Error::new(std::io::ErrorKind::InvalidData, error)
    }

    #[test]
    // A damaged record, a reply that is not TLS, or a network fault can pass on a retry; only a
    // refused certificate is a setting or a certificate to fix.
    fn only_a_refused_certificate_counts_as_a_certificate_failure() {
        use rustls::{AlertDescription, CertificateError};

        // Listed here rather than taken from the rule, so that an alert dropped from it fails.
        let alerts = [
            AlertDescription::BadCertificate,
            AlertDescription::UnsupportedCertificate,
            AlertDescription::CertificateRevoked,
            AlertDescription::CertificateExpired,
            AlertDescription::CertificateUnknown,
            AlertDescription::UnknownCA,
            AlertDescription::CertificateRequired,
        ]
        .into_iter()
        .flat_map(|alert| {
            [
                tls(rustls::Error::AlertReceived(alert)),
                // The same alert after the handshake, as h2 passes it on: the text only.
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    rustls::Error::AlertReceived(alert).to_string(),
                ),
            ]
        });
        for refused in [
            tls(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer,
            )),
            tls(rustls::Error::InvalidCertificate(CertificateError::Expired)),
            tls(rustls::Error::NoCertificatesPresented),
        ]
        .into_iter()
        .chain(alerts)
        {
            let failure = connect_failure(refused);
            assert!(is_certificate_failure(&failure), "{failure}");
        }
        for fault in [
            tls(rustls::Error::DecryptError),
            // An alert that is not about a certificate.
            tls(rustls::Error::AlertReceived(
                AlertDescription::HandshakeFailure,
            )),
            tls(rustls::Error::InvalidMessage(
                rustls::InvalidMessage::InvalidContentType,
            )),
            std::io::Error::new(std::io::ErrorKind::InvalidData, "not a TLS record"),
            std::io::Error::new(std::io::ErrorKind::ConnectionRefused, "network fault"),
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "network fault"),
            std::io::Error::new(std::io::ErrorKind::TimedOut, "network fault"),
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "network fault"),
        ] {
            let failure = connect_failure(fault);
            assert!(!is_certificate_failure(&failure), "{failure}");
            assert!(ShardManagerError::EtcdError(failure).is_retriable());
        }
    }

    #[test]
    // etcd's certificate out of its validity is etcd's to renew; etcd's alert about this replica's
    // own certificate - even an expired one - is this replica's to fix.
    fn only_an_etcd_certificate_out_of_its_validity_is_etcds_to_fix() {
        use rustls::pki_types::UnixTime;
        use rustls::{AlertDescription, CertificateError};

        let at = |seconds| UnixTime::since_unix_epoch(std::time::Duration::from_secs(seconds));
        let expired = connect_failure(tls(rustls::Error::InvalidCertificate(
            CertificateError::ExpiredContext {
                time: at(1_100_000_000),
                not_after: at(1_009_843_200),
            },
        )));
        assert_eq!(
            etcd_certificate_not_valid_now(&expired).as_deref(),
            Some("etcd's TLS certificate expired at Jan  1 00:00:00 2002 +00:00")
        );
        for not_valid in [
            CertificateError::Expired,
            CertificateError::NotValidYet,
            CertificateError::NotValidYetContext {
                time: at(1_000_000_000),
                not_before: at(1_009_843_200),
            },
        ] {
            let failure = connect_failure(tls(rustls::Error::InvalidCertificate(not_valid)));
            assert!(
                etcd_certificate_not_valid_now(&failure).is_some(),
                "{failure}"
            );
        }
        for ours in [
            tls(rustls::Error::InvalidCertificate(
                CertificateError::UnknownIssuer,
            )),
            tls(rustls::Error::AlertReceived(
                AlertDescription::CertificateExpired,
            )),
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                rustls::Error::AlertReceived(AlertDescription::CertificateExpired).to_string(),
            ),
        ] {
            let failure = connect_failure(ours);
            assert_eq!(etcd_certificate_not_valid_now(&failure), None, "{failure}");
        }
    }

    #[test]
    fn a_token_from_before_an_auth_change_is_recognised_from_both_services() {
        let status = |code, message| {
            ShardManagerError::EtcdError(etcd_client::Error::GRpcStatus(tonic::Status::new(
                code, message,
            )))
        };
        let stale = "etcdserver: revision of auth store is old";
        assert!(is_stale_auth_token(&status(
            tonic::Code::InvalidArgument,
            stale
        )));
        assert!(is_stale_auth_token(&status(tonic::Code::Unknown, stale)));
        assert!(!is_stale_auth_token(&status(
            tonic::Code::InvalidArgument,
            "etcdserver: authentication failed, invalid user ID or password"
        )));
        assert!(!is_stale_auth_token(&status(
            tonic::Code::Unavailable,
            stale
        )));
    }

    #[test]
    fn a_write_denied_by_the_election_service_is_not_retriable() {
        let denied = |code| {
            ShardManagerError::EtcdError(etcd_client::Error::GRpcStatus(tonic::Status::new(
                code,
                "etcdserver: permission denied",
            )))
        };
        assert!(!denied(tonic::Code::Unknown).is_retriable());
        assert!(!denied(tonic::Code::PermissionDenied).is_retriable());
        let other_unknown = ShardManagerError::EtcdError(etcd_client::Error::GRpcStatus(
            tonic::Status::new(tonic::Code::Unknown, "h2 protocol error"),
        ));
        assert!(other_unknown.is_retriable());
    }

    #[test]
    // While the replica runs, etcd's certificate can be fixed on etcd's side, so a refusal is
    // retried; before the campaign the startup turns it into `TlsHandshakeFailed` and stops.
    fn a_refused_certificate_is_retried_at_run_time_but_stops_the_startup() {
        let refused = || {
            connect_failure(tls(rustls::Error::InvalidCertificate(
                rustls::CertificateError::UnknownIssuer,
            )))
        };
        assert!(ShardManagerError::EtcdError(refused()).is_retriable());
        assert!(!ShardManagerError::TlsHandshakeFailed(refused()).is_retriable());
    }
}
