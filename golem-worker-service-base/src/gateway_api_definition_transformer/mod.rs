use crate::gateway_api_definition::http::HttpApiDefinition;
pub use api_definition_transformer::*;
pub use auth_transformer::*;
pub use cors_transformer::*;

mod api_definition_transformer;
mod auth_transformer;
mod cors_transformer;

pub fn auth_transformer() -> Box<dyn ApiDefinitionTransformer> {
    Box::new(AuthTransformer)
}

pub fn cors_transformer() -> Box<dyn ApiDefinitionTransformer> {
    Box::new(CorsTransformer)
}

// A curated list of transformations that gets applied to HttpApiDefinition
// Example: If a user defined pre-flight cors endpoint, then transformer
// has to ensure all the routes under the same resource also has a cors::add_headers
// middleware. This is handled using `CorsTransformer`.
// Similarly, if a user has configured for as security scheme for a route (or routes),
// then AuthTransformer ensures that for all the security schemes
// there exist a corresponding call back endpoint. We are not letting the users
// define this to have a reasonable DX.
pub fn transform_http_api_definition(
    input: &mut HttpApiDefinition,
) -> Result<(), ApiDefTransformationError> {
    let transformers = vec![auth_transformer(), cors_transformer()];

    for transformer in transformers {
        transformer.transform(input)?;
    }

    Ok(())
}
