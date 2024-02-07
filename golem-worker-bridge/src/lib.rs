mod tokeniser;
mod expr;
mod parser;
mod api_spec;
mod api_request;
mod api_request_route_resolver;
mod resolved_variables;
mod worker_request_executor;
mod worker;
mod app_config;

pub trait UriBackConversion {
    fn as_http_02(&self) -> http_02::Uri;
}

impl UriBackConversion for http::Uri {
    fn as_http_02(&self) -> http_02::Uri {
        self.to_string().parse().unwrap()
    }
}
