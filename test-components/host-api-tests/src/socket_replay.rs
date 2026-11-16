//! A durable agent that keeps P2 and P3 sockets in named slots across invocations, for the
//! socket crash-and-replay tests. Every connect and bind goes to `127.0.0.1`.

use golem_rust::{agent_definition, agent_implementation, read_only};
use std::cell::RefCell;
use std::collections::HashMap;

mod p3 {
    pub use golem_rust::wasip3::sockets::types::{
        ErrorCode, IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket, UdpSocket,
    };
    pub use golem_rust::wasip3::wit_bindgen::{
        FutureReader, StreamReader, StreamResult, StreamWriter,
    };
    pub use golem_rust::wasip3::wit_stream;
}

mod p2 {
    pub use wasi::io::streams::{InputStream, OutputStream, StreamError};
    pub use wasi::sockets::instance_network::instance_network;
    pub use wasi::sockets::network::{IpAddressFamily, IpSocketAddress, Ipv4SocketAddress};
    pub use wasi::sockets::tcp::TcpSocket;
    pub use wasi::sockets::udp::{IncomingDatagramStream, OutgoingDatagramStream, UdpSocket};
}

fn p3_loopback(port: u16) -> p3::IpSocketAddress {
    p3::IpSocketAddress::Ipv4(p3::Ipv4SocketAddress {
        address: (127, 0, 0, 1),
        port,
    })
}

fn p2_loopback(port: u16) -> p2::IpSocketAddress {
    p2::IpSocketAddress::Ipv4(p2::Ipv4SocketAddress {
        address: (127, 0, 0, 1),
        port,
    })
}

fn p3_address(address: p3::IpSocketAddress) -> String {
    match address {
        p3::IpSocketAddress::Ipv4(p3::Ipv4SocketAddress {
            address: (a, b, c, d),
            port,
        }) => format!("{a}.{b}.{c}.{d}:{port}"),
        other => format!("{other:?}"),
    }
}

fn p2_address(address: p2::IpSocketAddress) -> String {
    match address {
        p2::IpSocketAddress::Ipv4(p2::Ipv4SocketAddress {
            address: (a, b, c, d),
            port,
        }) => format!("{a}.{b}.{c}.{d}:{port}"),
        other => format!("{other:?}"),
    }
}

fn error(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

/// Reads from a P3 receive stream until `len` bytes arrived or the stream ended. Returns the bytes
/// and whether the stream ended.
async fn p3_read(stream: &mut p3::StreamReader<u8>, len: usize) -> Result<(Vec<u8>, bool), String> {
    let mut collected = Vec::new();
    while collected.len() < len {
        let (result, buffer) = stream
            .read(Vec::with_capacity((len - collected.len()).min(64 * 1024)))
            .await;
        match result {
            p3::StreamResult::Complete(_) => collected.extend_from_slice(&buffer),
            p3::StreamResult::Dropped => return Ok((collected, true)),
            p3::StreamResult::Cancelled => return Err("receive read cancelled".to_string()),
        }
    }
    Ok((collected, false))
}

/// Sends `data` over the send stream of a P3 TCP socket and closes the stream, awaiting the
/// result of the send while the bytes are written.
async fn p3_send(socket: &p3::TcpSocket, data: Vec<u8>) -> Result<(), String> {
    use futures_concurrency::future::Join;

    let (mut writer, reader) = p3::wit_stream::new::<u8>();
    let result = socket.send(reader);
    let write = async move {
        let remaining = writer.write_all(data).await;
        drop(writer);
        remaining
    };
    let (sent, remaining) = (result, write).join().await;
    let sent = sent.map_err(error);
    if !remaining.is_empty() {
        return Err(format!(
            "{} bytes were not accepted: {sent:?}",
            remaining.len()
        ));
    }
    sent
}

/// Writes `data` to a P2 output stream within its write budget, then flushes it.
fn p2_write(output: &p2::OutputStream, data: &[u8]) -> Result<(), String> {
    let mut rest = data;
    while !rest.is_empty() {
        output.subscribe().block();
        let budget = output.check_write().map_err(error)? as usize;
        let n = budget.min(rest.len());
        if n > 0 {
            output.write(&rest[..n]).map_err(error)?;
            rest = &rest[n..];
        }
    }
    output.blocking_flush().map_err(error)
}

/// Reads from a P2 input stream until `len` bytes arrived or the peer closed the stream.
fn p2_read(input: &p2::InputStream, len: usize) -> Result<Vec<u8>, String> {
    let mut collected = Vec::new();
    while collected.len() < len {
        match input.blocking_read((len - collected.len()) as u64) {
            Ok(bytes) => collected.extend_from_slice(&bytes),
            Err(p2::StreamError::Closed) => break,
            Err(other) => return Err(error(other)),
        }
    }
    Ok(collected)
}

fn p2_tcp_connect(port: u16) -> Result<(p2::TcpSocket, p2::InputStream, p2::OutputStream), String> {
    let socket = wasi::sockets::tcp_create_socket::create_tcp_socket(p2::IpAddressFamily::Ipv4)
        .map_err(error)?;
    socket
        .start_connect(&p2::instance_network(), p2_loopback(port))
        .map_err(error)?;
    socket.subscribe().block();
    let (input, output) = socket.finish_connect().map_err(error)?;
    Ok((socket, input, output))
}

enum Slot {
    P3Tcp {
        receive: Option<p3::StreamReader<u8>>,
        receive_result: Option<p3::FutureReader<Result<(), p3::ErrorCode>>>,
        send: Option<p3::StreamWriter<u8>>,
        send_result: Option<p3::FutureReader<Result<(), p3::ErrorCode>>>,
        socket: p3::TcpSocket,
    },
    P3Udp {
        socket: p3::UdpSocket,
    },
    // The streams come before their socket so that they are dropped first.
    P2Tcp {
        input: p2::InputStream,
        output: p2::OutputStream,
        socket: p2::TcpSocket,
    },
    P2Udp {
        incoming: p2::IncomingDatagramStream,
        outgoing: p2::OutgoingDatagramStream,
        _socket: p2::UdpSocket,
    },
}

#[agent_definition(snapshotting = "enabled")]
pub trait SocketSession {
    fn new(name: String) -> Self;

    /// Connects a P3 TCP socket, takes its receive stream and keeps both in `slot`.
    async fn p3_tcp_connect(&self, slot: String, port: u16) -> Result<(), String>;
    /// Sends `data` on the P3 TCP socket in `slot` and closes its send stream.
    async fn p3_tcp_send(&self, slot: String, data: Vec<u8>) -> Result<(), String>;
    /// Opens the send stream of the P3 TCP socket in `slot` and keeps it open in the slot.
    fn p3_tcp_open_send(&self, slot: String) -> Result<(), String>;
    /// Writes `data` to the send stream kept in `slot`. Returns the number of bytes the host
    /// accepted.
    async fn p3_tcp_write(&self, slot: String, data: Vec<u8>) -> Result<u32, String>;
    /// Sends `first`, waits `pause_ms` on the monotonic clock, sends `second`, closes the send
    /// stream and returns its result.
    async fn p3_tcp_send_with_pause(
        &self,
        slot: String,
        first: Vec<u8>,
        pause_ms: u64,
        second: Vec<u8>,
    ) -> Result<(), String>;
    /// Connects a P3 TCP socket and drops it, in a read-only method.
    #[read_only]
    async fn p3_tcp_connect_read_only(&self, port: u16) -> Result<(), String>;
    /// Reads up to `len` bytes from the receive stream in `slot`, and records them. When the
    /// stream ends first, returns the error of its result.
    async fn p3_tcp_receive(&self, slot: String, len: u32) -> Result<Vec<u8>, String>;
    /// Reads `len` bytes from each of two receive streams, alternating between them one byte at
    /// a time and starting with `first`.
    async fn p3_tcp_receive_interleaved(
        &self,
        first: String,
        second: String,
        len: u32,
    ) -> Result<(Vec<u8>, Vec<u8>), String>;
    /// Connects, sends `request`, closes the send stream and reads until the peer closes, in one
    /// invocation.
    async fn p3_tcp_conversation(&self, port: u16, request: Vec<u8>) -> Result<Vec<u8>, String>;
    fn p3_tcp_addresses(&self, slot: String) -> Result<(String, String), String>;

    /// Binds a P3 UDP socket on `127.0.0.1:port` and keeps it in `slot`.
    fn p3_udp_bind(&self, slot: String, port: u16) -> Result<(), String>;
    fn p3_udp_connect(&self, slot: String, port: u16) -> Result<(), String>;
    /// Sends `data` to `127.0.0.1:port`, or to the connected peer when `port` is `None`.
    async fn p3_udp_send(
        &self,
        slot: String,
        port: Option<u16>,
        data: Vec<u8>,
    ) -> Result<(), String>;
    /// Receives one datagram. Returns its data and source address.
    async fn p3_udp_receive(&self, slot: String) -> Result<(Vec<u8>, String), String>;
    fn p3_udp_local_address(&self, slot: String) -> Result<String, String>;

    /// Connects a P2 TCP socket and keeps it with its streams in `slot`.
    fn p2_tcp_connect(&self, slot: String, port: u16) -> Result<(), String>;
    fn p2_tcp_local_address(&self, slot: String) -> Result<String, String>;
    fn p2_tcp_send(&self, slot: String, data: Vec<u8>) -> Result<(), String>;
    fn p2_tcp_receive(&self, slot: String, len: u32) -> Result<Vec<u8>, String>;
    /// Connects a P2 TCP socket and drops it, in a read-only method.
    #[read_only]
    fn p2_tcp_connect_read_only(&self, port: u16) -> Result<(), String>;
    /// Connects, sends `request` and reads `len` bytes of the answer, in one invocation.
    fn p2_tcp_conversation(&self, port: u16, request: Vec<u8>, len: u32)
    -> Result<Vec<u8>, String>;

    /// Binds a P2 UDP socket on `127.0.0.1:port`, opens its unconnected streams and keeps them in
    /// `slot`.
    fn p2_udp_bind(&self, slot: String, port: u16) -> Result<(), String>;
    fn p2_udp_send(&self, slot: String, port: u16, data: Vec<u8>) -> Result<(), String>;
    fn p2_udp_receive(&self, slot: String) -> Result<(Vec<u8>, String), String>;

    /// The bytes and datagrams the agent received so far, per slot, in the order they arrived.
    fn received(&self) -> Vec<(String, Vec<u8>)>;
    /// Drops the socket in `slot` and its streams.
    fn close(&self, slot: String) -> bool;
}

pub struct SocketSessionImpl {
    slots: RefCell<HashMap<String, Slot>>,
    received: RefCell<Vec<(String, Vec<u8>)>>,
}

impl SocketSessionImpl {
    fn record(&self, slot: &str, data: &[u8]) {
        if !data.is_empty() {
            self.received
                .borrow_mut()
                .push((slot.to_string(), data.to_vec()));
        }
    }

    fn take_p3_receive(&self, slot: &str) -> Result<p3::StreamReader<u8>, String> {
        match self.slots.borrow_mut().get_mut(slot) {
            Some(Slot::P3Tcp { receive, .. }) => receive
                .take()
                .ok_or_else(|| format!("slot {slot} has no receive stream")),
            _ => Err(format!("slot {slot} holds no P3 TCP socket")),
        }
    }

    fn put_p3_receive(&self, slot: &str, stream: p3::StreamReader<u8>) {
        if let Some(Slot::P3Tcp { receive, .. }) = self.slots.borrow_mut().get_mut(slot) {
            *receive = Some(stream);
        }
    }

    /// Awaits the result of the ended receive stream in `slot`.
    async fn p3_receive_result(&self, slot: &str) -> Result<(), String> {
        let result = match self.slots.borrow_mut().get_mut(slot) {
            Some(Slot::P3Tcp { receive_result, .. }) => receive_result.take(),
            _ => None,
        };
        match result {
            Some(result) => result.await.map_err(error),
            None => Err(format!("slot {slot} has no receive result")),
        }
    }

    fn with_slot<T>(
        &self,
        slot: &str,
        f: impl FnOnce(&Slot) -> Result<T, String>,
    ) -> Result<T, String> {
        let slots = self.slots.borrow();
        let value = slots
            .get(slot)
            .ok_or_else(|| format!("slot {slot} is empty"))?;
        f(value)
    }

    fn insert(&self, slot: String, value: Slot) {
        self.slots.borrow_mut().insert(slot, value);
    }
}

#[agent_implementation]
impl SocketSession for SocketSessionImpl {
    fn new(_name: String) -> Self {
        Self {
            slots: RefCell::new(HashMap::new()),
            received: RefCell::new(Vec::new()),
        }
    }

    async fn p3_tcp_connect(&self, slot: String, port: u16) -> Result<(), String> {
        let socket = p3::TcpSocket::create(p3::IpAddressFamily::Ipv4).map_err(error)?;
        socket.connect(p3_loopback(port)).await.map_err(error)?;
        let (receive, receive_result) = socket.receive();
        self.insert(
            slot,
            Slot::P3Tcp {
                receive: Some(receive),
                receive_result: Some(receive_result),
                send: None,
                send_result: None,
                socket,
            },
        );
        Ok(())
    }

    async fn p3_tcp_send(&self, slot: String, data: Vec<u8>) -> Result<(), String> {
        let value = self
            .slots
            .borrow_mut()
            .remove(&slot)
            .ok_or_else(|| format!("slot {slot} is empty"))?;
        let result = match &value {
            Slot::P3Tcp { socket, .. } => p3_send(socket, data).await,
            _ => Err(format!("slot {slot} holds no P3 TCP socket")),
        };
        self.insert(slot, value);
        result
    }

    fn p3_tcp_open_send(&self, slot: String) -> Result<(), String> {
        match self.slots.borrow_mut().get_mut(&slot) {
            Some(Slot::P3Tcp {
                socket,
                send,
                send_result,
                ..
            }) => {
                let (writer, reader) = p3::wit_stream::new::<u8>();
                *send_result = Some(socket.send(reader));
                *send = Some(writer);
                Ok(())
            }
            _ => Err(format!("slot {slot} holds no P3 TCP socket")),
        }
    }

    async fn p3_tcp_write(&self, slot: String, data: Vec<u8>) -> Result<u32, String> {
        let mut writer = match self.slots.borrow_mut().get_mut(&slot) {
            Some(Slot::P3Tcp { send, .. }) => send
                .take()
                .ok_or_else(|| format!("slot {slot} has no send stream"))?,
            _ => return Err(format!("slot {slot} holds no P3 TCP socket")),
        };
        let len = data.len();
        let remaining = writer.write_all(data).await;
        if let Some(Slot::P3Tcp { send, .. }) = self.slots.borrow_mut().get_mut(&slot) {
            *send = Some(writer);
        }
        Ok((len - remaining.len()) as u32)
    }

    async fn p3_tcp_send_with_pause(
        &self,
        slot: String,
        first: Vec<u8>,
        pause_ms: u64,
        second: Vec<u8>,
    ) -> Result<(), String> {
        let value = self
            .slots
            .borrow_mut()
            .remove(&slot)
            .ok_or_else(|| format!("slot {slot} is empty"))?;
        let result = match &value {
            Slot::P3Tcp { socket, .. } => {
                let (mut writer, reader) = p3::wit_stream::new::<u8>();
                let result = socket.send(reader);
                let mut remaining = writer.write_all(first).await;
                golem_rust::wasip3::clocks::monotonic_clock::wait_for(pause_ms * 1_000_000).await;
                remaining.extend(writer.write_all(second).await);
                drop(writer);
                let sent = result.await.map_err(error);
                if remaining.is_empty() {
                    sent
                } else {
                    Err(format!(
                        "{} bytes were not accepted: {sent:?}",
                        remaining.len()
                    ))
                }
            }
            _ => Err(format!("slot {slot} holds no P3 TCP socket")),
        };
        self.insert(slot, value);
        result
    }

    async fn p3_tcp_connect_read_only(&self, port: u16) -> Result<(), String> {
        let socket = p3::TcpSocket::create(p3::IpAddressFamily::Ipv4).map_err(error)?;
        socket.connect(p3_loopback(port)).await.map_err(error)
    }

    async fn p3_tcp_receive(&self, slot: String, len: u32) -> Result<Vec<u8>, String> {
        let mut stream = self.take_p3_receive(&slot)?;
        let result = p3_read(&mut stream, len as usize).await;
        self.put_p3_receive(&slot, stream);
        let (data, ended) = result?;
        self.record(&slot, &data);
        if ended {
            self.p3_receive_result(&slot).await?;
        }
        Ok(data)
    }

    async fn p3_tcp_receive_interleaved(
        &self,
        first: String,
        second: String,
        len: u32,
    ) -> Result<(Vec<u8>, Vec<u8>), String> {
        let mut first_stream = self.take_p3_receive(&first)?;
        let mut second_stream = match self.take_p3_receive(&second) {
            Ok(stream) => stream,
            Err(error) => {
                self.put_p3_receive(&first, first_stream);
                return Err(error);
            }
        };
        let mut first_data = Vec::new();
        let mut second_data = Vec::new();
        let mut result = Ok(());
        for _ in 0..len {
            for (slot, stream, data) in [
                (&first, &mut first_stream, &mut first_data),
                (&second, &mut second_stream, &mut second_data),
            ] {
                match p3_read(stream, 1).await {
                    Ok((byte, _)) => {
                        self.record(slot, &byte);
                        data.extend_from_slice(&byte);
                    }
                    Err(error) => result = Err(error),
                }
            }
        }
        self.put_p3_receive(&first, first_stream);
        self.put_p3_receive(&second, second_stream);
        result.map(|()| (first_data, second_data))
    }

    async fn p3_tcp_conversation(&self, port: u16, request: Vec<u8>) -> Result<Vec<u8>, String> {
        let socket = p3::TcpSocket::create(p3::IpAddressFamily::Ipv4).map_err(error)?;
        socket.connect(p3_loopback(port)).await.map_err(error)?;
        let (mut receive, result) = socket.receive();
        p3_send(&socket, request)
            .await
            .map_err(|error| format!("send: {error}"))?;
        let (response, _) = p3_read(&mut receive, usize::MAX).await?;
        drop(receive);
        result
            .await
            .map_err(|error| format!("receive: {error:?}"))?;
        self.record("conversation", &response);
        Ok(response)
    }

    fn p3_tcp_addresses(&self, slot: String) -> Result<(String, String), String> {
        self.with_slot(&slot, |value| match value {
            Slot::P3Tcp { socket, .. } => Ok((
                p3_address(socket.get_local_address().map_err(error)?),
                p3_address(socket.get_remote_address().map_err(error)?),
            )),
            _ => Err(format!("slot {slot} holds no P3 TCP socket")),
        })
    }

    fn p3_udp_bind(&self, slot: String, port: u16) -> Result<(), String> {
        let socket = p3::UdpSocket::create(p3::IpAddressFamily::Ipv4).map_err(error)?;
        socket.bind(p3_loopback(port)).map_err(error)?;
        self.insert(slot, Slot::P3Udp { socket });
        Ok(())
    }

    fn p3_udp_connect(&self, slot: String, port: u16) -> Result<(), String> {
        self.with_slot(&slot, |value| match value {
            Slot::P3Udp { socket } => socket.connect(p3_loopback(port)).map_err(error),
            _ => Err(format!("slot {slot} holds no P3 UDP socket")),
        })
    }

    async fn p3_udp_send(
        &self,
        slot: String,
        port: Option<u16>,
        data: Vec<u8>,
    ) -> Result<(), String> {
        let socket = match self.slots.borrow_mut().remove(&slot) {
            Some(Slot::P3Udp { socket }) => socket,
            Some(other) => {
                self.slots.borrow_mut().insert(slot.clone(), other);
                return Err(format!("slot {slot} holds no P3 UDP socket"));
            }
            None => return Err(format!("slot {slot} is empty")),
        };
        let result = socket
            .send(data, port.map(p3_loopback))
            .await
            .map_err(error);
        self.insert(slot, Slot::P3Udp { socket });
        result
    }

    async fn p3_udp_receive(&self, slot: String) -> Result<(Vec<u8>, String), String> {
        let socket = match self.slots.borrow_mut().remove(&slot) {
            Some(Slot::P3Udp { socket }) => socket,
            Some(other) => {
                self.slots.borrow_mut().insert(slot.clone(), other);
                return Err(format!("slot {slot} holds no P3 UDP socket"));
            }
            None => return Err(format!("slot {slot} is empty")),
        };
        let result = socket.receive().await.map_err(error);
        self.insert(slot.clone(), Slot::P3Udp { socket });
        let (data, source) = result?;
        self.record(&slot, &data);
        Ok((data, p3_address(source)))
    }

    fn p3_udp_local_address(&self, slot: String) -> Result<String, String> {
        self.with_slot(&slot, |value| match value {
            Slot::P3Udp { socket } => socket.get_local_address().map(p3_address).map_err(error),
            _ => Err(format!("slot {slot} holds no P3 UDP socket")),
        })
    }

    fn p2_tcp_connect(&self, slot: String, port: u16) -> Result<(), String> {
        let (socket, input, output) = p2_tcp_connect(port)?;
        self.insert(
            slot,
            Slot::P2Tcp {
                input,
                output,
                socket,
            },
        );
        Ok(())
    }

    fn p2_tcp_local_address(&self, slot: String) -> Result<String, String> {
        self.with_slot(&slot, |value| match value {
            Slot::P2Tcp { socket, .. } => socket.local_address().map(p2_address).map_err(error),
            _ => Err(format!("slot {slot} holds no P2 TCP socket")),
        })
    }

    fn p2_tcp_send(&self, slot: String, data: Vec<u8>) -> Result<(), String> {
        self.with_slot(&slot, |value| match value {
            Slot::P2Tcp { output, .. } => p2_write(output, &data),
            _ => Err(format!("slot {slot} holds no P2 TCP socket")),
        })
    }

    fn p2_tcp_receive(&self, slot: String, len: u32) -> Result<Vec<u8>, String> {
        let data = self.with_slot(&slot, |value| match value {
            Slot::P2Tcp { input, .. } => p2_read(input, len as usize),
            _ => Err(format!("slot {slot} holds no P2 TCP socket")),
        })?;
        self.record(&slot, &data);
        Ok(data)
    }

    fn p2_tcp_connect_read_only(&self, port: u16) -> Result<(), String> {
        let (_socket, _input, _output) = p2_tcp_connect(port)?;
        Ok(())
    }

    fn p2_tcp_conversation(
        &self,
        port: u16,
        request: Vec<u8>,
        len: u32,
    ) -> Result<Vec<u8>, String> {
        let (_socket, input, output) = p2_tcp_connect(port)?;
        p2_write(&output, &request)?;
        let response = p2_read(&input, len as usize)?;
        self.record("conversation", &response);
        Ok(response)
    }

    fn p2_udp_bind(&self, slot: String, port: u16) -> Result<(), String> {
        let socket = wasi::sockets::udp_create_socket::create_udp_socket(p2::IpAddressFamily::Ipv4)
            .map_err(error)?;
        socket
            .start_bind(&p2::instance_network(), p2_loopback(port))
            .map_err(error)?;
        socket.subscribe().block();
        socket.finish_bind().map_err(error)?;
        let (incoming, outgoing) = socket.stream(None).map_err(error)?;
        self.insert(
            slot,
            Slot::P2Udp {
                incoming,
                outgoing,
                _socket: socket,
            },
        );
        Ok(())
    }

    fn p2_udp_send(&self, slot: String, port: u16, data: Vec<u8>) -> Result<(), String> {
        use wasi::sockets::udp::OutgoingDatagram;
        self.with_slot(&slot, |value| match value {
            Slot::P2Udp { outgoing, .. } => {
                outgoing.subscribe().block();
                outgoing.check_send().map_err(error)?;
                let sent = outgoing
                    .send(&[OutgoingDatagram {
                        data,
                        remote_address: Some(p2_loopback(port)),
                    }])
                    .map_err(error)?;
                if sent == 1 {
                    Ok(())
                } else {
                    Err(format!("{sent} datagrams sent"))
                }
            }
            _ => Err(format!("slot {slot} holds no P2 UDP socket")),
        })
    }

    fn p2_udp_receive(&self, slot: String) -> Result<(Vec<u8>, String), String> {
        let (data, source) = self.with_slot(&slot, |value| match value {
            Slot::P2Udp { incoming, .. } => loop {
                incoming.subscribe().block();
                let mut datagrams = incoming.receive(1).map_err(error)?;
                if let Some(datagram) = datagrams.pop() {
                    break Ok((datagram.data, p2_address(datagram.remote_address)));
                }
            },
            _ => Err(format!("slot {slot} holds no P2 UDP socket")),
        })?;
        self.record(&slot, &data);
        Ok((data, source))
    }

    fn received(&self) -> Vec<(String, Vec<u8>)> {
        self.received.borrow().clone()
    }

    fn close(&self, slot: String) -> bool {
        self.slots.borrow_mut().remove(&slot).is_some()
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        Ok(Vec::new())
    }

    async fn load_snapshot(
        _bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Ok(Self::new(String::new()))
    }
}
