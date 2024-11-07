pub use http::*;

mod http;

#[derive(Debug, Clone, PartialEq)]
pub enum Middleware {
    Http(HttpMiddleware)
}

impl Middleware {
    pub fn http(http_middleware: HttpMiddleware) -> Middleware {
        Middleware::Http(http_middleware)
    }
}
