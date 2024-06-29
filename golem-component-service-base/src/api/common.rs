mod conversion {
    use crate::service::component;
    use golem_api_grpc::proto::golem::common::{ErrorBody, ErrorsBody};
    use golem_api_grpc::proto::golem::component::{component_error, ComponentError};

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
