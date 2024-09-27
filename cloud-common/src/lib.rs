pub mod auth;
pub mod clients;
pub mod config;
pub mod grpc;
pub mod model;

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}

/// Trait to convert a value to a string which is safe to return through a public API.
// TODO: move to golem-common
pub trait SafeDisplay {
    fn to_safe_string(&self) -> String;
}
