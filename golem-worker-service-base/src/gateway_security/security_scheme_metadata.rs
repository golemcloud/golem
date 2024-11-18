use crate::gateway_security::{GolemIdentityProviderMetadata, IdentityProvider, SecurityScheme};
use std::sync::Arc;

// This can exist as part of the middleware to initiate the authorisation workflow
// redirecting user to provider login page, or it can be part of the static binding
// serving the auth_call_back endpoint that's called by the provider after the user logs in.
#[derive(Debug, Clone, PartialEq)]
pub struct SecuritySchemeWithProviderMetadata {
    pub security_scheme: SecurityScheme,
    pub provider_metadata: GolemIdentityProviderMetadata,
}

impl SecuritySchemeWithProviderMetadata {
    pub fn identity_provider(&self) -> Arc<dyn IdentityProvider + Send + Sync> {
        Arc::new(self.security_scheme.provider())
    }
}

impl TryFrom<golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata>
    for SecuritySchemeWithProviderMetadata
{
    type Error = String;

    fn try_from(
        value: golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata,
    ) -> Result<Self, Self::Error> {
        let security_scheme_proto = value
            .security_scheme
            .ok_or("Security Scheme missing".to_string())?;
        let security_scheme = SecurityScheme::try_from(security_scheme_proto)?;
        let provider_metadata_string = value
            .identity_provider_metadata
            .map(|x| x.metadata)
            .ok_or("Identity provider metadata missing".to_string())?;
        let provider_metadata: GolemIdentityProviderMetadata =
            serde_json::from_str(provider_metadata_string.as_str())
                .map_err(|err| err.to_string())?;

        Ok(SecuritySchemeWithProviderMetadata {
            security_scheme,
            provider_metadata,
        })
    }
}

impl TryFrom<SecuritySchemeWithProviderMetadata>
    for golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata
{
    type Error = String;
    fn try_from(value: SecuritySchemeWithProviderMetadata) -> Result<Self, String> {
        Ok(
            golem_api_grpc::proto::golem::apidefinition::SecurityWithProviderMetadata {
                security_scheme: Some(
                    golem_api_grpc::proto::golem::apidefinition::SecurityScheme::from(
                        value.security_scheme,
                    ),
                ),
                identity_provider_metadata: Some(
                    golem_api_grpc::proto::golem::apidefinition::IdentityProviderMetadata {
                        metadata: serde_json::to_string(&value.provider_metadata)
                            .map_err(|err| err.to_string())?,
                    },
                ),
            },
        )
    }
}
