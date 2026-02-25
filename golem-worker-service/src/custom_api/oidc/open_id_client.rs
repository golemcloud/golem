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

use openidconnect::core::{
    CoreAuthDisplay, CoreAuthPrompt, CoreErrorResponseType, CoreGenderClaim, CoreIdTokenVerifier,
    CoreJsonWebKey, CoreJweContentEncryptionAlgorithm, CoreRevocableToken,
    CoreRevocationErrorResponse, CoreTokenIntrospectionResponse, CoreTokenResponse,
};
use openidconnect::{
    Client, EmptyAdditionalClaims, EndpointMaybeSet, EndpointNotSet, EndpointSet,
    StandardErrorResponse,
};

pub type ConfiguredCoreClient = Client<
    EmptyAdditionalClaims,
    CoreAuthDisplay,
    CoreGenderClaim,
    CoreJweContentEncryptionAlgorithm,
    CoreJsonWebKey,
    CoreAuthPrompt,
    StandardErrorResponse<CoreErrorResponseType>,
    CoreTokenResponse,
    CoreTokenIntrospectionResponse,
    CoreRevocableToken,
    CoreRevocationErrorResponse,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

#[derive(Clone, Debug)]
pub struct OpenIdClient {
    pub client: ConfiguredCoreClient,
}

impl OpenIdClient {
    pub fn new(client: ConfiguredCoreClient) -> Self {
        OpenIdClient { client }
    }

    pub fn id_token_verifier(&self) -> CoreIdTokenVerifier<'_> {
        self.client.id_token_verifier()
    }
}
