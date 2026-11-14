// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

pub use crate::schema::StreamError;
use crate::schema::wit::direct::{self, FromWire, IntoWire, WireSchema};
use crate::schema::wit::wire::SchemaValueTree;
use crate::schema::{
    FromSchema, FromSchemaError, IntoSchema, MetadataEnvelope, SchemaBuilder, SchemaType,
    SchemaValue, SchemaValueStream, TypeId,
};
use std::future::Future;
use std::pin::Pin;

type RawReader = wit_bindgen::StreamReader<SchemaValueTree>;
type RawWriter = wit_bindgen::StreamWriter<SchemaValueTree>;

/// The readable end of a native component-model agent value stream.
///
/// Run the producer concurrently with the consumer: writes wait for acceptance.
/// Close the writer explicitly for success, or finish it with a declared failure.
/// Dropping it without an outcome is abandoned on normal body completion.
/// Dropping the reader makes subsequent writes fail. Per-item Result values remain items.
///
/// Generated bridges supply schema-bound producer factories. Prefer those over
/// [`Self::new`] for generated types: `String`, for example, can represent either
/// a string or a path, and each occurrence needs its own codec.
///
/// Forward an unread stream directly instead of collecting and recreating it.
/// Schema conversion transfers the original take-once endpoint; it does not run
/// item codecs or start a pump. Aliases of a schema stream share that take-once
/// state and cannot be transferred twice.
pub struct AgentStream<T> {
    stream: SchemaValueStream,
    decode: Box<dyn Fn(SchemaValueTree) -> Result<T, String> + Send + Sync>,
}

type EncodeFuture = Pin<Box<dyn Future<Output = Result<SchemaValueTree, String>>>>;

/// The writable end of an [`AgentStream`].
pub struct AgentStreamWriter<T> {
    raw: RawWriter,
    outcome: wit_bindgen::FutureWriter<Option<crate::schema::wit::wire::StreamOutcome>>,
    encode: Box<dyn Fn(T) -> EncodeFuture + Send + Sync>,
}

impl<T> AgentStream<T> {
    /// Creates a stream whose item codecs operate directly on the wire arena.
    pub fn new_with_wire_codecs<F: Future<Output = Result<SchemaValueTree, String>> + 'static>(
        encode: impl Fn(T) -> F + Send + Sync + 'static,
        decode: impl Fn(SchemaValueTree) -> Result<T, String> + Send + Sync + 'static,
    ) -> (AgentStreamWriter<T>, Self) {
        let (writer, reader) = crate::schema::wit::new_schema_value_stream();
        let (outcome_writer, outcome) = crate::schema::wit::new_schema_stream_outcome();
        (
            AgentStreamWriter {
                raw: writer,
                outcome: outcome_writer,
                encode: Box::new(move |value| Box::pin(encode(value))),
            },
            Self {
                stream: SchemaValueStream::from_native(reader, outcome),
                decode: Box::new(decode),
            },
        )
    }

    /// Creates a native stream with codecs for one specific schema occurrence.
    /// The codecs consume their values, including any nested stream endpoints.
    pub fn new_with_codecs(
        encode: fn(T) -> Result<SchemaValue, String>,
        decode: fn(SchemaValue) -> Result<T, String>,
    ) -> (AgentStreamWriter<T>, Self)
    where
        T: 'static,
    {
        Self::new_with_wire_codecs(
            move |value| {
                let value = encode(value);
                async move {
                    crate::schema::wit::encode_value_async(&value?)
                        .await
                        .map_err(|e| format!("failed to encode agent stream item: {e}"))
                }
            },
            move |tree| {
                crate::schema::wit::decode_value(tree)
                    .map_err(|e| format!("failed to decode agent stream item: {e}"))
                    .and_then(decode)
            },
        )
    }

    /// Lifts an existing endpoint with a concrete wire decoder, without reading it.
    pub fn from_schema_stream_with_wire_decoder(
        stream: SchemaValueStream,
        decode: impl Fn(SchemaValueTree) -> Result<T, String> + Send + Sync + 'static,
    ) -> Self {
        Self {
            stream,
            decode: Box::new(decode),
        }
    }

    /// Lifts an existing endpoint without reading or replacing it.
    pub fn from_schema_stream(
        stream: SchemaValueStream,
        decode: fn(SchemaValue) -> Result<T, String>,
    ) -> Self
    where
        T: 'static,
    {
        Self::from_schema_stream_with_wire_decoder(stream, move |tree| {
            crate::schema::wit::decode_value(tree)
                .map_err(|e| format!("failed to decode agent stream item: {e}"))
                .and_then(decode)
        })
    }

    /// Transfers the original endpoint without running an item codec.
    pub fn into_schema_stream(self) -> SchemaValueStream {
        self.stream
    }

    #[doc(hidden)]
    pub async fn into_raw(
        self,
    ) -> Result<
        (
            RawReader,
            wit_bindgen::FutureReader<crate::schema::wit::wire::StreamOutcome>,
        ),
        String,
    > {
        self.stream.take_native().await
    }

    /// Reads one item, or one terminal error followed by the end.
    pub async fn next(&mut self) -> Option<Result<T, StreamError>> {
        match self.stream.next_wire().await {
            Ok(Some(tree)) => Some((self.decode)(tree).map_err(StreamError::from)),
            Ok(None) => None,
            Err(error) => Some(Err(error)),
        }
    }

    pub async fn collect(mut self) -> Result<Vec<T>, StreamError> {
        let mut result = Vec::new();
        while let Some(item) = self.next().await {
            result.push(item?);
        }
        Ok(result)
    }
}

impl<T: IntoWire + FromWire + 'static> AgentStream<T> {
    /// Creates a stream using the item's direct wire traits.
    /// Generated bridge items should use their generated producer factory instead.
    pub fn new() -> (AgentStreamWriter<T>, Self) {
        Self::new_with_wire_codecs(
            |value| async move {
                direct::encode_async(&value)
                    .await
                    .map_err(|e| e.to_string())
            },
            |tree| direct::decode(tree).map_err(|e| e.to_string()),
        )
    }
}

impl<T: FromWire> AgentStream<T> {
    #[doc(hidden)]
    pub fn from_raw(
        raw: RawReader,
        outcome: wit_bindgen::FutureReader<crate::schema::wit::wire::StreamOutcome>,
    ) -> Self {
        Self {
            stream: SchemaValueStream::from_reader(raw, outcome),
            decode: Box::new(|tree| direct::decode(tree).map_err(|e| e.to_string())),
        }
    }
}

impl<T> AgentStreamWriter<T> {
    /// Close item admission and publish an explicit producer outcome.
    /// Only Ok and Failed may be authored by a guest; the host rejects other cases.
    pub async fn finish(self, outcome: Result<(), StreamError>) -> Result<(), String> {
        let outcome = match outcome {
            Ok(()) => Ok(()),
            Err(error) => Err(error.into_wire().await),
        };
        drop(self.raw);
        self.outcome
            .write(Some(outcome))
            .await
            .map_err(|_| "agent stream reader was dropped".to_string())
    }

    /// Finish a stream successfully. Dropping a writer is not a successful close.
    pub async fn close(self) -> Result<(), String> {
        self.finish(Ok(())).await
    }

    /// Encodes one item and waits for acceptance by the native stream.
    /// Encoding failures preserve the codec's error and drop the consumed item,
    /// including any nested endpoints it still owns. A codec rejection sends
    /// nothing and leaves the writer usable for subsequent valid items.
    pub async fn write_one(&mut self, value: T) -> Result<(), String> {
        let value = (self.encode)(value).await?;
        if self.raw.write_one(value).await.is_none() {
            Ok(())
        } else {
            Err("agent stream reader was dropped".to_string())
        }
    }

    pub async fn write_all(&mut self, values: impl IntoIterator<Item = T>) -> Result<(), String> {
        for value in values {
            self.write_one(value).await?;
        }
        Ok(())
    }
}

impl<T: IntoSchema> IntoSchema for AgentStream<T> {
    fn type_id() -> TypeId {
        TypeId::new(format!("golem.AgentStream<{}>", T::type_id()))
    }

    fn register_in(builder: &mut SchemaBuilder) -> SchemaType {
        SchemaType::Stream {
            inner: Some(Box::new(T::register_in(builder))),
            metadata: MetadataEnvelope::default(),
        }
    }

    fn to_value(&self) -> SchemaValue {
        SchemaValue::Stream(self.stream.clone())
    }
}

impl<T: FromSchema> FromSchema for AgentStream<T> {
    fn from_value(value: &SchemaValue) -> Result<Self, FromSchemaError> {
        match value {
            SchemaValue::Stream(stream) => Ok(Self {
                stream: stream.clone(),
                decode: Box::new(|tree| {
                    let value =
                        crate::schema::wit::decode_value(tree).map_err(|e| e.to_string())?;
                    T::from_value(&value).map_err(|e| e.to_string())
                }),
            }),
            other => Err(FromSchemaError::shape_mismatch(
                "stream",
                crate::schema::conversion::value_kind(other),
                "AgentStream",
            )),
        }
    }
}

impl<T: WireSchema> WireSchema for AgentStream<T> {
    fn wire_type_id() -> String {
        format!("golem.AgentStream<{}>", T::wire_type_id())
    }

    fn contains_stream(_: &mut std::collections::HashSet<&'static str>) -> bool {
        true
    }

    fn append_schema(builder: &mut direct::WireSchemaBuilder) -> i32 {
        let inner = T::append_schema(builder);
        builder.push(crate::schema::wit::wire::SchemaTypeBody::StreamType(Some(
            inner,
        )))
    }
}

impl<T: FromWire> FromWire for AgentStream<T> {
    fn read_wire(reader: &mut direct::WireReader, index: i32) -> Result<Self, direct::WireError> {
        Ok(Self {
            stream: SchemaValueStream::read_wire(reader, index)?,
            decode: Box::new(|tree| direct::decode(tree).map_err(|e| e.to_string())),
        })
    }
}

impl<T> IntoWire for AgentStream<T> {
    fn preflight(&self, resources: &mut direct::WirePreflight) -> Result<(), direct::WireError> {
        self.stream.preflight(resources)
    }

    async fn prepare_wire(&self) -> Result<(), direct::WireError> {
        self.stream.prepare_wire().await
    }

    fn write_wire(&self, writer: &mut direct::WireWriter) -> Result<i32, direct::WireError> {
        self.stream.write_wire(writer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::wit::wire::{
        ResultSpec, ResultValuePayload, SchemaTypeBody, SchemaValueNode,
    };
    use test_r::test;

    // This type deliberately has no model conversion traits.
    #[derive(Debug, PartialEq, crate::FromWire, crate::IntoWire)]
    struct Item {
        entries: Vec<Result<Option<u32>, String>>,
    }

    struct Items(std::collections::VecDeque<Result<SchemaValueTree, StreamError>>);

    impl crate::schema::stream::SchemaValueStreamSource for Items {
        fn next(
            &mut self,
        ) -> Pin<Box<dyn Future<Output = Result<Option<SchemaValueTree>, StreamError>> + Send + '_>>
        {
            Box::pin(async { self.0.pop_front().transpose() })
        }
    }

    #[test]
    async fn terminal_failures_are_yielded_once_after_the_prefix() {
        let declared = crate::schema::try_into_typed_schema_value(&73u32).unwrap();
        for error in [
            StreamError::Failed(Box::new(declared)),
            StreamError::Cancelled,
        ] {
            let mut stream = AgentStream::<u32>::from_schema_stream_with_wire_decoder(
                SchemaValueStream::from_source(Items(
                    [
                        Ok(direct::encode(&7u32).unwrap()),
                        Err(error.clone()),
                        Ok(direct::encode(&23u32).unwrap()),
                    ]
                    .into(),
                )),
                |tree| direct::decode(tree).map_err(|error| error.to_string()),
            );
            assert_eq!(stream.next().await, Some(Ok(7)));
            assert_eq!(stream.next().await, Some(Err(error)));
            assert_eq!(stream.next().await, None);
        }
    }

    #[test]
    async fn direct_stream_items_reject_aliases_and_continue_after_errors() {
        let item = Item {
            entries: vec![Ok(Some(19)), Err("bad".to_string()), Ok(None)],
        };
        let mut stream = AgentStream::<Item>::from_schema_stream_with_wire_decoder(
            SchemaValueStream::from_source(Items(
                [
                    SchemaValueTree {
                        value_nodes: vec![
                            SchemaValueNode::U32Value(19),
                            SchemaValueNode::OptionValue(Some(0)),
                            SchemaValueNode::ResultValue(ResultValuePayload::OkValue(Some(1))),
                            SchemaValueNode::ListValue(vec![2, 2]),
                            SchemaValueNode::RecordValue(vec![3]),
                        ],
                        root: 4,
                    },
                    direct::encode(&item).unwrap(),
                ]
                .map(Ok)
                .into(),
            )),
            |tree| direct::decode(tree).map_err(|e| e.to_string()),
        );
        assert_eq!(
            stream.next().await.unwrap().unwrap_err(),
            StreamError::from("wire value index 2 is referenced more than once".to_string())
        );
        assert_eq!(stream.next().await.unwrap(), Ok(item));
        assert_eq!(stream.next().await, None);
    }

    #[test]
    fn direct_stream_schema_and_forwarding_preserve_payload_and_ownership() {
        let graph = direct::schema::<AgentStream<Result<u32, String>>>();
        let SchemaTypeBody::StreamType(Some(inner)) = graph.type_nodes[graph.root as usize].body
        else {
            panic!("expected typed stream");
        };
        let SchemaTypeBody::ResultType(ResultSpec {
            ok: Some(ok),
            err: Some(err),
        }) = graph.type_nodes[inner as usize].body
        else {
            panic!("expected result stream item");
        };
        assert!(matches!(
            graph.type_nodes[ok as usize].body,
            SchemaTypeBody::U32Type(None)
        ));
        assert!(matches!(
            graph.type_nodes[err as usize].body,
            SchemaTypeBody::StringType
        ));

        let stream = direct::decode::<AgentStream<Item>>(SchemaValueTree {
            value_nodes: vec![SchemaValueNode::StreamValue(unsafe {
                crate::schema::wit::wire::SchemaValueStream::from_handle(59)
            })],
            root: 0,
        })
        .unwrap();
        let forwarded = direct::encode(&stream).unwrap();
        assert!(matches!(
            direct::encode(&stream),
            Err(direct::WireError::ConsumedResource("stream"))
        ));
        for node in forwarded.value_nodes {
            if let SchemaValueNode::StreamValue(handle) = node {
                assert_eq!(handle.take_handle(), 59);
            } else {
                panic!("unexpected stream encoding");
            }
        }
    }
}
