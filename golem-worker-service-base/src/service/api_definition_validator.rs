use std::fmt::Display;
use async_trait::async_trait;
use golem_service_base::model::{Template};
use serde::{Deserialize, Serialize};

// TODO; This is more specific to specific protocol validations
// There should be a separate validator for worker binding as it is a common to validation to all protocls
pub trait ApiDefinitionValidatorService<ApiDefinition, E> {
    fn validate(&self, api: &ApiDefinition, templates: &[Template]) -> Result<(), E>;
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, thiserror::Error)]
// TODO: Fix this display impl.
#[error("Validation error: {errors:?}")]
pub struct ValidationError<E> {
    pub errors: Vec<E>,
}

#[derive(Copy, Clone)]
pub struct ApiDefinitionValidatorNoop {}

#[async_trait]
impl<A, E> ApiDefinitionValidatorService<A, E> for ApiDefinitionValidatorNoop {
    fn validate(
        &self,
        _api: &A,
        _templates: &[Template],
    ) -> Result<(), ValidationError<E>> {
        Ok(())
    }
}
