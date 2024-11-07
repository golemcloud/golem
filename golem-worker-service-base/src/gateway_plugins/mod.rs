use crate::gateway_plugins::http::HttpPlugin;

mod http;

pub enum Plugin {
    Http(HttpPlugin)
}
