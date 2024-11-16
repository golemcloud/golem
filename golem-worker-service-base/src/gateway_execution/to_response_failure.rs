use golem_common::SafeDisplay;
use http::StatusCode;
use poem::Body;

pub trait ToResponseFailure<A> {
    fn to_failed_response<F>(&self, get_status_code: F) -> A
    where
        F: Fn(&Self) -> StatusCode,
        Self: Sized;
}

// Only SafeDisplay'd errors are allowed for embedding in any output response
impl<E: SafeDisplay> ToResponseFailure<poem::Response> for E {
    fn to_failed_response<F>(&self, get_status_code: F) -> poem::Response
    where
        F: Fn(&Self) -> StatusCode,
        Self: Sized,
    {
        poem::Response::builder()
            .status(get_status_code(self))
            .body(Body::from_string(self.to_safe_string()))
    }
}
