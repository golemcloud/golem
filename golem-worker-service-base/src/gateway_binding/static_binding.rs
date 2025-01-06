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

use crate::gateway_middleware::{HttpAuthenticationMiddleware, HttpCors};
use crate::gateway_security::SecuritySchemeWithProviderMetadata;

// Static bindings must NOT contain Rib, in either pre-compiled or raw form,
// as it may introduce unnecessary latency
// in serving the requests when not needed.
// Example of a static binding is a pre-flight request which can be handled by CorsPreflight
// Example: browser requests for preflights need only what's contained in a pre-flight CORS middleware and
// don't need to pass through to the backend.
#[derive(Debug, Clone, PartialEq)]
pub enum StaticBinding {
    HttpCorsPreflight(Box<HttpCors>),
    HttpAuthCallBack(Box<HttpAuthenticationMiddleware>),
}

impl StaticBinding {
    pub fn http_auth_call_back(value: HttpAuthenticationMiddleware) -> StaticBinding {
        StaticBinding::HttpAuthCallBack(Box::new(value))
    }

    pub fn from_http_cors(cors: HttpCors) -> Self {
        StaticBinding::HttpCorsPreflight(Box::new(cors))
    }

    pub fn get_cors_preflight(&self) -> Option<HttpCors> {
        match self {
            StaticBinding::HttpCorsPreflight(preflight) => Some(*preflight.clone()),
            _ => None,
        }
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::StaticBinding> for StaticBinding {
    type Error = String;
    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::StaticBinding,
    ) -> Result<Self, String> {
        match value.static_binding {
            Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::HttpCorsPreflight(cors_preflight)) => {
                Ok(StaticBinding::HttpCorsPreflight(Box::new(cors_preflight.try_into()?)))

            }
            Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::AuthCallback(auth_call_back)) => {
                let security_scheme_with_metadata_proto = auth_call_back.security_with_provider_metadata.ok_or("Security Scheme with provider metadata missing".to_string())?;

                let security_scheme_with_metadata = SecuritySchemeWithProviderMetadata::try_from(security_scheme_with_metadata_proto)?;

                Ok(StaticBinding::HttpAuthCallBack(Box::new(HttpAuthenticationMiddleware {
                    security_scheme_with_metadata
                })))
            }
            None => Err("Static Binding missing".to_string()),
        }
    }
}

impl TryFrom<StaticBinding> for golem_api_grpc::proto::golem::apidefinition::StaticBinding {
    type Error = String;
    fn try_from(value: StaticBinding) -> Result<Self, String> {
        match value {
            StaticBinding::HttpCorsPreflight(cors) => {
                Ok(golem_api_grpc::proto::golem::apidefinition::StaticBinding {
                    static_binding: Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::HttpCorsPreflight(
                        golem_api_grpc::proto::golem::apidefinition::CorsPreflight::from(*cors)
                    )),
                })
            }
            StaticBinding::HttpAuthCallBack(value) => {
                Ok(golem_api_grpc::proto::golem::apidefinition::StaticBinding {
                    static_binding: Some(golem_api_grpc::proto::golem::apidefinition::static_binding::StaticBinding::AuthCallback(
                        golem_api_grpc::proto::golem::apidefinition::AuthCallBack{
                            security_with_provider_metadata: Some(golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata::try_from(
                                value.security_scheme_with_metadata
                            )?)
                        }
                    )),
                })
            }
        }
    }
}
