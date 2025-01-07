// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::gateway_api_definition::http::MethodPattern;
use std::fmt::{Display, Formatter};

#[derive(Debug)]
pub enum ApiDefTransformationError {
    InvalidRoute {
        method: MethodPattern,
        path: String,
        detail: String,
    },
    Custom(String),
}

impl Display for ApiDefTransformationError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiDefTransformationError::InvalidRoute {
                method,
                path,
                detail,
            } => write!(
                f,
                "ApiDefinitionTransformationError: method: {}, path: {}, detail: {}",
                method, path, detail
            )?,
            ApiDefTransformationError::Custom(msg) => {
                write!(f, "ApiDefinitionTransformationError: {}", msg)?
            }
        }

        Ok(())
    }
}

impl ApiDefTransformationError {
    pub fn new(method: MethodPattern, path: String, detail: String) -> Self {
        ApiDefTransformationError::InvalidRoute {
            method,
            path,
            detail,
        }
    }
}
