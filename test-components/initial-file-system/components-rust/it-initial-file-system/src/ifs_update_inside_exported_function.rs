use golem_rust::{agent_definition, agent_implementation};
use wstd::http::{Client, Request};
use wstd::io::empty;

#[agent_definition]
pub trait IfsUpdateInsideExportedFunction {
    fn new(name: String) -> Self;
    async fn run(&self) -> (String, String);
}

struct IfsUpdateInsideExportedFunctionImpl {
    _name: String,
}

#[agent_implementation]
impl IfsUpdateInsideExportedFunction for IfsUpdateInsideExportedFunctionImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    async fn run(&self) -> (String, String) {
        let before = std::fs::read_to_string("/foo.txt").unwrap();

        let port = std::env::var("PORT").unwrap_or("9999".to_string());
        let url = format!("http://localhost:{port}/");

        let request = Request::get(&url)
            .body(empty())
            .expect("Failed to build request");

        let _response = Client::new().send(request).await.expect("Request failed");

        let after = std::fs::read_to_string("/foo.txt").unwrap();
        (before, after)
    }
}
