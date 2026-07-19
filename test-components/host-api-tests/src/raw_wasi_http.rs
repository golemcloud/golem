use futures_concurrency::future::Join;
use golem_rust::wasip3::http::{client, types};
use golem_rust::wasip3::{wit_future, wit_stream};
use golem_rust::{agent_definition, agent_implementation};

#[agent_definition]
pub trait RawWasiHttp {
    fn new(name: String) -> Self;
    async fn run(&self) -> String;
    async fn run_with_interrupt(&self) -> String;
    async fn send_request(&mut self);
    fn process_response(&mut self) -> String;
}

pub struct RawWasiHttpImpl {
    _name: String,
    stored_response: Option<(Vec<u8>, u16)>,
}

#[agent_implementation]
impl RawWasiHttp for RawWasiHttpImpl {
    fn new(name: String) -> Self {
        Self {
            _name: name,
            stored_response: None,
        }
    }

    async fn run(&self) -> String {
        let response = send_http_request("/").await;
        let status = response.get_status_code();
        let body = read_body(response).await;
        format!("{} {}", status, String::from_utf8(body).unwrap())
    }

    async fn run_with_interrupt(&self) -> String {
        let response = send_http_request("/").await;
        let status = response.get_status_code();
        let body = read_body(response).await;

        let restart_response = send_http_request("/restart").await;
        let _ = read_body(restart_response).await;

        format!("{} {}", status, String::from_utf8(body).unwrap())
    }

    async fn send_request(&mut self) {
        let response = send_http_request("/").await;
        let status = response.get_status_code();
        let body = read_body(response).await;
        self.stored_response = Some((body, status));
    }

    fn process_response(&mut self) -> String {
        let (body, status) = self.stored_response.take().unwrap();
        format!("{} {}", status, String::from_utf8(body).unwrap())
    }
}

/// Sends a POST request with a small body through the raw P3 `wasi:http`
/// bindings: the body is streamed through a `wit_stream` concurrently with the
/// send, and the trailers future is completed after the body stream is closed.
async fn send_http_request(path: &str) -> types::Response {
    let port = std::env::var("PORT").unwrap_or("9999".to_string());

    let headers =
        types::Fields::from_list(&[("x-test".to_string(), b"test-header".to_vec())]).unwrap();

    let (mut body_writer, body_reader) = wit_stream::new();
    let (trailers_writer, trailers_reader) =
        wit_future::new::<Result<Option<types::Fields>, types::ErrorCode>>(|| Ok(None));

    let options = types::RequestOptions::new();
    options.set_connect_timeout(Some(5_000_000_000)).unwrap();
    options.set_first_byte_timeout(Some(5_000_000_000)).unwrap();
    options
        .set_between_bytes_timeout(Some(5_000_000_000))
        .unwrap();

    let (request, transmitted) =
        types::Request::new(headers, Some(body_reader), trailers_reader, Some(options));
    request.set_method(&types::Method::Post).unwrap();
    request.set_path_with_query(Some(path)).unwrap();
    request.set_scheme(Some(&types::Scheme::Http)).unwrap();
    request
        .set_authority(Some(&format!("localhost:{port}")))
        .unwrap();

    let write_body = async move {
        let leftover = body_writer.write_all(b"test-body".to_vec()).await;
        assert!(leftover.is_empty());
        drop(body_writer);
        let _ = trailers_writer.write(Ok(None)).await;
    };

    let (response, transmitted, ()) = (client::send(request), transmitted, write_body)
        .join()
        .await;
    transmitted.expect("request body transmission failed");
    match response {
        Ok(response) => {
            println!("Got incoming response");
            response
        }
        Err(err) => {
            println!("Returned with error code: {err:?}");
            panic!("Error: {:?}", err)
        }
    }
}

/// Reads the full body of an incoming response through the raw P3 body stream.
async fn read_body(response: types::Response) -> Vec<u8> {
    let (result_writer, result_reader) = wit_future::new::<Result<(), types::ErrorCode>>(|| Ok(()));
    let (body_stream, trailers) = types::Response::consume_body(response, result_reader);
    let body = body_stream.collect().await;
    if let Err(err) = trailers.await {
        panic!("Error: {:?}", err)
    }
    result_writer
        .write(Ok(()))
        .await
        .expect("failed to acknowledge response body");
    body
}
