// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use golem_rust::agentic::{InputStream, OutputStream};
use golem_rust::{
    ToolError, agent_definition, agent_implementation, tool_definition, tool_implementation,
};

#[agent_definition]
pub trait BashOwner {
    fn new(name: String) -> Self;
    fn name(&self) -> String;
}

struct BashOwnerImpl(String);

#[agent_implementation]
impl BashOwner for BashOwnerImpl {
    fn new(name: String) -> Self {
        Self(name)
    }

    fn name(&self) -> String {
        self.0.clone()
    }
}

#[agent_definition]
pub trait BashOnlyOwner {
    fn new(name: String) -> Self;
    fn name(&self) -> String;
}

struct BashOnlyOwnerImpl(String);

#[agent_implementation]
impl BashOnlyOwner for BashOnlyOwnerImpl {
    fn new(name: String) -> Self {
        Self(name)
    }

    fn name(&self) -> String {
        self.0.clone()
    }
}

#[agent_definition]
pub trait DeniedFilesOwner {
    fn new(name: String) -> Self;
    fn name(&self) -> String;
}

struct DeniedFilesOwnerImpl(String);

#[agent_implementation]
impl DeniedFilesOwner for DeniedFilesOwnerImpl {
    fn new(name: String) -> Self {
        Self(name)
    }

    fn name(&self) -> String {
        self.0.clone()
    }
}

#[derive(Debug, Clone, ToolError)]
pub enum FixtureError {
    #[tool_error(kind = "runtime-error", exit_code = 42)]
    Selected(String),
    #[tool_error(kind = "runtime-error", exit_code = 23)]
    File(String),
    #[tool_error(kind = "runtime-error", exit_code = 24)]
    Checkpoint(String),
}

#[tool_definition(version = "0.1.0")]
pub trait Fixture {
    /// Prefix a streamed input and return it on stdout.
    async fn transfer(&self, prefix: String, stdin: InputStream, stdout: OutputStream);
    async fn emit(&self, bytes: u32, stdout: OutputStream);
    fn write(&self, path: String, text: String) -> Result<String, FixtureError>;
    fn read(&self, path: String) -> Result<String, FixtureError>;
    fn fail(&self) -> Result<(), FixtureError>;
    async fn delayed(&self, path: String) -> Result<String, FixtureError>;
    /// Append `before`, wait for the test to release an HTTP checkpoint, then append `after`.
    async fn checkpoint(&self, path: String, authority: String) -> Result<String, FixtureError>;
}

struct FixtureImpl;

#[tool_implementation]
impl Fixture for FixtureImpl {
    async fn transfer(&self, prefix: String, mut stdin: InputStream, mut stdout: OutputStream) {
        stdout.write(prefix.into_bytes()).await.unwrap();
        while let Some(chunk) = stdin.next().await {
            stdout.write(chunk.unwrap()).await.unwrap();
        }
    }

    async fn emit(&self, bytes: u32, mut stdout: OutputStream) {
        let mut remaining = bytes as usize;
        while remaining > 0 {
            let count = remaining.min(64 * 1024);
            if stdout.write(vec![b'x'; count]).await.is_err() {
                return;
            }
            remaining -= count;
        }
    }

    fn write(&self, path: String, text: String) -> Result<String, FixtureError> {
        std::fs::write(path, text).map_err(|error| FixtureError::File(error.to_string()))?;
        Ok("written".into())
    }

    fn read(&self, path: String) -> Result<String, FixtureError> {
        std::fs::read_to_string(path).map_err(|error| FixtureError::File(error.to_string()))
    }

    fn fail(&self) -> Result<(), FixtureError> {
        Err(FixtureError::Selected("fixture failure".into()))
    }

    async fn delayed(&self, path: String) -> Result<String, FixtureError> {
        golem_rust::wasip3::clocks::monotonic_clock::wait_for(200_000_000).await;
        std::fs::write(path, "completed").map_err(|error| FixtureError::File(error.to_string()))?;
        Ok("completed".into())
    }

    async fn checkpoint(&self, path: String, authority: String) -> Result<String, FixtureError> {
        append(&path, "before\n")?;
        wait_at_checkpoint(&authority).await?;
        append(&path, "after\n")?;
        Ok("completed".into())
    }
}

fn append(path: &str, text: &str) -> Result<(), FixtureError> {
    use std::io::Write;

    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| FixtureError::File(error.to_string()))
}

// GET is an idempotent method, so a send interrupted by a crash is re-sent under its original
// durable call (and idempotency key) instead of failing recovery.
async fn wait_at_checkpoint(authority: &str) -> Result<(), FixtureError> {
    use futures_concurrency::prelude::*;
    use golem_rust::wasip3::http::{client, types};
    use golem_rust::wasip3::wit_future;

    let failed = |error: String| FixtureError::Checkpoint(error);
    let headers = types::Fields::from_list(&[]).map_err(|error| failed(format!("{error:?}")))?;
    let (trailers_tx, trailers_rx) = wit_future::new(|| Ok(None));
    let (request, transmit) = types::Request::new(headers, None, trailers_rx, None);
    request
        .set_method(&types::Method::Get)
        .map_err(|()| failed("set method".into()))?;
    request
        .set_scheme(Some(&types::Scheme::Http))
        .map_err(|()| failed("set scheme".into()))?;
    request
        .set_authority(Some(authority))
        .map_err(|()| failed("set authority".into()))?;
    request
        .set_path_with_query(Some("/checkpoint"))
        .map_err(|()| failed("set path".into()))?;
    // The request head is only transmitted once its trailers resolve, so finish them concurrently.
    let send = async move { client::send(request).await };
    let finish = async move {
        let _ = trailers_tx.write(Ok(None)).await;
        transmit.await
    };
    let (response, transmitted) = (send, finish).join().await;
    let response = response.map_err(|error| failed(format!("send: {error:?}")))?;
    transmitted.map_err(|error| failed(format!("transmit: {error:?}")))?;
    let status = response.get_status_code();
    let (response_done_tx, response_done_rx) = wit_future::new(|| Ok(()));
    let (body, trailers) = types::Response::consume_body(response, response_done_rx);
    let _ = response_done_tx.write(Ok(())).await;
    drop(body);
    drop(trailers);
    if status == 204 {
        Ok(())
    } else {
        Err(failed(format!("status {status}")))
    }
}
