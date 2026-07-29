use golem_rust::{agent_definition, agent_implementation};
use wasi::sockets::instance_network::instance_network;
use wasi::sockets::ip_name_lookup::resolve_addresses;

#[agent_definition]
pub trait Networking {
    fn new(name: String) -> Self;
    fn get(&self) -> Vec<String>;
    async fn resolve_p3(&self, name: String) -> Result<Vec<String>, String>;
    /// Connects to `127.0.0.1:port` with a raw wasip3 TCP socket and reads the
    /// receive stream to completion, returning the collected bytes as a string.
    async fn tcp_collect_p3(&self, port: u16) -> Result<String, String>;
}

pub struct NetworkingImpl {
    _name: String,
}

#[agent_implementation]
impl Networking for NetworkingImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn get(&self) -> Vec<String> {
        let network = instance_network();
        let resolve_stream = resolve_addresses(&network, "golem.cloud").expect("resolve_addresses");
        let pollable = resolve_stream.subscribe();
        pollable.block();

        let mut result = Vec::new();
        loop {
            let next = resolve_stream
                .resolve_next_address()
                .expect("resolve_next_address");
            if let Some(next) = next {
                result.push(format!("{:?}", next));
            } else {
                break;
            }
        }
        result
    }

    async fn resolve_p3(&self, name: String) -> Result<Vec<String>, String> {
        golem_rust::wasip3::sockets::ip_name_lookup::resolve_addresses(name)
            .await
            .map(|addresses| {
                addresses
                    .into_iter()
                    .map(|address| format!("{address:?}"))
                    .collect()
            })
            .map_err(|error| format!("{error:?}"))
    }

    async fn tcp_collect_p3(&self, port: u16) -> Result<String, String> {
        use golem_rust::wasip3::sockets::types::{
            IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
        };
        use golem_rust::wasip3::wit_bindgen::StreamResult;

        let socket =
            TcpSocket::create(IpAddressFamily::Ipv4).map_err(|error| format!("{error:?}"))?;
        socket
            .connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port,
            }))
            .await
            .map_err(|error| format!("{error:?}"))?;

        let (mut stream, result) = socket.receive();
        let mut collected = Vec::new();
        let mut buffer = Vec::with_capacity(1024);
        loop {
            let (read_result, next_buffer) = stream.read(buffer).await;
            buffer = next_buffer;
            match read_result {
                StreamResult::Complete(n) => {
                    collected.extend_from_slice(&buffer[..n]);
                    buffer.clear();
                }
                StreamResult::Dropped => break,
                StreamResult::Cancelled => return Err("receive stream read cancelled".to_string()),
            }
        }
        drop(stream);
        result.await.map_err(|error| format!("{error:?}"))?;

        Ok(String::from_utf8_lossy(&collected).to_string())
    }
}
