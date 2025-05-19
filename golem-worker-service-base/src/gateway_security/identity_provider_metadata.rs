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

use golem_api_grpc::proto::golem::apidefinition::IdentityProviderMetadata as IdentityProviderMetadataProto;
use openidconnect::core::{
    CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClientAuthMethod, CoreGrantType,
    CoreJsonWebKey, CoreJsonWebKeyType, CoreJsonWebKeyUse, CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm, CoreJwsSigningAlgorithm, CoreResponseMode, CoreResponseType,
    CoreSubjectIdentifierType,
};
use openidconnect::{EmptyAdditionalProviderMetadata, ProviderMetadata};
use serde_json::Value;

pub type GolemIdentityProviderMetadata = ProviderMetadata<
    EmptyAdditionalProviderMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJwsSigningAlgorithm,
    CoreJsonWebKeyType,
    CoreJsonWebKeyUse,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

pub fn from_identity_provider_metadata_proto(
    value: IdentityProviderMetadataProto,
) -> Result<GolemIdentityProviderMetadata, String> {
    let provider_metadata_json = GolemIdentityProviderMetadataJson::from(value);

    GolemIdentityProviderMetadata::try_from(provider_metadata_json)
}

pub fn to_identity_provider_metadata_proto(
    value: GolemIdentityProviderMetadata,
) -> IdentityProviderMetadataProto {
    IdentityProviderMetadataProto {
        metadata: serde_json::to_string(&value).unwrap(),
    }
}

pub struct GolemIdentityProviderMetadataJson {
    pub json: Value,
}

impl From<IdentityProviderMetadataProto> for GolemIdentityProviderMetadataJson {
    fn from(value: IdentityProviderMetadataProto) -> Self {
        Self {
            json: serde_json::from_str(value.metadata.as_str()).unwrap(),
        }
    }
}

impl TryFrom<GolemIdentityProviderMetadataJson> for GolemIdentityProviderMetadata {
    type Error = String;

    fn try_from(value: GolemIdentityProviderMetadataJson) -> Result<Self, Self::Error> {
        let provider_metadata =
            serde_json::from_value(value.json).map_err(|err| err.to_string())?;

        Ok(provider_metadata)
    }
}
