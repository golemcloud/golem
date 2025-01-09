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

use async_trait::async_trait;
use wasmtime::component::Resource;

use crate::durable_host::serialized::{SerializableError, SerializableIpAddresses};
use crate::durable_host::{Durability, DurableWorkerCtx};
use crate::error::GolemError;
use crate::metrics::wasm::record_host_function_call;
use crate::workerctx::WorkerCtx;
use golem_common::model::oplog::WrappedFunctionType;
use wasmtime_wasi::bindings::sockets::ip_name_lookup::{
    Host, HostResolveAddressStream, IpAddress, Network, Pollable, ResolveAddressStream,
};
use wasmtime_wasi::bindings::sockets::network::ErrorCode;
use wasmtime_wasi::{SocketError, Subscribe};

#[async_trait]
impl<Ctx: WorkerCtx> HostResolveAddressStream for DurableWorkerCtx<Ctx> {
    fn resolve_next_address(
        &mut self,
        self_: Resource<ResolveAddressStream>,
    ) -> Result<Option<IpAddress>, SocketError> {
        record_host_function_call(
            "sockets::ip_name_lookup::resolve_address_stream",
            "resolve_next_address",
        );
        HostResolveAddressStream::resolve_next_address(&mut self.as_wasi_view(), self_)
    }

    fn subscribe(
        &mut self,
        self_: Resource<ResolveAddressStream>,
    ) -> anyhow::Result<Resource<Pollable>> {
        record_host_function_call(
            "sockets::ip_name_lookup::resolve_address_stream",
            "subscribe",
        );
        HostResolveAddressStream::subscribe(&mut self.as_wasi_view(), self_)
    }

    fn drop(&mut self, rep: Resource<ResolveAddressStream>) -> anyhow::Result<()> {
        record_host_function_call("sockets::ip_name_lookup::resolve_address_stream", "drop");
        HostResolveAddressStream::drop(&mut self.as_wasi_view(), rep)
    }
}

#[async_trait]
impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {
    async fn resolve_addresses(
        &mut self,
        network: Resource<Network>,
        name: String,
    ) -> Result<Resource<ResolveAddressStream>, SocketError> {
        let durability = Durability::<Ctx, SerializableIpAddresses, SerializableError>::new(
            self,
            "sockets::ip_name_lookup",
            "resolve_addresses",
            WrappedFunctionType::ReadRemote,
        )
        .await?;

        let addresses = if durability.is_live() {
            let result = resolve_and_drain_addresses(self, network, name.clone()).await;
            durability.persist(self, name, result).await
        } else {
            durability.replay(self).await
        };

        let stream = ResolveAddressStream::Done(Ok(addresses?.into_iter()));
        Ok(self.table().push(stream)?)
    }
}

async fn resolve_and_drain_addresses<Ctx: WorkerCtx>(
    ctx: &mut DurableWorkerCtx<Ctx>,
    network: Resource<Network>,
    name: String,
) -> Result<Vec<IpAddress>, SocketError> {
    let stream = Host::resolve_addresses(&mut ctx.as_wasi_view(), network, name).await?;
    let stream = ctx.table().delete(stream)?;
    let addresses = drain_resolve_address_stream(stream).await?;
    Ok(addresses)
}

async fn drain_resolve_address_stream(
    mut stream: ResolveAddressStream,
) -> Result<Vec<IpAddress>, SocketError> {
    let mut addresses = Vec::new();

    stream.ready().await;
    match stream {
        ResolveAddressStream::Waiting(_) => return Err(ErrorCode::WouldBlock.into()), // should never happen because of ready() above
        ResolveAddressStream::Done(iter) => {
            let iter = iter?;
            for address in iter {
                addresses.push(address);
            }
        }
    }
    Ok(addresses)
}

impl From<GolemError> for SocketError {
    fn from(value: GolemError) -> Self {
        Self::trap(value)
    }
}
