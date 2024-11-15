use crate::gateway_execution::to_response::ToResponse;
use http::StatusCode;
use poem::Body;
use std::fmt::Display;
use golem_common::SafeDisplay;

pub trait ToResponseFailure<A> {
    fn to_failed_response(&self, status_code: &StatusCode) -> A;
}

// Good to create only safe-display instances for these errors
impl<E: Display> ToResponseFailure<poem::Response> for E {
    fn to_failed_response(&self, status_code: &StatusCode) -> poem::Response {
        poem::Response::builder()
            .status(status_code.clone())
            .body(Body::from_string(format!("Error {}", self).to_string()))
    }
}

impl<E: SafeDisplay> ToResponseFailure<poem::Response> for E {
    fn to_failed_response(&self, status_code: &StatusCode) -> poem::Response {
        poem::Response::builder()
            .status(status_code.clone())
            .body(Body::from_string(self.to_safe_string()))
    }
}
