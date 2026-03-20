use golem_rust::{agent_definition, agent_implementation};
use wasi::sockets::instance_network::instance_network;
use wasi::sockets::ip_name_lookup::resolve_addresses;

#[agent_definition]
pub trait Networking {
    fn new(name: String) -> Self;
    fn get(&self) -> Vec<String>;
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
            let next = resolve_stream.resolve_next_address().expect("resolve_next_address");
            if let Some(next) = next {
                result.push(format!("{:?}", next));
            } else {
                break;
            }
        }
        result
    }
}
