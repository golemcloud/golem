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
use crate::durable_host::concurrent::DropEvent;
use crate::durable_host::durable_session::{
    DurableInputEndpoint, DurableInputProducer, ForwardedDurableInput,
};
use crate::durable_host::stream_transport::{
    LiveInputProducer, LiveStreamEndpoint, output_stream_pair, relay_stream_pair,
};
use crate::workerctx::WorkerCtx;
use golem_schema::schema::schema_value::{
    PermissionCardValuePayload, QuotaTokenValuePayload, SecretValuePayload,
};
use golem_schema::schema::tool::compatibility::{
    ProjectionPlan, ProjectionStreamHandler, ToolCompatibilityError, apply_projection,
};
use golem_schema::schema::wit::wire::{
    Host, HostQuotaToken, HostSchemaValueStream, HostSchemaValueStreamWithStore, HostSecret,
    HostWithStore, SchemaValueTree, Uuid,
};
use golem_schema::schema::wit::{
    PermissionCardHandleRep, PermissionCardResolver, QuotaTokenHandleRep, QuotaTokenResolver,
    SchemaValueStreamResolver, SecretHandleRep, SecretResolver,
};
use golem_schema::schema::{SchemaValue, SchemaValueStream, SchemaValueStreamHandleRep};
use golem_service_base::error::worker_executor::WorkerExecutorError;
use std::future::Future;
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::mpsc;
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

#[derive(Clone)]
pub(crate) struct ExecutorProjectionStreams {
    capacity: usize,
    runtime_teardown: Arc<dyn Fn() -> bool + Send + Sync + 'static>,
    drop_event_sink: mpsc::UnboundedSender<DropEvent>,
}

impl ExecutorProjectionStreams {
    pub(crate) fn new<Ctx: WorkerCtx>(ctx: &DurableWorkerCtx<Ctx>) -> Self {
        Self {
            capacity: ctx.live_stream_event_capacity(),
            runtime_teardown: ctx.stream_runtime_teardown_probe(),
            drop_event_sink: ctx
                .state
                .dropped_call_event_sender()
                .expect("dropped-call event sender is always available"),
        }
    }

    #[cfg(test)]
    fn for_test(capacity: usize) -> Self {
        let (drop_event_sink, _) = mpsc::unbounded_channel();
        Self {
            capacity,
            runtime_teardown: Arc::new(|| false),
            drop_event_sink,
        }
    }
}

impl ProjectionStreamHandler for ExecutorProjectionStreams {
    fn project_stream(
        &mut self,
        stream: SchemaValueStream,
        item_plan: Option<ProjectionPlan>,
    ) -> Result<SchemaValueStream, String> {
        let Some(item_plan) = item_plan else {
            return Ok(stream);
        };
        if let Err(error) = stream.with_host_endpoint::<LiveStreamEndpoint, _>(|_| ()) {
            self.discard_stream(stream);
            return Err(error);
        }
        let source = stream.take_host_endpoint::<LiveStreamEndpoint>()?;
        let source_lifecycle = source.lifecycle();
        let mut source = source.activate();
        let (publisher, target) = relay_stream_pair(self.capacity)?;
        let target_lifecycle = target.lifecycle();
        let item_context = self.clone();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    biased;
                    _ = source_lifecycle.cancelled() => {
                        target_lifecycle.abort();
                        return;
                    },
                    _ = target_lifecycle.cancelled() => return,
                    event = source.recv() => event,
                };
                let mut terminal = true;
                let publication: Pin<Box<dyn Future<Output = _> + Send>> = match event {
                    Ok(event) => match event.payload {
                        crate::durable_host::stream_bus::LiveStreamEventPayload::Item(value) => {
                            let mut context = item_context.clone();
                            match apply_projection(&item_plan, value, &mut context) {
                                Ok(value) => {
                                    terminal = false;
                                    Box::pin(publisher.publish_item(value))
                                }
                                Err(error) => Box::pin(publisher.publish_error(format!(
                                    "schema stream item projection failed at {}: {}",
                                    error.path, error.message
                                ))),
                            }
                        }
                        crate::durable_host::stream_bus::LiveStreamEventPayload::End => {
                            Box::pin(publisher.publish_end())
                        }
                        crate::durable_host::stream_bus::LiveStreamEventPayload::Error(error) => {
                            Box::pin(publisher.publish_error(error))
                        }
                    },
                    Err(error) => Box::pin(
                        publisher.publish_error(format!("live stream receive failed: {error:?}")),
                    ),
                };
                tokio::select! {
                    biased;
                    _ = source_lifecycle.cancelled() => {
                        target_lifecycle.abort();
                        return;
                    },
                    _ = target_lifecycle.cancelled() => return,
                    result = publication => {
                        if result.is_err() {
                            target_lifecycle.abort();
                            return;
                        }
                        if terminal { return; }
                    },
                }
            }
        });
        Ok(SchemaValueStream::from_host_endpoint(target))
    }

    fn discard_stream(&mut self, stream: SchemaValueStream) {
        DurableInputProducer::drop_unread(
            stream.clone(),
            self.drop_event_sink.clone(),
            self.runtime_teardown.clone(),
        );
        if let Ok(endpoint) = stream.take_host_endpoint::<LiveStreamEndpoint>() {
            drop(endpoint);
        }
    }
}

pub(crate) fn project_schema_value<Ctx: WorkerCtx>(
    ctx: &DurableWorkerCtx<Ctx>,
    plan: &ProjectionPlan,
    value: SchemaValue,
) -> Result<SchemaValue, ToolCompatibilityError> {
    apply_projection(plan, value, &mut ExecutorProjectionStreams::new(ctx))
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

impl<Ctx: WorkerCtx> PermissionCardResolver for StoreValueResolver<'_, '_, Ctx> {
    type Error = WorkerExecutorError;

    fn snapshot_permission_card_handle(
        &mut self,
        handle: Resource<PermissionCardHandleRep>,
    ) -> Result<PermissionCardValuePayload, Self::Error> {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .snapshot_permission_card_handle(handle)
    }

    fn permission_card_handle_from_snapshot(
        &mut self,
        snapshot: &PermissionCardValuePayload,
    ) -> Result<Resource<PermissionCardHandleRep>, Self::Error> {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .permission_card_handle_from_snapshot(snapshot)
    }

    fn drop_permission_card_handle(&mut self, handle: Resource<PermissionCardHandleRep>) {
        self.store
            .data_mut()
            .durable_ctx_mut()
            .drop_permission_card_handle(handle)
    }
}

impl<Ctx: WorkerCtx> SchemaValueStreamResolver for StoreValueResolver<'_, '_, Ctx> {
    type Error = WorkerExecutorError;

    fn handle_from_stream(
        &mut self,
        stream: SchemaValueStream,
    ) -> Result<Resource<SchemaValueStreamHandleRep>, Self::Error> {
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
                let reader = match reader.try_into::<ForwardedDurableInput>(&mut access) {
                    Ok(forwarded) => {
                        return access
                            .get()
                            .table()
                            .push(SchemaValueStreamHandleRep::new(
                                SchemaValueStream::from_host_endpoint(forwarded),
                            ))
                            .map_err(|error| wasmtime::Error::msg(error.to_string()));
                    }
                    Err(reader) => reader,
                };
                let capacity = access.get().live_stream_event_capacity();
                let runtime_teardown = access.get().stream_runtime_teardown_probe();
                let (consumer, stream) =
                    output_stream_pair(capacity, runtime_teardown).map_err(wasmtime::Error::msg)?;
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
                if stream
                    .with_host_endpoint::<DurableInputEndpoint, _>(|_| ())
                    .is_ok()
                {
                    let drop_event_sink = access
                        .get()
                        .state
                        .dropped_call_event_sender()
                        .expect("dropped-call event sender is always available");
                    let runtime_teardown = access.get().stream_runtime_teardown_probe();
                    let endpoint = stream
                        .take_host_endpoint::<DurableInputEndpoint>()
                        .map_err(wasmtime::Error::msg)?;
                    StreamReader::new(
                        &mut access,
                        DurableInputProducer::new(endpoint)
                            .with_drop_cleanup(drop_event_sink, runtime_teardown),
                    )
                } else {
                    let endpoint = stream
                        .take_host_endpoint::<LiveStreamEndpoint>()
                        .map_err(wasmtime::Error::msg)?;
                    StreamReader::new(&mut access, LiveInputProducer::new(endpoint))
                }
            })
            .map_err(|error| anyhow::anyhow!(error.to_string()))
    }

    async fn drop(
        accessor: &Accessor<T, Self>,
        rep: Resource<SchemaValueStreamHandleRep>,
    ) -> anyhow::Result<()> {
        accessor.with(|mut access| -> anyhow::Result<()> {
            let stream = access
                .get()
                .table()
                .delete(rep)
                .map_err(|error| anyhow::anyhow!(error.to_string()))?
                .into_stream();
            let drop_event_sink = access
                .get()
                .state
                .dropped_call_event_sender()
                .expect("dropped-call event sender is always available");
            let runtime_teardown = access.get().stream_runtime_teardown_probe();
            DurableInputProducer::drop_unread(stream, drop_event_sink, runtime_teardown);
            Ok(())
        })?;
        Ok(())
    }
}

impl<Ctx: WorkerCtx> Host for DurableWorkerCtx<Ctx> {}

impl<T: WorkerCtx, Ctx: WorkerCtx> HostWithStore<T> for CoreTypesHost<Ctx> {
    async fn parse_uuid(
        _accessor: &Accessor<T, Self>,
        uuid: String,
    ) -> anyhow::Result<Result<Uuid, String>> {
        Ok(uuid::Uuid::parse_str(&uuid)
            .map(Into::into)
            .map_err(|error| error.to_string()))
    }

    async fn uuid_to_string(_accessor: &Accessor<T, Self>, uuid: Uuid) -> anyhow::Result<String> {
        let uuid: uuid::Uuid = uuid.into();
        Ok(uuid.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable_host::stream_bus::LiveStreamEventPayload;
    use golem_schema::schema::metadata::MetadataEnvelope;
    use golem_schema::schema::schema_type::NamedFieldType;
    use golem_schema::schema::tool::compatibility::{ProjectionNode, RecordFieldProjection};
    use golem_schema::schema::{SchemaGraph, SchemaType};
    use test_r::{test, timeout};

    fn reordered_record_plan() -> ProjectionPlan {
        let source = SchemaType::record(vec![
            NamedFieldType {
                name: "a".into(),
                body: SchemaType::u64(),
                metadata: MetadataEnvelope::default(),
            },
            NamedFieldType {
                name: "b".into(),
                body: SchemaType::string(),
                metadata: MetadataEnvelope::default(),
            },
        ]);
        let target = SchemaType::record(vec![
            NamedFieldType {
                name: "b".into(),
                body: SchemaType::string(),
                metadata: MetadataEnvelope::default(),
            },
            NamedFieldType {
                name: "a".into(),
                body: SchemaType::u64(),
                metadata: MetadataEnvelope::default(),
            },
        ]);
        ProjectionPlan {
            source_schema: SchemaGraph::anonymous(source),
            target_schema: SchemaGraph::anonymous(target),
            nodes: vec![
                ProjectionNode::Record {
                    fields: vec![
                        RecordFieldProjection {
                            target_name: "b".into(),
                            source_index: Some(1),
                            plan: Some(1),
                            default: None,
                        },
                        RecordFieldProjection {
                            target_name: "a".into(),
                            source_index: Some(0),
                            plan: Some(1),
                            default: None,
                        },
                    ],
                    discard: vec![],
                },
                ProjectionNode::Identity,
            ],
            root: 0,
        }
    }

    #[test]
    #[timeout("2s")]
    async fn relay_maps_items() {
        let (source_publisher, source_endpoint) = relay_stream_pair(2).unwrap();
        let source = SchemaValueStream::from_host_endpoint(source_endpoint);
        let mut streams = ExecutorProjectionStreams::for_test(2);
        let target = streams
            .project_stream(source, Some(reordered_record_plan()))
            .unwrap();
        let target = target.take_host_endpoint::<LiveStreamEndpoint>().unwrap();
        let mut target = target.activate();

        source_publisher
            .publish_item(SchemaValue::Record {
                fields: vec![SchemaValue::U64(7), SchemaValue::String("seven".into())],
            })
            .await
            .unwrap();
        source_publisher.publish_end().await.unwrap();

        assert!(
            matches!(target.recv().await.unwrap().payload, LiveStreamEventPayload::Item(
            SchemaValue::Record { fields }
        ) if fields == vec![SchemaValue::String("seven".into()), SchemaValue::U64(7)])
        );
        assert!(matches!(
            target.recv().await.unwrap().payload,
            LiveStreamEventPayload::End
        ));
    }

    #[test]
    #[timeout("2s")]
    async fn dropping_unread_projection_target_cancels_source() {
        let (source_publisher, source_endpoint) = relay_stream_pair(1).unwrap();
        let source_lifecycle = source_endpoint.lifecycle();
        let mut streams = ExecutorProjectionStreams::for_test(1);
        let target = streams
            .project_stream(
                SchemaValueStream::from_host_endpoint(source_endpoint),
                Some(reordered_record_plan()),
            )
            .unwrap();
        drop(target.take_host_endpoint::<LiveStreamEndpoint>().unwrap());
        source_lifecycle.cancelled().await;
        assert!(matches!(
            source_publisher.publish_item(SchemaValue::Bool(true)).await,
            Err(_)
        ));
    }

    #[test]
    #[timeout("5s")]
    async fn relay_backpressure_and_source_abort_reach_downstream() {
        for buffered in [false, true] {
            let (publisher, endpoint) = relay_stream_pair(1).unwrap();
            let lifecycle = endpoint.lifecycle();
            let mut streams = ExecutorProjectionStreams::for_test(1);
            let target = streams
                .project_stream(
                    SchemaValueStream::from_host_endpoint(endpoint),
                    Some(reordered_record_plan()),
                )
                .unwrap()
                .take_host_endpoint::<LiveStreamEndpoint>()
                .unwrap();
            let target_lifecycle = target.lifecycle();
            let mut target = target.activate();
            if buffered {
                for i in 0..3 {
                    publisher
                        .publish_item(SchemaValue::Record {
                            fields: vec![SchemaValue::U64(i), SchemaValue::String(i.to_string())],
                        })
                        .await
                        .unwrap();
                }
                // One target item, one relay item, and one source item fill capacity.
                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(50),
                        publisher.publish_item(SchemaValue::Record {
                            fields: vec![SchemaValue::U64(3), SchemaValue::String("3".into())],
                        }),
                    )
                    .await
                    .is_err()
                );
                assert!(matches!(
                    target.recv().await.unwrap().payload,
                    LiveStreamEventPayload::Item(SchemaValue::Record { fields })
                        if fields == vec![SchemaValue::String("0".into()), SchemaValue::U64(0)]
                ));
            }
            lifecycle.abort();
            target_lifecycle.cancelled().await;
            assert!(target_lifecycle.is_aborted());
        }
    }

    #[test]
    #[timeout("5s")]
    async fn relay_abort_takes_priority_over_buffered_terminal() {
        let (publisher, endpoint) = relay_stream_pair(1).unwrap();
        publisher
            .publish_error("buffered failure".into())
            .await
            .unwrap();
        endpoint.lifecycle().abort();
        drop(publisher);
        let mut streams = ExecutorProjectionStreams::for_test(1);
        let target = streams
            .project_stream(
                SchemaValueStream::from_host_endpoint(endpoint),
                Some(reordered_record_plan()),
            )
            .unwrap()
            .take_host_endpoint::<LiveStreamEndpoint>()
            .unwrap();
        let lifecycle = target.lifecycle();
        let mut target = target.activate();
        lifecycle.cancelled().await;
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), target.recv(),)
                .await
                .is_err()
        );
    }

    #[test]
    #[timeout("5s")]
    async fn relay_errors_terminate_and_cancel_upstream() {
        for invalid_item in [false, true] {
            let (publisher, endpoint) = relay_stream_pair(1).unwrap();
            let lifecycle = endpoint.lifecycle();
            let mut streams = ExecutorProjectionStreams::for_test(1);
            let mut target = streams
                .project_stream(
                    SchemaValueStream::from_host_endpoint(endpoint),
                    Some(reordered_record_plan()),
                )
                .unwrap()
                .take_host_endpoint::<LiveStreamEndpoint>()
                .unwrap()
                .activate();
            if invalid_item {
                publisher
                    .publish_item(SchemaValue::Bool(true))
                    .await
                    .unwrap();
            } else {
                publisher
                    .publish_error("upstream failure".into())
                    .await
                    .unwrap();
            }
            let LiveStreamEventPayload::Error(error) = target.recv().await.unwrap().payload else {
                panic!("expected error terminal");
            };
            if invalid_item {
                assert!(error.contains("projection failed"));
                lifecycle.cancelled().await;
            } else {
                assert_eq!(error, "upstream failure");
            }
        }
    }
}
