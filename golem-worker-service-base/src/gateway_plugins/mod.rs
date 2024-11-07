pub use http::*;

mod http;

#[derive(Debug, Clone, PartialEq)]
pub enum Plugin {
    Http(HttpPlugin)
}
