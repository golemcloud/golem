use golem_common::SafeDisplay;
use http::StatusCode;
use poem::Body;

pub trait ToResponseFailure<A> {
    fn to_failed_response(&self, status_code: &StatusCode) -> A;
}

// Only SafeDisplay'd errors are allowed for embedding in any output response
impl<E: SafeDisplay> ToResponseFailure<poem::Response> for E {
    fn to_failed_response(&self, status_code: &StatusCode) -> poem::Response {
        poem::Response::builder()
            .status(status_code.clone())
            .body(Body::from_string(self.to_safe_string()))
    }
}
