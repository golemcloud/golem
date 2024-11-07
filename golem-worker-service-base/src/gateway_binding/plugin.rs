use crate::gateway_binding::http::HttpPlugin;

pub enum Plugin {
    Http(HttpPlugin)
}
