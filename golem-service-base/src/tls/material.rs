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

//! The current certificate, key and CA of one TLS use, kept behind an `ArcSwap`. File sources are
//! re-read on a timer and swapped in when their content changes and the new set is usable.

use super::pem::{CertificateInfo, PemSource, TlsError, check_key_matches, describe_certificates};
use crate::metrics::tls::{record_certificate_not_after, record_reload_failure};
use arc_swap::ArcSwap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, SystemTime};
use tokio::sync::OnceCell;
use tokio::task::{JoinError, JoinHandle};
use tracing::{error, info};

/// How often the gRPC servers and clients, and the shard manager's etcd clients, look at their TLS
/// files. The kubelet rewrites a mounted Secret only on its own pod sync, once a minute, so a
/// faster poll would mostly read files that have not changed yet.
pub const TLS_RELOAD_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct TlsSources {
    pub cert: PemSource,
    pub key: PemSource,
    pub ca: PemSource,
}

impl TlsSources {
    fn all(&self) -> [&PemSource; 3] {
        [&self.cert, &self.key, &self.ca]
    }
}

pub struct TlsLoaded {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
    pub ca_pem: Vec<u8>,
    /// The leaf: the first certificate in `cert_pem`.
    pub certificate: CertificateInfo,
    pub ca: Vec<CertificateInfo>,
}

impl std::fmt::Debug for TlsLoaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsLoaded")
            .field("certificate", &self.certificate)
            .field("ca", &self.ca)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub enum ReloadOutcome {
    Unchanged,
    Reloaded,
    Rejected(TlsError),
}

pub struct TlsMaterial {
    name: String,
    sources: TlsSources,
    current: ArcSwap<TlsLoaded>,
    /// The file bytes of the last rejected set, so one bad set is reported once.
    rejected: Mutex<Option<[Vec<u8>; 3]>>,
    reloader: OnceLock<JoinHandle<()>>,
    /// Set once a client's first look at the files is done; see [`Self::start_reloader_checked`].
    first_check: OnceCell<()>,
    /// Serializes the read, compare, validate and store of one reload, so an older read can never
    /// be stored over a newer one. Readers of `current` never take it. Only
    /// [`Self::reload_if_changed`] takes it, and the services run that on Tokio's blocking pool,
    /// so a wait for it never holds up an async worker thread.
    reload_lock: Mutex<()>,
    /// Test-only seam: called once the files are read, before anything is compared or stored.
    #[cfg(test)]
    after_read: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl TlsMaterial {
    /// Reads, validates and logs. `name` is the use, for example `grpc server`. Errors name the
    /// setting.
    pub fn load(name: impl Into<String>, sources: TlsSources) -> Result<Arc<Self>, TlsError> {
        let name = name.into();
        let loaded = read_and_check(&name, &sources, SystemTime::now())?;
        log_loaded(&name, &sources, &loaded, "loaded");
        record_certificate_not_after(&name, loaded.certificate.not_after);
        Ok(Arc::new(Self {
            name,
            sources,
            current: ArcSwap::from_pointee(loaded),
            rejected: Mutex::new(None),
            reloader: OnceLock::new(),
            first_check: OnceCell::new(),
            reload_lock: Mutex::new(()),
            #[cfg(test)]
            after_read: Mutex::new(None),
        }))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn current(&self) -> Arc<TlsLoaded> {
        self.current.load_full()
    }

    /// Whether `loaded` is still the current material: one `ArcSwap` load and a pointer compare,
    /// without a lock, so that a client can ask on every call. Only a reload that swaps in a
    /// changed set stores a new value, so an unchanged or a rejected set never makes this false.
    pub fn is_current(&self, loaded: &Arc<TlsLoaded>) -> bool {
        Arc::ptr_eq(&self.current.load(), loaded)
    }

    /// Whether any source is a file, and so can change.
    pub fn has_files(&self) -> bool {
        self.sources.all().iter().any(|s| s.is_file())
    }

    /// Reads the sources. Equal bytes: `Unchanged`. Different bytes are validated on every call
    /// (a certificate that is not yet valid becomes usable without its bytes changing); a usable
    /// set is swapped in, an unusable one is kept out. The error log and the failure counter fire
    /// once per distinct rejected set of bytes.
    ///
    /// This blocks: it reads files, checks certificates and waits for any other reload. The poller
    /// and a client's first look run it on Tokio's blocking pool.
    pub fn reload_if_changed(&self) -> ReloadOutcome {
        self.reload_if_changed_at(SystemTime::now())
    }

    fn reload_if_changed_at(&self, now: SystemTime) -> ReloadOutcome {
        if !self.has_files() {
            return ReloadOutcome::Unchanged;
        }
        let _serialized = self
            .reload_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (snapshot, read_error) = read_snapshot(&self.sources);
        #[cfg(test)]
        {
            let hook = self
                .after_read
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            if let Some(hook) = hook {
                hook();
            }
        }
        let outcome = match read_error {
            Some(error) => Err(error),
            None => {
                let current = self.current.load();
                if snapshot[0] == current.cert_pem
                    && snapshot[1] == current.key_pem
                    && snapshot[2] == current.ca_pem
                {
                    *self
                        .rejected
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                    return ReloadOutcome::Unchanged;
                }
                check(&self.sources, snapshot.clone(), now)
            }
        };
        match outcome {
            Ok(loaded) => {
                log_loaded(&self.name, &self.sources, &loaded, "reloaded");
                record_certificate_not_after(&self.name, loaded.certificate.not_after);
                self.current.store(Arc::new(loaded));
                *self
                    .rejected
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                ReloadOutcome::Reloaded
            }
            Err(error) => {
                let first_time = {
                    let mut rejected = self
                        .rejected
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    let same = rejected.as_ref() == Some(&snapshot);
                    if !same {
                        *rejected = Some(snapshot);
                    }
                    !same
                };
                if first_time {
                    record_reload_failure(&self.name);
                    error!(
                        r#use = %self.name,
                        error = %error,
                        "Changed TLS files cannot be used; keeping the current certificate"
                    );
                }
                ReloadOutcome::Rejected(error)
            }
        }
    }

    /// Spawns, once, a task that calls `reload_if_changed` every `interval` while any `Arc` of
    /// this value lives. Spawns nothing when no source is a file. Needs a Tokio runtime. Does not
    /// block: it reads no file, and each reload runs on Tokio's blocking pool. A reload that
    /// panics is logged, and the next one runs at the next interval.
    ///
    /// For a server, which loaded the files just before. A client uses
    /// [`Self::start_reloader_checked`].
    pub fn start_reloader(self: &Arc<Self>, interval: Duration) {
        if !self.has_files() {
            return;
        }
        self.reloader.get_or_init(|| {
            let weak = Arc::downgrade(self);
            let name = self.name.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    if weak.strong_count() == 0 {
                        break;
                    }
                    match Self::reload_on_blocking_thread(weak.clone()).await {
                        Ok(Some(_)) => {}
                        Ok(None) => break,
                        Err(error) => error!(
                            r#use = %name,
                            error = %error,
                            "The TLS file reload failed; trying again at the next poll"
                        ),
                    }
                }
            })
        });
    }

    /// [`Self::start_reloader`] for a client, which starts it at its first connection: that can
    /// come long after the files were loaded, and the connection must not wait a whole interval
    /// for a certificate that was renewed in the meantime. So this looks at the files once first,
    /// on Tokio's blocking pool, and returns when that look is done. Concurrent first connections
    /// all wait for the same look; later calls return at once.
    pub async fn start_reloader_checked(self: &Arc<Self>, interval: Duration) {
        if !self.has_files() {
            return;
        }
        self.first_check
            .get_or_init(|| async {
                if let Err(error) = Self::reload_on_blocking_thread(Arc::downgrade(self)).await {
                    error!(
                        r#use = %self.name,
                        error = %error,
                        "The first look at the TLS files failed; the poller tries again"
                    );
                }
                self.start_reloader(interval);
            })
            .await;
    }

    /// Runs [`Self::reload_if_changed`] on Tokio's blocking pool, so that the file reads, the
    /// checks and `reload_lock` stay off the async worker threads. The material is held only
    /// while the reload runs; `None` when it is gone. A panic in the reload is the `JoinError`.
    async fn reload_on_blocking_thread(
        weak: Weak<Self>,
    ) -> Result<Option<ReloadOutcome>, JoinError> {
        tokio::task::spawn_blocking(move || weak.upgrade().map(|me| me.reload_if_changed())).await
    }

    #[cfg(test)]
    fn reloader_started(&self) -> bool {
        self.reloader.get().is_some()
    }
}

/// The bytes of the three sources; a source that cannot be read counts as empty bytes. Returns the
/// first read error.
fn read_snapshot(sources: &TlsSources) -> ([Vec<u8>; 3], Option<TlsError>) {
    let mut error = None;
    let mut read = |source: &PemSource| match source.read() {
        Ok(bytes) => bytes,
        Err(e) => {
            error.get_or_insert(e);
            Vec::new()
        }
    };
    let snapshot = [read(&sources.cert), read(&sources.key), read(&sources.ca)];
    (snapshot, error)
}

fn read_and_check(
    _name: &str,
    sources: &TlsSources,
    now: SystemTime,
) -> Result<TlsLoaded, TlsError> {
    let (snapshot, error) = read_snapshot(sources);
    if let Some(error) = error {
        return Err(error);
    }
    check(sources, snapshot, now)
}

fn check(
    sources: &TlsSources,
    [cert_pem, key_pem, ca_pem]: [Vec<u8>; 3],
    now: SystemTime,
) -> Result<TlsLoaded, TlsError> {
    let cert_name = sources.cert.describe();
    let key_name = sources.key.describe();
    let ca_name = sources.ca.describe();

    let certificates = describe_certificates(&cert_name, &cert_pem)?;
    let certificate = certificates[0].clone();
    if now < certificate.not_before || now > certificate.not_after {
        return Err(TlsError::NotValidNow {
            source: cert_name,
            subject: certificate.subject,
            not_before: certificate.not_before,
            not_after: certificate.not_after,
        });
    }
    check_key_matches(&cert_name, &cert_pem, &key_name, &key_pem)?;
    let ca = describe_certificates(&ca_name, &ca_pem)?;
    if !ca.iter().any(|c| c.not_after >= now) {
        return Err(TlsError::AllCaExpired { source: ca_name });
    }
    Ok(TlsLoaded {
        cert_pem,
        key_pem,
        ca_pem,
        certificate,
        ca,
    })
}

fn rfc3339(time: SystemTime) -> String {
    chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn log_loaded(name: &str, sources: &TlsSources, loaded: &TlsLoaded, what: &str) {
    info!(
        r#use = name,
        source = %sources.cert.describe(),
        subject = %loaded.certificate.subject,
        not_after = %rfc3339(loaded.certificate.not_after),
        sha256 = %loaded.certificate.sha256,
        "TLS certificate {what}"
    );
    for ca in &loaded.ca {
        info!(
            r#use = name,
            source = %sources.ca.describe(),
            subject = %ca.subject,
            not_after = %rfc3339(ca.not_after),
            sha256 = %ca.sha256,
            "TLS CA certificate {what}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::tls::RELOAD_FAILURES_TOTAL;
    use golem_common::model::base64::Base64;
    use rcgen::{
        BasicConstraints, CertificateParams, CertifiedIssuer, DnType, ExtendedKeyUsagePurpose,
        IsCa, Issuer, KeyPair, KeyUsagePurpose,
    };
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use test_r::test;

    fn test_ca() -> CertifiedIssuer<'static, KeyPair> {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(DnType::CommonName, "material test CA");
        params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        params.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign];
        CertifiedIssuer::self_signed(params, KeyPair::generate().unwrap()).unwrap()
    }

    /// A leaf certificate and its key, both PEM, valid in `[not_before, not_after]` when given.
    fn signed_by(
        ca: &CertifiedIssuer<'static, KeyPair>,
        window: Option<(SystemTime, SystemTime)>,
    ) -> (String, String) {
        let mut params = CertificateParams::new(vec!["localhost".to_string()]).unwrap();
        params
            .distinguished_name
            .push(DnType::CommonName, "localhost");
        if let Some((from, to)) = window {
            let epoch = rcgen::date_time_ymd(1970, 1, 1);
            params.not_before = epoch + from.duration_since(SystemTime::UNIX_EPOCH).unwrap();
            params.not_after = epoch + to.duration_since(SystemTime::UNIX_EPOCH).unwrap();
        }
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
        let key = KeyPair::generate().unwrap();
        let issuer: &Issuer<'static, KeyPair> = ca;
        let cert = params.signed_by(&key, issuer).unwrap();
        (cert.pem(), key.serialize_pem())
    }

    fn file_sources(dir: &Path) -> TlsSources {
        let file = |setting: &str, name: &str| PemSource::File {
            setting: setting.to_string(),
            path: dir.join(name),
        };
        TlsSources {
            cert: file("server_cert", "tls.crt"),
            key: file("server_key", "tls.key"),
            ca: file("client_ca_cert", "ca.crt"),
        }
    }

    fn sha(pem: &str) -> String {
        let der = rustls::pki_types::CertificateDer::from_pem_slice(pem.as_bytes());
        hex::encode(Sha256::digest(der.unwrap()))
    }

    struct Fixture {
        dir: tempfile::TempDir,
        ca: CertifiedIssuer<'static, KeyPair>,
        cert: String,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let ca = test_ca();
            let (cert, key) = signed_by(&ca, None);
            std::fs::write(dir.path().join("tls.crt"), &cert).unwrap();
            std::fs::write(dir.path().join("tls.key"), key).unwrap();
            std::fs::write(dir.path().join("ca.crt"), ca.pem()).unwrap();
            Self { dir, ca, cert }
        }

        fn sources(&self) -> TlsSources {
            file_sources(self.dir.path())
        }

        fn write(&self, name: &str, content: &str) {
            std::fs::write(self.dir.path().join(name), content).unwrap();
        }

        fn failures(name: &str) -> u64 {
            RELOAD_FAILURES_TOTAL.with_label_values(&[name]).get()
        }
    }

    use rustls::pki_types::pem::PemObject;

    #[test]
    fn material_loads_from_files_and_reports_the_leaf_certificate() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-load", f.sources()).unwrap();
        assert_eq!(m.name(), "t-load");
        assert_eq!(m.current().certificate.sha256, sha(&f.cert));
        assert!(m.has_files());
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Unchanged));
    }

    #[test]
    fn changed_files_are_reloaded_for_the_next_reader() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-reload", f.sources()).unwrap();
        let (cert, key) = signed_by(&f.ca, None);
        f.write("tls.crt", &cert);
        f.write("tls.key", &key);
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Reloaded));
        assert_eq!(m.current().certificate.sha256, sha(&cert));
    }

    #[test]
    fn a_poisoned_reload_lock_does_not_stop_later_reloads() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-poisoned", f.sources()).unwrap();
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = m.reload_lock.lock().unwrap();
            panic!("boom");
        }));
        assert!(poisoned.is_err());
        let (cert, key) = signed_by(&f.ca, None);
        f.write("tls.crt", &cert);
        f.write("tls.key", &key);
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Reloaded));
        assert_eq!(m.current().certificate.sha256, sha(&cert));
    }

    #[test]
    fn a_mismatched_key_is_rejected_and_the_next_consistent_pair_is_loaded() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-mismatch", f.sources()).unwrap();
        let (cert, key) = signed_by(&f.ca, None);
        f.write("tls.crt", &cert);
        assert!(matches!(
            m.reload_if_changed(),
            ReloadOutcome::Rejected(TlsError::KeyMismatch { .. })
        ));
        assert_eq!(m.current().certificate.sha256, sha(&f.cert));
        f.write("tls.key", &key);
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Reloaded));
        assert_eq!(m.current().certificate.sha256, sha(&cert));
    }

    #[test]
    fn an_unreadable_file_keeps_the_current_material_and_logs_once() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-unreadable", f.sources()).unwrap();
        let key = std::fs::read_to_string(f.dir.path().join("tls.key")).unwrap();
        std::fs::remove_file(f.dir.path().join("tls.key")).unwrap();
        let before = Fixture::failures("t-unreadable");
        assert!(matches!(
            m.reload_if_changed(),
            ReloadOutcome::Rejected(TlsError::Read { .. })
        ));
        assert_eq!(Fixture::failures("t-unreadable"), before + 1);
        assert!(matches!(
            m.reload_if_changed(),
            ReloadOutcome::Rejected(TlsError::Read { .. })
        ));
        assert_eq!(Fixture::failures("t-unreadable"), before + 1);
        assert_eq!(m.current().certificate.sha256, sha(&f.cert));
        f.write("tls.key", &key);
        // Same content as the current material: nothing to reload.
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Unchanged));
        // A different certificate and key after the outage is loaded.
        let (cert, key) = signed_by(&f.ca, None);
        std::fs::remove_file(f.dir.path().join("tls.key")).unwrap();
        f.write("tls.crt", &cert);
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Rejected(_)));
        f.write("tls.key", &key);
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Reloaded));
    }

    #[test]
    fn a_certificate_outside_its_validity_is_rejected() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-expired", f.sources()).unwrap();
        let past =
            |years: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(365 * 24 * 3600 * years);
        let (cert, key) = signed_by(&f.ca, Some((past(31), past(32))));
        f.write("tls.crt", &cert);
        f.write("tls.key", &key);
        assert!(matches!(
            m.reload_if_changed(),
            ReloadOutcome::Rejected(TlsError::NotValidNow { .. })
        ));
        assert_eq!(m.current().certificate.sha256, sha(&f.cert));
    }

    #[test]
    fn a_not_yet_valid_certificate_is_loaded_once_it_becomes_valid() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-not-yet", f.sources()).unwrap();
        let now = SystemTime::now();
        let hour = Duration::from_secs(3600);
        let (cert, key) = signed_by(&f.ca, Some((now + hour, now + 24 * hour)));
        f.write("tls.crt", &cert);
        f.write("tls.key", &key);
        let before = Fixture::failures("t-not-yet");
        assert!(matches!(
            m.reload_if_changed_at(now),
            ReloadOutcome::Rejected(TlsError::NotValidNow { .. })
        ));
        assert!(matches!(
            m.reload_if_changed_at(now),
            ReloadOutcome::Rejected(TlsError::NotValidNow { .. })
        ));
        assert_eq!(Fixture::failures("t-not-yet"), before + 1);
        assert!(matches!(
            m.reload_if_changed_at(now + 2 * hour),
            ReloadOutcome::Reloaded
        ));
        assert_eq!(m.current().certificate.sha256, sha(&cert));
        assert_eq!(Fixture::failures("t-not-yet"), before + 1);
    }

    #[test]
    fn concurrent_reloads_never_store_an_older_read_over_a_newer_one() {
        use std::sync::mpsc;
        let f = Fixture::new();
        let m = TlsMaterial::load("t-serial", f.sources()).unwrap();
        let (cert1, key1) = signed_by(&f.ca, None);
        let (cert2, key2) = signed_by(&f.ca, None);
        f.write("tls.crt", &cert1);
        f.write("tls.key", &key1);
        let (parked_tx, parked_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        *m.after_read.lock().unwrap() = Some(Arc::new(move || {
            parked_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        }));
        let first = {
            let m = m.clone();
            std::thread::spawn(move || m.reload_if_changed())
        };
        // The first reload has read cert1 and is parked.
        parked_rx.recv().unwrap();
        f.write("tls.crt", &cert2);
        f.write("tls.key", &key2);
        let second = {
            let m = m.clone();
            std::thread::spawn(move || m.reload_if_changed())
        };
        std::thread::sleep(Duration::from_millis(300));
        release_tx.send(()).unwrap();
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        assert!(matches!(first, ReloadOutcome::Reloaded), "{first:?}");
        assert!(matches!(second, ReloadOutcome::Reloaded), "{second:?}");
        assert_eq!(m.current().certificate.sha256, sha(&cert2));
    }

    #[test]
    fn a_held_material_stays_current_until_a_changed_set_is_swapped_in() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-is-current", f.sources()).unwrap();
        let held = m.current();
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Unchanged));
        assert!(m.is_current(&held));
        f.write("tls.key", "not a key");
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Rejected(_)));
        assert!(m.is_current(&held));
        let (cert, key) = signed_by(&f.ca, None);
        f.write("tls.crt", &cert);
        f.write("tls.key", &key);
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Reloaded));
        assert!(!m.is_current(&held));
        assert!(m.is_current(&m.current()));
    }

    #[test]
    async fn a_panic_in_one_reload_does_not_end_the_poller() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-panic", f.sources()).unwrap();
        m.start_reloader(Duration::from_millis(20));
        *m.after_read.lock().unwrap() = Some(Arc::new(|| panic!("a reload panics")));
        let (cert, key) = signed_by(&f.ca, None);
        f.write("tls.crt", &cert);
        f.write("tls.key", &key);
        let expected = sha(&cert);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while m.current().certificate.sha256 != expected {
            assert!(
                std::time::Instant::now() < deadline,
                "no reload within 2 s after a panicking one"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(m.after_read.lock().unwrap().is_none(), "the hook never ran");
    }

    #[cfg(unix)]
    #[test]
    fn a_kubernetes_style_symlink_swap_is_seen() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let ca = test_ca();
        let put = |version: &str| {
            let (cert, key) = signed_by(&ca, None);
            let sub = root.join(version);
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join("tls.crt"), &cert).unwrap();
            std::fs::write(sub.join("tls.key"), key).unwrap();
            std::fs::write(sub.join("ca.crt"), ca.pem()).unwrap();
            cert
        };
        let first = put("..2026_a");
        symlink("..2026_a", root.join("..data")).unwrap();
        for name in ["tls.crt", "tls.key", "ca.crt"] {
            symlink(format!("..data/{name}"), root.join(name)).unwrap();
        }
        let m = TlsMaterial::load("t-symlink", file_sources(root)).unwrap();
        assert_eq!(m.current().certificate.sha256, sha(&first));
        let second = put("..2026_b");
        symlink("..2026_b", root.join("..data_tmp")).unwrap();
        std::fs::rename(root.join("..data_tmp"), root.join("..data")).unwrap();
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Reloaded));
        assert_eq!(m.current().certificate.sha256, sha(&second));
    }

    #[test]
    fn inline_material_never_reloads() {
        let ca = test_ca();
        let (cert, key) = signed_by(&ca, None);
        let inline = |setting: &str, text: String| PemSource::Inline {
            setting: setting.to_string(),
            bytes: Base64(text.into_bytes()).0,
        };
        let m = TlsMaterial::load(
            "t-inline",
            TlsSources {
                cert: inline("server_cert", cert),
                key: inline("server_key", key),
                ca: inline("client_ca_cert", ca.pem()),
            },
        )
        .unwrap();
        assert!(!m.has_files());
        assert!(matches!(m.reload_if_changed(), ReloadOutcome::Unchanged));
        m.start_reloader(Duration::from_millis(10));
        assert!(!m.reloader_started());
    }

    #[test]
    async fn the_reloader_task_picks_up_a_change() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-task", f.sources()).unwrap();
        m.start_reloader(Duration::from_millis(50));
        assert!(m.reloader_started());
        let (cert, key) = signed_by(&f.ca, None);
        f.write("tls.crt", &cert);
        f.write("tls.key", &key);
        let expected = sha(&cert);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while m.current().certificate.sha256 != expected {
            assert!(std::time::Instant::now() < deadline, "no reload within 2 s");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // The task holds only a weak reference: once a reload in flight ends, no strong reference
        // outlives the last `Arc`.
        let weak = Arc::downgrade(&m);
        drop(m);
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while weak.strong_count() > 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "the material outlives its last Arc"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[test]
    async fn concurrent_first_connections_all_wait_for_the_one_first_look() {
        let f = Fixture::new();
        let m = TlsMaterial::load("t-first-look", f.sources()).unwrap();
        let (cert, key) = signed_by(&f.ca, None);
        f.write("tls.crt", &cert);
        f.write("tls.key", &key);
        let (parked_tx, parked_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        *m.after_read.lock().unwrap() = Some(Arc::new(move || {
            parked_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        }));
        let first_connection = |m: Arc<TlsMaterial>| {
            tokio::spawn(async move {
                m.start_reloader_checked(Duration::from_secs(3600)).await;
                m.current().certificate.sha256.clone()
            })
        };
        let first = first_connection(m.clone());
        // The first look has read the renewed files and is parked on its blocking thread.
        tokio::task::spawn_blocking(move || parked_rx.recv().unwrap())
            .await
            .unwrap();
        // Taken by any other look that reads the files.
        *m.after_read.lock().unwrap() = Some(Arc::new(|| {}));
        let second = first_connection(m.clone());
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(
            !second.is_finished(),
            "a concurrent first connection did not wait for the first look"
        );
        release_tx.send(()).unwrap();
        assert_eq!(first.await.unwrap(), sha(&cert));
        assert_eq!(second.await.unwrap(), sha(&cert));
        assert!(
            m.after_read.lock().unwrap().is_some(),
            "the second first connection made a look of its own"
        );
        assert!(m.reloader_started());
    }
}
