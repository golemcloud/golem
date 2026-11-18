// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::durable_host::authorization::targets::dns_target;
use crate::durable_host::concurrent::{OnDelivered, reserve_invocation_id_before_await};
use crate::durable_host::durability::{ClassifiedHostError, GuestTaskContext, HostFailureKind};
use crate::durable_host::p3::{
    DurableP3, DurableP3View, durable_worker_ctx, run_deniable_access_classified, wasi_sockets_view,
};
use crate::workerctx::WorkerCtx;
use golem_common::model::RetryContext;
use golem_common::model::oplog::host_functions::P3SocketsIpNameLookupResolveAddresses;
use golem_common::model::oplog::types::{SerializableIpAddresses, SerializableP3IpNameLookupError};
use golem_common::model::oplog::{
    DurableFunctionType, HostRequestP3SocketsResolveName, HostResponseP3SocketsResolveName,
};
use std::net::IpAddr;
use wasmtime::component::Accessor;
use wasmtime_wasi::p3::bindings::sockets::{ip_name_lookup, types};
use wasmtime_wasi::sockets::WasiSockets;
impl<Ctx: WorkerCtx> ip_name_lookup::Host for DurableP3View<'_, Ctx> {}

impl<U: Send + 'static, Ctx: WorkerCtx> ip_name_lookup::HostWithStore<U> for DurableP3<Ctx> {
    async fn resolve_addresses(
        store: &Accessor<U, Self>,
        name: String,
    ) -> wasmtime::Result<Result<Vec<types::IpAddress>, ip_name_lookup::ErrorCode>> {
        reserve_invocation_id_before_await(store, durable_worker_ctx::<Ctx, U>)?;
        let custom_invocation = store.guest_task_context::<GuestTaskContext>()?;
        // The link waits for the guest to receive the result, as the status fold reads it from
        // the delivery marker: a result that the guest discards links nothing.
        let links = store.with(|mut access| {
            let ctx = durable_worker_ctx::<Ctx, U>(access.data_mut());
            ctx.lookup_links(custom_invocation.as_deref())
                .map(|links| links.then(|| ctx.shared_resolved_names()))
        })?;
        let targets = dns_target(&name)
            .map(|target| vec![target])
            .map_err(|error| error.to_string());

        // Worker-level retry classification, mirroring the P2 `resolve_addresses` path: a
        // transient resolver failure raises a retry trap (the worker goes to `Retrying` per its
        // retry policy and re-executes the lookup from the abandoned `Start` on replay) instead
        // of surfacing as a guest-visible error value. Permanent failures — and transient ones
        // whose retry budget is exhausted — are recorded and returned to the guest, which is also
        // what a recorded error replays as.
        let response = run_deniable_access_classified::<
            _,
            _,
            Ctx,
            P3SocketsIpNameLookupResolveAddresses,
            _,
            _,
        >(
            store,
            HostRequestP3SocketsResolveName { name: name.clone() },
            DurableFunctionType::ReadRemote,
            targets,
            |response| {
                response
                    .result
                    .as_ref()
                    .err()
                    .map(|error| ClassifiedHostError {
                        kind: classify_p3_ip_name_lookup_error(error),
                        message: format!("DNS resolution of '{name}' failed: {error:?}"),
                    })
            },
            RetryContext::dns(&name),
            |response| -> OnDelivered {
                match (&links, &response.result) {
                    (Some(names), Ok(addresses)) => {
                        let names = names.clone();
                        let name = name.clone();
                        let addresses: Vec<IpAddr> =
                            addresses.0.iter().cloned().map(IpAddr::from).collect();
                        Box::new(move || names.link(&name, addresses))
                    }
                    _ => Box::new(|| {}),
                }
            },
            || async {
                let sockets = store.with_getter::<WasiSockets>(wasi_sockets_view::<Ctx, U>);
                let result = <WasiSockets as ip_name_lookup::HostWithStore<U>>::resolve_addresses(
                    &sockets,
                    name.clone(),
                )
                .await?;

                Ok(HostResponseP3SocketsResolveName {
                    result: result
                        .map(SerializableIpAddresses::from)
                        .map_err(Into::into),
                })
            },
        )
        .await?;

        Ok(response
            .result
            .map(Vec::<types::IpAddress>::from)
            .map_err(Into::into))
    }
}

/// Classifies P3 DNS lookup failures for worker-level retry, mirroring the P2
/// `resolve_addresses` classification: resolver failures that cannot succeed on a retry
/// (`NameUnresolvable`, `PermanentResolverFailure`, `AccessDenied`) are permanent, everything
/// else is transient. Unlike P2's generic network error code, the P3 lookup error also carries
/// `InvalidArgument` (an unparseable name), which is deterministic and therefore also permanent.
pub(super) fn classify_p3_ip_name_lookup_error(
    error: &SerializableP3IpNameLookupError,
) -> HostFailureKind {
    match error {
        SerializableP3IpNameLookupError::AccessDenied
        | SerializableP3IpNameLookupError::InvalidArgument
        | SerializableP3IpNameLookupError::NameUnresolvable
        | SerializableP3IpNameLookupError::PermanentResolverFailure => HostFailureKind::Permanent,
        SerializableP3IpNameLookupError::TemporaryResolverFailure
        | SerializableP3IpNameLookupError::Other(_) => HostFailureKind::Transient,
    }
}
