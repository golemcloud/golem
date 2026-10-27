//! Reading one PEM item from a file or an inline value, describing certificates, and checking
//! that a key belongs to a certificate.

use golem_common::model::base64::Base64;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::sign::CertifiedKey;
use sha2::{Digest, Sha256};
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const ENCRYPTED_KEY_LABEL: &[u8] = b"-----BEGIN ENCRYPTED PRIVATE KEY-----";

/// Where one PEM item comes from. `setting` is the name of the value setting, for example
/// `server_cert`; the file setting is `<setting>_file`.
#[derive(Clone, PartialEq, Eq)]
pub enum PemSource {
    File { setting: String, path: PathBuf },
    Inline { setting: String, bytes: Vec<u8> },
}

/// Prints no inline bytes: an inline item may be a private key.
impl fmt::Debug for PemSource {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            PemSource::File { setting, path } => f
                .debug_struct("File")
                .field("setting", setting)
                .field("path", path)
                .finish(),
            PemSource::Inline { setting, bytes } => f
                .debug_struct("Inline")
                .field("setting", setting)
                .field("len", &bytes.len())
                .finish(),
        }
    }
}

impl PemSource {
    /// Exactly one of `value` and `file` must be set.
    pub fn from_config(
        name: &str,
        value: Option<&Base64>,
        file: Option<&Path>,
    ) -> Result<Self, TlsError> {
        match (value, file) {
            (Some(_), Some(_)) => Err(TlsError::BothSet {
                name: name.to_string(),
            }),
            (None, None) => Err(TlsError::NeitherSet {
                name: name.to_string(),
            }),
            (None, Some(path)) => Ok(PemSource::File {
                setting: name.to_string(),
                path: path.to_path_buf(),
            }),
            (Some(value), None) => Ok(PemSource::Inline {
                setting: name.to_string(),
                bytes: value.0.clone(),
            }),
        }
    }

    /// An inline value is returned as it is; a file is read from disk.
    pub fn read(&self) -> Result<Vec<u8>, TlsError> {
        match self {
            PemSource::Inline { bytes, .. } => Ok(bytes.clone()),
            PemSource::File { path, .. } => std::fs::read(path).map_err(|error| TlsError::Read {
                source: self.describe(),
                error,
            }),
        }
    }

    pub fn is_file(&self) -> bool {
        matches!(self, PemSource::File { .. })
    }

    /// `<name>_file <path>` or `<name> (inline)`: what a log line or a refusal names.
    pub fn describe(&self) -> String {
        match self {
            PemSource::File { setting, path } => format!("{setting}_file {}", path.display()),
            PemSource::Inline { setting, .. } => format!("{setting} (inline)"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertificateInfo {
    pub subject: String,
    pub not_before: SystemTime,
    pub not_after: SystemTime,
    /// Hex of the SHA-256 of the DER encoding.
    pub sha256: String,
}

#[derive(Debug)]
pub enum TlsError {
    BothSet {
        name: String,
    },
    NeitherSet {
        name: String,
    },
    Read {
        source: String,
        error: std::io::Error,
    },
    NoCertificate {
        source: String,
    },
    BadCertificate {
        source: String,
        reason: String,
    },
    BadKey {
        source: String,
        reason: String,
    },
    EncryptedKey {
        source: String,
    },
    KeyMismatch {
        cert: String,
        key: String,
    },
    NotValidNow {
        source: String,
        subject: String,
        not_before: SystemTime,
        not_after: SystemTime,
    },
    AllCaExpired {
        source: String,
    },
}

impl Display for TlsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            TlsError::BothSet { name } => {
                write!(f, "{name} and {name}_file must not both be set")
            }
            TlsError::NeitherSet { name } => {
                write!(f, "one of {name} and {name}_file must be set")
            }
            TlsError::Read { source, error } => write!(f, "cannot read {source}: {error}"),
            TlsError::NoCertificate { source } => {
                write!(f, "{source} holds no PEM certificate")
            }
            TlsError::BadCertificate { source, reason } => {
                write!(
                    f,
                    "{source} holds a certificate that cannot be parsed: {reason}"
                )
            }
            TlsError::BadKey { source, reason } => {
                write!(f, "{source} holds no usable PEM private key: {reason}")
            }
            TlsError::EncryptedKey { source } => write!(
                f,
                "{source} holds an encrypted private key, which is not supported: give the key \
                 unencrypted, as a Kubernetes TLS Secret holds it"
            ),
            TlsError::KeyMismatch { cert, key } => {
                write!(f, "{key} does not belong to the certificate in {cert}")
            }
            TlsError::NotValidNow {
                source,
                subject,
                not_before,
                not_after,
            } => write!(
                f,
                "{source}: the certificate of {subject} is valid only from {} to {}, which does \
                 not include now; check this node's clock",
                unix_seconds(*not_before),
                unix_seconds(*not_after)
            ),
            TlsError::AllCaExpired { source } => {
                write!(f, "{source}: every certificate in it has expired")
            }
        }
    }
}

impl std::error::Error for TlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            TlsError::Read { error, .. } => Some(error),
            _ => None,
        }
    }
}

fn unix_seconds(time: SystemTime) -> String {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => format!("unix {}", d.as_secs()),
        Err(_) => "before the unix epoch".to_string(),
    }
}

fn from_unix(seconds: i64) -> SystemTime {
    if seconds >= 0 {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds as u64)
    } else {
        SystemTime::UNIX_EPOCH - Duration::from_secs(seconds.unsigned_abs())
    }
}

/// Describes every certificate in `pem`, in order. Fails when there is none, or when one of
/// them cannot be parsed.
pub fn describe_certificates(source: &str, pem: &[u8]) -> Result<Vec<CertificateInfo>, TlsError> {
    let source = source.to_string();
    let mut infos = Vec::new();
    for der in CertificateDer::pem_slice_iter(pem) {
        let der = der.map_err(|err| TlsError::BadCertificate {
            source: source.clone(),
            reason: err.to_string(),
        })?;
        let (_, certificate) =
            x509_parser::parse_x509_certificate(&der).map_err(|err| TlsError::BadCertificate {
                source: source.clone(),
                reason: err.to_string(),
            })?;
        let validity = certificate.validity();
        infos.push(CertificateInfo {
            subject: certificate.subject().to_string(),
            not_before: from_unix(validity.not_before.timestamp()),
            not_after: from_unix(validity.not_after.timestamp()),
            sha256: hex::encode(Sha256::digest(&der)),
        });
    }
    if infos.is_empty() {
        return Err(TlsError::NoCertificate { source });
    }
    Ok(infos)
}

/// Refuses a key that does not belong to the certificate (chain) - or that is encrypted, or not
/// a key at all - instead of letting every TLS handshake fail later.
pub fn check_key_matches(
    cert_source: &str,
    cert_pem: &[u8],
    key_source: &str,
    key_pem: &[u8],
) -> Result<(), TlsError> {
    let cert = cert_source.to_string();
    let key = key_source.to_string();
    if key_pem
        .windows(ENCRYPTED_KEY_LABEL.len())
        .any(|window| window == ENCRYPTED_KEY_LABEL)
    {
        return Err(TlsError::EncryptedKey { source: key });
    }
    let certificates: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|err| TlsError::BadCertificate {
            source: cert.clone(),
            reason: err.to_string(),
        })?;
    if certificates.is_empty() {
        return Err(TlsError::NoCertificate { source: cert });
    }
    let key_der = PrivateKeyDer::from_pem_slice(key_pem).map_err(|err| TlsError::BadKey {
        source: key.clone(),
        reason: err.to_string(),
    })?;
    match CertifiedKey::from_der(
        certificates,
        key_der,
        &rustls::crypto::ring::default_provider(),
    ) {
        Ok(_) => Ok(()),
        Err(rustls::Error::InconsistentKeys(rustls::InconsistentKeys::KeyMismatch)) => {
            Err(TlsError::KeyMismatch { cert, key })
        }
        Err(err) => Err(TlsError::BadKey {
            source: key,
            reason: err.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use golem_common::model::base64::Base64;
    use sha2::{Digest, Sha256};
    use std::path::Path;
    use std::time::SystemTime;
    use test_r::test;

    fn self_signed() -> rcgen::CertifiedKey<rcgen::KeyPair> {
        rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap()
    }

    #[test]
    fn a_source_needs_exactly_one_of_value_and_file() {
        let value = Base64(b"x".to_vec());
        let file = Path::new("/some/file.pem");
        assert!(matches!(
            PemSource::from_config("server_cert", Some(&value), Some(file)),
            Err(TlsError::BothSet { name }) if name == "server_cert"
        ));
        assert!(matches!(
            PemSource::from_config("server_cert", None, None),
            Err(TlsError::NeitherSet { name }) if name == "server_cert"
        ));
        let from_file = PemSource::from_config("server_cert", None, Some(file)).unwrap();
        assert!(from_file.is_file());
        assert_eq!(from_file.describe(), "server_cert_file /some/file.pem");
        let inline = PemSource::from_config("server_cert", Some(&value), None).unwrap();
        assert!(!inline.is_file());
        assert_eq!(inline.describe(), "server_cert (inline)");
        assert_eq!(inline.read().unwrap(), b"x".to_vec());
    }

    #[test]
    fn a_file_source_reads_the_file_and_names_a_missing_one() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pem");
        std::fs::write(&path, b"abc").unwrap();
        let source = PemSource::from_config("server_cert", None, Some(&path)).unwrap();
        assert_eq!(source.read().unwrap(), b"abc".to_vec());
        let missing =
            PemSource::from_config("server_cert", None, Some(&dir.path().join("none"))).unwrap();
        assert!(matches!(missing.read(), Err(TlsError::Read { .. })));
    }

    #[test]
    fn describe_certificates_gives_subject_validity_and_fingerprint() {
        let generated = self_signed();
        let pem = generated.cert.pem();
        let infos = describe_certificates("server_cert (inline)", pem.as_bytes()).unwrap();
        assert_eq!(infos.len(), 1);
        let info = &infos[0];
        assert!(!info.subject.is_empty());
        let now = SystemTime::now();
        assert!(info.not_before <= now && now <= info.not_after);
        assert_eq!(info.sha256.len(), 64);
        assert_eq!(
            info.sha256,
            hex::encode(Sha256::digest(generated.cert.der()))
        );
    }

    #[test]
    fn describe_certificates_rejects_text_without_a_certificate() {
        assert!(matches!(
            describe_certificates("server_cert (inline)", b"hello"),
            Err(TlsError::NoCertificate { .. })
        ));
    }

    #[test]
    fn a_key_that_does_not_belong_to_the_certificate_is_refused() {
        let a = self_signed();
        let b = self_signed();
        let cert_a = a.cert.pem();
        assert!(matches!(
            check_key_matches(
                "server_cert_file /c.pem",
                cert_a.as_bytes(),
                "server_key_file /k.pem",
                b.signing_key.serialize_pem().as_bytes()
            ),
            Err(TlsError::KeyMismatch { .. })
        ));
        check_key_matches(
            "server_cert_file /c.pem",
            cert_a.as_bytes(),
            "server_key_file /k.pem",
            a.signing_key.serialize_pem().as_bytes(),
        )
        .unwrap();
    }

    #[test]
    fn an_encrypted_key_is_refused_with_its_own_message() {
        let a = self_signed();
        let key =
            b"-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----\n";
        assert!(matches!(
            check_key_matches("c", a.cert.pem().as_bytes(), "server_key (inline)", key),
            Err(TlsError::EncryptedKey { .. })
        ));
    }

    #[test]
    fn debug_of_an_inline_source_shows_no_bytes() {
        let source = PemSource::Inline {
            setting: "server_key".to_string(),
            bytes: b"-----BEGIN PRIVATE KEY-----SECRET".to_vec(),
        };
        let text = format!("{:?}", source);
        assert!(!text.contains("SECRET"), "{text}");
        assert!(text.contains("server_key"), "{text}");
    }

    #[test]
    fn errors_name_the_settings() {
        let a = self_signed();
        let b = self_signed();
        let err = check_key_matches(
            "server_cert_file /c.pem",
            a.cert.pem().as_bytes(),
            "server_key_file /k.pem",
            b.signing_key.serialize_pem().as_bytes(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("server_cert_file /c.pem"), "{err}");
        assert!(err.contains("server_key_file /k.pem"), "{err}");
        let err = describe_certificates("client_ca_cert (inline)", b"hello")
            .unwrap_err()
            .to_string();
        assert!(err.contains("client_ca_cert (inline)"), "{err}");
    }
}
