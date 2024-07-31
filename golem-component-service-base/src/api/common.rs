use golem_api_grpc::proto::golem::component::v1::{component_error, ComponentError};
use golem_common::metrics::api::TraceErrorKind;
use std::fmt::{Debug, Formatter};

pub struct ComponentTraceErrorKind<'a>(pub &'a ComponentError);

impl<'a> Debug for ComponentTraceErrorKind<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl<'a> TraceErrorKind for ComponentTraceErrorKind<'a> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                component_error::Error::BadRequest(_) => "BadRequest",
                component_error::Error::NotFound(_) => "NotFound",
                component_error::Error::AlreadyExists(_) => "AlreadyExists",
                component_error::Error::LimitExceeded(_) => "LimitExceeded",
                component_error::Error::Unauthorized(_) => "Unauthorized",
                component_error::Error::InternalError(_) => "InternalError",
            },
        }
    }
}

mod conversion {
    use crate::service::component;
    use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
    use golem_api_grpc::proto::golem::component::v1::{component_error, ComponentError};

    impl From<component::ComponentError> for ComponentError {
        fn from(value: component::ComponentError) -> Self {
            let error = match value {
                component::ComponentError::AlreadyExists(_) => {
                    component_error::Error::AlreadyExists(ErrorBody {
                        error: value.to_string(),
                    })
                }
                component::ComponentError::UnknownComponentId(_)
                | component::ComponentError::UnknownVersionedComponentId(_) => {
                    component_error::Error::NotFound(ErrorBody {
                        error: value.to_string(),
                    })
                }
                component::ComponentError::ComponentProcessingError(error) => {
                    component_error::Error::BadRequest(ErrorsBody {
                        errors: vec![error.to_string()],
                    })
                }
                component::ComponentError::Internal(error) => {
                    component_error::Error::InternalError(ErrorBody {
                        error: error.to_string(),
                    })
                }
            };
            ComponentError { error: Some(error) }
        }
    }
}
