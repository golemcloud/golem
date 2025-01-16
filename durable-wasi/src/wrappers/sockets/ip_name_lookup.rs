// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::bindings::exports::wasi::sockets::ip_name_lookup::{
    ErrorCode, IpAddress, NetworkBorrow, Pollable, ResolveAddressStream,
};
use crate::bindings::golem::durability::durability::{observe_function_call, DurableFunctionType};
use crate::bindings::wasi::sockets::ip_name_lookup::resolve_addresses;
use crate::durability::Durability;
use crate::wrappers::io::poll::WrappedPollable;
use crate::wrappers::sockets::network::WrappedNetwork;
use crate::wrappers::{SerializableError, SerializableIpAddresses};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::mem::transmute;

impl From<crate::bindings::wasi::sockets::ip_name_lookup::ErrorCode> for ErrorCode {
    fn from(value: crate::bindings::wasi::sockets::ip_name_lookup::ErrorCode) -> Self {
        unsafe { transmute(value) }
    }
}

pub struct WrappedResolveAddressStream {
    pub addresses: RefCell<VecDeque<IpAddress>>,
}

impl WrappedResolveAddressStream {
    pub fn new(addresses: Vec<IpAddress>) -> Self {
        Self {
            addresses: RefCell::new(VecDeque::from(addresses)),
        }
    }
}

impl crate::bindings::exports::wasi::sockets::ip_name_lookup::GuestResolveAddressStream
    for WrappedResolveAddressStream
{
    fn resolve_next_address(&self) -> Result<Option<IpAddress>, ErrorCode> {
        observe_function_call(
            "sockets::ip_name_lookup::resolve_address_stream",
            "resolve_next_address",
        );
        let result = self.addresses.borrow_mut().pop_front();
        Ok(result)
    }

    fn subscribe(&self) -> Pollable {
        observe_function_call(
            "sockets::ip_name_lookup::resolve_address_stream",
            "subscribe",
        );
        Pollable::new(WrappedPollable::Ready)
    }
}

impl Drop for WrappedResolveAddressStream {
    fn drop(&mut self) {
        observe_function_call("sockets::ip_name_lookup::resolve_address_stream", "drop");
    }
}

impl crate::bindings::exports::wasi::sockets::ip_name_lookup::Guest for crate::Component {
    type ResolveAddressStream = WrappedResolveAddressStream;

    fn resolve_addresses(
        network: NetworkBorrow<'_>,
        name: String,
    ) -> Result<ResolveAddressStream, ErrorCode> {
        let durability = Durability::<SerializableIpAddresses, SerializableError>::new(
            "sockets::ip_name_lookup",
            "resolve_addresses",
            DurableFunctionType::ReadRemote,
        );

        let addresses = if durability.is_live() {
            let result = resolve_and_drain_addresses(network, &name);
            durability.persist(name, result)
        } else {
            durability.replay()
        }?;

        let stream = ResolveAddressStream::new(WrappedResolveAddressStream::new(addresses));
        Ok(stream)
    }
}

fn resolve_and_drain_addresses(
    network: NetworkBorrow<'_>,
    name: &str,
) -> Result<Vec<IpAddress>, ErrorCode> {
    let network = &network.get::<WrappedNetwork>().network;
    let stream = resolve_addresses(network, name)?;
    let addresses = drain_resolve_address_stream(stream)?;
    Ok(addresses)
}

fn drain_resolve_address_stream(
    stream: crate::bindings::wasi::sockets::ip_name_lookup::ResolveAddressStream,
) -> Result<Vec<IpAddress>, ErrorCode> {
    let mut addresses = Vec::new();

    let pollable = stream.subscribe();
    pollable.block();

    while let Some(address) = stream.resolve_next_address()? {
        let address = unsafe { transmute(address) };
        addresses.push(address);
        pollable.block();
    }

    Ok(addresses)
}
