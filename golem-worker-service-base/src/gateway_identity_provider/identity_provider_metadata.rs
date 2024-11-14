use openidconnect::{EmptyAdditionalProviderMetadata, ProviderMetadata};
use openidconnect::core::{CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClientAuthMethod, CoreGrantType, CoreJsonWebKey, CoreJsonWebKeyType, CoreJsonWebKeyUse, CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreJwsSigningAlgorithm, CoreResponseMode, CoreResponseType, CoreSubjectIdentifierType};

pub type GolemIdentityProviderMetadata = ProviderMetadata<EmptyAdditionalProviderMetadata, CoreAuthDisplay, CoreClientAuthMethod, CoreClaimName, CoreClaimType, CoreGrantType, CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm, CoreJwsSigningAlgorithm, CoreJsonWebKeyType, CoreJsonWebKeyUse, CoreJsonWebKey, CoreResponseMode, CoreResponseType, CoreSubjectIdentifierType>;
