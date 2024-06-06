use std::fmt::Display;

use async_trait::async_trait;

// TODO; We could optimise this further
// to pick the exact API Definition (instead of a vector),
// by doing route resolution at this stage rather than
// delegating that task to worker-binding resolver.
// However, requires lot more work.
#[async_trait]
pub trait ApiDefinitionsLookup<Input, ApiDefinition> {
    async fn get(&self, input: Input) -> Result<Vec<ApiDefinition>, ApiDefinitionLookupError>;
}

pub struct ApiDefinitionLookupError(pub String);

impl Display for ApiDefinitionLookupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ApiDefinitionLookupError: {}", self.0)
    }
}
