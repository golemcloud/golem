use crate::model::{EncodedOAuth2Session, OAuth2Session};
use golem_common::SafeDisplay;
use jsonwebtoken::{decode, encode, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use serde_with::serde_as;
use std::fmt::Debug;

pub trait OAuth2SessionService {
    fn encode_session(
        &self,
        session: &OAuth2Session,
    ) -> Result<EncodedOAuth2Session, OAuth2SessionError>;

    fn decode_session(
        &self,
        encoded_session: &EncodedOAuth2Session,
    ) -> Result<OAuth2Session, OAuth2SessionError>;
}

#[derive(Debug, thiserror::Error)]
pub enum OAuth2SessionError {
    #[error("Invalid Session: {0}")]
    InvalidSession(String),
    #[error("Failed to encode OAuth2 session: {0}")]
    EncodeError(#[from] jsonwebtoken::errors::Error),
}

impl SafeDisplay for OAuth2SessionError {
    fn to_safe_string(&self) -> String {
        match self {
            OAuth2SessionError::InvalidSession(_) => self.to_string(),
            OAuth2SessionError::EncodeError(_) => self.to_string(),
        }
    }
}

#[allow(dead_code)]
pub struct OAuth2SessionServiceDefault {
    header: Header,
    validation: Validation,
    private_key: EncodingKey,
    public_key: DecodingKey,
}

impl OAuth2SessionServiceDefault {
    pub fn new(private_key: EncodingKey, public_key: DecodingKey) -> Self {
        OAuth2SessionServiceDefault {
            header: Header::new(Algorithm::EdDSA),
            validation: Validation::new(Algorithm::EdDSA),
            private_key,
            public_key,
        }
    }

    pub fn from_config(config: &crate::config::EdDsaConfig) -> Result<Self, String> {
        let private_key = format_key(config.private_key.as_str(), "PRIVATE");
        let public_key = format_key(config.public_key.as_str(), "PUBLIC");

        let encoding_key = EncodingKey::from_ed_pem(private_key.as_bytes())
            .map_err(|err| format!("Failed to create encoding key {}", err))?;

        let decoding_key = DecodingKey::from_ed_pem(public_key.as_bytes())
            .map_err(|err| format!("Failed to create decoding key {}", err))?;

        Ok(Self::new(encoding_key, decoding_key))
    }
}

impl OAuth2SessionService for OAuth2SessionServiceDefault {
    fn encode_session(
        &self,
        session: &OAuth2Session,
    ) -> Result<EncodedOAuth2Session, OAuth2SessionError> {
        let header = Header::new(Algorithm::EdDSA);
        let claims = Claims::from(session.clone());
        let encoded = encode(&header, &claims, &self.private_key)?;
        Ok(EncodedOAuth2Session { value: encoded })
    }

    fn decode_session(
        &self,
        encoded_session: &EncodedOAuth2Session,
    ) -> Result<OAuth2Session, OAuth2SessionError> {
        let token_data =
            decode::<Claims>(&encoded_session.value, &self.public_key, &self.validation)
                .map_err(|e| OAuth2SessionError::InvalidSession(e.to_string()))?;

        let session = OAuth2Session::try_from(token_data.claims)
            .map_err(OAuth2SessionError::InvalidSession)?;

        Ok(session)
    }
}

/// Formats a cryptographic key with PEM (Privacy Enhanced Mail) encoding delimiters.
///
/// # Arguments
/// * `key: &str` - The raw key content to be formatted. This should not include any PEM encoding delimiters.
/// * `key_type: &str` - The type of the key. Acceptable values are "PUBLIC" or "PRIVATE", case-insensitive.
///
/// # Returns
/// A String containing the key formatted with PEM encoding delimiters.
/// If the key is already in the correct PEM format, it is returned unchanged.
/// Otherwise, it adds "-----BEGIN {} KEY-----" and "-----END {} KEY-----" around the key, with `{}` replaced by the specified key type.
fn format_key(key: &str, key_type: &str) -> String {
    let key_type = key_type.to_uppercase();
    let begin_marker = format!("-----BEGIN {key_type} KEY-----");
    let end_marker = format!("-----END {key_type} KEY-----");

    if key.trim_start().starts_with(&begin_marker) && key.trim_end().ends_with(&end_marker) {
        key.to_string()
    } else {
        format!("{begin_marker}\n{key}\n{end_marker}")
    }
}

#[serde_as]
#[derive(Debug, Serialize, Deserialize)]
struct Claims {
    exp: u64,
    device_code: String,
    #[serde_as(as = "serde_with::DurationSeconds<u64>")]
    interval: std::time::Duration,
}

impl From<OAuth2Session> for Claims {
    fn from(session: OAuth2Session) -> Self {
        Claims {
            exp: session.expires_at.timestamp() as u64,
            device_code: session.device_code,
            interval: session.interval,
        }
    }
}

impl TryFrom<Claims> for OAuth2Session {
    type Error = String;

    fn try_from(value: Claims) -> Result<Self, Self::Error> {
        let expires_at = chrono::DateTime::from_timestamp(value.exp as i64, 0)
            .ok_or_else(|| "Invalid Timestamp".to_string())?
            .naive_utc();
        let expires_at = chrono::DateTime::from_naive_utc_and_offset(expires_at, chrono::Utc);
        Ok(OAuth2Session {
            expires_at,
            device_code: value.device_code,
            interval: value.interval,
        })
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use super::*;
    use crate::model::{EncodedOAuth2Session, OAuth2Session};

    // Generate new keys with:
    // openssl genpkey -algorithm Ed25519 -out private_key.pem
    // openssl pkey -pubout -in private_key.pem -out public_key.pem

    const PUBLIC_KEY: &str = "MCowBQYDK2VwAyEAtKkMHoxrjJ52D/OEJ9Gww9hBl22m2YLU3qkWwTka02w=";
    const PRIVATE_KEY: &str = "MC4CAQAwBQYDK2VwBCIEIGCD+oyHo7U5CP/6n/hYqkT4OeccA+a+OVqr526PMNJY";

    // Mock data for tests
    fn setup_service() -> OAuth2SessionServiceDefault {
        OAuth2SessionServiceDefault::from_config(&crate::config::EdDsaConfig {
            private_key: PRIVATE_KEY.into(),
            public_key: PUBLIC_KEY.into(),
        })
        .expect("Create Service")
    }

    #[test]
    fn test_decode_session_success() -> Result<(), OAuth2SessionError> {
        let now = chrono::Utc::now();

        // Remove milliseconds from timestamp.
        let expires_at =
            chrono::DateTime::from_timestamp(now.timestamp(), 0).expect("Valid Timestamp");

        let session = OAuth2Session {
            device_code: "device".into(),
            expires_at,
            interval: std::time::Duration::from_secs(5),
        };
        let service = setup_service();
        let encoded_session = service.encode_session(&session).expect("Encode Session");
        let result = service.decode_session(&encoded_session)?;

        assert_eq!(result, session);

        Ok(())
    }

    #[test]
    fn test_decode_session_failure() {
        let service = setup_service();
        let invalid_encoded_session = EncodedOAuth2Session {
            value: "invalid_token".to_string(),
        };
        let result = service.decode_session(&invalid_encoded_session);
        assert!(result.is_err(), "Expected Error");
    }

    #[test]
    fn test_invalid_decode() {
        let service = setup_service();
        let invalid_encoded_session = EncodedOAuth2Session {
            value: "invalid_token".to_string(),
        };
        let result = service.decode_session(&invalid_encoded_session);

        assert!(result.is_err(), "Expected Error");
    }

    #[test]
    fn test_format_key_unformatted() {
        let key = "example key content";
        let expected = "-----BEGIN PRIVATE KEY-----
example key content
-----END PRIVATE KEY-----";
        assert_eq!(format_key(key, "PRIVATE"), expected);
    }

    #[test]
    fn test_format_key_already_formatted() {
        let key = "-----BEGIN PUBLIC KEY-----
        123456789 
        -----END PUBLIC KEY-----";
        assert_eq!(format_key(key, "PUBLIC"), key);
    }

    #[test]
    fn test_format_key_case_insensitivity() {
        let key = "example key content";
        let expected = "\
-----BEGIN PUBLIC KEY-----
example key content
-----END PUBLIC KEY-----";
        assert_eq!(format_key(key, "public"), expected);
    }
}
