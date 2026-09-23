// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

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
/// Dropping the writer ends the stream; dropping the reader makes subsequent
/// writes fail. A decoding error is an error, not end-of-stream. Model recoverable
/// application errors as items such as `AgentStream<Result<T, E>>`.
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
    encode: Box<dyn Fn(T) -> EncodeFuture + Send + Sync>,
}

impl<T> AgentStream<T> {
    /// Creates a stream whose item codecs operate directly on the wire arena.
    pub fn new_with_wire_codecs<F: Future<Output = Result<SchemaValueTree, String>> + 'static>(
        encode: impl Fn(T) -> F + Send + Sync + 'static,
        decode: impl Fn(SchemaValueTree) -> Result<T, String> + Send + Sync + 'static,
    ) -> (AgentStreamWriter<T>, Self) {
        let (writer, reader) = crate::schema::wit::new_schema_value_stream();
        (
            AgentStreamWriter {
                raw: writer,
                encode: Box::new(move |value| Box::pin(encode(value))),
            },
            Self {
                stream: SchemaValueStream::from_native(reader),
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

    /// Lifts an existing endpoint without reading or replacing it.
    pub fn from_schema_stream(
        stream: SchemaValueStream,
        decode: fn(SchemaValue) -> Result<T, String>,
    ) -> Self
    where
        T: 'static,
    {
        Self {
            stream,
            decode: Box::new(move |tree| {
                crate::schema::wit::decode_value(tree)
                    .map_err(|e| format!("failed to decode agent stream item: {e}"))
                    .and_then(decode)
            }),
        }
    }

    /// Transfers the original endpoint without running an item codec.
    pub fn into_schema_stream(self) -> SchemaValueStream {
        self.stream
    }

    #[doc(hidden)]
    pub async fn into_raw(self) -> Result<RawReader, String> {
        self.stream.take_native().await
    }

    /// Reads and decodes one item lazily. `Ok(None)` is clean EOF.
    pub async fn next(&mut self) -> Result<Option<T>, String> {
        match self.stream.next_wire().await? {
            Some(tree) => (self.decode)(tree).map(Some),
            None => Ok(None),
        }
    }

    pub async fn collect(mut self) -> Result<Vec<T>, String> {
        let mut result = Vec::new();
        while let Some(item) = self.next().await? {
            result.push(item);
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
    pub fn from_raw(raw: RawReader) -> Self {
        Self {
            stream: SchemaValueStream::from_native(raw),
            decode: Box::new(|tree| direct::decode(tree).map_err(|e| e.to_string())),
        }
    }
}

impl<T> AgentStreamWriter<T> {
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

    struct Items(std::collections::VecDeque<SchemaValueTree>);

    impl crate::schema::stream::SchemaValueStreamSource for Items {
        fn next(
            &mut self,
        ) -> Pin<Box<dyn Future<Output = Result<Option<SchemaValueTree>, String>> + Send + '_>>
        {
            Box::pin(async { Ok(self.0.pop_front()) })
        }
    }

    #[test]
    async fn direct_stream_items_reject_aliases_and_continue_after_errors() {
        let item = Item {
            entries: vec![Ok(Some(19)), Err("bad".to_string()), Ok(None)],
        };
        let mut stream = AgentStream::<Item> {
            stream: SchemaValueStream::from_source(Items(
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
                .into(),
            )),
            decode: Box::new(|tree| direct::decode(tree).map_err(|e| e.to_string())),
        };
        assert_eq!(
            stream.next().await.unwrap_err(),
            "wire value index 2 is referenced more than once"
        );
        assert_eq!(stream.next().await.unwrap(), Some(item));
        assert_eq!(stream.next().await.unwrap(), None);
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
