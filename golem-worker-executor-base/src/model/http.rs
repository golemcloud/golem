use std::collections::HashMap;
use bytes::Bytes;

pub type Fields = HashMap<String, String>;

pub enum HttpMethod {
    GET,
    HEAD,
    POST,
    PUT,
    DELETE,
    CONNECT,
    OPTIONS,
    TRACE,
    PATCH,
    Custom(String)
}

pub struct BodyAndTrailers {
    pub body: Bytes,
    pub trailers: Option<Fields>
}

pub struct IncomingHttpHandlerInvocation {
    pub uri: String,
    pub method: HttpMethod,
    pub headers: HashMap<String, String>,
    pub body_and_trailers: BodyAndTrailers
}