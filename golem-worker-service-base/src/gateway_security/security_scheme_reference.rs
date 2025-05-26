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

use crate::gateway_security::{SecuritySchemeIdentifier, SecuritySchemeWithProviderMetadata};

#[derive(Debug, Clone, PartialEq)]
pub struct SecuritySchemeReference {
    pub security_scheme_identifier: SecuritySchemeIdentifier,
}

impl SecuritySchemeReference {
    pub fn new(security_scheme_identifier: String) -> Self {
        SecuritySchemeReference {
            security_scheme_identifier: SecuritySchemeIdentifier::new(security_scheme_identifier),
        }
    }
}

impl From<SecuritySchemeWithProviderMetadata> for SecuritySchemeReference {
    fn from(value: SecuritySchemeWithProviderMetadata) -> Self {
        SecuritySchemeReference {
            security_scheme_identifier: value.security_scheme.scheme_identifier(),
        }
    }
}
