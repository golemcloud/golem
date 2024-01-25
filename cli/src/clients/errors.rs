use golem_client::api::{TemplateError, WorkerError};

pub trait ResponseContentErrorMapper {
    fn map(self) -> String;
}

impl ResponseContentErrorMapper for TemplateError {
    fn map(self) -> String {
        match self {
            TemplateError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            TemplateError::Error401(error) => {
                format!("Unauthorized: {error:?}")
            }
            TemplateError::Error403(error) => {
                format!("Forbidden: {error:?}")
            }
            TemplateError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            TemplateError::Error409(error) => {
                format!("Conflict: {error:?}")
            }
            TemplateError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}

impl ResponseContentErrorMapper for WorkerError {
    fn map(self) -> String {
        match self {
            WorkerError::Error400(errors) => {
                format!("BadRequest: {errors:?}")
            }
            WorkerError::Error404(error) => {
                format!("NotFound: {error:?}")
            }
            WorkerError::Error409(error) => {
                format!("Conflict: {error:?}")
            }
            WorkerError::Error500(error) => {
                format!("InternalError: {error:?}")
            }
        }
    }
}
