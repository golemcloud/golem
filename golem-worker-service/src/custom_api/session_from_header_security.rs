// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use crate::custom_api::error::RequestHandlerError;
use crate::custom_api::model::OidcSession;
use crate::custom_api::route_resolver::ResolvedRouteEntry;
use crate::custom_api::{ResponseBody, RichRequest, RichRouteSecurity, RouteExecutionResult};
use chrono::{DateTime, TimeDelta, Utc};
use golem_service_base::custom_api::SessionFromHeaderRouteSecurity;
use http::StatusCode;
use openidconnect::{
    EmptyAdditionalClaims, IdTokenClaims, IssuerUrl, Scope, StandardClaims, SubjectIdentifier,
};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use tracing::debug;

pub fn apply_session_from_header_security_middleware(
    request: &mut RichRequest,
    resolved_route: &ResolvedRouteEntry,
) -> Result<Option<RouteExecutionResult>, RequestHandlerError> {
    debug!("Begin executing SessionFromHeaderSecurity middleware");

    let RichRouteSecurity::SessionFromHeader(SessionFromHeaderRouteSecurity { header_name }) =
        &resolved_route.route.security
    else {
        return Ok(None);
    };

    let Some(header_value) = request.header_string_value(header_name)? else {
        debug!("did not find oidc session header, rejecting request");
        return Ok(Some(RouteExecutionResult {
            status: StatusCode::UNAUTHORIZED,
            headers: HashMap::new(),
            body: ResponseBody::NoBody,
        }));
    };

    let session: EasyOidcSession = serde_json::from_str(header_value).map_err(|_| {
        RequestHandlerError::ValueParsingFailed {
            value: header_value.to_string(),
            expected: "EasyOidcSession",
        }
    })?;

    request.set_authenticated_session(session.into());

    Ok(None)
}

#[derive(Debug, Clone, Deserialize)]
struct EasyOidcSession {
    #[serde(default = "EasyOidcSession::default_subject")]
    pub subject: String,
    #[serde(default = "EasyOidcSession::default_issuer")]
    pub issuer: IssuerUrl,

    pub email: Option<String>,
    pub name: Option<String>,
    pub email_verified: Option<bool>,
    pub given_name: Option<String>,
    pub family_name: Option<String>,
    pub picture: Option<String>,
    pub preferred_username: Option<String>,

    #[serde(default = "EasyOidcSession::default_scopes")]
    pub scopes: HashSet<Scope>,

    #[serde(default = "EasyOidcSession::default_issued_at")]
    pub issued_at: DateTime<Utc>,
    #[serde(default = "EasyOidcSession::default_expires_at")]
    pub expires_at: DateTime<Utc>,
}

impl EasyOidcSession {
    fn default_subject() -> String {
        "test-user".to_string()
    }

    fn default_issuer() -> IssuerUrl {
        IssuerUrl::new("http://test-idp.com".to_string()).unwrap()
    }

    fn default_scopes() -> HashSet<Scope> {
        HashSet::from_iter(vec![Scope::new("openid".to_string())])
    }

    fn default_issued_at() -> DateTime<Utc> {
        Utc::now()
    }

    fn default_expires_at() -> DateTime<Utc> {
        Utc::now().checked_add_signed(TimeDelta::hours(8)).unwrap()
    }
}

impl From<EasyOidcSession> for OidcSession {
    fn from(value: EasyOidcSession) -> Self {
        Self {
            subject: value.subject.clone(),
            issuer: value.issuer.to_string(),
            email: value.email,
            name: value.name,
            email_verified: value.email_verified,
            given_name: value.given_name,
            family_name: value.family_name,
            picture: value.picture,
            preferred_username: value.preferred_username,
            claims: IdTokenClaims::new(
                value.issuer,
                Vec::new(),
                Utc::now(),
                value.issued_at,
                StandardClaims::new(SubjectIdentifier::new(value.subject)),
                EmptyAdditionalClaims {},
            ),
            scopes: value.scopes,
            expires_at: value.expires_at,
        }
    }
}
