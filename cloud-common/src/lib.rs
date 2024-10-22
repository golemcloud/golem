pub mod auth;
pub mod clients;
pub mod config;
pub mod grpc;
pub mod model;

#[cfg(test)]
test_r::enable!();

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
