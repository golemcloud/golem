use golem_rust::{agent_definition, agent_implementation, read_only};
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

fn tcp_input_ready(port: u16) -> Result<bool, String> {
    let connection = tcp_input_connection(port)?;
    Ok(connection.input.subscribe().ready())
}

/// A P2 TCP connection. The streams come before their socket so that they are dropped first.
struct Connection {
    input: wasi::io::streams::InputStream,
    output: wasi::io::streams::OutputStream,
    _socket: wasi::sockets::tcp::TcpSocket,
}

fn tcp_input_connection(port: u16) -> Result<Connection, String> {
    use wasi::sockets::network::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};
    let socket = wasi::sockets::tcp_create_socket::create_tcp_socket(IpAddressFamily::Ipv4)
        .map_err(|error| format!("{error:?}"))?;
    socket
        .start_connect(
            &instance_network(),
            IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port,
            }),
        )
        .map_err(|error| format!("{error:?}"))?;
    socket.subscribe().block();
    let (input, output) = socket
        .finish_connect()
        .map_err(|error| format!("{error:?}"))?;
    Ok(Connection {
        input,
        output,
        _socket: socket,
    })
}

/// Why one attempt of a [`reconnecting`] operation failed.
enum Failure {
    Stream(wasi::io::streams::StreamError),
    Other(String),
}

impl From<wasi::io::streams::StreamError> for Failure {
    fn from(error: wasi::io::streams::StreamError) -> Self {
        Self::Stream(error)
    }
}

impl From<String> for Failure {
    fn from(error: String) -> Self {
        Self::Other(error)
    }
}

/// The most lost connections a [`reconnecting`] operation accepts before it gives up.
const MAX_LOSSES: u32 = 3;

/// Runs `operation` on new connections to `ports`. When the operation fails because a connection
/// was lost (`connection-reset`, also after a reconstruction), connects again and runs the
/// operation again, as a client does after a network error. Returns the number of lost
/// connections and the result.
fn reconnecting<T>(
    ports: &[u16],
    operation: impl Fn(&[Connection]) -> Result<T, Failure>,
) -> Result<(u32, T), String> {
    use wasi::io::streams::StreamError;

    let mut losses = 0;
    loop {
        let connections = ports
            .iter()
            .map(|port| tcp_input_connection(*port))
            .collect::<Result<Vec<_>, _>>()?;
        match operation(&connections) {
            Ok(value) => return Ok((losses, value)),
            Err(Failure::Stream(StreamError::LastOperationFailed(error)))
                if error
                    .to_debug_string()
                    .to_lowercase()
                    .contains("connection reset")
                    && losses < MAX_LOSSES =>
            {
                losses += 1;
            }
            Err(Failure::Stream(error)) => return Err(format!("{error:?}")),
            Err(Failure::Other(error)) => return Err(error),
        }
    }
}

/// Blocks until the input of a connection is ready, then reads one byte from it.
fn tcp_input_block(port: u16) -> Result<(u32, Vec<u8>), String> {
    reconnecting(&[port], |connections| {
        let input = &connections[0].input;
        input.subscribe().block();
        Ok(input.blocking_read(1)?)
    })
}

/// Polls the inputs of two connections, then reads one byte from each ready input.
fn tcp_inputs_poll(
    first_port: u16,
    second_port: u16,
    duplicate: bool,
) -> Result<(u32, Vec<u32>), String> {
    reconnecting(&[first_port, second_port], |connections| {
        let first = connections[0].input.subscribe();
        let second = connections[1].input.subscribe();
        let (ready, inputs) = if duplicate {
            (wasi::io::poll::poll(&[&second, &first, &second]), [1, 0, 1])
        } else {
            (wasi::io::poll::poll(&[&first, &second]), [0, 1, 1])
        };
        let mut read = Vec::new();
        for index in &ready {
            let input = inputs[*index as usize];
            if !read.contains(&input) {
                read.push(input);
                connections[input].input.read(1)?;
            }
        }
        Ok(ready)
    })
}

fn tcp_blocking_read(port: u16) -> Result<(u32, Vec<u8>), String> {
    reconnecting(&[port], |connections| {
        Ok(connections[0].input.blocking_read(1)?)
    })
}

fn tcp_blocking_skip(port: u16) -> Result<(u32, u64), String> {
    reconnecting(&[port], |connections| {
        Ok(connections[0].input.blocking_skip(1)?)
    })
}

fn tcp_blocking_splice(port: u16) -> Result<(u32, u64), String> {
    reconnecting(&[port], |connections| {
        let connection = &connections[0];
        Ok(connection.output.blocking_splice(&connection.input, 1)?)
    })
}

fn tcp_cross_socket_blocking_splice(
    input_port: u16,
    output_port: u16,
) -> Result<(u32, u64), String> {
    reconnecting(&[input_port, output_port], |connections| {
        Ok(connections[1]
            .output
            .blocking_splice(&connections[0].input, 1)?)
    })
}

fn tcp_file_blocking_splice(port: u16) -> Result<(u32, (u64, Vec<u8>)), String> {
    use wasi::filesystem::types::{DescriptorFlags, OpenFlags, PathFlags};

    reconnecting(&[port], |connections| {
        let (root, _) = wasi::filesystem::preopens::get_directories()
            .into_iter()
            .next()
            .ok_or("no preopened directory".to_string())?;
        let file = root
            .open_at(
                PathFlags::empty(),
                "tcp-splice-output",
                OpenFlags::CREATE | OpenFlags::TRUNCATE,
                DescriptorFlags::READ | DescriptorFlags::WRITE,
            )
            .map_err(|error| format!("open splice destination: {error:?}"))?;
        let output = file
            .write_via_stream(0)
            .map_err(|error| format!("open output stream: {error:?}"))?;
        let written = output.blocking_splice(&connections[0].input, 1)?;
        output
            .blocking_flush()
            .map_err(|error| format!("flush: {error:?}"))?;
        drop(output);
        let size = file
            .stat()
            .map_err(|error| format!("stat: {error:?}"))?
            .size;
        if size != written {
            return Err(format!("file size {size} does not match spliced bytes {written}").into());
        }
        let (contents, _) = file
            .read(written, 0)
            .map_err(|error| format!("read: {error:?}"))?;
        Ok((written, contents))
    })
}

fn p2_address(address: std::net::SocketAddr) -> wasi::sockets::network::IpSocketAddress {
    use wasi::sockets::network::{IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress};

    match address {
        std::net::SocketAddr::V4(address) => {
            let [a, b, c, d] = address.ip().octets();
            IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (a, b, c, d),
                port: address.port(),
            })
        }
        std::net::SocketAddr::V6(address) => {
            let [a, b, c, d, e, f, g, h] = address.ip().segments();
            IpSocketAddress::Ipv6(Ipv6SocketAddress {
                address: (a, b, c, d, e, f, g, h),
                port: address.port(),
                flow_info: 0,
                scope_id: 0,
            })
        }
    }
}

fn p3_address(
    address: std::net::SocketAddr,
) -> golem_rust::wasip3::sockets::types::IpSocketAddress {
    use golem_rust::wasip3::sockets::types::{
        IpSocketAddress, Ipv4SocketAddress, Ipv6SocketAddress,
    };

    match address {
        std::net::SocketAddr::V4(address) => {
            let [a, b, c, d] = address.ip().octets();
            IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (a, b, c, d),
                port: address.port(),
            })
        }
        std::net::SocketAddr::V6(address) => {
            let [a, b, c, d, e, f, g, h] = address.ip().segments();
            IpSocketAddress::Ipv6(Ipv6SocketAddress {
                address: (a, b, c, d, e, f, g, h),
                port: address.port(),
                flow_info: 0,
                scope_id: 0,
            })
        }
    }
}

/// Runs one socket operation of the P2 API on `address`: `tcp-connect`, `udp-send`, `tcp-bind`,
/// `udp-bind` or `tcp-listen` (which first binds to `address`).
fn socket_policy_p2(operation: &str, address: &str) -> Result<String, String> {
    use wasi::sockets::network::IpAddressFamily;
    use wasi::sockets::udp::OutgoingDatagram;

    let address: std::net::SocketAddr = address.parse().map_err(|error| format!("{error}"))?;
    let family = if address.is_ipv4() {
        IpAddressFamily::Ipv4
    } else {
        IpAddressFamily::Ipv6
    };
    let unspecified = if address.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let unspecified = p2_address(unspecified.parse().unwrap());
    let remote = p2_address(address);
    let network = instance_network();
    let error = |error| format!("{error:?}");
    match operation {
        "tcp-connect" => {
            let socket =
                wasi::sockets::tcp_create_socket::create_tcp_socket(family).map_err(error)?;
            socket.start_connect(&network, remote).map_err(error)?;
            socket.subscribe().block();
            socket.finish_connect().map_err(error)?;
            Ok("connected".to_string())
        }
        "udp-send" => {
            let socket =
                wasi::sockets::udp_create_socket::create_udp_socket(family).map_err(error)?;
            socket.start_bind(&network, unspecified).map_err(error)?;
            socket.subscribe().block();
            socket.finish_bind().map_err(error)?;
            let (_, outgoing) = socket.stream(None).map_err(error)?;
            outgoing.check_send().map_err(error)?;
            outgoing
                .send(&[OutgoingDatagram {
                    data: b"policy".to_vec(),
                    remote_address: Some(remote),
                }])
                .map(|sent| sent.to_string())
                .map_err(error)
        }
        "tcp-bind" | "tcp-listen" => {
            let socket =
                wasi::sockets::tcp_create_socket::create_tcp_socket(family).map_err(error)?;
            socket.start_bind(&network, remote).map_err(error)?;
            socket.subscribe().block();
            socket.finish_bind().map_err(error)?;
            if operation == "tcp-bind" {
                return Ok("bound".to_string());
            }
            socket.start_listen().map_err(error)?;
            socket.subscribe().block();
            socket.finish_listen().map_err(error)?;
            Ok("listening".to_string())
        }
        "udp-bind" => {
            let socket =
                wasi::sockets::udp_create_socket::create_udp_socket(family).map_err(error)?;
            socket.start_bind(&network, remote).map_err(error)?;
            socket.subscribe().block();
            socket.finish_bind().map_err(error)?;
            Ok("bound".to_string())
        }
        _ => Err(format!("unknown P2 socket operation: {operation}")),
    }
}

/// [`socket_policy_p2`] on the P3 API; `tcp-listen` also listens on a socket that was never
/// bound.
async fn socket_policy_p3(operation: &str, address: &str) -> Result<String, String> {
    use golem_rust::wasip3::sockets::types::{IpAddressFamily, TcpSocket, UdpSocket};

    let address: std::net::SocketAddr = address.parse().map_err(|error| format!("{error}"))?;
    let family = if address.is_ipv4() {
        IpAddressFamily::Ipv4
    } else {
        IpAddressFamily::Ipv6
    };
    let remote = p3_address(address);
    let error = |error| format!("{error:?}");
    match operation {
        "tcp-connect" => {
            let socket = TcpSocket::create(family).map_err(error)?;
            socket.connect(remote).await.map_err(error)?;
            Ok("connected".to_string())
        }
        "udp-send" => {
            let socket = UdpSocket::create(family).map_err(error)?;
            socket
                .send(b"policy".to_vec(), Some(remote))
                .await
                .map_err(error)?;
            Ok("sent".to_string())
        }
        "tcp-bind" => {
            let socket = TcpSocket::create(family).map_err(error)?;
            socket.bind(remote).map_err(error)?;
            Ok("bound".to_string())
        }
        "udp-bind" => {
            let socket = UdpSocket::create(family).map_err(error)?;
            socket.bind(remote).map_err(error)?;
            Ok("bound".to_string())
        }
        "tcp-listen" => {
            let socket = TcpSocket::create(family).map_err(error)?;
            socket.bind(remote).map_err(error)?;
            socket
                .listen()
                .map(|_| "listening".to_string())
                .map_err(error)
        }
        "tcp-listen-unbound" => {
            let socket = TcpSocket::create(family).map_err(error)?;
            socket
                .listen()
                .map(|_| "listening".to_string())
                .map_err(error)
        }
        _ => Err(format!("unknown P3 socket operation: {operation}")),
    }
}

async fn udp_receive(port: u16, ready_port: u16) -> Result<Vec<u8>, String> {
    use golem_rust::wasip3::sockets::types::{
        IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, UdpSocket,
    };
    let socket = UdpSocket::create(IpAddressFamily::Ipv4).map_err(|error| format!("{error:?}"))?;
    socket
        .bind(IpSocketAddress::Ipv4(Ipv4SocketAddress {
            address: (127, 0, 0, 1),
            port,
        }))
        .map_err(|error| format!("{error:?}"))?;
    socket
        .send(
            b"ready".to_vec(),
            Some(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port: ready_port,
            })),
        )
        .await
        .map_err(|error| format!("{error:?}"))?;
    socket
        .receive()
        .await
        .map(|(data, _)| data)
        .map_err(|error| format!("{error:?}"))
}

#[agent_definition(ephemeral)]
pub trait EphemeralNetworking {
    fn new(name: String) -> Self;
    fn tcp_inputs_poll_p2(
        &self,
        first_port: u16,
        second_port: u16,
    ) -> Result<(u32, Vec<u32>), String>;
    async fn tcp_collect_p3(&self, port: u16) -> Result<String, String>;
    fn tcp_input_ready_p2(&self, port: u16) -> Result<bool, String>;
    fn tcp_input_block_p2(&self, port: u16) -> Result<(u32, Vec<u8>), String>;
    fn tcp_blocking_read_p2(&self, port: u16) -> Result<(u32, Vec<u8>), String>;
    fn tcp_blocking_skip_p2(&self, port: u16) -> Result<(u32, u64), String>;
    fn tcp_blocking_splice_p2(&self, port: u16) -> Result<(u32, u64), String>;
    fn tcp_file_blocking_splice_p2(&self, port: u16) -> Result<(u32, (u64, Vec<u8>)), String>;
    fn tcp_cross_socket_blocking_splice_p2(
        &self,
        input_port: u16,
        output_port: u16,
    ) -> Result<(u32, u64), String>;
    async fn udp_receive_wait_p3(&self, port: u16, ready_port: u16) -> Result<Vec<u8>, String>;
}

pub struct EphemeralNetworkingImpl;

#[agent_implementation]
impl EphemeralNetworking for EphemeralNetworkingImpl {
    fn new(_name: String) -> Self {
        Self
    }

    fn tcp_inputs_poll_p2(
        &self,
        first_port: u16,
        second_port: u16,
    ) -> Result<(u32, Vec<u32>), String> {
        tcp_inputs_poll(first_port, second_port, false)
    }

    async fn tcp_collect_p3(&self, port: u16) -> Result<String, String> {
        tcp_collect(port).await
    }

    fn tcp_input_ready_p2(&self, port: u16) -> Result<bool, String> {
        tcp_input_ready(port)
    }

    fn tcp_input_block_p2(&self, port: u16) -> Result<(u32, Vec<u8>), String> {
        tcp_input_block(port)
    }

    fn tcp_blocking_read_p2(&self, port: u16) -> Result<(u32, Vec<u8>), String> {
        tcp_blocking_read(port)
    }

    fn tcp_blocking_skip_p2(&self, port: u16) -> Result<(u32, u64), String> {
        tcp_blocking_skip(port)
    }

    fn tcp_blocking_splice_p2(&self, port: u16) -> Result<(u32, u64), String> {
        tcp_blocking_splice(port)
    }

    fn tcp_file_blocking_splice_p2(&self, port: u16) -> Result<(u32, (u64, Vec<u8>)), String> {
        tcp_file_blocking_splice(port)
    }

    fn tcp_cross_socket_blocking_splice_p2(
        &self,
        input_port: u16,
        output_port: u16,
    ) -> Result<(u32, u64), String> {
        tcp_cross_socket_blocking_splice(input_port, output_port)
    }

    async fn udp_receive_wait_p3(&self, port: u16, ready_port: u16) -> Result<Vec<u8>, String> {
        udp_receive(port, ready_port).await
    }
}

#[agent_definition]
pub trait Networking {
    fn new(name: String) -> Self;
    fn tcp_blocking_splice_p2(&self, port: u16) -> Result<(u32, u64), String>;
    fn tcp_file_blocking_splice_p2(&self, port: u16) -> Result<(u32, (u64, Vec<u8>)), String>;
    fn tcp_cross_socket_blocking_splice_p2(
        &self,
        input_port: u16,
        output_port: u16,
    ) -> Result<(u32, u64), String>;
    fn tcp_inputs_poll_p2(
        &self,
        first_port: u16,
        second_port: u16,
    ) -> Result<(u32, Vec<u32>), String>;
    fn tcp_inputs_duplicate_poll_p2(
        &self,
        first_port: u16,
        second_port: u16,
    ) -> Result<(u32, Vec<u32>), String>;
    fn get(&self) -> Vec<String>;
    fn tcp_input_ready_p2(&self, port: u16) -> Result<bool, String>;
    fn tcp_input_block_p2(&self, port: u16) -> Result<(u32, Vec<u8>), String>;
    fn tcp_blocking_read_p2(&self, port: u16) -> Result<(u32, Vec<u8>), String>;
    fn tcp_blocking_skip_p2(&self, port: u16) -> Result<(u32, u64), String>;
    async fn udp_receive_wait_p3(&self, port: u16, ready_port: u16) -> Result<Vec<u8>, String>;
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
    async fn udp_receive_p3(&self, port: u16, ready_port: u16) -> Result<String, String>;
    fn socket_policy_p2(&self, operation: String, address: String) -> Result<String, String>;
    async fn socket_policy_p3(&self, operation: String, address: String) -> Result<String, String>;
    #[read_only]
    async fn read_only_tcp_connect_p3(&self, address: String) -> Result<String, String>;
}

pub struct NetworkingImpl {
    _name: String,
}

#[agent_implementation]
impl Networking for NetworkingImpl {
    fn new(name: String) -> Self {
        Self { _name: name }
    }

    fn tcp_inputs_poll_p2(
        &self,
        first_port: u16,
        second_port: u16,
    ) -> Result<(u32, Vec<u32>), String> {
        tcp_inputs_poll(first_port, second_port, false)
    }

    fn tcp_inputs_duplicate_poll_p2(
        &self,
        first_port: u16,
        second_port: u16,
    ) -> Result<(u32, Vec<u32>), String> {
        tcp_inputs_poll(first_port, second_port, true)
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

    fn tcp_input_ready_p2(&self, port: u16) -> Result<bool, String> {
        tcp_input_ready(port)
    }

    fn tcp_input_block_p2(&self, port: u16) -> Result<(u32, Vec<u8>), String> {
        tcp_input_block(port)
    }

    fn tcp_blocking_read_p2(&self, port: u16) -> Result<(u32, Vec<u8>), String> {
        tcp_blocking_read(port)
    }

    fn tcp_blocking_skip_p2(&self, port: u16) -> Result<(u32, u64), String> {
        tcp_blocking_skip(port)
    }

    fn tcp_blocking_splice_p2(&self, port: u16) -> Result<(u32, u64), String> {
        tcp_blocking_splice(port)
    }

    fn tcp_file_blocking_splice_p2(&self, port: u16) -> Result<(u32, (u64, Vec<u8>)), String> {
        tcp_file_blocking_splice(port)
    }

    fn tcp_cross_socket_blocking_splice_p2(
        &self,
        input_port: u16,
        output_port: u16,
    ) -> Result<(u32, u64), String> {
        tcp_cross_socket_blocking_splice(input_port, output_port)
    }

    async fn udp_receive_wait_p3(&self, port: u16, ready_port: u16) -> Result<Vec<u8>, String> {
        udp_receive(port, ready_port).await
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

    async fn udp_receive_p3(&self, port: u16, ready_port: u16) -> Result<String, String> {
        use golem_rust::wasip3::sockets::types::{
            IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, UdpSocket,
        };

        let socket =
            UdpSocket::create(IpAddressFamily::Ipv4).map_err(|error| format!("{error:?}"))?;
        socket
            .bind(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port,
            }))
            .map_err(|error| format!("{error:?}"))?;
        socket
            .send(
                b"ready".to_vec(),
                Some(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                    address: (127, 0, 0, 1),
                    port: ready_port,
                })),
            )
            .await
            .map_err(|error| format!("{error:?}"))?;
        let (data, _) = socket
            .receive()
            .await
            .map_err(|error| format!("{error:?}"))?;
        Ok(String::from_utf8_lossy(&data).to_string())
    }

    fn socket_policy_p2(&self, operation: String, address: String) -> Result<String, String> {
        socket_policy_p2(&operation, &address)
    }

    async fn socket_policy_p3(&self, operation: String, address: String) -> Result<String, String> {
        socket_policy_p3(&operation, &address).await
    }

    async fn read_only_tcp_connect_p3(&self, address: String) -> Result<String, String> {
        socket_policy_p3("tcp-connect", &address).await
    }
}
