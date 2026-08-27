// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

use std::marker::PhantomData;

use crate::schema::wit::wire::SchemaValueTree;
use crate::schema::{
    FromSchema, FromSchemaError, IntoSchema, MetadataEnvelope, SchemaBuilder, SchemaType,
    SchemaValue, SchemaValueStream, TypeId,
};

type RawReader = wit_bindgen::StreamReader<SchemaValueTree>;
type RawWriter = wit_bindgen::StreamWriter<SchemaValueTree>;

/// The readable end of a native component-model agent value stream.
pub struct AgentStream<T> {
    stream: SchemaValueStream,
    marker: PhantomData<T>,
}

/// The writable end of an [`AgentStream`].
pub struct AgentStreamWriter<T> {
    raw: RawWriter,
    marker: PhantomData<T>,
}

impl<T> AgentStream<T> {
    pub fn new() -> (AgentStreamWriter<T>, Self) {
        let (writer, reader) = crate::schema::wit::new_schema_value_stream();
        (
            AgentStreamWriter {
                raw: writer,
                marker: PhantomData,
            },
            Self::from_raw(reader),
        )
    }

    #[doc(hidden)]
    pub fn from_raw(raw: RawReader) -> Self {
        Self {
            stream: SchemaValueStream::from_native(raw),
            marker: PhantomData,
        }
    }

    #[doc(hidden)]
    pub async fn into_raw(self) -> Result<RawReader, String> {
        self.stream.take_native().await
    }
}

impl<T: FromSchema> AgentStream<T> {
    pub async fn next(&mut self) -> Result<Option<T>, String> {
        match self.stream.next_wire().await? {
            Some(tree) => crate::schema::wit::decode_value(tree)
                .map_err(|e| format!("failed to decode agent stream item: {e}"))
                .and_then(|value| T::from_value(&value).map_err(|e| e.to_string()))
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

impl<T: IntoSchema> AgentStreamWriter<T> {
    pub async fn write_one(&mut self, value: T) -> Result<(), String> {
        let value = crate::schema::wit::encode_value_async(&value.to_value())
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
                marker: PhantomData,
            }),
            other => Err(FromSchemaError::shape_mismatch(
                "stream",
                crate::schema::conversion::value_kind(other),
                "AgentStream",
            )),
        }
    }
}
