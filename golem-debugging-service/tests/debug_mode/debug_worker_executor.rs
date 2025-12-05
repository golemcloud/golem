use anyhow::Context;
use axum_jrpc::error::JsonRpcError;
use axum_jrpc::{Id, JsonRpcAnswer, JsonRpcRequest, JsonRpcResponse};
use futures::stream::{SplitSink, SplitStream};
use futures::{SinkExt, StreamExt};
use golem_common::model::auth::TokenSecret;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::task::JoinSet;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
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
    read_messages: Vec<UntypedJrpcMessage>,
    join_set: Option<JoinSet<anyhow::Result<()>>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct UntypedJrpcMessage {
    pub jsonrpc: String,
    pub method: Option<String>,
    pub id: Option<String>,
    pub params: Option<Value>,
    pub error: Option<JsonRpcError>,
    pub result: Option<Value>,
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

    pub async fn read_jrpc_response<T: DeserializeOwned>(&mut self, id: Id) -> anyhow::Result<T> {
        let time = std::time::Instant::now();
        loop {
            if let Some(msg) = self.read_msg.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        {
                            let message =
                                serde_json::from_str::<UntypedJrpcMessage>(text.as_str())?;
                            self.read_messages.push(message);
                        }

                        let maybe_response = serde_json::from_str::<JsonRpcResponse>(text.as_str());

                        match maybe_response {
                            Ok(response) if response.id == id => {
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
                            _ => {}
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

    pub async fn drain_connection(&mut self) -> anyhow::Result<()> {
        while let Some(msg) = self.read_msg.next().await {
            if let Message::Text(text) = msg? {
                let message = serde_json::from_str::<UntypedJrpcMessage>(text.as_str())?;
                self.read_messages.push(message);
            }
        }

        Ok(())
    }

    pub async fn connect(port: u16, token: TokenSecret) -> Result<Self, anyhow::Error> {
        let server_url = format!("ws://127.0.0.1:{port}/v1/debugger");

        let mut connection_request = server_url
            .into_client_request()
            .context("Failed to create request")?;

        {
            let headers = connection_request.headers_mut();

            headers.insert(
                "Authorization",
                format!("Bearer {}", token.secret()).parse()?,
            );
        }

        // Connect to the WebSocket server
        let ws_stream = connect_async(connection_request)
            .await
            .map(|x| x.0)
            .map_err(|e| anyhow::anyhow!("Failed to connect to WebSocket server: {:?}", e))?;

        let (write, read) = ws_stream.split();

        Ok(DebugWorkerExecutorClient {
            write_msg: write,
            read_msg: read,
            read_messages: Vec::new(),
            join_set: None,
        })
    }

    pub async fn close(&mut self) -> anyhow::Result<()> {
        self.write_msg.send(Message::Close(None)).await?;

        self.drain_connection().await?;

        Ok(())
    }

    pub fn all_read_messages(&self) -> Vec<UntypedJrpcMessage> {
        self.read_messages.clone()
    }

    pub fn set_worker_executor_join_set(&mut self, join_set: JoinSet<anyhow::Result<()>>) {
        let _ = self.join_set.insert(join_set);
    }
}
