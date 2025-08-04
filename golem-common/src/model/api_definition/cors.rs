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

use crate::declare_structs;
use bigdecimal::BigDecimal;
use http::header::*;
use rib::{Expr, GetLiteralValue, RibCompiler, RibInput, TypeName};

declare_structs! {
    // Make sure to store CORS headers as Vec<HeaderValue> and not as String
    // avoiding computation in the hot path
    // Other strings don't hurt as it is only done for pre-flight request, which gets cached in browsers,
    // however, good to change when we can break the backward
    // compatibility.
    // Tracked under: https://github.com/golemcloud/golem/issues/1512
    pub struct CorsConfiguration {
        pub allow_origin: String,
        pub allow_methods: String,
        pub allow_headers: String,
        pub expose_headers: Option<String>,
        pub allow_credentials: Option<bool>,
        pub max_age: Option<u64>,
    }

}

impl Default for CorsConfiguration {
    fn default() -> CorsConfiguration {
        CorsConfiguration {
            allow_origin: "*".to_string(),
            allow_methods: "GET, POST, PUT, DELETE, OPTIONS".to_string(),
            allow_headers: "Content-Type, Authorization".to_string(),
            expose_headers: None,
            max_age: None,
            allow_credentials: None,
        }
    }
}

impl CorsConfiguration {
    fn check_origin(&self, origin: &HeaderValue) -> OriginStatus {
        let origin_str = match origin.to_str() {
            Ok(s) => s,
            Err(_) => return OriginStatus::NotAllowed,
        };

        if Self::split_origin(&self.allow_origin).any(|o| o == origin_str) {
            return OriginStatus::AllowedExact;
        }

        if Self::split_origin(&self.allow_origin)
            .any(|pattern| pattern.contains('*') && Self::wildcard_match(pattern, origin_str))
        {
            return OriginStatus::AllowedWildcard;
        }

        OriginStatus::NotAllowed
    }

    fn wildcard_match(pattern: &str, text: &str) -> bool {
        if !pattern.contains('*') {
            return pattern == text;
        }

        let parts: Vec<&str> = pattern.split('*').collect();
        if parts.len() == 2 {
            text.starts_with(parts[0]) && text.ends_with(parts[1])
        } else {
            false
        }
    }

    fn split_origin(input: &str) -> impl Iterator<Item = &str> {
        input.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
    }

    pub fn add_header_in_response(&self, response: &mut poem::Response) {
        response.headers_mut().insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            self.get_allow_origin().clone().parse().unwrap(),
        );

        if let Some(allow_credentials) = &self.get_allow_credentials() {
            response.headers_mut().insert(
                ACCESS_CONTROL_ALLOW_CREDENTIALS,
                allow_credentials.to_string().clone().parse().unwrap(),
            );
        }

        if let Some(expose_headers) = &self.get_expose_headers() {
            response.headers_mut().insert(
                ACCESS_CONTROL_EXPOSE_HEADERS,
                expose_headers.clone().parse().unwrap(),
            );
        }
    }

    pub fn get_allow_origin(&self) -> String {
        self.allow_origin.clone()
    }

    pub fn get_allow_methods(&self) -> String {
        self.allow_methods.clone()
    }

    pub fn get_allow_headers(&self) -> String {
        self.allow_headers.clone()
    }

    pub fn get_expose_headers(&self) -> Option<String> {
        self.expose_headers.clone()
    }

    pub fn get_allow_credentials(&self) -> Option<bool> {
        self.allow_credentials
    }

    pub fn get_max_age(&self) -> Option<u64> {
        self.max_age
    }

    pub fn from_parameters(
        allow_origin: Option<String>,
        allow_methods: Option<String>,
        allow_headers: Option<String>,
        expose_headers: Option<String>,
        allow_credentials: Option<bool>,
        max_age: Option<u64>,
    ) -> Result<CorsConfiguration, String> {
        let mut cors_preflight = CorsConfiguration::default();

        if let Some(allow_origin) = allow_origin {
            cors_preflight.set_allow_origin(allow_origin.as_str())?;
        }

        if let Some(allow_methods) = allow_methods {
            cors_preflight.set_allow_methods(allow_methods.as_str())?;
        }

        if let Some(allow_headers) = allow_headers {
            cors_preflight.set_allow_headers(allow_headers.as_str())?;
        }
        if let Some(expose_headers) = expose_headers {
            cors_preflight.set_expose_headers(expose_headers.as_str())?;
        }

        if let Some(allow_credentials) = allow_credentials {
            cors_preflight.set_allow_credentials(allow_credentials);
        }

        if let Some(max_age) = max_age {
            cors_preflight.set_max_age(max_age);
        }

        Ok(cors_preflight)
    }

    pub async fn from_cors_preflight_expr(
        expr: &CorsPreflightExpr,
    ) -> Result<CorsConfiguration, String> {
        let rib_compiler = RibCompiler::default();
        let compiled_expr = rib_compiler
            .compile(expr.0.clone())
            .map_err(|err| format!("Rib compilation for cors-preflight response. {err}"))?;

        let rib_input = RibInput::default();
        let result = rib::interpret_pure(compiled_expr.byte_code, rib_input, None)
            .await
            .map_err(|err| {
                format!("Failed to evaluate Rib script to form pre-flight CORS {err}")
            })?;

        let record = result
            .get_record()
            .ok_or("Invalid pre-flight CORS response mapping")?;

        let mut cors = CorsConfiguration::default();

        for (key, value) in record {
            let value = value
                .get_literal()
                .ok_or(format!(
                    "Invalid value for key {key} in CORS preflight response"
                ))?
                .as_string();

            internal::set_cors_field(&mut cors, &key, &value)?;
        }

        Ok(cors)
    }

    pub fn set_allow_headers(&mut self, allow_headers: &str) -> Result<(), String> {
        if !allow_headers.is_empty() {
            self.allow_headers = allow_headers.to_string();
            Ok(())
        } else {
            Err("allow_headers cannot be empty.".to_string())
        }
    }

    pub fn set_allow_origin(&mut self, allow_origin: &str) -> Result<(), String> {
        if allow_origin == "*" || !allow_origin.is_empty() {
            self.allow_origin = allow_origin.to_string();
            Ok(())
        } else {
            Err("Invalid allow_origin value. It must be a valid URI or '*'.".to_string())
        }
    }

    pub fn set_allow_methods(&mut self, allow_methods: &str) -> Result<(), String> {
        let valid_methods = [
            "GET", "POST", "PUT", "DELETE", "OPTIONS", "PATCH", "HEAD", "TRACE", "CONNECT",
        ];
        let methods: Vec<&str> = allow_methods.split(',').collect();

        if methods
            .into_iter()
            .all(|m| valid_methods.contains(&m.trim().to_uppercase().as_str()))
        {
            self.allow_methods = allow_methods.to_string();
            Ok(())
        } else {
            Err("Invalid HTTP method in allow_methods.".to_string())
        }
    }

    pub fn set_expose_headers(&mut self, expose_headers: &str) -> Result<(), String> {
        if !expose_headers.is_empty() {
            self.expose_headers = Some(expose_headers.to_string());
            Ok(())
        } else {
            Err("expose_headers cannot be empty.".to_string())
        }
    }

    pub fn set_allow_credentials(&mut self, allow_credentials: bool) {
        self.allow_credentials = Some(allow_credentials);
    }

    pub fn set_max_age(&mut self, max_age: u64) {
        self.max_age = Some(max_age);
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::CorsPreflight> for CorsConfiguration {
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::CorsPreflight,
    ) -> Result<Self, Self::Error> {
        Ok(CorsConfiguration {
            allow_origin: value.allow_origin.ok_or("Missing allow origin")?,
            allow_methods: value.allow_methods.ok_or("Missing allow methods")?,
            allow_headers: value.allow_headers.ok_or("Missing allow headers")?,
            expose_headers: value.expose_headers,
            max_age: value.max_age,
            allow_credentials: value.allow_credentials,
        })
    }
}

impl From<CorsConfiguration> for golem_api_grpc::proto::golem::apidefinition::CorsPreflight {
    fn from(value: CorsConfiguration) -> Self {
        golem_api_grpc::proto::golem::apidefinition::CorsPreflight {
            allow_origin: Some(value.allow_origin),
            allow_methods: Some(value.allow_methods),
            allow_headers: Some(value.allow_headers),
            expose_headers: value.expose_headers,
            max_age: value.max_age,
            allow_credentials: value.allow_credentials,
        }
    }
}

enum OriginStatus {
    AllowedExact,
    AllowedWildcard,
    NotAllowed,
}

pub struct CorsPreflightExpr(pub Expr);

impl CorsPreflightExpr {
    pub fn from_cors(cors: &CorsConfiguration) -> CorsPreflightExpr {
        let mut cors_parameters = vec![
            (
                ACCESS_CONTROL_ALLOW_ORIGIN.to_string(),
                Expr::literal(cors.allow_origin.as_str()),
            ),
            (
                ACCESS_CONTROL_ALLOW_METHODS.to_string(),
                Expr::literal(cors.allow_methods.as_str()),
            ),
            (
                ACCESS_CONTROL_ALLOW_HEADERS.to_string(),
                Expr::literal(cors.allow_headers.as_str()),
            ),
        ];

        if let Some(allow_credentials) = &cors.allow_credentials {
            cors_parameters.push((
                ACCESS_CONTROL_ALLOW_CREDENTIALS.to_string(),
                Expr::literal(allow_credentials.to_string().as_str()),
            ));
        }

        if let Some(expose_headers) = &cors.expose_headers {
            cors_parameters.push((
                ACCESS_CONTROL_EXPOSE_HEADERS.to_string(),
                Expr::literal(expose_headers.as_str()),
            ));
        }

        if let Some(max_age) = &cors.max_age {
            cors_parameters.push((
                ACCESS_CONTROL_MAX_AGE.to_string(),
                Expr::number(BigDecimal::from(*max_age)).with_type_annotation(TypeName::U64),
            ));
        }

        let expr = Expr::record(cors_parameters);

        CorsPreflightExpr(expr)
    }
}

mod internal {
    use super::CorsConfiguration;

    pub(crate) fn set_cors_field(
        cors: &mut CorsConfiguration,
        key: &str,
        value: &str,
    ) -> Result<(), String> {
        match key.to_lowercase().as_str() {
            "access-control-allow-origin" => {
                cors.set_allow_origin(value)
            },
            "access-control-allow-methods" => {
                cors.set_allow_methods(value)
            },
            "access-control-allow-headers" => {
                cors.set_allow_headers(value)
            },
            "access-control-expose-headers" => {
                cors.set_expose_headers(value)
            },
            "access-control-allow-credentials" => {
                let allow = value
                    .parse::<bool>()
                    .map_err(|_| "Invalid value for max age".to_string())?;

                cors.set_allow_credentials(allow);

                Ok(())

            },
            "access-control-max-age" => {
                let max_age = value
                    .parse::<u64>()
                    .map_err(|_| "Invalid value for max age".to_string())?;

                cors.set_max_age(max_age);
                Ok(())
            },
            _ => Err("Invalid cors header in the rib for pre-flight. Allowed keys: access-control-allow-origin, access-control-allow-methods, access-control-allow-headers, access-control-expose-headers, and access-control-max-age".to_string()),
        }
    }
}
