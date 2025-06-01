use axum_jrpc::{Id, JsonRpcAnswer, JsonRpcRequest, JsonRpcResponse};
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::protocol::frame::Utf8Payload;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

pub type DebugServiceClient = WebSocketStream<MaybeTlsStream<TcpStream>>;
pub type DebugWrite = SplitSink<DebugServiceClient, Message>;
pub type DebugRead = SplitStream<DebugServiceClient>;

// A client to interact with debug worker executor
#[derive(Debug)]
pub struct DebugWorkerExecutorClient {
    write_msg: DebugWrite,
    read_msg: DebugRead,
}

impl DebugWorkerExecutorClient {
    pub async fn send_jrpc_msg<T: Serialize>(
        &mut self,
        method_name: &str,
        params: T,
    ) -> anyhow::Result<Id> {
        let uuid = uuid::Uuid::new_v4();

        let jrpc_request = JsonRpcRequest {
            id: Id::Str(uuid.to_string()),
            method: method_name.to_string(),
            params: serde_json::to_value(params)?,
        };

        let id = Id::Str(uuid.to_string());

        self.write_msg
            .send(Message::Text(Utf8Payload::from(serde_json::to_string(
                &jrpc_request,
            )?)))
            .await?;

        Ok(id)
    }

    pub async fn read_jrpc_msg<T: DeserializeOwned>(&mut self, id: Id) -> anyhow::Result<T> {
        let time = std::time::Instant::now();
        loop {
            if let Some(msg) = self.read_msg.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        let response: JsonRpcResponse = serde_json::from_str(text.as_str())
                            .map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))?;

                        if response.id == id {
                            match response.result {
                                JsonRpcAnswer::Result(result) => {
                                    let result: T =
                                        serde_json::from_value(result).map_err(|e| {
                                            anyhow::anyhow!("Failed to parse response: {}", e)
                                        })?;
                                    break Ok(result); // Break out of the loop with a Result
                                }
                                JsonRpcAnswer::Error(_) => {
                                    break Err(anyhow::anyhow!("Error response"))
                                }
                            }
                        }
                    }
                    _ => {
                        if time.elapsed().as_secs() > 10 {
                            break Err(anyhow::anyhow!("Timeout")); // Break with an error
                        }
                    }
                }
            } else {
                break Err(anyhow::anyhow!("Stream ended unexpectedly")); // Handle end of stream
            }
        }
    }

    pub async fn connect(port: u16) -> Result<Self, anyhow::Error> {
        let server_url = format!("ws://127.0.0.1:{port}/ws");

        // Connect to the WebSocket server
        let ws_stream = connect_async(server_url)
            .await
            .map(|x| x.0)
            .map_err(|e| anyhow::anyhow!("Failed to connect to WebSocket server: {:?}", e))?;

        let (write, read) = ws_stream.split();

        Ok(DebugWorkerExecutorClient {
            write_msg: write,
            read_msg: read,
        })
    }
}
