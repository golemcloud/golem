use std::collections::HashMap;
use bincode::{Decode, Encode};
use bytes::Bytes;

pub type Fields = HashMap<String, String>;

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
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

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct BodyAndTrailers {
    pub body: Bytes,
    pub trailers: Option<Fields>
}

#[derive(Clone, Debug, PartialEq, Encode, Decode)]
pub struct IncomingHttpHandlerInvocation {
    pub uri: String,
    pub method: HttpMethod,
    pub headers: HashMap<String, String>,
    pub body_and_trailers: Option<BodyAndTrailers>
}