use crate::gateway_api_definition::http::HttpApiDefinition;
use crate::gateway_api_definition_transformer::{
    ApiDefTransformationError, ApiDefinitionTransformer,
};

// Auth transformer ensures that for all security schemes
pub struct AuthTransformer;

impl ApiDefinitionTransformer for AuthTransformer {
    fn transform(
        &self,
        _api_definition: &mut HttpApiDefinition,
    ) -> Result<(), ApiDefTransformationError> {
        //let security_schemes = vec![];

        // for route in api_definition.routes {
        //
        // }

        Ok(())
    }
}
