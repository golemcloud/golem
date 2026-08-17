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

use crate::durable_host::DurableWorkerCtx;
use crate::durable_host::stream_transport::{RelayProducer, RelayReceiver, relay_pair};
use crate::workerctx::WorkerCtx;
use golem_schema::schema::schema_value::{QuotaTokenValuePayload, SecretValuePayload};
use golem_schema::schema::wit::wire::{
    Host, HostQuotaToken, HostSchemaValueStream, HostSchemaValueStreamWithStore, HostSecret,
    HostWithStore, SchemaValueTree, Uuid,
};
use golem_schema::schema::wit::{
    QuotaTokenHandleRep, QuotaTokenResolver, SchemaValueStreamResolver, SecretHandleRep,
    SecretResolver,
};
use golem_schema::schema::{SchemaValue, SchemaValueStream, SchemaValueStreamHandleRep};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::marker::PhantomData;
use wasmtime::StoreContextMut;
use wasmtime::component::{Accessor, HasData, Resource, StreamReader};

pub(crate) fn contains_stream(value: &SchemaValue) -> bool {
    match value {
        SchemaValue::Stream(_) => true,
        SchemaValue::Record { fields } => fields.iter().any(contains_stream),
        SchemaValue::Tuple { elements }
        | SchemaValue::List { elements }
        | SchemaValue::FixedList { elements } => elements.iter().any(contains_stream),
        SchemaValue::Variant(payload) => payload.payload.as_deref().is_some_and(contains_stream),
        SchemaValue::Map { entries } => entries
            .iter()
            .any(|(key, value)| contains_stream(key) || contains_stream(value)),
        SchemaValue::Option { inner } => inner.as_deref().is_some_and(contains_stream),
        SchemaValue::Result(payload) => match payload {
            golem_schema::schema::schema_value::ResultValuePayload::Ok { value }
            | golem_schema::schema::schema_value::ResultValuePayload::Err { value } => {
                value.as_deref().is_some_and(contains_stream)
            }
        },
        SchemaValue::Union(payload) => contains_stream(&payload.body),
        _ => false,
    }
}

pub struct StoreValueResolver<'a, 'store, Ctx: WorkerCtx> {
    store: &'a mut StoreContextMut<'store, Ctx>,
}

impl<'a, 'store, Ctx: WorkerCtx> StoreValueResolver<'a, 'store, Ctx> {
    pub fn new(store: &'a mut StoreContextMut<'store, Ctx>) -> Self {
        Self { store }
    }
}

impl<Ctx: WorkerCtx> QuotaTokenResolver for StoreValueResolver<'_, '_, Ctx> {
    type Error = WorkerExecutorError;

    fn snapshot_handle(
        &mut self,
        handle: Resource<QuotaTokenHandleRep>,
    ) -> Result<QuotaTokenValuePayload, Self::Error> {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .snapshot_handle(handle)
    }

    fn handle_from_snapshot(
        &mut self,
        snapshot: &QuotaTokenValuePayload,
    ) -> Result<Resource<QuotaTokenHandleRep>, Self::Error> {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .handle_from_snapshot(snapshot)
    }

    fn drop_handle(&mut self, handle: Resource<QuotaTokenHandleRep>) {
        self.store.data_mut().durable_ctx_mut().drop_handle(handle)
    }
}

impl<Ctx: WorkerCtx> SecretResolver for StoreValueResolver<'_, '_, Ctx> {
    type Error = WorkerExecutorError;

    fn snapshot_secret_handle(
        &mut self,
        handle: Resource<SecretHandleRep>,
    ) -> Result<SecretValuePayload, Self::Error> {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .snapshot_secret_handle(handle)
    }

    fn secret_handle_from_snapshot(
        &mut self,
        snapshot: &SecretValuePayload,
    ) -> Result<Resource<SecretHandleRep>, Self::Error> {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .secret_handle_from_snapshot(snapshot)
    }

    fn drop_secret_handle(&mut self, handle: Resource<SecretHandleRep>) {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .drop_secret_handle(handle)
    }
}

impl<Ctx: WorkerCtx> SchemaValueStreamResolver for StoreValueResolver<'_, '_, Ctx> {
    type Error = WorkerExecutorError;

    fn handle_from_stream(
        &mut self,
        stream: SchemaValueStream,
    ) -> Result<Resource<SchemaValueStreamHandleRep>, Self::Error> {
        if let Some(tracker) = self.store.data().durable_ctx().live_stream_tracker() {
            stream
                .with_host_endpoint::<RelayReceiver, _>(|receiver| receiver.attach(tracker))
                .map_err(WorkerExecutorError::runtime)?;
        }
        self.store
            .data_mut()
            .durable_ctx_mut()
            .table()
            .push(SchemaValueStreamHandleRep::new(stream))
            .map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to create schema-value-stream handle: {error}"
                ))
            })
    }

    fn stream_from_handle(
        &mut self,
        handle: Resource<SchemaValueStreamHandleRep>,
    ) -> Result<SchemaValueStream, Self::Error> {
        let stream = self
            .store
            .data_mut()
            .durable_ctx_mut()
            .table()
            .delete(handle)
            .map_err(|error| {
                WorkerExecutorError::runtime(format!("invalid schema-value-stream handle: {error}"))
            })?
            .into_stream();
        Ok(stream)
    }

    fn drop_stream_handle(&mut self, handle: Resource<SchemaValueStreamHandleRep>) {
        let _ = self
            .store
            .data_mut()
            .durable_ctx_mut()
            .table()
            .delete(handle);
    }
}

impl<Ctx: WorkerCtx> SchemaValueStreamResolver for DurableWorkerCtx<Ctx> {
    type Error = WorkerExecutorError;

    fn handle_from_stream(
        &mut self,
        stream: SchemaValueStream,
    ) -> Result<Resource<SchemaValueStreamHandleRep>, Self::Error> {
        if let Some(tracker) = self.live_stream_tracker() {
            stream
                .with_host_endpoint::<RelayReceiver, _>(|receiver| receiver.attach(tracker))
                .map_err(WorkerExecutorError::runtime)?;
        }
        self.table()
            .push(SchemaValueStreamHandleRep::new(stream))
            .map_err(|error| {
                WorkerExecutorError::runtime(format!(
                    "failed to create schema-value-stream handle: {error}"
                ))
            })
    }

    fn stream_from_handle(
        &mut self,
        handle: Resource<SchemaValueStreamHandleRep>,
    ) -> Result<SchemaValueStream, Self::Error> {
        self.table()
            .delete(handle)
            .map(SchemaValueStreamHandleRep::into_stream)
            .map_err(|error| {
                WorkerExecutorError::runtime(format!("invalid schema-value-stream handle: {error}"))
            })
    }

    fn drop_stream_handle(&mut self, handle: Resource<SchemaValueStreamHandleRep>) {
        let _ = self.table().delete(handle);
    }
}

pub struct CoreTypesHost<Ctx: WorkerCtx>(PhantomData<Ctx>);

impl<Ctx: WorkerCtx> HasData for CoreTypesHost<Ctx> {
    type Data<'a> = &'a mut DurableWorkerCtx<Ctx>;
}

impl<Ctx: WorkerCtx> HostQuotaToken for DurableWorkerCtx<Ctx> {}
impl<Ctx: WorkerCtx> HostSecret for DurableWorkerCtx<Ctx> {}
impl<Ctx: WorkerCtx> HostSchemaValueStream for DurableWorkerCtx<Ctx> {}

impl<T: WorkerCtx, Ctx: WorkerCtx> HostSchemaValueStreamWithStore<T> for CoreTypesHost<Ctx> {
    async fn wrap(
        accessor: &Accessor<T, Self>,
        reader: StreamReader<SchemaValueTree>,
    ) -> anyhow::Result<Resource<SchemaValueStreamHandleRep>> {
        accessor
            .with(|mut access| -> wasmtime::Result<_> {
                let tracker = access.get().live_stream_tracker();
                let (consumer, stream) = relay_pair(tracker);
                reader.pipe(&mut access, consumer)?;
                access
                    .get()
                    .table()
                    .push(SchemaValueStreamHandleRep::new(stream))
                    .map_err(|error| wasmtime::Error::msg(error.to_string()))
            })
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    async fn unwrap(
        accessor: &Accessor<T, Self>,
        value: Resource<SchemaValueStreamHandleRep>,
    ) -> anyhow::Result<StreamReader<SchemaValueTree>> {
        accessor
            .with(|mut access| -> wasmtime::Result<_> {
                let stream = access
                    .get()
                    .table()
                    .delete(value)
                    .map_err(|error| wasmtime::Error::msg(error.to_string()))
                    .map(SchemaValueStreamHandleRep::into_stream)?;
                let receiver = stream
                    .take_host_endpoint::<RelayReceiver>()
                    .map_err(wasmtime::Error::msg)?;
                StreamReader::new(&mut access, RelayProducer::new(receiver))
            })
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    fn drop(
        accessor: &Accessor<T, Self>,
        rep: Resource<SchemaValueStreamHandleRep>,
    ) -> impl Future<Output = anyhow::Result<()>> + Send {
        async move {
            accessor.with(|mut access| {
                access
                    .get()
                    .table()
                    .delete(rep)
                    .map_err(|error| anyhow::anyhow!(error.to_string()))
            })?;
            Ok(())
        }
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<T: WorkerCtx, Ctx: WorkerCtx> HostWithStore<T> for CoreTypesHost<Ctx> {
    async fn parse_uuid(_accessor: &Accessor<T, Self>, uuid: String) -> Result<Uuid, String> {
        uuid::Uuid::parse_str(&uuid)
            .map(Into::into)
            .map_err(|error| error.to_string())
    }

    async fn uuid_to_string(_accessor: &Accessor<T, Self>, uuid: Uuid) -> String {
        let uuid: uuid::Uuid = uuid.into();
        uuid.to_string()
    }
}
