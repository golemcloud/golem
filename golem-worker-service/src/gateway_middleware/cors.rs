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

use crate::gateway_execution::request::RichRequest;
use golem_service_base::custom_api::HttpCors;
use http::{HeaderValue, Method};

#[derive(Debug)]
#[allow(clippy::enum_variant_names)]
pub enum CorsError {
    OriginNotAllowed,
    MethodNotAllowed,
    HeadersNotAllowed,
}

pub fn apply_cors(cors: &HttpCors, request: &RichRequest) -> Result<(), CorsError> {
    let origin = match request.headers().get(http::header::ORIGIN) {
        Some(origin) => origin.clone(),
        None => return Ok(()),
    };

    if let OriginStatus::NotAllowed = check_origin(cors, &origin) {
        return Err(CorsError::OriginNotAllowed);
    }

    if request.underlying.method() == Method::OPTIONS {
        let allow_method = request
            .headers()
            .get(http::header::ACCESS_CONTROL_REQUEST_METHOD)
            .and_then(|val| val.to_str().ok())
            .and_then(|m| m.parse::<Method>().ok());

        if let Some(method) = allow_method {
            if !cors.allow_methods.trim().is_empty()
                && !split_origin(&cors.allow_methods)
                    .any(|m| m.eq_ignore_ascii_case(method.as_str()))
            {
                return Err(CorsError::MethodNotAllowed);
            }
        } else {
            return Err(CorsError::MethodNotAllowed);
        }

        check_headers_allowed(cors, request)?;
    }

    Ok(())
}

pub fn add_cors_headers_to_response(cors: &HttpCors, response: &mut poem::Response) {
    response.headers_mut().insert(
        http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
        cors.allow_origin.clone().parse().unwrap(),
    );

    if let Some(allow_credentials) = &cors.allow_credentials {
        response.headers_mut().insert(
            http::header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
            allow_credentials.to_string().clone().parse().unwrap(),
        );
    }

    if let Some(expose_headers) = &cors.expose_headers {
        response.headers_mut().insert(
            http::header::ACCESS_CONTROL_EXPOSE_HEADERS,
            expose_headers.clone().parse().unwrap(),
        );
    }
}

fn split_origin(input: &str) -> impl Iterator<Item = &str> {
    input.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
}

enum OriginStatus {
    AllowedExact,
    AllowedWildcard,
    NotAllowed,
}

fn check_origin(cors: &HttpCors, origin: &HeaderValue) -> OriginStatus {
    let origin_str = match origin.to_str() {
        Ok(s) => s,
        Err(_) => return OriginStatus::NotAllowed,
    };

    if split_origin(&cors.allow_origin).any(|o| o == origin_str) {
        return OriginStatus::AllowedExact;
    }

    if split_origin(&cors.allow_origin)
        .any(|pattern| pattern.contains('*') && wildcard_match(pattern, origin_str))
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

fn check_headers_allowed<'a>(
    cors: &HttpCors,
    req: &'a RichRequest,
) -> Result<Option<&'a HeaderValue>, CorsError> {
    let request_headers = req
        .headers()
        .get(http::header::ACCESS_CONTROL_REQUEST_HEADERS);

    if let Some(headers_value) = request_headers {
        let allow_list: Vec<_> = split_origin(&cors.allow_headers).collect();
        if allow_list.is_empty() {
            return Ok(Some(headers_value));
        }

        let header_str = headers_value
            .to_str()
            .map_err(|_| CorsError::HeadersNotAllowed)?;

        let all_allowed = split_origin(header_str).all(|h| {
            allow_list
                .iter()
                .any(|&allowed| allowed.eq_ignore_ascii_case(h))
        });

        if !all_allowed {
            return Err(CorsError::HeadersNotAllowed);
        }

        Ok(Some(headers_value))
    } else {
        Ok(None)
    }
}
