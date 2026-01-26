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

use super::request_handler::RequestHandlerError;
use http::HeaderMap;
use std::collections::HashMap;
use uuid::Uuid;

const COOKIE_HEADER_NAMES: [&str; 2] = ["cookie", "Cookie"];

pub struct RichRequest {
    pub underlying: poem::Request,
    pub request_id: Uuid,
}

impl RichRequest {
    pub fn new(underlying: poem::Request) -> RichRequest {
        RichRequest {
            underlying,
            request_id: Uuid::new_v4(),
        }
    }

    pub fn origin(&self) -> Result<Option<&str>, RequestHandlerError> {
        match self.underlying.headers().get("Origin") {
            Some(header) => {
                let result =
                    header
                        .to_str()
                        .map_err(|_| RequestHandlerError::HeaderIsNotAscii {
                            header_name: "Origin".to_string(),
                        })?;
                Ok(Some(result))
            }
            None => Ok(None),
        }
    }

    pub fn headers(&self) -> &HeaderMap {
        self.underlying.headers()
    }

    pub fn query_params(&self) -> HashMap<String, Vec<String>> {
        let mut params: HashMap<String, Vec<String>> = HashMap::new();

        if let Some(q) = self.underlying.uri().query() {
            for (key, value) in url::form_urlencoded::parse(q.as_bytes()).into_owned() {
                params.entry(key).or_default().push(value);
            }
        }

        params
    }

    pub fn cookies(&self) -> HashMap<&str, &str> {
        let mut result = HashMap::new();

        for header_name in COOKIE_HEADER_NAMES.iter() {
            if let Some(value) = self.underlying.header(header_name) {
                let parts: Vec<&str> = value.split(';').collect();
                for part in parts {
                    let key_value: Vec<&str> = part.split('=').collect();
                    if let (Some(key), Some(value)) = (key_value.first(), key_value.get(1)) {
                        result.insert(key.trim(), value.trim());
                    }
                }
            }
        }

        result
    }
}
