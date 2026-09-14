// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

use crate::schema::wit::wire::SchemaValueTree;
use crate::schema::{
    FromSchema, FromSchemaError, IntoSchema, MetadataEnvelope, SchemaBuilder, SchemaType,
    SchemaValue, SchemaValueStream, TypeId,
};

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
    decode: fn(SchemaValue) -> Result<T, String>,
}

/// The writable end of an [`AgentStream`].
pub struct AgentStreamWriter<T> {
    raw: RawWriter,
    encode: fn(T) -> Result<SchemaValue, String>,
}

impl<T> AgentStream<T> {
    /// Creates a native stream with codecs for one specific schema occurrence.
    /// The codecs consume their values, including any nested stream endpoints.
    pub fn new_with_codecs(
        encode: fn(T) -> Result<SchemaValue, String>,
        decode: fn(SchemaValue) -> Result<T, String>,
    ) -> (AgentStreamWriter<T>, Self) {
        let (writer, reader) = crate::schema::wit::new_schema_value_stream();
        (
            AgentStreamWriter {
                raw: writer,
                encode,
            },
            Self::from_schema_stream(SchemaValueStream::from_native(reader), decode),
        )
    }

    /// Lifts an existing endpoint without reading or replacing it.
    pub fn from_schema_stream(
        stream: SchemaValueStream,
        decode: fn(SchemaValue) -> Result<T, String>,
    ) -> Self {
        Self { stream, decode }
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
            Some(tree) => crate::schema::wit::decode_value(tree)
                .map_err(|e| format!("failed to decode agent stream item: {e}"))
                .and_then(self.decode)
                .map(Some),
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

impl<T: IntoSchema + FromSchema> AgentStream<T> {
    /// Creates a stream using the item's native schema traits.
    /// Generated bridge items should use their generated producer factory instead.
    pub fn new() -> (AgentStreamWriter<T>, Self) {
        Self::new_with_codecs(
            |value| Ok(value.to_value()),
            |value| T::from_value(&value).map_err(|e| e.to_string()),
        )
    }
}

impl<T: FromSchema> AgentStream<T> {
    #[doc(hidden)]
    pub fn from_raw(raw: RawReader) -> Self {
        Self::from_schema_stream(SchemaValueStream::from_native(raw), |value| {
            T::from_value(&value).map_err(|e| e.to_string())
        })
    }
}

impl<T> AgentStreamWriter<T> {
    /// Encodes one item and waits for acceptance by the native stream.
    /// Encoding failures preserve the codec's error and drop the consumed item,
    /// including any nested endpoints it still owns. A codec rejection sends
    /// nothing and leaves the writer usable for subsequent valid items.
    pub async fn write_one(&mut self, value: T) -> Result<(), String> {
        let value = (self.encode)(value)?;
        let value = crate::schema::wit::encode_value_async(&value)
            .await
            .map_err(|e| format!("failed to encode agent stream item: {e}"))?;
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
                decode: |value| T::from_value(&value).map_err(|e| e.to_string()),
            }),
            other => Err(FromSchemaError::shape_mismatch(
                "stream",
                crate::schema::conversion::value_kind(other),
                "AgentStream",
            )),
        }
    }
}
