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

use crate::bindings::exports::wasi::sockets::udp::{
    ErrorCode, IncomingDatagram, IncomingDatagramStream, IpAddressFamily, IpSocketAddress,
    NetworkBorrow, OutgoingDatagram, OutgoingDatagramStream, Pollable,
};
use crate::bindings::golem::durability::durability::observe_function_call;
use crate::wrappers::io::poll::WrappedPollable;
use crate::wrappers::sockets::network::WrappedNetwork;
use std::mem::transmute;

pub struct WrappedUdpScoket {
    pub udp_socket: crate::bindings::wasi::sockets::udp::UdpSocket,
}

impl crate::bindings::exports::wasi::sockets::udp::GuestUdpSocket for WrappedUdpScoket {
    fn start_bind(
        &self,
        network: NetworkBorrow<'_>,
        local_address: IpSocketAddress,
    ) -> Result<(), ErrorCode> {
        observe_function_call("sockets::udp", "start_bind");
        let network = &network.get::<WrappedNetwork>().network;
        let local_address = unsafe { transmute(local_address) };
        self.udp_socket.start_bind(network, local_address)?;
        Ok(())
    }

    fn finish_bind(&self) -> Result<(), ErrorCode> {
        observe_function_call("sockets::udp", "finish_bind");
        self.udp_socket.finish_bind()?;
        Ok(())
    }

    fn stream(
        &self,
        remote_address: Option<IpSocketAddress>,
    ) -> Result<(IncomingDatagramStream, OutgoingDatagramStream), ErrorCode> {
        observe_function_call("sockets::udp", "stream");
        let remote_address = unsafe { transmute(remote_address) };
        let (incoming, outgoing) = self.udp_socket.stream(remote_address)?;
        Ok((
            IncomingDatagramStream::new(WrappedIncomingDatagramStream { stream: incoming }),
            OutgoingDatagramStream::new(WrappedOutgoingDatagramStream { stream: outgoing }),
        ))
    }

    fn local_address(&self) -> Result<IpSocketAddress, ErrorCode> {
        observe_function_call("sockets::udp", "local_address");
        let address = self.udp_socket.local_address()?;
        let address = unsafe { transmute(address) };
        Ok(address)
    }

    fn remote_address(&self) -> Result<IpSocketAddress, ErrorCode> {
        observe_function_call("sockets::udp", "remote_address");
        let address = self.udp_socket.remote_address()?;
        let address = unsafe { transmute(address) };
        Ok(address)
    }

    fn address_family(&self) -> IpAddressFamily {
        observe_function_call("sockets::udp", "address_family");
        let address_family = self.udp_socket.address_family();
        let address_family = unsafe { transmute(address_family) };
        address_family
    }

    fn unicast_hop_limit(&self) -> Result<u8, ErrorCode> {
        observe_function_call("sockets::udp", "unicast_hop_limit");
        let hop_limit = self.udp_socket.unicast_hop_limit()?;
        Ok(hop_limit)
    }

    fn set_unicast_hop_limit(&self, value: u8) -> Result<(), ErrorCode> {
        observe_function_call("sockets::udp", "set_unicast_hop_limit");
        self.udp_socket.set_unicast_hop_limit(value)?;
        Ok(())
    }

    fn receive_buffer_size(&self) -> Result<u64, ErrorCode> {
        observe_function_call("sockets::udp", "receive_buffer_size");
        Ok(self.udp_socket.receive_buffer_size()?)
    }

    fn set_receive_buffer_size(&self, value: u64) -> Result<(), ErrorCode> {
        observe_function_call("sockets::udp", "set_receive_buffer_size");
        self.udp_socket.set_receive_buffer_size(value)?;
        Ok(())
    }

    fn send_buffer_size(&self) -> Result<u64, ErrorCode> {
        observe_function_call("sockets::udp", "send_buffer_size");
        Ok(self.udp_socket.send_buffer_size()?)
    }

    fn set_send_buffer_size(&self, value: u64) -> Result<(), ErrorCode> {
        observe_function_call("sockets::udp", "set_send_buffer_size");
        self.udp_socket.set_send_buffer_size(value)?;
        Ok(())
    }

    fn subscribe(&self) -> Pollable {
        observe_function_call("sockets::udp", "subscribe");
        let pollable = self.udp_socket.subscribe();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }
}

impl Drop for WrappedUdpScoket {
    fn drop(&mut self) {
        observe_function_call("sockets::udp", "drop");
    }
}

pub struct WrappedIncomingDatagramStream {
    pub stream: crate::bindings::wasi::sockets::udp::IncomingDatagramStream,
}

impl crate::bindings::exports::wasi::sockets::udp::GuestIncomingDatagramStream
    for WrappedIncomingDatagramStream
{
    fn receive(&self, max_results: u64) -> Result<Vec<IncomingDatagram>, ErrorCode> {
        observe_function_call("sockets::udp", "receive");
        let result = self.stream.receive(max_results)?;
        let result = unsafe { transmute(result) };
        Ok(result)
    }

    fn subscribe(&self) -> Pollable {
        observe_function_call("sockets::udp", "subscribe");
        let pollable = self.stream.subscribe();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }
}

impl Drop for WrappedIncomingDatagramStream {
    fn drop(&mut self) {
        observe_function_call("sockets::udp", "drop");
    }
}

pub struct WrappedOutgoingDatagramStream {
    pub stream: crate::bindings::wasi::sockets::udp::OutgoingDatagramStream,
}

impl crate::bindings::exports::wasi::sockets::udp::GuestOutgoingDatagramStream
    for WrappedOutgoingDatagramStream
{
    fn check_send(&self) -> Result<u64, ErrorCode> {
        observe_function_call("sockets::udp", "check_send");
        let result = self.stream.check_send()?;
        Ok(result)
    }

    fn send(&self, datagrams: Vec<OutgoingDatagram>) -> Result<u64, ErrorCode> {
        observe_function_call("sockets::udp", "send");
        let datagrams: Vec<crate::bindings::wasi::sockets::udp::OutgoingDatagram> =
            unsafe { transmute(datagrams) };
        let result = self.stream.send(&datagrams)?;
        Ok(result)
    }

    fn subscribe(&self) -> Pollable {
        observe_function_call("sockets::udp", "subscribe");
        let pollable = self.stream.subscribe();
        Pollable::new(WrappedPollable::Proxy(pollable))
    }
}

impl Drop for WrappedOutgoingDatagramStream {
    fn drop(&mut self) {
        observe_function_call("sockets::udp", "drop");
    }
}

impl crate::bindings::exports::wasi::sockets::udp::Guest for crate::Component {
    type UdpSocket = WrappedUdpScoket;
    type IncomingDatagramStream = WrappedIncomingDatagramStream;
    type OutgoingDatagramStream = WrappedOutgoingDatagramStream;
}
