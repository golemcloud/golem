//! Models idle endpoint collection at different points after snapshot restoration.
//! Both executions perform exactly one completed read and the same durable calls.

use golem_rust::{PromiseId, agent_definition, agent_implementation};

#[agent_definition(snapshotting = "enabled")]
pub trait TcpGc {
    fn new(name: String, early: bool) -> Self;
    fn prepare(&self) -> PromiseId;
    async fn run(&mut self, port: u16, gate: PromiseId) -> String;
    fn inspect(&self) -> (bool, String);
}

struct TcpGcImpl {
    early: bool,
    restored: bool,
    bytes: String,
}

#[agent_implementation]
impl TcpGc for TcpGcImpl {
    fn new(_name: String, early: bool) -> Self {
        Self {
            early,
            restored: false,
            bytes: String::new(),
        }
    }

    fn prepare(&self) -> PromiseId {
        golem_rust::create_promise()
    }

    async fn run(&mut self, port: u16, gate: PromiseId) -> String {
        use golem_rust::wasip3::sockets::types::{
            IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
        };
        use golem_rust::wasip3::wit_bindgen::StreamResult;

        let socket = TcpSocket::create(IpAddressFamily::Ipv4).unwrap();
        socket
            .connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port,
            }))
            .await
            .unwrap();
        let (mut reader, completion) = socket.receive();
        let (status, bytes) = reader.read(Vec::with_capacity(1)).await;
        assert!(matches!(status, StreamResult::Complete(1)));
        self.bytes = String::from_utf8(bytes).unwrap();
        let mut unreachable = Some(reader);
        if self.early {
            drop(unreachable.take());
        }
        let _ = wasi::random::random::get_random_u64();
        drop(unreachable);
        completion.await.unwrap();
        drop(socket);
        golem_rust::get_promise(&gate).get().await;
        self.bytes.clone()
    }

    fn inspect(&self) -> (bool, String) {
        (self.restored, self.bytes.clone())
    }

    async fn save_snapshot(&self) -> Result<Vec<u8>, String> {
        let mut bytes = vec![u8::from(self.early)];
        bytes.extend_from_slice(self.bytes.as_bytes());
        Ok(bytes)
    }

    async fn load_snapshot(
        bytes: Vec<u8>,
        _context: golem_rust::agentic::SnapshotRestoreContext,
    ) -> Result<Self, String> {
        Ok(Self {
            early: bytes[0] == 0,
            restored: true,
            bytes: String::from_utf8(bytes[1..].to_vec()).map_err(|error| error.to_string())?,
        })
    }
}
