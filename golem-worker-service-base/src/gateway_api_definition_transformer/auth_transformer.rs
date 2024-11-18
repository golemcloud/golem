use crate::gateway_api_definition::http::HttpApiDefinition;
use crate::gateway_api_definition_transformer::{
    ApiDefTransformationError, ApiDefinitionTransformer,
};

// Auth transformer ensures that to have auth-call-back endpoint route
// corresponding to every security scheme that is in use in ApiDefinition
pub struct AuthTransformer;

impl ApiDefinitionTransformer for AuthTransformer {
    fn transform(
        &self,
        _api_definition: &mut HttpApiDefinition,
    ) -> Result<(), ApiDefTransformationError> {
        Ok(())
    }
}
