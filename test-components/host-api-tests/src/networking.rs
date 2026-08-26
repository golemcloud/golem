use golem_rust::{agent_definition, agent_implementation};
use wasi::sockets::instance_network::instance_network;
use wasi::sockets::ip_name_lookup::resolve_addresses;

async fn tcp_collect(port: u16) -> Result<String, String> {
    use golem_rust::wasip3::sockets::types::{
        IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
    };
    use golem_rust::wasip3::wit_bindgen::StreamResult;

    let socket = TcpSocket::create(IpAddressFamily::Ipv4).map_err(|error| format!("{error:?}"))?;
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

#[agent_definition]
pub trait Networking {
    fn new(name: String) -> Self;
    fn get(&self) -> Vec<String>;
    fn probe_p2(
        &self,
        operation: String,
        name: String,
        port: u16,
        data: Vec<u8>,
    ) -> Result<String, String>;
    async fn resolve_p3(&self, name: String) -> Result<Vec<String>, String>;
    /// Connects to `127.0.0.1:port` with a raw wasip3 TCP socket and reads the
    /// receive stream to completion, returning the collected bytes as a string.
    async fn tcp_collect_p3(&self, port: u16) -> Result<String, String>;
    async fn tcp_collect_two_p3(
        &self,
        first_port: u16,
        second_port: u16,
    ) -> (Result<String, String>, Result<String, String>);
    async fn probe_udp_p3(&self, operation: String, port: u16, data: Vec<u8>)
    -> Result<(), String>;
    async fn udp_send_p3(&self, port: u16, data: Vec<u8>) -> Result<(), String>;
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

    fn probe_p2(
        &self,
        operation: String,
        name: String,
        port: u16,
        data: Vec<u8>,
    ) -> Result<String, String> {
        use wasi::sockets::network::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};

        let remote = IpSocketAddress::Ipv4(Ipv4SocketAddress {
            address: (127, 0, 0, 1),
            port,
        });
        match operation.as_str() {
            "resolve-addresses" => resolve_addresses(&instance_network(), &name)
                .map(|_| "ok".to_string())
                .map_err(|error| format!("{error:?}")),
            "tcp-start-connect" => {
                let socket =
                    wasi::sockets::tcp_create_socket::create_tcp_socket(IpAddressFamily::Ipv4)
                        .map_err(|error| format!("{error:?}"))?;
                socket
                    .start_connect(&instance_network(), remote)
                    .map_err(|error| format!("{error:?}"))?;
                socket.subscribe().block();
                socket
                    .finish_connect()
                    .map(|_| "ok".to_string())
                    .map_err(|error| format!("{error:?}"))
            }
            "udp-stream-send" | "udp-unconnected-send" => {
                use wasi::sockets::udp::OutgoingDatagram;

                let socket =
                    wasi::sockets::udp_create_socket::create_udp_socket(IpAddressFamily::Ipv4)
                        .map_err(|error| format!("{error:?}"))?;
                let local = IpSocketAddress::Ipv4(Ipv4SocketAddress {
                    address: (0, 0, 0, 0),
                    port: 0,
                });
                socket
                    .start_bind(&instance_network(), local)
                    .map_err(|error| format!("{error:?}"))?;
                socket.subscribe().block();
                socket.finish_bind().map_err(|error| format!("{error:?}"))?;
                let connected = operation == "udp-stream-send";
                let (_, outgoing) = socket
                    .stream(connected.then_some(remote))
                    .map_err(|error| format!("{error:?}"))?;
                outgoing
                    .check_send()
                    .map_err(|error| format!("{error:?}"))?;
                outgoing
                    .send(&[OutgoingDatagram {
                        data,
                        remote_address: (!connected).then_some(remote),
                    }])
                    .map(|sent| sent.to_string())
                    .map_err(|error| format!("{error:?}"))
            }
            _ => Err(format!("unknown P2 networking operation: {operation}")),
        }
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
        tcp_collect(port).await
    }

    async fn tcp_collect_two_p3(
        &self,
        first_port: u16,
        second_port: u16,
    ) -> (Result<String, String>, Result<String, String>) {
        use futures_concurrency::future::Join;

        (tcp_collect(first_port), tcp_collect(second_port))
            .join()
            .await
    }

    async fn probe_udp_p3(
        &self,
        operation: String,
        port: u16,
        data: Vec<u8>,
    ) -> Result<(), String> {
        use golem_rust::wasip3::sockets::types::{
            IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, UdpSocket,
        };

        let socket =
            UdpSocket::create(IpAddressFamily::Ipv4).map_err(|error| format!("{error:?}"))?;
        let remote = IpSocketAddress::Ipv4(Ipv4SocketAddress {
            address: (127, 0, 0, 1),
            port,
        });
        match operation.as_str() {
            "udp-connect" => socket.connect(remote).map_err(|error| format!("{error:?}")),
            "udp-send-unconnected" => socket
                .send(data, Some(remote))
                .await
                .map_err(|error| format!("{error:?}")),
            _ => Err(format!("unknown P3 UDP operation: {operation}")),
        }
    }

    async fn udp_send_p3(&self, port: u16, data: Vec<u8>) -> Result<(), String> {
        use golem_rust::wasip3::sockets::types::{
            IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, UdpSocket,
        };

        let socket =
            UdpSocket::create(IpAddressFamily::Ipv4).map_err(|error| format!("{error:?}"))?;
        socket
            .send(
                data,
                Some(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                    address: (127, 0, 0, 1),
                    port,
                })),
            )
            .await
            .map_err(|error| format!("{error:?}"))
    }
}
