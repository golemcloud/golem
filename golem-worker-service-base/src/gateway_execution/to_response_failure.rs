use std::fmt::Display;
use async_trait::async_trait;
use http::StatusCode;
use poem::{Body};
use crate::gateway_binding::{RibInputTypeMismatch};
use crate::gateway_execution::to_response::ToResponse;

pub trait ToResponseFailure<A> {
    fn to_failed_response(&self, status_code: &StatusCode) -> A;
}

// TODO; Use SafeDisplay instead of Display
impl<E: Display> ToResponseFailure<poem::Response> for E {
    fn to_failed_response(&self, status_code: &StatusCode) -> poem::Response {
        poem::Response::builder()
            .status(status_code.clone())
            .body(Body::from_string(format!("Error {}", self).to_string()))
    }
}


