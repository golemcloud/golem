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

use crate::custom_api::{ResponseBody, RouteExecutionResult};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::{HeaderMap, HeaderValue, StatusCode};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use url::Url;

pub const MAX_ENCODED_REQUEST_LENGTH: usize = 8 * 1024;
const MAX_PARAMETER_NAME_LENGTH: usize = 64;
const MAX_PARAMETER_VALUE_LENGTH: usize = 4 * 1024;
const MAX_REDIRECT_URI_LENGTH: usize = 2 * 1024;
const MAX_STATE_LENGTH: usize = 1024;
const MAX_CREDENTIAL_LENGTH: usize = 512;
const MIN_PKCE_LENGTH: usize = 43;
const MAX_PKCE_LENGTH: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorizationRequest {
    pub redirect_uri: String,
    pub state: String,
    pub code_challenge: String,
}

impl AuthorizationRequest {
    pub fn parse(query: &str, registered_redirect_uris: &[String]) -> Result<Self, ProtocolError> {
        let parameters = Parameters::parse(query.as_bytes())?;
        let response_type = parameters.required("response_type")?;
        let redirect_uri = parameters.required("redirect_uri")?;
        let state = parameters.required("state")?;
        let code_challenge = parameters.required("code_challenge")?;
        let code_challenge_method = parameters.required("code_challenge_method")?;

        if response_type != "code" {
            return Err(ProtocolError::UnsupportedResponseType);
        }
        if redirect_uri.len() > MAX_REDIRECT_URI_LENGTH
            || !registered_redirect_uris
                .iter()
                .any(|registered| registered == redirect_uri)
            || redirect_has_response_parameter_collision(redirect_uri)
        {
            return Err(ProtocolError::InvalidRedirectUri);
        }
        if state.is_empty() || state.len() > MAX_STATE_LENGTH {
            return Err(ProtocolError::InvalidParameter("state"));
        }
        if code_challenge_method != "S256" {
            return Err(ProtocolError::UnsupportedCodeChallengeMethod);
        }
        validate_s256_challenge(code_challenge)?;

        Ok(Self {
            redirect_uri: redirect_uri.to_string(),
            state: state.to_string(),
            code_challenge: code_challenge.to_string(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRequest {
    pub code: String,
    pub code_verifier: String,
    pub redirect_uri: String,
}

impl TokenRequest {
    pub fn parse(body: &[u8]) -> Result<Self, ProtocolError> {
        let parameters = Parameters::parse(body)?;
        let grant_type = parameters.required("grant_type")?;
        let code = parameters.required("code")?;
        let code_verifier = parameters.required("code_verifier")?;
        let redirect_uri = parameters.required("redirect_uri")?;

        if grant_type != "authorization_code" {
            return Err(ProtocolError::UnsupportedGrantType);
        }
        if code.is_empty() || code.len() > MAX_CREDENTIAL_LENGTH {
            return Err(ProtocolError::InvalidParameter("code"));
        }
        validate_pkce_value(code_verifier, "code_verifier")?;
        if redirect_uri.is_empty() || redirect_uri.len() > MAX_REDIRECT_URI_LENGTH {
            return Err(ProtocolError::InvalidParameter("redirect_uri"));
        }

        Ok(Self {
            code: code.to_string(),
            code_verifier: code_verifier.to_string(),
            redirect_uri: redirect_uri.to_string(),
        })
    }

    pub fn verifies_challenge(&self, expected_challenge: &str) -> bool {
        let digest = Sha256::digest(self.code_verifier.as_bytes());
        URL_SAFE_NO_PAD.encode(digest) == expected_challenge
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    pub token_type: String,
    pub expires_in: u64,
}

impl TokenResponse {
    pub fn bearer(access_token: String, expires_in: u64) -> Self {
        Self {
            access_token,
            token_type: "Bearer".to_string(),
            expires_in,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolErrorCode {
    InvalidRequest,
    InvalidGrant,
    UnsupportedGrantType,
    UnsupportedResponseType,
    AccessDenied,
    ServerError,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolErrorResponse {
    pub error: ProtocolErrorCode,
    pub error_description: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtocolError {
    #[error("request is too large")]
    RequestTooLarge,
    #[error("request encoding is malformed")]
    MalformedEncoding,
    #[error("parameter {0} is missing")]
    MissingParameter(&'static str),
    #[error("parameter {0} must occur exactly once")]
    DuplicateParameter(&'static str),
    #[error("parameter {0} is invalid")]
    InvalidParameter(&'static str),
    #[error("redirect_uri is not registered")]
    InvalidRedirectUri,
    #[error("response_type is unsupported")]
    UnsupportedResponseType,
    #[error("grant_type is unsupported")]
    UnsupportedGrantType,
    #[error("code_challenge_method is unsupported")]
    UnsupportedCodeChallengeMethod,
}

impl ProtocolError {
    pub fn response(&self) -> ProtocolErrorResponse {
        let (error, error_description) = match self {
            Self::UnsupportedResponseType => (
                ProtocolErrorCode::UnsupportedResponseType,
                "response_type must be code",
            ),
            Self::UnsupportedGrantType => (
                ProtocolErrorCode::UnsupportedGrantType,
                "grant_type must be authorization_code",
            ),
            Self::UnsupportedCodeChallengeMethod => (
                ProtocolErrorCode::InvalidRequest,
                "code_challenge_method must be S256",
            ),
            Self::InvalidRedirectUri => (
                ProtocolErrorCode::InvalidRequest,
                "redirect_uri is not registered",
            ),
            _ => (ProtocolErrorCode::InvalidRequest, "request is invalid"),
        };

        ProtocolErrorResponse {
            error,
            error_description,
        }
    }
}

pub fn upstream_denial_redirect(request: &AuthorizationRequest) -> Url {
    let mut redirect = Url::parse(&request.redirect_uri)
        .expect("registered frontend redirect URI must have been validated");
    redirect
        .query_pairs_mut()
        .append_pair("error", "access_denied")
        .append_pair("state", &request.state);
    redirect
}

pub fn bearer_authentication_failure() -> RouteExecutionResult {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer"),
    );

    RouteExecutionResult {
        status: StatusCode::UNAUTHORIZED,
        headers,
        body: ResponseBody::NoBody,
    }
}

fn validate_pkce_value(value: &str, parameter: &'static str) -> Result<(), ProtocolError> {
    if !(MIN_PKCE_LENGTH..=MAX_PKCE_LENGTH).contains(&value.len())
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~'))
    {
        return Err(ProtocolError::InvalidParameter(parameter));
    }
    Ok(())
}

fn validate_s256_challenge(value: &str) -> Result<(), ProtocolError> {
    let digest = URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ProtocolError::InvalidParameter("code_challenge"))?;
    if digest.len() != 32 || URL_SAFE_NO_PAD.encode(digest) != value {
        return Err(ProtocolError::InvalidParameter("code_challenge"));
    }
    Ok(())
}

fn redirect_has_response_parameter_collision(redirect_uri: &str) -> bool {
    match Url::parse(redirect_uri) {
        Ok(redirect) => redirect.query_pairs().any(|(name, _)| {
            matches!(
                name.as_ref(),
                "code" | "error" | "error_description" | "state"
            )
        }),
        Err(_) => true,
    }
}

struct Parameters(HashMap<String, Vec<String>>);

impl Parameters {
    fn parse(encoded: &[u8]) -> Result<Self, ProtocolError> {
        if encoded.len() > MAX_ENCODED_REQUEST_LENGTH {
            return Err(ProtocolError::RequestTooLarge);
        }
        validate_percent_encoding(encoded)?;

        let mut result: HashMap<String, Vec<String>> = HashMap::new();
        for (name, value) in url::form_urlencoded::parse(encoded) {
            if name.is_empty()
                || name.len() > MAX_PARAMETER_NAME_LENGTH
                || value.len() > MAX_PARAMETER_VALUE_LENGTH
                || name.contains('\u{fffd}')
                || value.contains('\u{fffd}')
            {
                return Err(ProtocolError::MalformedEncoding);
            }
            result
                .entry(name.into_owned())
                .or_default()
                .push(value.into_owned());
        }
        Ok(Self(result))
    }

    fn required(&self, name: &'static str) -> Result<&str, ProtocolError> {
        match self.0.get(name).map(Vec::as_slice) {
            None | Some([]) => Err(ProtocolError::MissingParameter(name)),
            Some([value]) => Ok(value),
            Some(_) => Err(ProtocolError::DuplicateParameter(name)),
        }
    }
}

fn validate_percent_encoding(encoded: &[u8]) -> Result<(), ProtocolError> {
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'%' {
            if index + 2 >= encoded.len()
                || !encoded[index + 1].is_ascii_hexdigit()
                || !encoded[index + 2].is_ascii_hexdigit()
            {
                return Err(ProtocolError::MalformedEncoding);
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    const REDIRECT: &str = "https://frontend.example/callback";
    const VERIFIER: &str = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const CHALLENGE: &str = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

    fn authorization_query(overrides: &[(&str, &str)]) -> String {
        let mut values = vec![
            ("response_type", "code"),
            ("redirect_uri", REDIRECT),
            ("state", "frontend state"),
            ("code_challenge", CHALLENGE),
            ("code_challenge_method", "S256"),
        ];
        for (name, value) in overrides {
            values.retain(|(existing, _)| existing != name);
            values.push((name, value));
        }
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(values)
            .finish()
    }

    fn token_body(verifier: &str) -> Vec<u8> {
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("grant_type", "authorization_code")
            .append_pair("code", "opaque-code")
            .append_pair("code_verifier", verifier)
            .append_pair("redirect_uri", REDIRECT)
            .finish()
            .into_bytes()
    }

    #[test]
    fn parses_authorization_request_and_preserves_state() {
        let request =
            AuthorizationRequest::parse(&authorization_query(&[]), &[REDIRECT.to_string()])
                .unwrap();

        assert_eq!(request.redirect_uri, REDIRECT);
        assert_eq!(request.state, "frontend state");
        assert_eq!(request.code_challenge, CHALLENGE);
    }

    #[test]
    fn rejects_duplicate_security_sensitive_authorization_parameters() {
        for parameter in [
            "response_type",
            "redirect_uri",
            "state",
            "code_challenge",
            "code_challenge_method",
        ] {
            let query = format!("{}&{parameter}=duplicate", authorization_query(&[]));
            assert_eq!(
                AuthorizationRequest::parse(&query, &[REDIRECT.to_string()]),
                Err(ProtocolError::DuplicateParameter(parameter))
            );
        }
    }

    #[test]
    fn rejects_unregistered_redirect_without_returning_it() {
        let error = AuthorizationRequest::parse(
            &authorization_query(&[("redirect_uri", "https://attacker.example/callback")]),
            &[REDIRECT.to_string()],
        )
        .unwrap_err();

        assert_eq!(error, ProtocolError::InvalidRedirectUri);
        assert_eq!(
            error.response().error_description,
            "redirect_uri is not registered"
        );
        assert!(!format!("{error:?}").contains("attacker.example"));
    }

    #[test]
    fn rejects_unsupported_authorization_values() {
        let cases = [
            (
                authorization_query(&[("response_type", "token")]),
                ProtocolError::UnsupportedResponseType,
            ),
            (
                authorization_query(&[("code_challenge_method", "plain")]),
                ProtocolError::UnsupportedCodeChallengeMethod,
            ),
            (
                authorization_query(&[("state", "")]),
                ProtocolError::InvalidParameter("state"),
            ),
        ];

        for (query, expected) in cases {
            assert_eq!(
                AuthorizationRequest::parse(&query, &[REDIRECT.to_string()]),
                Err(expected)
            );
        }
    }

    #[test]
    fn rejects_noncanonical_s256_challenges() {
        for challenge in [
            "a".repeat(42),
            "a".repeat(44),
            ".".repeat(43),
            "~".repeat(43),
            format!("{}=", "a".repeat(42)),
            format!("{}+", "a".repeat(42)),
            format!("{}B", "a".repeat(42)),
        ] {
            assert_eq!(
                AuthorizationRequest::parse(
                    &authorization_query(&[("code_challenge", &challenge)]),
                    &[REDIRECT.to_string()],
                ),
                Err(ProtocolError::InvalidParameter("code_challenge"))
            );
        }
    }

    #[test]
    fn rejects_redirects_with_response_parameter_collisions() {
        for parameter in ["code", "error", "error_description", "state", "%73tate"] {
            let redirect = format!("{REDIRECT}?tenant=one&{parameter}=preseeded");
            assert_eq!(
                AuthorizationRequest::parse(
                    &authorization_query(&[("redirect_uri", &redirect)]),
                    &[redirect],
                ),
                Err(ProtocolError::InvalidRedirectUri)
            );
        }
    }

    #[test]
    fn validates_pkce_verifier_boundaries_and_characters() {
        for valid in ["a".repeat(43), "Z".repeat(128), "a-._~".repeat(9)] {
            TokenRequest::parse(&token_body(&valid)).unwrap();
        }

        for invalid in [
            "a".repeat(42),
            "a".repeat(129),
            format!("{}+", "a".repeat(42)),
            format!("{}=", "a".repeat(42)),
        ] {
            assert_eq!(
                TokenRequest::parse(&token_body(&invalid)),
                Err(ProtocolError::InvalidParameter("code_verifier"))
            );
        }
    }

    #[test]
    fn verifies_independent_rfc7636_s256_vector() {
        let request = TokenRequest::parse(&token_body(VERIFIER)).unwrap();
        assert!(request.verifies_challenge(CHALLENGE));
        assert!(!request.verifies_challenge(&"a".repeat(43)));
    }

    #[test]
    fn rejects_duplicate_token_parameters_and_unsupported_grant() {
        for parameter in ["grant_type", "code", "code_verifier", "redirect_uri"] {
            let mut body = token_body(VERIFIER);
            body.extend_from_slice(format!("&{parameter}=duplicate").as_bytes());
            assert_eq!(
                TokenRequest::parse(&body),
                Err(ProtocolError::DuplicateParameter(parameter))
            );
        }

        let body = String::from_utf8(token_body(VERIFIER))
            .unwrap()
            .replace("authorization_code", "client_credentials");
        assert_eq!(
            TokenRequest::parse(body.as_bytes()),
            Err(ProtocolError::UnsupportedGrantType)
        );
    }

    #[test]
    fn rejects_oversized_and_malformed_input() {
        assert_eq!(
            TokenRequest::parse(&vec![b'a'; MAX_ENCODED_REQUEST_LENGTH + 1]),
            Err(ProtocolError::RequestTooLarge)
        );
        assert_eq!(
            TokenRequest::parse(b"grant_type=authorization_code&code=%GG"),
            Err(ProtocolError::MalformedEncoding)
        );
    }

    #[test]
    fn denial_redirect_preserves_existing_query_and_frontend_state() {
        let request = AuthorizationRequest {
            redirect_uri: "https://frontend.example/callback?tenant=one".to_string(),
            state: "opaque / frontend state".to_string(),
            code_challenge: CHALLENGE.to_string(),
        };

        let redirect = upstream_denial_redirect(&request);
        let pairs: Vec<_> = redirect.query_pairs().collect();
        assert!(pairs.contains(&("tenant".into(), "one".into())));
        assert!(pairs.contains(&("error".into(), "access_denied".into())));
        assert!(pairs.contains(&("state".into(), request.state.as_str().into())));
        assert_eq!(pairs.iter().filter(|(name, _)| name == "error").count(), 1);
        assert_eq!(pairs.iter().filter(|(name, _)| name == "state").count(), 1);
    }

    #[test]
    fn token_response_and_errors_do_not_expose_credentials() {
        let response = TokenResponse::bearer("opaque-token".to_string(), 3600);
        assert_eq!(
            serde_json::to_value(response).unwrap(),
            serde_json::json!({
                "access_token": "opaque-token",
                "token_type": "Bearer",
                "expires_in": 3600
            })
        );

        let body = ProtocolError::InvalidParameter("code_verifier").response();
        let json = serde_json::to_string(&body).unwrap();
        assert!(!json.contains(VERIFIER));
        assert!(!json.contains("opaque-token"));
    }

    #[test]
    fn bearer_failure_is_a_challenge_not_a_login_redirect() {
        let result = bearer_authentication_failure();
        assert_eq!(result.status, StatusCode::UNAUTHORIZED);
        assert_eq!(result.headers[http::header::WWW_AUTHENTICATE], "Bearer");
        assert!(!result.headers.contains_key(http::header::LOCATION));
        assert!(matches!(result.body, ResponseBody::NoBody));
    }
}
