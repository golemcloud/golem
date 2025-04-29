use axum::{Router, response::sse, routing};
use futures::{Stream, stream};
use std::{convert::Infallible, net::SocketAddr};
use tokio::sync::oneshot;

pub struct Server {
    pub base_url: String,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl Server {
    pub fn start(sse_path: String, events: Vec<String>) -> Self {
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<SocketAddr>();

        let _handle = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let app =
                    Router::new().route(&sse_path, routing::post(async || sse_stream(events)));

                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                ready_tx.send(addr).unwrap();

                axum::serve(listener, app)
                    .with_graceful_shutdown(async {
                        shutdown_rx.await.ok();
                    })
                    .await
                    .unwrap();
            });
        });

        let addr = ready_rx.recv().unwrap();

        Self {
            base_url: format!("http://{}", addr),
            shutdown_tx: Some(shutdown_tx),
        }
    }

    pub fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

fn sse_stream(
    messages: Vec<String>,
) -> sse::Sse<impl Stream<Item = Result<sse::Event, Infallible>>> {
    let events = messages.into_iter().map(|msg| {
        let parts: Vec<&str> = msg.split("\n").collect();

        if parts.len() > 1 {
            return Ok(sse::Event::default().event(parts[0].strip_prefix("event: ").unwrap()).data(parts[1].strip_prefix("data: ").unwrap()));
        }
        
        if msg.starts_with("data: ") {
            return Ok(sse::Event::default().data(msg.strip_prefix("data: ").unwrap()));
        }
        Ok(sse::Event::default().data(msg))
    });
    sse::Sse::new(stream::iter(events))
}
