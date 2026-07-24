use golem_rust::{
    agent_definition, agent_implementation,
    wasip3::{
        http::{client, types},
        spawn,
        wit_bindgen::StreamResult,
        wit_bindgen::yield_async,
        wit_future,
    },
};

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
        let authority = format!("localhost:{port}");

        let headers = types::Headers::new();
        let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
        let (request, request_sent) = types::Request::new(headers, None, trailers_rx, None);
        spawn(async move {
            trailers_tx
                .write(Ok(None))
                .await
                .expect("Failed to finish request trailers");
        });
        yield_async().await;

        request
            .set_scheme(Some(&types::Scheme::Http))
            .expect("Failed to set request scheme");
        request
            .set_authority(Some(&authority))
            .expect("Failed to set request authority");
        request
            .set_path_with_query(Some("/"))
            .expect("Failed to set request path");

        let response = client::send(request).await.expect("Request failed");
        request_sent.await.expect("Failed to transmit request");
        drain_response(response).await;

        let after = std::fs::read_to_string("/foo.txt").unwrap();
        (before, after)
    }
}

async fn drain_response(response: types::Response) {
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (mut body, trailers) = types::Response::consume_body(response, response_done_rx);
    let mut buffer = Vec::with_capacity(16 * 1024);

    loop {
        let (result, next_buffer) = body.read(buffer).await;
        buffer = next_buffer;
        match result {
            StreamResult::Complete(_) => buffer.clear(),
            StreamResult::Dropped => break,
            StreamResult::Cancelled => panic!("response body read was cancelled"),
        }
    }

    trailers.await.expect("Failed to receive response trailers");
    response_done_tx
        .write(Ok(()))
        .await
        .expect("Failed to acknowledge response body");
}
