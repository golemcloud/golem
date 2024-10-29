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
