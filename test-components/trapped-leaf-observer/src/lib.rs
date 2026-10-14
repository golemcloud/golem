use golem_rust::golem_agentic::golem::tool::host::ToolRpc;
use golem_rust::wasip3::sockets::types::{
    IpAddressFamily, IpSocketAddress, Ipv4SocketAddress, TcpSocket,
};
use golem_rust::{IntoSchema, IntoTypedSchemaValue, agent_definition, agent_implementation};

#[derive(IntoSchema)]
struct MiddlewareInput {
    value: String,
}

#[agent_definition]
pub trait TrappedLeafObserver {
    fn new(name: String) -> Self;

    async fn nested_trap_while_primary_receives(&self);
}

struct TrappedLeafObserverImpl;

#[agent_implementation]
impl TrappedLeafObserver for TrappedLeafObserverImpl {
    fn new(_name: String) -> Self {
        Self
    }

    async fn nested_trap_while_primary_receives(&self) {
        let port = std::env::var("PRIMARY_SILENT_TCP_PORT")
            .expect("primary silent TCP port is configured")
            .parse()
            .expect("primary silent TCP port is valid");
        let socket = TcpSocket::create(IpAddressFamily::Ipv4).expect("create primary socket");
        socket
            .connect(IpSocketAddress::Ipv4(Ipv4SocketAddress {
                address: (127, 0, 0, 1),
                port,
            }))
            .await
            .expect("connect primary silent socket");

        let input = MiddlewareInput {
            value: "fail-after-parent".to_string(),
        }
        .into_typed_schema_value()
        .expect("encode middleware input");
        ToolRpc::create("middleware-probe")
            .expect("create middleware tool")
            .invoke(
                &["apply".to_string()],
                golem_rust::encode_typed_schema_value(&input)
                    .expect("encode middleware wire input"),
                None,
            )
            .expect("admit detached middleware invocation");

        let (mut bytes, _received) = socket.receive();
        let result = bytes.read(Vec::with_capacity(1)).await;
        panic!(
            "silent primary receive unexpectedly returned: {:?}",
            result.0
        );
    }
}
